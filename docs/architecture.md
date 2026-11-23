# Arquitetura do Aurora Control

## Divisão de responsabilidades

| Módulo                                               | Responsabilidade                                                             |
| ---------------------------------------------------- | ---------------------------------------------------------------------------- |
| `src/components/`, `src/rail.tsx`, `src/summary.tsx` | Indicadores semicirculares e cartão de resumo em React.                      |
| `src/desktop/railGeometry.ts`                        | Geometria lógica dos indicadores e do arco, compartilhada com `position.rs`. |
| `src/desktop/useRailMotion.ts`                       | Animação do deslizamento, sincronizada quadro a quadro com a região Win32.   |
| `src/desktop/commands.ts`                            | Ponte tipada entre React e o IPC do Tauri.                                   |
| `src/usage/`                                         | Contrato `Usage` em TypeScript e textos de apresentação.                     |
| `src-tauri/src/usage/`                               | Contrato serializado, adaptadores, transporte HTTP, agendador e serviço.     |
| `src-tauri/src/windows/`                             | Estado do hover, geometria física, regiões e posicionamento Win32.           |
| `src-tauri/src/tray.rs`                              | Ícone e ações da bandeja: atualizar, mostrar/ocultar, lateral e encerrar.    |

## Indicadores

Cada provedor é um círculo de 52 px lógicos. Recolhido, o centro fica exatamente na borda da área útil, e só a metade voltada para dentro aparece. No hover, o círculo desliza 32 px para dentro e fica inteiro a 6 px da borda. A janela `rail` mede 60 × 118 px lógicos e é escalada pelo fator do monitor.

O progresso usa um medidor semicircular, não um anel de 360° recortado. O arco percorre a metade voltada para dentro, de baixo para cima: 0% não desenha nada, 50% chega ao ponto mais interno e 100% chega ao topo. `arcPath` desenha um arco SVG real até o ponto exato da fração, dividido no meio para nenhum segmento cobrir 180°. Os dois lados são espelhados (`sweep` 1 à direita, 0 à esquerda). Expandido, o círculo mostra o arco em "C" na mesma metade, o percentual e a duração da janela principal (`5 h`, `7 d`).

O arco sempre usa a cor do provedor. O estado crítico (a partir de 90%) aparece no número e no aro, sem trocar a cor que identifica o provedor. Dado desatualizado usa o arco esmaecido e o número em cinza. Indisponível mostra só o trilho e um travessão.

## Janelas, foco e hover

As janelas `rail` e `summary` são criadas ocultas, sem decoração e não focalizáveis. O código nativo aplica `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` e remove `WS_EX_APPWINDOW`, para que fiquem fora do Alt+Tab e o Win+D não as minimize. `SetWindowPos` usa `HWND_TOPMOST` e `SWP_NOACTIVATE`. Se algo externo minimizar a barra, ela é restaurada com `SW_SHOWNOACTIVATE`; só a bandeja a oculta.

A região Win32 da barra é a união dos dois círculos, cada um com 1 px de folga em relação ao desenho. Assim, o serrilhado do recorte GDI cai em pixels transparentes e a borda visível continua suave. Fora dos círculos, os cliques passam para o aplicativo de trás. A cada quadro da animação, o React envia os deslocamentos a `set_rail_motion`, e só desenha o que o Rust aceitou, com uma atualização nativa em andamento por vez.

O estado do hover pertence ao Rust (`windows/hover.rs`):

- A barra informa o indicador sob o ponteiro (`rail_pointer`).
- Abrir exige 150 ms de permanência, e trocar de indicador exige 120 ms. Medição no Windows 11: um círculo de 52 px é atravessado em cerca de 100 ms a 500 px/s, então varreduras rápidas pela borda não abrem nada.
- O fechamento depende da posição real do cursor (`GetCursorPos`), lida a cada 80 ms enquanto há um indicador ativo. O painel fica aberto no círculo expandido, no cartão e na ponte entre eles, e fecha após 240 ms fora dessa zona. Isso também recupera eventos `pointerleave` que o WebView2 não entrega.
- O resumo ignora o ponteiro (`WS_EX_TRANSPARENT`): o cursor pode repousar nele, mas os cliques vão para o aplicativo de trás.

