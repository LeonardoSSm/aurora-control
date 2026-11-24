# Grok e Cursor: fontes da franquia compartilhada

Data: 24/11/2026. Máquina do proprietário, Windows 11, Grok Build 1.0.92 (`b06756fd6256`, canal stable). Nenhum valor de credencial, e-mail ou identificador de conta aparece neste documento: identificadores foram comparados por igualdade, sem impressão.

## Conclusão

| Fonte                                                | Disponível                                              | Autorização                                               | Usada                                 |
| ---------------------------------------------------- | ------------------------------------------------------- | --------------------------------------------------------- | ------------------------------------- |
| Registro local `logs/unified.jsonl` do Grok Build    | Sim, a cada consulta do próprio CLI                     | Leitura passiva de um arquivo do usuário, gerado pelo CLI | **Sim (fonte principal)**             |
| `grok usage --json` (subcomando oficial)             | Só na "daemon route"; nesta instalação sai com código 1 | Interface oficial documentada                             | Não (indisponível; ver abaixo)        |
| ACP `grok agent stdio` + `_x.ai/billing`             | `initialize` responde; a extensão não é anunciada       | Extensão privada, sem documentação nem autorização        | Não                                   |
| Endpoint `cli-chat-proxy.grok.com/v1/billing`        | Sim (é o que o `/usage` usa)                            | Interno; termos da xAI inacessíveis (HTTP 403)            | Não (`LIVE_QUERY_AUTHORIZED = false`) |
| Cursor (CLI, arquivos locais, API)                   | Cursor não está instalado nesta máquina                 | Admin API só para equipes; API pessoal não documentada    | Não                                   |
| Cookies de navegador, gRPC-web do grok.com, scraping | —                                                       | Proibido pelo escopo do ciclo                             | Não                                   |

O quarto indicador, **Grok / Cursor**, mostra a franquia semanal compartilhada a partir do registro que o Grok Build grava a cada consulta dele, vinculado à conta atual. A correspondência com a franquia do Cursor é sustentada pelo campo `isUnifiedBillingUser` e pela documentação do Cursor; a comparação direta com a tela do Cursor depende de homologação pelo proprietário (o Cursor não está instalado aqui).

## Etapa A: registro local

### Versão e arquivo

- `grok --version`: `grok 1.0.92 (b06756fd6256) [stable]`. O executável fica em `%USERPROFILE%\.grok\bin\grok.exe` (há um `agent.exe` idêntico, com o mesmo hash, que é um alias do próprio Grok).
- `GROK_HOME` não está definido; o registro em `AuthManager::new` confirma `grok_home = %USERPROFILE%\.grok`.
- Arquivo: `%USERPROFILE%\.grok\logs\unified.jsonl`, 118.703 bytes e 412 linhas no início da investigação. A documentação empacotada (`~/.grok/docs/user-guide/05-configuration.md`) descreve `~/.grok/logs/` como "Internal log files (e.g. `unified.jsonl`…)".

### Estrutura real

Linha observada (valores de período reais, sem dados pessoais):

```json
{
  "ts": "2026-11-24T11:37:08.435Z",
  "src": "shell",
  "pid": 24352,
  "ver": "1.0.92",
  "lvl": "info",
  "msg": "billing: fetched credits config",
  "ctx": {
    "config": {
      "creditUsagePercent": 100.0,
      "currentPeriod": {
        "type": "USAGE_PERIOD_TYPE_WEEKLY",
        "start": "2026-11-20T03:30:32.116+00:00",
        "end": "2026-11-27T03:30:32.116+00:00"
      },
      "prepaidBalance": { "val": 0 },
      "isUnifiedBillingUser": true,
      "billingPeriodStart": "2026-11-20T03:30:32.116+00:00",
      "billingPeriodEnd": "2026-11-27T03:30:32.116+00:00",
      "historyLen": 0
    },
    "onDemandEnabled": null,
    "subscriptionTier": "SuperGrok Lite"
  }
}
```

