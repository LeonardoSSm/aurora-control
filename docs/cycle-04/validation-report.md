# Relatório de validação do Ciclo 04

Data: 24/11/2026. Máquina do proprietário: Windows 11 Pro (26300), 8 núcleos lógicos, Node.js 24.15.0, Rust estável. "Real" significa dados de contas autenticadas desta máquina; "fixture" significa dados sintéticos de teste.

## Verificações automatizadas

| Verificação                              | Resultado                                                                                             |
| ---------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| Prettier (`format:check`)                | Aprovado                                                                                              |
| ESLint                                   | Aprovado, 0 avisos                                                                                    |
| TypeScript (`tsc --noEmit`)              | Aprovado                                                                                              |
| Testes de frontend (Vitest)              | **51 aprovados** (50 na partida)                                                                      |
| Build de frontend                        | Aprovado                                                                                              |
| `cargo fmt --check`                      | Aprovado                                                                                              |
| `cargo clippy --all-targets -D warnings` | Aprovado                                                                                              |
| Testes Rust (`cargo test --locked`)      | **217 aprovados**, 3 ignorados (testes opcionais com dados reais)                                     |
| Build desktop de depuração               | Aprovado                                                                                              |
| Build de produção + NSIS                 | `Aurora Control_1.0.0_x64-setup.exe`                                                                  |
| CI Windows do PR #4                      | Executada a cada push no PR #4; o resultado final fica registrado no PR e no relatório final do ciclo |

### Testes novos (fixtures, salvo indicação)

| Cenário pedido                     | Testes                                                                                                                                                                                                                                                   |
| ---------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Logs locais válidos e inválidos    | `grok_log::the_newest_record_is_attributed_to_the_identity_of_its_own_process`, `partial_malformed_and_foreign_lines_are_skipped`, `crlf_line_endings_are_accepted`                                                                                      |
| JSONL parcialmente escrito         | `partial_malformed_and_foreign_lines_are_skipped` (última linha cortada ao meio, sem `\n`)                                                                                                                                                               |
| Arquivos extensos                  | `a_large_log_is_read_only_up_to_the_scan_limit` (6,5 MiB), `records_across_chunk_boundaries_and_oversized_lines`                                                                                                                                         |
| Rotação de logs                    | `a_log_trimmed_or_replaced_between_scans_is_read_again`, `grok::a_new_record_is_picked_up_and_an_unchanged_log_is_not_rescanned`                                                                                                                         |
| Dados antigos / períodos expirados | `grok::a_record_of_an_expired_period_is_rejected`; envelhecimento coberto pelo `scheduler` (15 min)                                                                                                                                                      |
| Troca de conta                     | `grok::a_record_of_another_account_is_never_shown_as_the_current_one`, `grok_log::a_reused_pid_takes_the_nearest_earlier_identity`, `grok::signing_out_hides_the_record_even_if_the_log_remains`                                                         |
| Ausência de identidade             | `grok::a_record_without_an_identity_is_not_attributed`, `grok_log::missing_empty_and_unattributed_logs`                                                                                                                                                  |
| Franquia compartilhada             | `grok::the_local_record_of_the_signed_in_account_is_the_shared_weekly_allowance`, `without_unified_billing_the_window_is_not_called_shared`                                                                                                              |
| Ausência ≠ 0%                      | `grok::no_log_or_no_record_means_no_reading_not_zero_percent`                                                                                                                                                                                            |
| Cancelamento de subprocessos       | `process::a_cancel_before_the_spawn_starts_nothing`, `a_cancel_during_the_run_kills_it_even_if_enabled_again_at_once`, `the_timeout_kills_the_whole_tree`, `a_descendant_holding_the_pipes_does_not_hold_the_run` (**processos reais** `cmd.exe`/`ping`) |
| Desativação e reativação rápidas   | `antigravity::rapid_disable_and_enable_cycles_never_run_two_clis`, `process::a_cancel_during_the_run_kills_it_even_if_enabled_again_at_once`                                                                                                             |
| Persistência do disjuntor          | `safety::a_trip_survives_a_restart_until_the_user_resets_it`, `an_unreadable_or_foreign_file_keeps_the_source_suspended`, `antigravity::the_breaker_survives_a_restart_and_only_an_explicit_reset_lifts_it`                                              |
| Respostas sem prova de não consumo | `antigravity::an_answer_that_cannot_prove_the_absence_of_a_turn_trips_the_breaker`, `a_confirmed_report_without_usable_data_is_malformed_not_a_trip`                                                                                                     |
| Migração das preferências          | Testes existentes `preferences::a_version_1_file_is_migrated_keeping_every_choice`, `a_version_2_file_becomes_antigravity_disabled_and_keeps_the_rest` (o identificador `grok` não mudou) e validação real abaixo                                        |
| ACL do novo comando                | `main::ipc_tests::each_window_reaches_only_its_own_commands`                                                                                                                                                                                             |
| Interface                          | `settings.test.tsx`: reativação em duas etapas, estado `awaitingClient`; `providers.test.tsx`: nome "Grok / Cursor", marcador `waiting`                                                                                                                  |

