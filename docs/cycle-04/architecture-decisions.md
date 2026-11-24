# Decisões de arquitetura do Ciclo 04

Data: 24/11/2026. Versão 1.0.0.

## D1. Grok / Cursor lê o registro local do Grok Build

**Contexto.** Grok e Cursor dividem a franquia semanal da assinatura da xAI. A consulta direta ao endpoint do `/usage` usa o token do Grok Build num endpoint interno, e os termos da xAI continuam inacessíveis (HTTP 403). A extensão ACP `x.ai/billing` é privada e não anunciada. `grok usage --json` só funciona na daemon route.

**Decisão.** A fonte é o registro `billing: fetched credits config` que o próprio Grok Build grava em `logs/unified.jsonl` a cada consulta dele. O app só lê o arquivo.

**Consequências.** Nenhuma credencial é usada e nada é enviado à xAI. A leitura é tão recente quanto o último uso do Grok Build: sem uso, o indicador fica desatualizado (estado explícito, com o horário da consulta do Grok). O formato do log não é documentado; uma mudança vira `malformed` ou `awaitingClient`, nunca um número inventado.

## D2. Identidade pelo `pid` do processo que escreveu o registro

**Contexto.** A linha de cobrança não traz conta. Processos que não consultam a cobrança (por exemplo, `grok agent stdio`) também registram usuário.

**Decisão.** O registro pertence ao usuário da linha `auth init user_info check` do **mesmo `pid`** imediatamente anterior a ele. O registro só é exibido se esse usuário for o do `auth.json` atual. Sem associação, nada é exibido.

**Consequências.** Alertas de consumo do Grok / Cursor ficam permitidos (conta verificada). Um `pid` reaproveitado pelo Windows usa a identidade mais próxima, que é a do processo novo. Se a linha de identidade for aparada pelo limite de 5 MiB do log, o registro fica sem atribuição até o próximo uso do Grok.

## D3. Leitura de trás para frente, sem offset persistente

**Contexto.** O Grok apara o arquivo no próprio inode (mantém a metade mais nova) e versões antigas o substituíam por `rename`; vários processos escrevem em paralelo.

**Decisão.** Cada varredura lê do fim para o início em blocos de 64 KiB, até 6 MiB e 750 ms, ignora linhas parciais ou acima de 256 KiB, e para no primeiro registro com identidade. O adaptador guarda o resultado enquanto tamanho e data de modificação não mudam.

**Consequências.** Truncamento, rotação e substituição não exigem tratamento especial. Sem uso do Grok, cada ciclo de 60 s custa um `stat`.

## D4. Um indicador para a franquia compartilhada, rotulado só com prova

**Decisão.** O identificador continua `grok` (preferências preservadas); o nome passa a ser "Grok / Cursor". O rótulo "Franquia semanal compartilhada" só aparece com `isUnifiedBillingUser: true`; caso contrário, "Créditos semanais". Nada é somado; créditos pré-pagos e sob demanda não alimentam o anel.

## D5. Novos estados do contrato

- `Origin::ClientLog` (`clientLog`): número que o cliente oficial consultou e gravou; a interface diz "registro do cliente oficial" em vez de "Conectado".
- `Issue::AwaitingClient` (`awaitingClient`): não há registro atual e atribuível; a interface pede para abrir o Grok Build. Indicador recolhido com o marcador neutro `waiting`.
- `BlockReason::TermsUnverified` foi removido (não há mais fonte bloqueada por termos no app; a consulta direta desligada não produz estado).

## D6. Subprocesso do Antigravity sob época e Job Object

**Contexto.** A revisão anterior apontou uma janela entre a verificação do cancelamento e o `spawn`, cancelamento detectado só por polling de 50 ms, descendentes que podem manter os pipes abertos e processos órfãos no encerramento.

**Decisão.** `usage/process.rs`: `Control` com trava, flag e época. O `spawn` acontece sob a mesma trava que `cancel`. `cancel` incrementa a época e encerra o Job na hora. Toda tentativa compara a época do início com a do fim: se mudou, o resultado é descartado, mesmo que o provedor tenha sido reativado. Cada execução tem um Job Object com `KILL_ON_JOB_CLOSE`, encerrado depois de qualquer término, de modo que descendentes não sobrevivem e os pipes fecham; leitores de saída têm 2 s de tolerância. No encerramento do Aurora Control, o sistema fecha o handle e mata o que restar.

**Limite conhecido.** O processo é associado ao Job logo depois do `spawn`; um filho criado nesses microssegundos ficaria fora (o `agy` leva centenas de milissegundos para iniciar o runtime). A alternativa (`PROC_THREAD_ATTRIBUTE_JOB_LIST`) exige API instável da biblioteca padrão.

## D7. Disjuntor persistente com reativação informada

**Decisão.** `usage/safety.rs` grava `safety-antigravity.json` na pasta de configuração ao disparar. Dispara com rodada do modelo ou tokens (como antes) e também quando o CLI sai com sucesso **sem provar** zero rodadas e zero tokens (sem JSON, contadores ausentes, comando diferente de `usage`). Arquivo ilegível conta como disparado. A reativação exige duas ações nas configurações, com a causa e o risco descritos, pelo comando `reset_provider_safety` (só a janela de configurações). Desativar e reativar o provedor não reinicia o disjuntor. Saídas com código diferente de zero e sem envelope (por exemplo, sem login) não disparam.

**Consequências.** Uma mudança de formato do CLI suspende a fonte até o usuário decidir, em vez de continuar executando algo que pode gastar cota. Timeout não dispara o disjuntor (o processo é morto; não há evidência de rodada).

## D8. Identidade do Antigravity

Inalterada: a saída do `agy` não identifica a conta, e a credencial fica no cofre do sistema. Extrair a credencial para descobrir a conta foi descartado. Sem identidade, a leitura vale só enquanto é a mais recente e não gera alertas de consumo nem de renovação.
