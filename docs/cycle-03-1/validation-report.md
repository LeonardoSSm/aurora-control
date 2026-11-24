# Ciclo 03.1 — Relatório de validação

Ambiente: Windows 11 Pro build 26300, um monitor de 1920 × 1080 a 100%, Node 24.15.0, Rust 1.97.1. A cópia instalada do proprietário (0.3.0) estava em uso durante toda a validação; a build de validação rodou ao lado dela (`AURORA_INSTANCE`), com preferências isoladas (`AURORA_CONFIG_DIR`).

Legenda: **Real** = validado no Windows com sessões reais; **Script** = validado no Windows com o app real em modo demo; **Teste** = coberto por teste automatizado; **N** = não verificado.

## Comparação com os clientes oficiais

Mesmo minuto, 24/11/2026:

| Provedor                     | Aurora Control                       | Cliente oficial                                             | Confere |
| ---------------------------- | ------------------------------------ | ----------------------------------------------------------- | ------- |
| Antigravity · Gemini 5 h     | 69% usado, renova 09:20:44 UTC       | `agy -p /usage`: "Five Hour Limit Remaining 31%", 09:20:44Z | Sim     |
| Antigravity · Gemini semanal | 18% usado, renova 01/12 04:20:44 UTC | "Weekly Limit Remaining 82%", 2026-12-01T04:20:44Z          | Sim     |
| Antigravity · Claude e GPT   | 0% (5 h e semanal)                   | "100%" restante nos dois                                    | Sim     |
| Codex                        | 77% semanal, 8% na de 5 h (API)      | Não comparado com o painel do Codex                         | N       |
| Claude Code                  | 99% na de 5 h, 40% semanal (API)     | Não comparado com o `/usage` do Claude Code                 | N       |
| Grok                         | Sessão detectada; fonte bloqueada    | —                                                           | —       |

Indicadores exibidos no mesmo momento: Codex "77", Claude Code "99", Antigravity "69" e Grok "—" com sinalizador de bloqueio (inspeção do DOM real da barra).

## Matriz pedida

| Item                            | Resultado                                                                                             |
| ------------------------------- | ----------------------------------------------------------------------------------------------------- |
| Detecção das sessões            | **Real**: Codex, Claude Code e Grok (`auth.json`); Antigravity pelo `agy.exe` instalado               |
| Correspondência dos percentuais | **Real** para Antigravity (tabela acima)                                                              |
| Correspondência das janelas     | **Real** para Antigravity (5 h e semanal, por grupo)                                                  |
| Horários de renovação           | **Real** para Antigravity (idênticos ao CLI, em UTC)                                                  |
| Troca de conta                  | **Teste** (Codex, Claude, Grok). Antigravity não tem identidade (ver D3). **N** com contas reais      |
| Sessão expirada                 | **Teste** (Grok, Claude, Codex)                                                                       |
| Ausência de autenticação        | **Teste** (CLI ausente, mensagens de login); **Real** na 0.3.0 para Gemini e Grok antes da instalação |
| Dados indisponíveis             | **Real**: Grok bloqueado; **Teste**: respostas malformadas e incompletas                              |
| Timeout                         | **Teste** (`failures_map_to_user_states`, `cancel_and_single_run_are_honored`)                        |
| Rate limiting                   | **Teste** (Grok 429/503 com `Retry-After`; agendador)                                                 |
| Desativação e reativação        | **Script**: "live change" (56/56); **Teste**: cancelamento do CLI ao desativar                        |
| Alertas                         | **Teste** (motor de alertas, 19 testes); **N** notificação real neste ciclo                           |
| Persistência das preferências   | **Script**: `validate-settings.ps1` 15/15 (arquivos 0.2.0 → v3), `validate-settings-ui.ps1` 23/23     |
| Quatro indicadores simultâneos  | **Real** (acima) e **Script** (0–4 indicadores, 56/56 à esquerda)                                     |

## Qualidade

| Verificação                                                | Resultado                                                                                                            |
| ---------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| `npm run format:check`, `npm run lint`, `npx tsc --noEmit` | Aprovados                                                                                                            |
| `npm test`                                                 | 50 aprovados                                                                                                         |
| `npm run build`                                            | Aprovado                                                                                                             |
| `cargo fmt --check`                                        | Aprovado                                                                                                             |
| `cargo clippy --locked --all-targets -- -D warnings`       | Aprovado                                                                                                             |
| `cargo test --locked`                                      | 188 aprovados, 2 ignorados                                                                                           |
| `cargo test --locked live_antigravity -- --ignored`        | Aprovado: 4 janelas em 4,6 s, origem CLI, sem rodada do modelo                                                       |
| `npm run desktop:debug`                                    | Aprovado                                                                                                             |
| `npm run desktop:build` (release + NSIS)                   | `Aurora Control_0.3.1_x64-setup.exe` gerado; não instalado, porque a cópia instalada estava em uso pelo proprietário |

## Regressão dos ciclos anteriores

Evidências em `docs/evidence/cycle-03-1-*.txt`:

| Script                              | Resultado |
| ----------------------------------- | --------- |
| `validate-providers.ps1 -Side left` | 56/56     |
| `validate-overlay.ps1 -Side left`   | 19/19     |
| `validate-settings.ps1`             | 15/15     |
| `validate-settings-ui.ps1`          | 23/23     |

A lateral esquerda foi usada nos testes de hover porque a barra do proprietário ocupava a direita.

## Não verificado

- IDE do Antigravity × CLI: mesmo pool de cota.
- Alertas reais e notificação nativa neste ciclo.
- Troca de conta real em qualquer provedor.
- Codex e Claude Code comparados lado a lado com os painéis oficiais.
- Grok: nenhuma métrica real (fonte bloqueada).
