# Ciclo 03.1 — Relatório de implementação

Versão 0.3.1. Branch `feat/cycle-03-1-provider-usage`, criada a partir de `feat/cycle-03-multi-provider` (`eca2a91`, PR #2 ainda aberto). O PR deste ciclo aponta para essa branch, para não conflitar com o PR #2.

## Resultado por provedor

| Provedor    | Fonte                                    | Classificação                         | Situação                                                          |
| ----------- | ---------------------------------------- | ------------------------------------- | ----------------------------------------------------------------- |
| Codex       | `chatgpt.com/backend-api/wham/usage`     | Endpoint do cliente oficial (interno) | Inalterado; dados reais validados                                 |
| Claude Code | `api.anthropic.com/api/oauth/usage`      | Endpoint do cliente oficial (interno) | Inalterado; dados reais validados                                 |
| Antigravity | `agy -p /usage --output-format json`     | **CLI oficial**                       | **Novo; dados reais validados e conferidos com o cliente**        |
| Grok        | (`cli-chat-proxy.grok.com` implementado) | **Fonte bloqueada**                   | Sessão detectada; consulta desligada até a verificação dos termos |

O Cursor não foi integrado: não há fonte programática pessoal (ver [Uso do Cursor](cursor-usage-research.md)).

## Mudanças

### Backend (Rust)

- `usage/antigravity.rs` (novo): adaptador e executor (`Runner`) do `agy`, com todos os controles listados em [Decisões de arquitetura](architecture-decisions.md#d2-fonte-do-antigravity-cli-oficial-em-modo-não-interativo). Normaliza cada bucket em uma cota (`agy-gemini-5h`, `agy-gemini-weekly`, `agy-3p-5h`, `agy-3p-weekly`), com percentual usado = `1 − remaining_fraction`, duração (5 h ou semanal) e renovação plausível. Janelas desconhecidas são listadas, mas nunca viram o indicador automaticamente.
- `usage/gemini.rs` removido; `Provider::Gemini` → `Provider::Antigravity`; `BlockReason::ProviderTerms` removido e `CliSafety` adicionado; `Origin::Cli` adicionado.
- `UsageAdapter::cancel` (novo, padrão vazio): `Service::set_visible` o chama ao desativar um provedor.
- Catálogo: entrada `antigravity` (`integration: cli`, cadência de 600 s); `Integration::AuthOnly` substituído por `Cli`.
- Preferências versão 3, com migração v2 → v3 (e a cadeia v1 → v2 → v3).
- `instance::name()`: sufixo `AURORA_INSTANCE` só em builds de depuração.
- `settings.rs`: `ProviderView.origin`.
- Versão 0.3.1.

### Frontend

- `Provider` com `antigravity`; textos de sessão, conta incompatível e bloqueio por provedor; `originText` e a linha "Fonte: …" no resumo e nas configurações; nota de integração "Cotas reais pelo CLI oficial"; o sinalizador trata `cliSafety` como problema (âmbar) e `termsUnverified` como bloqueio por desenho.
- Identidade visual `--antigravity` e `--antigravity-2` (azul → violeta).

### Scripts

Todos os scripts de validação definem `AURORA_INSTANCE=validation` e seguem a versão 3 das preferências.

## Testes adicionados

| Teste                                                                           | Cobre                                                                                                        |
| ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `antigravity::the_real_report_becomes_four_windows_in_two_groups`               | Fixture com o formato real do CLI 1.5.16 (sanitizada); percentuais, rótulos, renovações e métrica automática |
| `antigravity::a_successful_run_is_a_cli_reading_without_an_account`             | Origem `cli`, sem identidade                                                                                 |
| `antigravity::a_model_turn_trips_the_breaker_and_stops_all_later_runs`          | Disjuntor por `num_turns` ou `total_tokens`; nenhuma execução depois                                         |
| `antigravity::a_prompt_answered_by_the_model_is_never_a_report`                 | O caso real do incidente do Git Bash                                                                         |
| `antigravity::incomplete_malformed_and_out_of_range_reports_are_rejected`       | JSON inválido, comando errado, grupos vazios, fração fora de 0–1, bucket desativado, renovação implausível   |
| `antigravity::failures_map_to_user_states`                                      | Saída 2, mensagens de login, timeout, cancelamento, falha ao iniciar, saída grande, executável ausente       |
| `antigravity::without_the_cli_the_session_is_signed_out_and_nothing_runs`       | Sem `agy.exe`: sessão encerrada, nenhum processo                                                             |
| `antigravity::the_arguments_are_fixed_and_keep_slash_commands_on`               | Argumentos constantes                                                                                        |
| `antigravity::drain_stops_collecting_past_the_limit`                            | Limite de saída                                                                                              |
| `antigravity::cancel_and_single_run_are_honored`                                | Cancelamento e execução única                                                                                |
| `antigravity::live_antigravity_cli` (`#[ignore]`)                               | Execução real opcional do `agy`                                                                              |
| `preferences::a_version_2_file_becomes_antigravity_disabled_and_keeps_the_rest` | Migração v2 → v3                                                                                             |
| `instance::only_a_short_plain_suffix_changes_the_name`                          | Sufixo de instância                                                                                          |
| `providers.test.tsx`, `settings.test.tsx`                                       | Catálogo com Antigravity, textos novos, sinalizador, linha "Fonte: …"                                        |

Totais: 188 testes Rust aprovados e 2 ignorados (eram 185 e 1); 50 testes de frontend (eram 49).

## Segurança

- Nenhum token chega ao frontend: o Antigravity não tem token no app, e o Grok continua lendo o `auth.json` só no Rust, sem rede.
- Nenhuma saída do CLI vai para logs; só o caminho de campos anômalos.
- Provedores desativados não executam o CLI. Desativar durante uma execução encerra o processo.
- Cadência mínima de 10 minutos para o CLI; as regras de recuperação (backoff, portão manual) continuam valendo.
- Nenhuma opção para ignorar restrições foi introduzida.

## Limitações

- Antigravity sem identidade de conta: sem alertas de consumo ou renovação para ele (ver D3).
- Não foi verificado se a IDE e o CLI compartilham exatamente o mesmo pool.
- Cada execução deixa um log de falha vazio na pasta do CLI (até 144 por dia).
- O Grok continua sem métricas reais.
- Durante a investigação, uma execução do `agy` disparada pelo Git Bash consumiu uma rodada do modelo (~23 mil tokens no total). Ver [Investigação do Antigravity](antigravity-research.md#incidente-durante-a-investigação).
