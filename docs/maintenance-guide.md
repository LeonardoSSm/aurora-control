# Guia de manutenção — Aurora Control

Para quem assume o projeto depois da 1.0.0. Complementa a [Arquitetura](architecture.md), a [Compatibilidade dos provedores](provider-compatibility.md) e a [Passagem de manutenção](cycle-04/final-handoff.md).

## Mapa rápido

| Área                     | Onde                                                                                  |
| ------------------------ | ------------------------------------------------------------------------------------- |
| Catálogo de provedores   | `src/usage/catalog.json` (Rust e TypeScript leem o mesmo arquivo)                     |
| Adaptadores              | `src-tauri/src/usage/{codex,claude,antigravity,grok}.rs`                              |
| Registro local do Grok   | `src-tauri/src/usage/grok_log.rs`                                                     |
| Subprocessos e disjuntor | `src-tauri/src/usage/process.rs`, `src-tauri/src/usage/safety.rs`                     |
| Agendamento e frescor    | `src-tauri/src/usage/scheduler.rs` (lógica pura, relógio injetado)                    |
| Contrato com o WebView   | `src-tauri/src/usage/model.rs` ↔ `src/usage/model.ts`                                 |
| Textos da interface      | `src/usage/presentation.ts`                                                           |
| Alertas                  | `src-tauri/src/alerts/`, regras em [Motor de notificações](notification-engine.md)    |
| Preferências e migrações | `src-tauri/src/preferences.rs` (formato atual: v3)                                    |
| Permissões IPC           | `src-tauri/build.rs`, `src-tauri/capabilities/*.json`, teste `ipc_tests` em `main.rs` |
| Validações interativas   | `scripts/validate-*.ps1`, `scripts/capture-toast.ps1`                                 |

## Verificação completa (antes de qualquer PR)

```powershell
npm ci
npm run format:check
npm run lint
npx tsc --noEmit
npm test
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml
npm run desktop:build
```

Testes com dados reais (opcionais, só na máquina do proprietário; imprimem apenas dados normalizados):

```powershell
cargo test --locked --manifest-path src-tauri/Cargo.toml live_grok_log -- --ignored --nocapture
cargo test --locked --manifest-path src-tauri/Cargo.toml live_antigravity -- --ignored --nocapture
```

Validações interativas (sessão de desktop; isolam preferências com `AURORA_CONFIG_DIR` e a instância com `AURORA_INSTANCE`, que só valem em build de depuração):

```powershell
npm run desktop:debug
pwsh -File scripts/validate-providers.ps1 -Exe src-tauri/target/debug/aurora-control.exe -Side right
pwsh -File scripts/validate-overlay.ps1 -Exe src-tauri/target/debug/aurora-control.exe -Side right
pwsh -File scripts/validate-settings.ps1 -Exe src-tauri/target/debug/aurora-control.exe
pwsh -File scripts/validate-settings-ui.ps1 -Exe src-tauri/target/debug/aurora-control.exe
```

Cuidados: `$env:APPDATA` **não** isola as preferências (o Tauri usa a API de pastas conhecidas); use sempre `AURORA_CONFIG_DIR`. Uma cópia de validação esquecida em execução segura o mutex de instância única e faz o executável de depuração sair em silêncio: confira `Get-Process aurora-control` antes.

## Quando um provedor muda

| Sintoma                                                 | Onde olhar                                                                                                                                                                   |
| ------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `malformed` constante em Codex ou Claude Code           | `parse_usage` em `codex.rs`/`claude.rs`; compare com a resposta atual do cliente oficial                                                                                     |
| Antigravity suspenso por segurança após atualizar o CLI | Rode `agy -p /usage --output-format json` à mão; confira `num_turns`, `usage.total_tokens`, `command.name`; ajuste `parse_usage` e os testes antes de reativar               |
| Grok / Cursor em `awaitingClient` mesmo com uso         | Procure `billing: fetched credits config` e `auth init user_info check` em `~/.grok/logs/unified.jsonl` (o texto das mensagens pode ter mudado); constantes em `grok_log.rs` |
| Grok / Cursor em `malformed`                            | Formato do `config` mudou; `grok::parse_response`                                                                                                                            |

