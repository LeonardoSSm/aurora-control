# Auditoria de segurança — Ciclo 04 (1.0.0)

Data: 24/11/2026. Escopo: código da branch `feat/cycle-04-final-release`, dependências, configuração de produção, histórico Git e o instalador candidato. Nenhum segredo ou dado pessoal é reproduzido aqui.

## Resultado

| Classe      | Abertos | Observação                                                                                           |
| ----------- | ------- | ---------------------------------------------------------------------------------------------------- |
| Bloqueantes | 0       | Três encontrados neste ciclo, todos corrigidos (B1–B3)                                               |
| Importantes | 6       | Riscos aceitos ou dependentes do proprietário (I1–I6); 2 vulnerabilidades de dependências corrigidas |
| Melhorias   | 6       | Para ciclos de manutenção (M1–M6)                                                                    |

## Bloqueantes encontrados e corrigidos

- **B1. Ausência de prova tratada como zero consumo (Antigravity).** `parse_usage` usava `unwrap_or(0)` para `num_turns` e `usage.total_tokens`: um CLI que mudasse o formato, ou respondesse um prompt sem envelope com código 0, não disparava o disjuntor e voltaria a ser executado a cada 10 min. Corrigido: contadores obrigatórios; sem prova, o disjuntor dispara (`Unconfirmed`).
- **B2. Disjuntor só em memória.** Reiniciar o app (ou a inicialização com o Windows) voltava a executar um CLI que já tinha gastado cota. Corrigido: `safety-antigravity.json`, com falha segura (arquivo ilegível = suspenso) e reativação só por ação explícita do usuário.
- **B3. Subprocesso sem contenção da árvore.** Descendentes do CLI podiam manter os pipes abertos (o worker ficava preso no `join`), sobreviver ao timeout ou ao encerramento do app, e um desligar/religar rápido não invalidava a execução em andamento. Corrigido: Job Object com `KILL_ON_JOB_CLOSE`, época de cancelamento, `spawn` sob a mesma trava do `cancel`, leitores com tolerância. Testado com processos reais (`process::tests`).

## Importantes (abertos)