## Validação com dados reais

| Provedor      | Como                                                     | Resultado                                                                                                                                                                                                                |
| ------------- | -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Grok / Cursor | `live_grok_log` e app instalado (release)                | 100% da franquia semanal compartilhada, período 20/11 03:30 → 27/11 03:30 UTC, observado 11:37:24 UTC, plano SuperGrok Lite, conta atribuída; varredura em 4 ms. Idêntico ao último `/usage` registrado pelo Grok Build. |
| Antigravity   | `live_antigravity` (executor de produção com Job Object) | 4 janelas (Gemini semanal 24%, Gemini 5 h 0%, Claude e GPT semanal 0%, 5 h 0%), 4,8 s, sem rodada do modelo (disjuntor intacto), **nenhum processo `agy`/`node` remanescente**.                                          |
| Codex         | App instalado (release), preferências reais              | 77% semanal, plano Plus, origem API.                                                                                                                                                                                     |
| Claude Code   | App instalado (release), preferências reais              | 73% na sessão de 5 h, plano Pro, origem API.                                                                                                                                                                             |

Não comparado diretamente: o percentual do Grok / Cursor com a tela do Cursor (não instalado) e com `grok.com/dashboard` (exige sessão de navegador). Codex e Claude Code foram conferidos com os clientes oficiais nos ciclos anteriores; neste ciclo não houve mudança nesses adaptadores.

## Validação interativa (build de depuração, preferências e instância isoladas)

| Script                               | Resultado | Evidência                                                                                                                                                |
| ------------------------------------ | --------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `validate-providers.ps1 -Side right` | 56/56     | [cycle-04-validate-providers-right.txt](../evidence/cycle-04-validate-providers-right.txt)                                                               |
| `validate-providers.ps1 -Side left`  | 56/56     | [cycle-04-validate-providers-left.txt](../evidence/cycle-04-validate-providers-left.txt)                                                                 |
| `validate-overlay.ps1 -Side right`   | 19/19     | [cycle-04-validate-overlay-right.txt](../evidence/cycle-04-validate-overlay-right.txt)                                                                   |
| `validate-settings.ps1`              | 15/15     | [cycle-04-validate-settings.txt](../evidence/cycle-04-validate-settings.txt)                                                                             |
| `validate-settings-ui.ps1`           | 23/23     | [cycle-04-validate-settings-ui.txt](../evidence/cycle-04-validate-settings-ui.txt) (a primeira execução falhou só pela versão fixa no script, corrigida) |
| `capture-toast.ps1`                  | Entregue  | `send_test_notification: delivered to Windows`; o "Não incomodar" do Windows estava ativo, então não houve banner                                        |

Todos os scripts confirmaram que o arquivo real de preferências não foi tocado.

## Instalador candidato