Das hipóteses da pesquisa externa, foram confirmados `creditUsagePercent`, `currentPeriod.{type,start,end}`, `subscriptionTier` e `prepaidBalance`. `productUsage` **não existe** na versão instalada; `onDemandUsed` só aparece quando há gasto sob demanda (proto3 omite zeros). O campo novo relevante é `isUnifiedBillingUser`.

No código-fonte público (`xai-org/grok-build`, Apache-2.0, commit `2bdd1d6`, versão 1.0.45 no `Cargo.toml`, mais antiga que a instalada), `crates/codegen/xai-grok-shell/src/extensions/billing.rs`:

- o evento é emitido por `handle_get_billing` **depois** de uma resposta bem-sucedida, com o mesmo `BillingConfigResponse` devolvido ao `/usage`; falhas são registradas com outras mensagens (`billing: upstream error`, `billing: upstream request failed`, `billing: failed to parse response`);
- o comentário do código diz que "every prompt, `/usage`, and poll path hits `x.ai/billing`" e que o registro existe para correlacionar a interface de limite com os saldos reais;
- `isUnifiedBillingUser` é documentado como "whether this user is on unified usage billing (a shared weekly/monthly pool)".

### Comparação com `/usage` e frequência

- `~/.grok/slash-mru.json` mostra `/usage` usado nesta conta. Os três registros do processo das 11:37 (início da sessão e `/usage`) trazem 100%, e os de 05:04 trazem 71%; é a mesma resposta que o `/usage` desenha, pelo código acima.
- Atualização: na inicialização (busca antecipada do pager), a cada prompt, a cada `/usage` e no poll do pager. Quando o Grok Build não está em uso, o registro envelhece; o app trata isso como dado desatualizado (15 min) e mostra o horário da consulta do Grok, não o da leitura.
- Não houve geração de texto para produzir métricas. Uma sonda ACP (ver Etapa C) gravou eventos de inicialização, mas nenhum de cobrança.

### Rotação, truncamento e substituição

`crates/codegen/xai-grok-telemetry/src/logs/unified_log.rs`: limite `MAX_SIZE = 5 MiB`; ao atingir o limite, `trim_file` mantém a **metade mais nova no próprio inode** (reescreve a cauda sobre o início e reduz o tamanho), sem renomear, porque vários processos mantêm descritores `O_APPEND`. Versões antigas aparavam por arquivo temporário e `rename`. Linhas de processos diferentes podem se intercalar. Conclusão para o leitor: ler de trás para frente, sem guardar offset, tolerar linha final parcial e ler de novo quando tamanho ou data mudarem.

### Identidade

A linha de cobrança não traz conta. Cada processo registra, na inicialização, `auth init user_info check` com `ctx.user_id`. Comparado por igualdade, sem impressão: os três `user_id` do log são iguais ao `user_id` do `auth.json` atual. Uma sonda posterior criou um processo novo, com `user_id` próprio e **sem** evento de cobrança, o que mostrou que "o último usuário do log" não serve como identidade: a associação correta é a do **mesmo `pid`**, na linha de identidade imediatamente anterior ao registro (o que também trata reaproveitamento de `pid` pelo Windows).

### Política implementada

- O registro só é exibido se o usuário do processo que o escreveu for o do `auth.json` atual.
- Sem linha de identidade (aparada pelo limite do log), de outra conta, de período encerrado ou sem percentual: nenhuma leitura (`awaitingClient` ou `malformed`), nunca 0%.
- Com a conta verificada, os alertas de consumo e renovação funcionam como nos demais provedores, e só com dado recente (menos de 15 min).

## Etapa C: Agent Client Protocol