O handshake do resumo continua por revisão: o `summary` sinaliza que está pronto, o Rust envia a seleção, o React renderiza e chama `show_summary(revision, height)` com a altura do conteúdo. O Rust só mostra a janela se a revisão ainda for a atual. Uma atualização de dados republica a mesma revisão: o cartão é re-renderizado no lugar, sem repetir a animação e sem mover a barra.

`AURORA_SIDE=left` inicia na lateral esquerda. A bandeja troca o lado durante a execução.

## Contrato de utilização

`Usage` em `src/usage/model.ts` espelha `src-tauri/src/usage/model.rs` (campos em `camelCase`). Nenhum campo carrega credenciais.

| Campo             | Semântica                                                                                                                                                                              |
| ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `provider`        | `codex` ou `claudeCode`.                                                                                                                                                               |
| `account`, `plan` | Rótulos de exibição. E-mails chegam mascarados (`d•••@dominio`).                                                                                                                       |
| `source`          | `demo` ou `live`.                                                                                                                                                                      |
| `origin`          | De onde veio a última leitura válida: `api`, `sessionLog` (registro local do próprio cliente) ou `fixture`.                                                                            |
| `status`          | `available`, `stale` ou `unavailable`.                                                                                                                                                 |
| `percentUsed`     | Percentual **consumido** da métrica principal, de 0 a 100. Presente em `available` e `stale`.                                                                                          |
| `window`          | A métrica principal, identificada por `id` e `label`, com duração e renovação.                                                                                                         |
| `quotas`          | As demais cotas (`window` ou `credits`), sem a principal.                                                                                                                              |
| `activity`        | Tokens das sessões locais nas últimas 5 h. **Não é cota da conta.**                                                                                                                    |
| `updatedAt`       | Momento da observação da última leitura válida.                                                                                                                                        |
| `checkedAt`       | Última tentativa, bem-sucedida ou não.                                                                                                                                                 |
| `issue`           | Causa da falta de leitura atual: `authMissing`, `authExpired`, `authRejected`, `unsupported`, `rateLimited` (com `retryAt`), `network`, `timeout`, `malformed`, `http` (com `status`). |
| `refreshing`      | Há uma consulta em andamento.                                                                                                                                                          |

A métrica principal é a janela mais restritiva, ou seja, a de maior percentual consumido. Em empate, vence a mais curta. Créditos nunca são a principal. Exemplo real: com 0% na janela de 5 h e 73% na semanal, o círculo mostra 73% e o rótulo `7 d`.

## Adaptadores

`UsageAdapter::fetch(now)` devolve um `Outcome`: o resultado (`Fetched` normalizado ou `FetchError`), um eventual fallback local, rótulos de conta e plano, e a atividade local. Adicionar um provedor exige um adaptador e um identificador em `Provider`. Os componentes visuais não mudam.

O transporte HTTP é o trait `Transport`. A implementação de produção usa `ureq` com TLS do sistema (Schannel, via `native-tls`), timeout global de 15 s, sem redirecionamentos e com corpo limitado a 1 MiB. Os testes injetam respostas roteirizadas. Cabeçalhos de requisição nunca são registrados.

### Codex

- Autenticação: `auth.json` em `CODEX_HOME` (padrão `~/.codex`), somente leitura. Lê `tokens.access_token` e `tokens.account_id`, além de e-mail e plano no `id_token`. Esses valores são decodificados sem verificação de assinatura e servem apenas para exibição.
- Login por chave de API: `unsupported`, porque não há janelas de plano.
- Token expirado (`exp` do JWT): não é enviado. O Codex o renova na próxima execução.
- Fonte principal: `GET https://chatgpt.com/backend-api/wham/usage` com `Authorization` e `ChatGPT-Account-Id`. Lê `rate_limit.primary_window` e `secondary_window` (`used_percent` de 0 a 100, `limit_window_seconds`, `reset_at` em epoch ou `reset_after_seconds`), `code_review_rate_limit`, `additional_rate_limits[]` e `spend_control.individual_limit`.
- Fallback: o registro `token_count` com `rate_limits` mais recente que o próprio Codex grava em `sessions/AAAA/MM/DD/rollout-*.jsonl`. São lidos os últimos 512 KiB dos 6 arquivos mais novos, com idade máxima de 8 dias. Essa leitura sempre aparece como `stale` com origem `sessionLog`.