| Item                         | Resultado                                                                                                                                                                                      |
| ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Arquivo                      | `src-tauri/target/release/bundle/nsis/Aurora Control_1.0.0_x64-setup.exe`, 2.562.850 bytes                                                                                                     |
| SHA-256                      | `F65999DD526F6D4F9B0EE7A3583936590468A4F417CEE2A8311E8D50D8978763`                                                                                                                             |
| Assinatura                   | **Não assinado** (`NotSigned`)                                                                                                                                                                 |
| Atualização 0.3.0 → 1.0.0    | `/S`: código 0 em 2 s; a 0.3.0 em execução foi encerrada pelo instalador; versão 1.0.0 no mesmo local; atalho atualizado                                                                       |
| Preservação das preferências | Arquivo real (formato v2) com SHA-256 idêntico antes e depois de atualização, execução, desinstalação e reinstalação; migrado em memória (Gemini → Antigravity desativado; Grok mantido ativo) |
| Inicialização                | Cópia instalada inicia e lê as quatro fontes com as preferências reais                                                                                                                         |
| Instância única              | Segunda execução sai com código 0 em menos de 15 s; um único processo                                                                                                                          |
| Encerramento                 | `WM_CLOSE` não encerra o app de bandeja (esperado); terminação forçada não deixou processos do WebView2; "Encerrar" na bandeja não foi acionado (sem automação de interface)                   |
| Desinstalação                | `/S`: código 0; arquivos, chave de desinstalação e atalho removidos; dados do app mantidos (opção desmarcada)                                                                                  |
| Instalação limpa             | `/S /D=%LOCALAPPDATA%\Aurora Control`: só `aurora-control.exe` e `uninstall.exe`; atalhos no Menu Iniciar e na Área de Trabalho                                                                |
| Identidade no Windows        | Atalho com `System.AppUserModel.ID = app.auroracontrol.desktop`; editor `auroracontrol`                                                                                                        |
| Arquivos indevidos           | Nenhum                                                                                                                                                                                         |
| Binário instalado × build    | Mesmo tamanho; 3 bytes diferentes (marcador de tipo de bundle gravado pelo Tauri)                                                                                                              |

Os testes de atualização, desinstalação e instalação limpa acima foram feitos com o primeiro build da 1.0.0 (antes da correção das dependências `cookie`, `url` e `tokio`). O instalador final, cujo hash está acima, foi gerado depois da correção e validado como atualização sobre aquele build: código 0, versão 1.0.0, uma única instância, segunda execução encerrada com código 0 e preferências reais com o mesmo hash.

Estado deixado na máquina: 1.0.0 instalada em `%LOCALAPPDATA%\Aurora Control`, em execução, sem inicialização com o Windows (como antes). A instalação de validação 0.3.0 que estava registrada dentro de `target\` foi substituída.

## Desempenho (cópia instalada, release)

| Medida                                | Valor                                                                                                  |
| ------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| CPU em repouso (árvore inteira, 90 s) | 0,56 s de CPU = **0,078%** da máquina                                                                  |
| CPU no primeiro minuto (com leituras) | 1,28 s de CPU = 0,27% da máquina                                                                       |
| Memória do `aurora-control.exe`       | 33 MB de working set, 8 MB privados, 24 threads, 450 handles                                           |
| WebView2 (7 processos)                | cerca de 187 MB privados no total                                                                      |
| Subprocessos próprios                 | 0 em repouso; 1 `agy` por consulta do Antigravity (só se ativado), encerrado com a árvore              |
| Frequência                            | Codex e Claude Code: 5 min; Antigravity: 10 min; Grok / Cursor: 60 s (só `stat` quando o log não muda) |
| Varredura do log do Grok              | 4 ms (118 KB); teto de 6 MiB / 750 ms                                                                  |

Execução prolongada (horas) não foi medida neste ciclo; procedimento na [lista de verificação](release-checklist.md).

## Limitações da validação

- Cursor não instalado: a equivalência com a tela do Cursor depende de homologação.
- Notificações com banner não observadas (Não incomodar ativo).
- Encerramento pelo menu da bandeja e aparência do resumo do Grok / Cursor no hover não foram vistos na cópia instalada (sem automação de interface); a regressão do hover foi coberta pelos scripts no build de depuração.
- Monitores múltiplos reais: só o monitor desta máquina; o comportamento com vários monitores é coberto pelos testes de geometria e pelo `validate-settings.ps1` (monitor desconectado).