- **I1. Instalador sem assinatura de código.** `Get-AuthenticodeSignature`: `NotSigned`. Integridade só pelo SHA-256 publicado; o SmartScreen alerta. Requer certificado do proprietário.
- **I2. Termos do Antigravity.** A seção 6 dos termos adicionais proíbe software de terceiros que acesse o serviço (exemplo: OAuth do Antigravity em outro cliente). O app não acessa o serviço nem toca na credencial: executa o CLI oficial no modo headless documentado para scripts. O texto não exclui explicitamente esse uso. Mitigações: provedor desativado por padrão, cadência de 10 min, comando somente leitura, disjuntor persistente. Decisão do proprietário: manter ou desativar.
- **I3. Interfaces não documentadas.** Endpoints de Codex e Claude Code (internos, os mesmos dos clientes oficiais), o formato do `unified.jsonl` do Grok Build e o JSON do `agy`. Mudanças viram `malformed`/`awaitingClient`/disjuntor, nunca números inventados.
- **I4. Revisão independente pendente.** O Codex revisou `142f3b9` (PR #2) e `5a7d2b4` (PR #3); as correções `eca2a91` e `e7c06d6` e todo o PR #4 ainda não foram revisados.
- **I5. Janela de associação ao Job.** O processo entra no Job logo após o `spawn`; um filho criado nesse intervalo de microssegundos escaparia. Risco baixo (o `agy` leva centenas de milissegundos para iniciar).
- **I6. Antigravity sem identidade de conta.** Leituras sem conta verificada não geram alertas de consumo; uma troca de conta no CLI é percebida apenas na próxima consulta. Extrair a credencial para resolver isso foi descartado.

## Melhorias futuras

- **M1.** Assinatura de código (Authenticode) no pipeline de release.
- **M2.** `grok usage --json` como fonte primária quando o proprietário usar a daemon route do Grok Build.
- **M3.** Consulta direta ao Grok, se os termos da xAI ficarem legíveis e permitirem.
- **M4.** Associar o processo ao Job na criação (`PROC_THREAD_ATTRIBUTE_JOB_LIST`) quando a API estiver estável no Rust.
- **M6.** Substituir `chrono` (sem manutenção, `RUSTSEC-2026-0543`).
- **M5.** Memória do WebView2 (cerca de 190 MB privados em 7 processos, típico do runtime): avaliar desligar recursos não usados do runtime.

## Itens revisados

| Item                        | Conclusão                                                                                                                                                                                                                                                            |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Capabilities Tauri          | `rail`, `summary` e `settings` separadas; `reset_provider_safety` só em `settings`. `ipc_tests` roda o ACL real: cada janela só alcança os próprios comandos, janela desconhecida não alcança nenhum.                                                                |
| CSP                         | Inalterada: `default-src 'none'`, só `self` e IPC; sem `unsafe-*`, sem frames, sem formulários.                                                                                                                                                                      |
| Comandos IPC                | Tipos validados pelo serde (`Provider` é enum); nenhum aceita caminho, comando ou URL. O reset do disjuntor não executa nada por si; só pede nova leitura.                                                                                                           |
| Autenticação dos provedores | Somente leitura de arquivos dos clientes; sem renovação, cópia ou gravação de tokens. O Grok usa `auth.json` só para identidade (o token não é usado).                                                                                                               |
| Arquivos de credenciais     | Limites de tamanho mantidos (256 KiB no Grok); erros não expõem conteúdo.                                                                                                                                                                                            |
| Logs                        | `eprintln!` só com nomes de campo, causas e provedores; release sem console. `BillingRecord` tem `Debug` manual sem o usuário. Testes verificam que valores-sentinela nunca aparecem.                                                                                |
| Registro do Grok            | Leitura limitada (6 MiB, 750 ms, linhas ≤ 256 KiB); só percentuais e datas saem do adaptador; o `user_id` é comparado em memória.                                                                                                                                    |
| Subprocessos                | Um único executável (`agy.exe`), caminho e argumentos fixos, sem janela, sem stdin, sem entrada do WebView, dentro de Job Object.                                                                                                                                    |
| Concorrência                | `spawn` sob trava compartilhada com `cancel`; época; execução única por `try_lock`; teste de 4 threads × 50 consultas com 200 ciclos de desligar/religar sem execução simultânea.                                                                                    |
| Cache                       | Leituras presas à identidade da conta; registro do Grok de outra conta nunca exibido; cache de varredura por tamanho e data.                                                                                                                                         |
| Backoff e Retry-After       | Inalterados e testados; `awaitingClient` é estado local (2 min, sem rede).                                                                                                                                                                                           |
| Requisições externas        | Só `chatgpt.com/backend-api/wham/usage` e `api.anthropic.com/api/oauth/usage`. `cli-chat-proxy.grok.com` existe no código, desligado.                                                                                                                                |
| Dependências                | `npm audit` (inclusive desenvolvimento): 0 vulnerabilidades. `cargo audit`: 2 vulnerabilidades corrigidas por atualização de patch, 0 restantes (abaixo). Nenhuma dependência nova; só duas features do `windows-sys` (`Win32_Security`, `Win32_System_JobObjects`). |
| Configuração de produção    | `withGlobalTauri: false`; devtools não habilitado; `AURORA_CONFIG_DIR` e `AURORA_INSTANCE` ignorados em release; instalação por usuário, sem administrador.                                                                                                          |
| Histórico Git               | Varredura de padrões (chaves OpenAI/xAI/AWS/GitHub, JWT, `ya29.`, `refresh_token` real, e-mail do proprietário): 0 ocorrências.                                                                                                                                      |
| Artefatos de build          | Nada de `target/`, `dist/`, executáveis ou logs versionado. O instalador contém só `aurora-control.exe` e `uninstall.exe`.                                                                                                                                           |

### `cargo audit`

`cargo-audit` (RustSec) contra o `Cargo.lock`:

- **Primeira execução: 2 vulnerabilidades**, ambas com correção compatível e alcançáveis no binário Windows via Tauri:
  - `RUSTSEC-2026-0594`, `cookie` 0.18.1 (nonces de cookies privados podem se repetir em processos que fazem fork; o app não usa cookies privados nem fork no Windows) → **0.18.3**.
  - `RUSTSEC-2026-0592`, `url` 2.5.8 (estouro de pilha em `Url::origin()` com URLs `blob:` aninhadas) → **2.5.9** (com `form_urlencoded` 1.2.3).
  - Também atualizado: `tokio` 1.53.1 → **1.54.1** (`RUSTSEC-2026-0386`, construtores de `AsyncFd`, unsound; correção a partir de 1.53.3).
- **Depois das atualizações pontuais (`cargo update -p cookie -p url -p tokio`): 0 vulnerabilidades.** Clippy e os 217 testes Rust aprovados novamente; o instalador foi refeito depois disso.
- Avisos restantes (sem vulnerabilidade):
  - `chrono` 0.4.45, sem manutenção (`RUSTSEC-2026-0543`), dependência direta. Melhoria futura: avaliar substituto (por exemplo, `jiff`), com os testes de aritmética de tempo do `time.rs`.
  - `unic-*` 0.9.0, sem manutenção, via `tauri-utils` → `urlpattern`; depende do Tauri.
  - `glib` (unsound), `zbus_names`, `zvariant`, `zvariant_derive` e `proc-macro-error`: só no alvo Linux ou em tempo de build, fora do binário Windows.

## Termos dos provedores

| Documento                                                                  | Resultado (24/11/2026) | Efeito                                                                            |
| -------------------------------------------------------------------------- | ---------------------- | --------------------------------------------------------------------------------- |
| [xAI Terms of Service](https://x.ai/legal/terms-of-service)                | HTTP 403               | Não lido. Nenhuma autorização presumida; consulta direta desligada.               |
| [X Subscription Terms](https://x.ai/legal/x-subscription-terms-of-service) | HTTP 403               | Idem.                                                                             |
| [xAI Acceptable Use Policy](https://x.ai/legal/acceptable-use-policy)      | HTTP 403               | Idem.                                                                             |
| [Antigravity Additional Terms](https://antigravity.google/terms)           | Lido                   | Seção 6 (software de terceiros acessando o serviço). Ver I2.                      |
| [Cursor: modelos e uso](https://cursor.com/docs/models)                    | Lido                   | Confirma o pool semanal compartilhado entre Grok e Cursor e onde o uso é exibido. |

Bloqueios de acesso não foram contornados (sem troca de User-Agent nem espelhos). A fonte adotada para o Grok / Cursor é a **leitura passiva de um arquivo local** que o cliente oficial produz para o próprio usuário: não acessa o serviço, não usa credencial e não automatiza o cliente. A finalidade pessoal do app não foi tratada como autorização.

Não implementados, por decisão: extração de cookies, interceptação de tráfego, reutilização de tokens OAuth, scraping autenticado, falsificação de cliente, extensões privadas do ACP.