- `grok agent --no-leader stdio` respondeu a `initialize` (`protocolVersion: 1`, `agentVersion: 1.0.92`, métodos de autenticação `cached_token` e `grok.com`). As capacidades anunciam `x.ai/fs_notify`, `x.ai/hooks` e `x.ai/capabilities`; **`x.ai/billing` não é anunciado**. Por decisão, a extensão de cobrança **não foi chamada**.
- No código público, `x.ai/billing` é uma extensão privada do agente (`acp_agent.rs`), consumida pelo pager e pelo app desktop da própria xAI. Não há documentação para terceiros.
- Inicializar o agente carrega configuração gerenciada, catálogo de modelos, servidores MCP do usuário (houve um `_x.ai/mcp/servers_updated`) e sincronização do marketplace. Usar isso a cada consulta seria pesado e teria efeitos colaterais.
- Referências analisadas (sem tomá-las como autorização): [CodexBar](https://github.com/steipete/CodexBar) (MIT) usa `grok agent stdio` + `_x.ai/billing`, depois o endpoint com o token e, por fim, cookies do grok.com; [ai-usage-dashboard](https://github.com/neyham/ai-usage-dashboard) (MIT) usa o token do `auth.json` e relata que a sonda ACP devolvia `Method not found` em versões antigas; [KodexBar-Suite](https://github.com/Karasowl/KodexBar-Suite) (MIT), [quotabar](https://github.com/majiayu000/quotabar) (MIT) e [quota-usage-monitor](https://github.com/rozsazoltan/quota-usage-monitor) (AGPL-3.0) leem o token do `auth.json`. Nenhum lê o `unified.jsonl` e nenhum discute os termos. O código deles não foi copiado.
- Conclusão: suporte oficial ao ACP não autoriza a extensão privada. Não usada.

## `grok usage --json`

O Grok 1.0.92 tem o subcomando oficial `grok usage` ("Print your plan's usage limit"), documentado em `17-sessions.md`. Na "shell route" (a desta instalação, `connect_target: embedded`) ele sai com código 1 e a mensagem "can't show your plan's usage here yet. Run /usage inside grok instead", sem ler a conta; só funciona na "daemon route", com o serviço em segundo plano do Grok ativo. Iniciar esse serviço só para medir seria mudar o modo de operação do cliente. **Condição para adotá-lo no futuro:** quando o proprietário usar a daemon route (ou quando ela virar o padrão), `grok usage --json` pode substituir o registro local como fonte primária, executado pelo mesmo controle de subprocesso do Antigravity.

## Etapa D: Cursor

- Instalação: nada em `%LOCALAPPDATA%\Programs\cursor`, `%APPDATA%\Cursor` ou `~/.cursor`, e nenhum `cursor`/`cursor-agent` no `PATH`. Não houve validação com o Cursor.
- Documentação ([cursor.com/docs/models](https://cursor.com/docs/models), lida em 24/11/2026): "Paid plans include a weekly pool of usage for Cursor models, shared across Grok and Cursor"; o pool cobre o Agent no editor, o CLI, Cloud Agents, o Grok Bot e os apps Grok; o restante aparece nas configurações do editor e em `grok.com/dashboard`; o Cursor é vendido como parte da X Subscription.
- Admin API: só para equipes, com chave de administrador. As ferramentas de referência leem o token do Cursor (`state.vscdb`, `auth.json` do CLI) ou cookies e chamam `cursor.com/api/usage-summary`, interface não documentada; fora do escopo autorizado.
- **Representação:** com `isUnifiedBillingUser: true`, o percentual do Grok Build é o do pool unificado; nada é somado. Créditos pré-pagos, uso sob demanda e limites de API são tratados como coisas distintas e não alimentam o indicador.
- **Pendência de homologação:** comparar o percentual do indicador com a tela de uso do Cursor (ou com `grok.com/dashboard`) logo depois de um `/usage` no Grok Build.

## Etapa E: termos

Ver [Auditoria de segurança](security-audit.md#termos-dos-provedores). Em resumo: as três páginas da xAI responderam 403 novamente; a leitura passiva de um arquivo local produzido pelo cliente oficial não envolve acesso ao serviço, uso de credencial nem automação do cliente, e foi a única fonte adotada. `LIVE_QUERY_AUTHORIZED` permanece `false`.

## Condições para evoluir

1. Termos da xAI legíveis e compatíveis: liberar a consulta direta (leitura em tempo real), mantendo o registro local como alternativa.
2. Daemon route em uso: `grok usage --json` como fonte primária.
3. Assinaturas independentes de Grok e Cursor: um segundo indicador, em outro ciclo, com fonte própria do Cursor documentada.
