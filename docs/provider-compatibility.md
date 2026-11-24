# Compatibilidade dos provedores

Situação na versão 0.3.1, em 24/11/2026. O catálogo dos provedores fica em [`src/usage/catalog.json`](../src/usage/catalog.json), compartilhado por Rust e TypeScript. A investigação do Ciclo 03.1 está em [`docs/cycle-03-1/`](cycle-03-1/implementation-report.md).

## Resumo

| Provedor    | Classificação da fonte                | Autenticação detectada              | Consulta real validada                   | Cotas acessíveis                                        | Situação                                                    |
| ----------- | ------------------------------------- | ----------------------------------- | ---------------------------------------- | ------------------------------------------------------- | ----------------------------------------------------------- |
| Codex       | Endpoint do cliente oficial (interno) | Sim (`~/.codex/auth.json`)          | **Sim**, 24/11/2026                      | 5 h, semanal, créditos; registro local como fallback    | Integrado                                                   |
| Claude Code | Endpoint do cliente oficial (interno) | Sim (`~/.claude/.credentials.json`) | **Sim**, 24/11/2026                      | 5 h, semanal, por modelo, uso extra; tokens locais      | Integrado                                                   |
| Antigravity | **CLI oficial** (`agy -p /usage`)     | Pelo próprio CLI (`agy.exe`)        | **Sim**, 24/11/2026, conferida com o CLI | 5 h e semanal por grupo (Gemini; Claude e GPT)          | Integrado, sem identidade de conta (sem alertas de consumo) |
| Grok        | **Fonte bloqueada**                   | Sim (`~/.grok/auth.json`)           | **Não**: consulta desligada              | Franquia semanal ou mensal identificada, parser testado | Bloqueado até a verificação dos termos da xAI               |

"Consulta real validada" significa que o app leu dados reais de uma conta autenticada, não apenas fixtures. Os testes automatizados usam só fixtures sintéticas.

O indicador "Gemini" da 0.3.0 foi substituído pelo Antigravity na 0.3.1: o Google encerrou o login do Gemini CLI para Gemini Code Assist for individuals. A seção do Gemini abaixo fica como registro da investigação.

## Antigravity (Google)