### Claude Code

- Autenticação: `.credentials.json` em `CLAUDE_CONFIG_DIR` (padrão `~/.claude`), chave `claudeAiOauth`, somente leitura. O e-mail vem de `oauthAccount.emailAddress` em `.claude.json`.
- Token vazio ou ausente: `authMissing`. Expirado (`expiresAt`, em ms): `authExpired`. Sem o escopo `user:profile` (token só de inferência): `unsupported`. Login por chave de API (`primaryApiKey`, só a presença): `unsupported`, porque custos de API não têm fonte local.
- Fonte: `GET https://api.anthropic.com/api/oauth/usage` com `anthropic-beta: oauth-2025-04-20`. Lê os objetos com `utilization` (0 a 100) e `resets_at`: `five_hour`, `seven_day`, `seven_day_opus`, `seven_day_sonnet`, `seven_day_oauth_apps` e chaves novas no mesmo formato. `extra_usage` habilitado vira cota `credits`.
- Atividade local: soma `input_tokens + cache_creation_input_tokens + output_tokens` das mensagens do assistente gravadas nas transcrições de `projects/*/*.jsonl` nas últimas 5 h. Conta cada `message.id` uma vez e exclui leituras de cache. Só arquivos modificados no período são lidos, até 8 MiB do final de cada um.
- O adaptador não executa o CLI, não renova tokens, não instala hooks e não altera configurações. Ele só lê arquivos, com o modo de compartilhamento padrão do Windows, então o Remote Control e as sessões em execução não são afetados.

### Riscos de compatibilidade

Os dois endpoints são **internos e não documentados**. Eles são os mesmos que os clientes oficiais usam para exibir seus limites, e podem mudar sem aviso. Todo conhecimento de schema está confinado a `parse_usage` em `codex.rs` e `claude.rs`. Uma mudança de formato vira `malformed`: o último dado válido continua visível como desatualizado, e a interface informa que o formato pode ter mudado. Não há API pública equivalente para cotas de assinatura.

## Atualização

Há um worker por provedor (`usage/mod.rs`) e a política fica em `usage/scheduler.rs`, que é lógica pura com relógio injetado:

| Regra                         | Valor                                                                                                                           |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| Intervalo regular             | 5 min, com a primeira consulta na inicialização.                                                                                |
| Atualização manual            | Bandeja ("Atualizar agora") ou clique no indicador. Intervalo mínimo de 20 s.                                                   |
| Concorrência                  | No máximo uma consulta por provedor. O IPC lê apenas o cache.                                                                   |
| Falha de rede, timeout, 5xx   | Backoff de 1, 2, 4, 8, 16 e no máximo 30 min.                                                                                   |
| HTTP 429                      | Espera `max(Retry-After, backoff, 60 s)`, que vale também para a atualização manual. `Retry-After` aceita segundos e data HTTP. |
| 401/403 e resposta malformada | Espera `max(backoff, 5 min)`.                                                                                                   |
| Login ausente ou expirado     | Nova verificação local a cada 2 min, sem rede.                                                                                  |
| Desatualizado                 | Leitura com mais de 15 min, ou última tentativa falhou com leitura anterior válida.                                             |
| Janela renovada               | Uma janela armazenada cujo `resets_at` passou é exibida com 0% e renovação desconhecida.                                        |

Cada mudança de estado emite `usage-updated` para a barra e republica o resumo aberto.

## Segurança e empacotamento

O Vite gera duas páginas estáticas, que o Tauri serve pelo protocolo interno do WebView2, sem servidor HTTP local. A CSP parte de `default-src 'none'` e só libera a própria origem e o IPC. Cada janela navega apenas para a própria página. `withGlobalTauri` continua desativado. As capabilities separam `rail` (`get-usage`, `refresh-usage`, `get-rail-placement`, `rail-pointer`, `set-rail-motion`) de `summary` (`summary-ready`, `show-summary`).

As credenciais são lidas no Rust a cada consulta, usadas só no cabeçalho `Authorization` e descartadas. Elas nunca são gravadas, copiadas, registradas em log ou enviadas ao WebView. O app não executa processos externos. [Capabilities do Tauri](https://v2.tauri.app/security/capabilities/)