Regra geral: um formato desconhecido nunca vira número. Prefira `malformed`/`awaitingClient` a adivinhar.

## Adicionar um provedor

1. Variante em `Provider` (`model.rs`) e em `Provider` (`catalog.ts`), mais a entrada em `catalog.json` (nome, fornecedor, `defaultEnabled: false`, cadência, integração, capacidades, texto de reconexão).
2. Adaptador que implemente `UsageAdapter`: `fetch` (nunca bloqueia indefinidamente), `session` (só arquivos locais), `diagnostics` (sem valores sensíveis). Identidade de conta por `Session::account`, nunca por rótulo de exibição. Se o adaptador executar processos, use `usage/process.rs` e um `safety::Breaker`.
3. Registre-o em `Service::from_environment`.
4. Cores em `src/styles.css` (o teste de catálogo exige) e textos em `presentation.ts` (`missingText`, `expiredText`, `unavailableText`, `unsupportedText`, `awaitingText`).
5. Preferências: um provedor novo entra desativado; se mudar o formato, crie a migração em `preferences.rs` com teste (v3 → v4) e preserve todas as escolhas existentes.
6. Antes de usar qualquer endpoint ou credencial, leia os termos do provedor e registre a conclusão. Na dúvida, mantenha a fonte desligada (padrão `LIVE_QUERY_AUTHORIZED` do Grok).

## Atualizar dependências

- JavaScript: `npm outdated`; atualize com `npm install <pacote>@<versão>` e rode a verificação completa. `npm audit` deve terminar com 0 vulnerabilidades.
- Rust: `cargo update -p <crate>` (prefira atualizações pontuais a `cargo update` geral); `cargo audit` (instale com `cargo install cargo-audit --locked`). Tauri: mantenha `tauri`, `tauri-build` e `tauri-plugin-notification` na mesma linha menor e confira o changelog de segurança.
- GitHub Actions: as ações estão fixadas por SHA; atualize o SHA e o comentário da tag juntos.
- Depois de atualizar o Tauri, rode `validate-settings-ui.ps1` e o teste `ipc_tests` (o ACL muda entre versões).

## Gerar um instalador

1. Atualize a versão em `package.json`, `package-lock.json` (as duas primeiras ocorrências), `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` (`cargo update -p aurora-control --offline`) e `src-tauri/tauri.conf.json`.
2. Atualize `CHANGELOG.md`.
3. `npm run desktop:build`. O instalador fica em `src-tauri/target/release/bundle/nsis/Aurora Control_<versão>_x64-setup.exe`.
4. `Get-FileHash <instalador> -Algorithm SHA256` e `Get-AuthenticodeSignature <instalador>`; publique o hash junto.
5. Teste atualização sobre a versão anterior, instalação limpa e desinstalação (o instalador silencioso `/S` **encerra** qualquer `aurora-control.exe` do usuário; a desinstalação remove o valor de inicialização com o Windows).
6. Release no GitHub em rascunho; publique só depois da aprovação do proprietário.

## Roteiro de auditoria independente

1. Ler `docs/cycle-04/security-audit.md` e `architecture-decisions.md`.
2. Conferir que nenhum adaptador envia dados além dos endpoints documentados (`grep -rn "https://" src-tauri/src`).
3. `grok_log.rs`: limites de leitura, linhas parciais, identidade por `pid`, nenhum conteúdo bruto fora do módulo.
4. `process.rs`: ordem trava → verificação → `spawn` → Job; comportamento de `cancel` durante e antes da execução; testes reais em `process::tests`.
5. `safety.rs` e `antigravity.rs::parse_usage`: o disjuntor dispara para toda saída que não prove zero rodadas.
6. Capabilities e `ipc_tests`: `reset_provider_safety` só na janela de configurações.
7. Rodar a verificação completa e os testes reais opcionais.