- **Modalidades suportadas:** conta Google autenticada no Antigravity CLI (`agy`), planos individuais (gratuito, Google AI Plus, Pro, Ultra). Login por chave da Gemini API é conta incompatível (o CLI sai com código 2). Contas Enterprise e Business não têm cotas por modelo.
- **Fonte de autenticação:** a do próprio CLI (cofre do sistema). O app não lê nem copia a credencial.
- **Fonte das métricas:** `%LOCALAPPDATA%\agy\bin\agy.exe -p /usage --output-format json`, modo documentado para scripts, que responde sem rodada do agente e sem gastar cota. Cada bucket vira uma cota: `agy-gemini-5h`, `agy-gemini-weekly`, `agy-3p-5h` e `agy-3p-weekly`.
- **Limitações:** a resposta não identifica a conta (sem alertas de consumo e renovação); cerca de 4 s por consulta; cadência de 10 min; cada execução deixa um log de falha vazio na pasta do CLI; não verificado se a IDE e o CLI compartilham o mesmo pool.
- **Interfaces internas:** nenhuma usada pelo app; quem consulta o serviço é o CLI oficial.
- **Proteções:** executável e argumentos fixos, timeout, limite de saída, cancelamento ao desativar, execução única e disjuntor que suspende a fonte se o CLI executar uma rodada do modelo.
- **Termos:** ver [Revisão de termos](cycle-03-1/provider-terms-review.md#google-antigravity) (seção 6 dos termos adicionais e a interpretação adotada).
- **Validação real:** 24/11/2026: percentuais, janelas e renovações idênticos à saída do `agy`.
- **Reconexão:** executar `agy` no terminal e entrar com a conta Google.

## Codex (OpenAI)

- **Modalidades suportadas:** login do Codex com conta ChatGPT (`codex login`). Login por chave de API é uma conta incompatível (`unsupported`), pois não tem janelas de plano.
- **Fonte de autenticação:** `auth.json` em `CODEX_HOME` (padrão `~/.codex`), somente leitura. Com `cli_auth_credentials_store = "keyring"` ou `"auto"`, a credencial fica no cofre do sistema, que o app não lê (`credentialsUnavailable`).
- **Fonte das métricas:** `GET https://chatgpt.com/backend-api/wham/usage`. Fallback: o registro `token_count` mais recente em `sessions/AAAA/MM/DD/rollout-*.jsonl`, sempre exibido como desatualizado e sem conta associada.
- **Cotas acessíveis:** `codex-primary` (janela de 5 h), `codex-secondary` (semanal), revisão de código, limites adicionais e limite de gastos (créditos).
- **Limitações:** o token expirado não é renovado pelo app; o Codex o renova na próxima execução.
- **Interfaces internas:** sim. O endpoint é interno e não documentado; é o mesmo que o cliente oficial usa.
- **Validação real:** 24/11/2026, conta Plus, leitura `api` com janela semanal e de 5 h.
- **Reconexão:** `codex login`.

## Claude Code (Anthropic)

- **Modalidades suportadas:** Claude Code autenticado no terminal (`claude`) com assinatura Pro ou Max. Chave de API e tokens só de inferência são incompatíveis (`unsupported`). Login apenas dentro do app Claude Desktop: `credentialsUnavailable`.
- **Fonte de autenticação:** `.credentials.json` em `CLAUDE_CONFIG_DIR` (padrão `~/.claude`), somente leitura; e-mail de `.claude.json`.
- **Fonte das métricas:** `GET https://api.anthropic.com/api/oauth/usage` com `anthropic-beta: oauth-2025-04-20`. A atividade local soma tokens das transcrições das últimas 5 h (não é cota).
- **Cotas acessíveis:** `fiveHour`, `sevenDay`, `sevenDayOpus`, `sevenDaySonnet`, `sevenDayOauthApps` e uso extra.
- **Limitações:** sem renovação de token pelo app. A atividade local é limitada por orçamento de leitura e pode ser parcial.
- **Interfaces internas:** sim, endpoint interno e não documentado.
- **Validação real:** 24/11/2026, conta Pro, leitura `api` com janelas de 5 h e semanal. A autenticação tinha sido renovada pelo proprietário via CLI.
- **Reconexão:** autenticar novamente pelo Claude Code CLI.

## Gemini (Google) — 0.3.0, substituído pelo Antigravity

### O que foi investigado

Código-fonte do cliente oficial ([google-gemini/gemini-cli](https://github.com/google-gemini/gemini-cli), ramo `main`):

- `packages/core/src/code_assist/server.ts`: `retrieveUserQuota` é um `POST https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota` com `{ project }`. A resposta traz `buckets[]` com `remainingAmount`, `remainingFraction`, `resetTime`, `tokenType` e `modelId`. É o que alimenta o `/stats` do CLI ("% used", "Limit resets in …", "Usage limits span all sessions and reset daily").
- `packages/core/src/code_assist/setup.ts`: o projeto vem de `loadCodeAssist` (`cloudaicompanionProject`) ou de `GOOGLE_CLOUD_PROJECT`. `onboardUser` só roda quando não há `currentTier`.
- `packages/core/src/code_assist/oauth2.ts` e `config/storage.ts`: o login com Google fica em `~/.gemini/oauth_creds.json` (texto puro por padrão; no cofre do sistema ou em arquivo cifrado com `GEMINI_FORCE_ENCRYPTED_FILE_STORAGE=true`). A conta ativa fica em `~/.gemini/google_accounts.json`. A pasta base pode ser movida com `GEMINI_CLI_HOME`.
- `packages/core/src/core/contentGenerator.ts`: modos `oauth-personal`, `gemini-api-key`, `vertex-ai`, `cloud-shell`, `compute-default-credentials` e `gateway`, gravados em `security.auth.selectedType` de `~/.gemini/settings.json`.
- `docs/resources/quota-and-pricing.md`: limites diários por modalidade. Conta Google (Code Assist individual): 1.000 requisições por dia. Google AI Pro: 1.500. Google AI Ultra: 2.000. Workspace Code Assist Standard e Enterprise: 1.500 e 2.000. Chave da Gemini API gratuita: 250, só Flash. Vertex AI e chave paga: variável, por projeto.

### Diferenças que importam

- **Cotas do Gemini CLI com conta Google** (Code Assist individual, Google AI Pro/Ultra, Workspace) são por usuário e por modelo, renovadas diariamente, e só aparecem pela API interna `cloudcode-pa`.
- **Limites da Gemini API** (chave do AI Studio) pertencem ao projeto da chave. Não existe endpoint oficial que exponha o consumo restante para a própria chave.
- **Limites de projetos Google Cloud / Vertex AI** são cotas de projeto, consultáveis pela Cloud Monitoring/Service Usage com credenciais de nuvem próprias.
- **Consumo de tokens** (por exemplo, o que o CLI mostra por sessão) não é cota restante. **Custos financeiros** são faturamento do projeto.

Nenhuma dessas grandezas é tratada como equivalente a outra.

### Bloqueio

O documento [`docs/resources/tos-privacy.md`](https://github.com/google-gemini/gemini-cli/blob/main/docs/resources/tos-privacy.md) do próprio Gemini CLI afirma que acessar diretamente os serviços por trás do CLI (por exemplo, o Gemini Code Assist) com software de terceiros, usando o OAuth do Gemini CLI, viola os termos aplicáveis e pode levar à suspensão ou ao encerramento da conta. Consultar `retrieveUserQuota` com o token do CLI a partir do Aurora Control seria exatamente isso. Por isso:

- **Modalidades suportadas:** detecção da sessão "Login with Google". Chave de API, Vertex AI e demais modos aparecem como conta incompatível.
- **Fonte de autenticação:** `settings.json` (método), presença de `oauth_creds.json` (o token não é usado nem guardado) e `google_accounts.json` (e-mail ativo, mascarado; troca de conta muda a identidade).
- **Fonte das métricas:** nenhuma. O app nunca contata o Google; o estado é `sourceBlocked: providerTerms`.
- **Cotas acessíveis:** nenhuma. A barra mostra "—" com um sinalizador neutro, e o resumo explica o motivo.
- **Interfaces internas:** nenhuma usada.
- **Validação real:** nesta máquina não há login do Gemini CLI (`~/.gemini` tem só `antigravity` e `config`); o app informou `authMissing`, que é o estado verdadeiro. A detecção de uma sessão Google só foi verificada com fixtures.
- **Riscos de contornar:** suspensão da conta Google do usuário; dependência de API interna.
- **Alternativas legítimas:** (1) mostrar os números do `/stats` dentro do próprio CLI; (2) ler, sem rede, estatísticas de tokens que o CLI grava localmente, como atividade e nunca como percentual de cota; (3) para chave de API ou Vertex, consultar cotas de projeto pela Cloud Monitoring com credenciais do próprio usuário e consentimento explícito; (4) uma API oficial de cotas, se o Google publicar uma.
- **Reconexão:** executar `gemini` e escolher "Login with Google".

## Grok (xAI)

### O que foi investigado

- **Grok Build**, o CLI oficial de código da xAI ([xai-org/grok-build](https://github.com/xai-org/grok-build), Apache-2.0, commit `2bdd1d6` de 29/09/2026). O login (`grok login`, OAuth em `auth.x.ai`) grava `~/.grok/auth.json` (ou `$GROK_HOME/auth.json`): um objeto por emissor e cliente, com `key`, `auth_mode` (`oidc`, `external`, `api_key`), `user_id`, `email`, `team_id`, `expires_at` (ou `create_time` mais 30 dias) e `oidc_issuer`. O CLI renova o token sozinho e relê o arquivo quando ele muda.
- O comando `/usage` do Grok Build (`crates/codegen/xai-grok-shell/src/extensions/billing.rs`) chama `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits` com `Authorization: Bearer <sessão>`, `X-XAI-Token-Auth: xai-grok-cli` e `x-userid`. A resposta traz `config.creditUsagePercent` (0–100 da franquia incluída), `config.currentPeriod` (`USAGE_PERIOD_TYPE_WEEKLY` ou mensal, com início e fim), gastos sob demanda e saldo pré-pago. É a franquia da **conta grok.com** (SuperGrok e planos afins), não um limite de chave de API.
- **API xAI** ([docs.x.ai](https://docs.x.ai/docs/management-api)): uso, saldo, limites de gastos e limites por modelo de uma **equipe** só pela Management API (`management-api.x.ai`), com chave de gerenciamento. Uma chave de API comum só consulta o próprio estado em `GET https://api.x.ai/v1/api-key` (o Grok Build usa isso para saber se a chave está bloqueada ou desativada), sem consumo nem saldo. Nada disso é cota de assinatura pessoal, e o app não apresenta como tal.
- **Planos do X (Premium/Premium+)** e o grok.com na web: nenhuma API documentada expõe a cota restante. Scraping e cookies estão fora de questão.

### Bloqueio

O endpoint do Grok Build é interno, e os termos de serviço da xAI (`x.ai/legal/terms-of-service`) responderam HTTP 403 durante a investigação. Não foi possível verificar se permitem que software de terceiros use a sessão do Grok Build. Até essa verificação, a consulta fica desligada:

- **Modalidades suportadas:** sessão do Grok Build emitida por `x.ai` (`oidc` ou `external`). Chave de API é conta incompatível.
- **Fonte de autenticação:** `~/.grok/auth.json`, somente leitura, até 256 KiB. Sessão expirada vira `authExpired`; arquivo ilegível vira `credentialsInvalid`.
- **Fonte das métricas:** implementada e testada com fixtures (`grok::parse_billing`), mas **desativada** por `grok::LIVE_QUERY_AUTHORIZED = false`. Sem essa permissão, o estado é `sourceBlocked: termsUnverified` e nenhuma requisição sai do app.
- **Cotas acessíveis (quando liberado):** `grok-credits` (franquia semanal ou mensal, com renovação no fim do período) e uso sob demanda como créditos.
- **Interfaces internas:** sim, `cli-chat-proxy.grok.com` é interno ao Grok Build.
- **Validação real:** nesta máquina não há Grok Build (`~/.grok` não existe); o app informou `authMissing`, o estado verdadeiro. A consulta real nunca foi executada.
- **Como liberar:** depois que o proprietário ler os termos da xAI e concluir que o uso é permitido, mudar `LIVE_QUERY_AUTHORIZED` para `true` em [`grok.rs`](../src-tauri/src/usage/grok.rs) e validar com uma conta real. O teste `without_verified_terms_the_session_is_read_but_xai_is_never_contacted` precisa ser ajustado na mesma revisão.
- **Riscos:** endpoint interno sujeito a mudanças; possível violação dos termos; a renovação do token continua sendo do Grok Build.
- **Alternativas legítimas:** o `/usage` do próprio Grok Build; a Management API para equipes que optarem por fornecer uma chave de gerenciamento (fora do escopo deste ciclo).
- **Reconexão:** `grok login`.

## Regras comuns

- Nenhum adaptador renova tokens, executa CLIs, grava arquivos dos provedores, lê cookies ou navegadores, ou acessa memória de processos.
- Credenciais são lidas no Rust, usadas apenas no cabeçalho da requisição e descartadas; nunca chegam ao WebView, aos logs ou às preferências.
- Provedores desativados não leem arquivos, não fazem requisições, não têm diagnóstico e não geram alertas.
- Cada provedor tem worker, cache, identidade, política de atualização e tratamento de falhas próprios; um pânico ou um `Retry-After` longo de um não afeta os outros.
