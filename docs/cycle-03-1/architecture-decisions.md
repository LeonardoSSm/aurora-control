# Decisões de arquitetura — Ciclo 03.1

## D1. Antigravity substitui Gemini no catálogo

**Contexto.** O Google encerrou o login do Gemini CLI para Gemini Code Assist for individuals e encaminhou os usuários ao Antigravity. O indicador "Gemini" da 0.3.0 só detectava esse login, sem cotas.

**Decisão.** O slot passa a ser `antigravity` (nome "Antigravity", fornecedor Google, identidade azul → violeta mantida). As preferências vão para a versão 3: a entrada `gemini` vira `antigravity`, **sempre desativada** (ela executa o CLI), com a métrica escolhida zerada (os ids de cota mudaram) e o interruptor de alertas preservado. O módulo `gemini.rs` foi removido; a investigação dele continua em [Compatibilidade dos provedores](../provider-compatibility.md).

**Alternativa descartada.** Manter os cinco provedores: o catálogo deste ciclo é de quatro, e o Gemini CLI individual deixou de ser um produto utilizável.

## D2. Fonte do Antigravity: CLI oficial em modo não interativo

**Decisão.** O adaptador executa `agy.exe -p /usage --output-format json --settings {"disableSlashCommands":false} --log-file <temp>`. A fonte é classificada como **CLI oficial** (`origin: cli`, `integration: cli`).

**Controles do subprocesso.**

| Requisito                 | Implementação                                                                                                  |
| ------------------------- | -------------------------------------------------------------------------------------------------------------- |
| Executável controlado     | Caminho fixo do instalador oficial: `%LOCALAPPDATA%\agy\bin\agy.exe`; não usa `PATH`                           |
| Sem comandos arbitrários  | Argumentos constantes no código; nada vem do WebView ou das preferências                                       |
| Timeout                   | 45 s, com encerramento do processo                                                                             |
| Limite de saída           | 256 KiB em stdout e stderr; o excesso é descartado e a execução rejeitada                                      |
| Cancelamento              | `UsageAdapter::cancel`, chamado quando o provedor é desativado; o processo em andamento é encerrado            |
| Sem duplicidade           | Um worker por provedor com `in_flight`, mais um `try_lock` no adaptador                                        |
| Falhas                    | Saída 2 (chave de API) → conta incompatível; mensagens de login → sessão ausente; demais → resposta inesperada |
| Logs                      | Nenhuma saída do CLI é registrada; só o caminho de campos anômalos                                             |
| Credenciais               | Nunca lidas pelo app; o CLI usa o próprio login                                                                |
| Sem janela                | `CREATE_NO_WINDOW`, stdin nulo, diretório de trabalho = pasta pessoal                                          |
| Slash commands garantidos | `--settings` de sessão (nunca gravado) força `disableSlashCommands: false`                                     |
| Pasta do CLI limpa        | `--log-file` aponta para um arquivo próprio, sobrescrito a cada execução                                       |

**Disjuntor.** Se uma resposta trouxer `num_turns > 0` ou `total_tokens > 0`, o CLI executou uma rodada do modelo em vez de responder ao comando. O adaptador marca a fonte como suspensa (`sourceBlocked: cliSafety`) e não executa mais nada até o app reiniciar. Assim, uma mudança futura no CLI não consegue gastar a cota do usuário repetidamente.

**Cadência.** 10 minutos (`refreshSeconds: 600`), com o clique manual sujeito às regras de recuperação já existentes. As cotas são de 5 h e semanais; a cadência menor reduz os efeitos colaterais na pasta do CLI.

## D3. Leituras do Antigravity sem conta verificada

**Contexto.** Nem o `/usage` nem o `/config` informam a conta, e o login fica no cofre do sistema.

**Decisão.** A sessão é `Unverified` e a leitura não tem identidade. O `Tracker` exibe a leitura enquanto ela é a da última tentativa (regra B01 do Ciclo 02), e o motor de alertas **não** emite alertas de consumo ou renovação para ela, porque não pode garantir que uma troca de conta não carregue níveis de outra. Alertas de sessão (CLI ausente) continuam valendo.

**Caminho futuro.** O `statusLine` do CLI recebe o e-mail e o plano, mas só em sessões interativas e exige configurar o `settings.json` do usuário.

## D4. Grok continua bloqueado

Ver [Investigação do Grok](grok-research.md) e [Revisão de termos](provider-terms-review.md). Nenhuma opção de "ignorar restrições" foi adicionada; a ativação continua sendo uma mudança de código revisável (`LIVE_QUERY_AUTHORIZED`).

## D5. Validação ao lado da cópia instalada

O proprietário usa uma cópia instalada, e encerrá-la para validar foi recusado. Builds de depuração aceitam `AURORA_INSTANCE`, um sufixo no nome do mutex de instância única, para rodar uma instância de validação ao lado dela. Assim como `AURORA_CONFIG_DIR` e `AURORA_OPEN_SETTINGS`, builds de produção ignoram a variável. Os scripts de validação a definem.

## D6. Origem dos dados visível

`Origin` ganhou `cli`. O resumo e as configurações mostram "Fonte: …": consulta à conta (mesmo endpoint do cliente oficial), CLI oficial, registro local ou demonstração.
