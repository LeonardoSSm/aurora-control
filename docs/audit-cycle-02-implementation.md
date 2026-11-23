# Aurora Control 0.2.0 — Implementação do Ciclo 02

Resposta à [auditoria da baseline](audit-cycle-02-baseline.md) (`07455f4`). Branch `feat/cycle-02-daily-use`, sem merge na `main`. Ambiente: Windows 11 Pro build 26300, um monitor de 1920 × 1080 a 100%, Node 24.15.0, Rust 1.97.1.

## Commits

| Commit    | Grupo                        | Conteúdo                                                                             |
| --------- | ---------------------------- | ------------------------------------------------------------------------------------ |
| `b50f0ee` | 1. Correções bloqueantes     | B01 e B02.                                                                           |
| `ce88eb8` | 2. Estabilização             | I01–I06 e I08, M02 (rótulos).                                                        |
| `4355c0a` | 3. Configurações e monitores | I07, M01 (matriz IPC), M04 (instância única), janela de configurações, versão 0.2.0. |
| (este)    | 4. Testes, docs e entrega    | CI, scripts de validação, evidências, documentação e este relatório.                 |

## Como os cenários foram demonstrados

Antes de qualquer correção, oito testes temporários escritos contra a API da baseline reproduziram os achados. Todos falharam, inclusive três pânicos reais. A saída está em [evidence/cycle-02-baseline-repro.txt](evidence/cycle-02-baseline-repro.txt). Os testes foram removidos da árvore, e o mesmo cenário de cada um está agora nos testes de regressão citados abaixo, que passam.

| Cenário da baseline                            | Resultado na baseline                   | Regressão que agora passa                                                              |
| ---------------------------------------------- | --------------------------------------- | -------------------------------------------------------------------------------------- |
| B01 · logout após 91% da conta A               | `Stale { 91 }`, conta A                 | `scheduler::b01_logout_discards_quotas_labels_and_retry_state`                         |
| B01 · conta B com erro de rede                 | conta A e 91%                           | `scheduler::b01_account_switch_with_the_network_down_never_shows_the_previous_account` |
| B02 · `Retry-After: 18446744073709551615`      | pânico em `finish` (`scheduler.rs:158`) | `scheduler::b02_an_absurd_retry_after_is_bounded_and_never_panics`                     |
| B02 · `reset_after_seconds: 1e300` no Codex    | pânico em `TimeDelta::seconds`          | `codex::b02_out_of_range_times_in_the_response_never_panic`                            |
| B02 · `resets_in_seconds: i64::MAX` no rollout | pânico em `TimeDelta::seconds`          | `codex::b02_out_of_range_times_in_the_response_never_panic`                            |
| I01 · 503 com `Retry-After: 300`               | próximo retry em 60 s                   | `scheduler::i01_service_unavailable_retry_after_binds_both_triggers`, `http::i01_*`    |
| I01 · clique 20 s após 401                     | aceito                                  | `scheduler::i01_a_click_cannot_retry_a_rejected_credential_early`                      |
| I02 · 91% renovando em 60 s, lido em 61 s      | `Available { 0 }`                       | `scheduler::i02_a_single_renewed_window_is_never_a_fresh_zero`                         |

## Tratamento de cada achado

### B01 · Isolamento do cache por identidade

**Correção.** Os adaptadores devolvem uma `Session` derivada dos arquivos de login do próprio provedor: `SignedOut`, `Unverified` ou `Account { identity, credential }`. A identidade do Codex vem de `account_id` mais o `sub` do `id_token`. A do Claude vem de `accountUuid` e `organizationUuid` do perfil, com o e-mail apenas como alternativa para perfis antigos. Ambas viram `Fingerprint`, um SipHash com chave aleatória por processo e `Debug` redigido. Nada disso é serializado, gravado ou enviado pelo IPC. `Fetched.identity` liga cada leitura à conta que a produziu. O `Tracker`:

- descarta cotas, rótulos, falhas, `not_before` e o portão manual em logout (`SignedOut`) ou troca comprovada (identidade diferente);
- mantém o cache quando só a credencial muda (renovação da mesma conta);
- exibe uma cota só enquanto a sessão atual for a mesma identidade. Em sessão `Unverified`, a cota fica guardada, mas não aparece;
- não reaproveita rótulos de tentativas anteriores;
- grava os fallbacks de rollout sem identidade e sem rótulo, e os mostra como "registro local sem conta verificada";
- só confia numa leitura da API sem identidade enquanto ela é o resultado da última tentativa.

A sondagem local de sessão (`UsageAdapter::session`) roda a cada despertar do worker. Assim, logout e troca aparecem antes da próxima consulta.

**Arquivos.** `usage/session.rs` (novo), `usage/scheduler.rs`, `usage/adapter.rs`, `usage/codex.rs`, `usage/claude.rs`, `usage/demo.rs`, `src/usage/presentation.ts`.

**Regressões.** `scheduler::b01_*` (8 testes: logout, troca com rede indisponível, rótulo diferente sem identidade, renovação da mesma conta, sucesso sem rótulo, logs sem identidade, leitura da API sem identidade, sondagem antes da tentativa, estado de retry por conta), `codex::b01_*` (identidade por conta e não por rótulo ou token; logout e chave de API versus arquivo parcial), `claude::b01_*`, `session::*`.

### B02 · Segurança dos cálculos temporais

**Correção.** `usage/time.rs` concentra toda entrada externa de tempo:

- `whole_seconds` rejeita NaN, infinitos e valores fora de `i64`;
- `from_epoch`, `from_epoch_millis` e `offset` usam operações checadas;
- `plausible_reset` só aceita renovações entre 1 dia antes e 45 dias depois da observação;
- `window_minutes` só aceita janelas de 1 min a 45 dias;
- `after` soma esperas limitadas e satura no fim do intervalo representável.

O Codex, o Claude, o rollout, a expiração dos tokens e a atividade local passaram a usar esses helpers. Nos workers, `catch_unwind` envolve o adaptador e o `finish`: um pânico encerra a tentativa como `malformed` (ou `Tracker::abort`), e `refreshing` sempre termina. `sync::lock` recupera mutexes envenenados, inclusive no IPC. O controlador de janelas também isola pânicos por lote e nunca fica marcado como ocupado.

**Política de `Retry-After` acima de 24 h (revisada).** Um valor válido acima do teto de backoff de 30 min é respeitado integralmente, até 24 h (`b02_a_valid_retry_after_beyond_the_backoff_ceiling_is_preserved`, 2 h). Acima disso, o valor é limitado a 24 h, não descartado. A alternativa de tratar o valor como inválido e cair no piso de 60 s desrespeitaria um servidor que pede espera longa. Ignorar o limite paralisaria o provedor por tempo indefinido com um cabeçalho defeituoso. Com a escolha feita, o pior caso é de 24 h, a interface mostra a hora exata da próxima tentativa (`retryAt`) e a espera vive só em memória: reiniciar o app ou trocar de conta a libera. **Risco residual:** um 429 legítimo com mais de 24 h faria o app consultar antes do pedido. Esse cenário não é plausível para um endpoint de uso, e o servidor responderia de novo com 429.

**Recuperação após pânico (revisada).** O teste `usage::b02_a_panicking_attempt_neither_kills_the_worker_nor_poisons_the_cache` dispara um pânico real de `chrono` dentro do adaptador. Depois dele, o snapshot continua legível, `refreshing` é falso, o problema registrado é `malformed` e o mesmo worker atende o pedido seguinte com sucesso. `sync::a_panic_while_locked_does_not_poison_later_readers` cobre o envenenamento. O perfil de release usa `panic = unwind`, o padrão, condição necessária para essa recuperação.

**Arquivos.** `usage/time.rs` (novo), `sync.rs` (novo), `usage/http.rs`, `usage/scheduler.rs`, `usage/mod.rs`, `usage/codex.rs`, `usage/claude.rs`.

**Regressões.** `time::*` (4), `scheduler::b02_*` (4), `http::b02_retry_after_is_bounded`, `codex::b02_*` (2), `claude::b02_*`, `usage::b02_*`, `sync::*`.

### I01 · Status antes do corpo e portões de recuperação

**Correção.** `http::respond` lê o corpo só em respostas 2xx. Erros são classificados por status e cabeçalhos já recebidos, e uma falha de leitura do corpo não os apaga. Um 503 ou outro erro preserva `Retry-After` (`FetchError::Http { status, retry_after }`, `Issue::Http { retryAt }`). O agendador usa o relógio monotônico (`Now { instant, utc }`) e três barreiras: a regular, a manual e a imposta pelo servidor. A tabela completa está na [arquitetura](architecture.md#atualização). Um clique não antecipa 401/403, 429, 503 com `Retry-After` nem resposta malformada. A sondagem libera a espera local quando a credencial muda (novo login).

**Regressões.** `http::i01_service_unavailable_keeps_its_retry_after`, `http::i01_a_failing_body_never_hides_the_status_and_headers_of_an_error`, `scheduler::i01_*` (5).

### I02 · Cota renovada não é leitura fresca

**Correção.** `roll_over` marca `renewed: true` (no contrato, em `Quota` e em `ConsumptionWindow`). `primary_index` prefere qualquer janela medida a uma renovada. Uma principal renovada vira `Stale { 0 }` e a interface mostra "—" e "Renovada · aguardando nova leitura". `Tracker::next_wake` inclui as renovações e o momento em que a leitura envelhece. O worker publica essas transições sem resultado da rede. O resumo reavalia as renovações em cada renderização, inclusive no intervalo de 30 s.

**Regressões.** `scheduler::i02_*` (3), `adapter::i02_*`, `stability.test.tsx` (I02, 2 testes, incluindo resumo aberto durante a renovação).

### I03 · Ordem de eventos

**Correção.** `Service::update` numera cada publicação sob o mesmo lock do snapshot, e `get_usage` devolve `UsageUpdate { sequence, snapshot }`. As seleções levam `revision` (u64) e `sequence`. `isNewerUpdate` e `isNewerSelection` (`src/desktop/ordering.ts`) descartam eventos antigos antes de renderizar e antes de `show_summary`. No Rust, os jobs do controlador revalidam geração e revisão imediatamente antes de `SetWindowPos` e `SetWindowRgn`.

**Regressões.** `usage::i03_publication_sequences_strictly_increase_across_threads` (4 threads × 50 publicações), `stability.test.tsx` (I03, entrega invertida de snapshots e seleções).

### I04 · Abertura sem `pointerleave` confiável

**Correção.** Há um único prazo de intenção cancelável por barra, dentro do controlador, sem uma thread por intenção. Quando ele vence, `GetCursorPos` é testado contra a elipse recortada (`Shape::Ellipse`) do indicador no último quadro aceito. Se o cursor estiver fora, o ponteiro é esquecido e `rail-pointer-reset` faz a barra voltar a informar. A zona de fechamento usa a mesma elipse, a ponte e o cartão.

**Regressões.** `hover::i04_a_stale_pointer_is_forgotten_instead_of_opening`, `position::i04_transparent_corners_and_the_unreached_final_frame_are_outside` (100%, 125% e 150%, nas duas laterais), `controller::i04_a_single_cancelable_intent_per_rail`. Validação interativa: "sweep does not open".

### I05 · Propriedade de thread das mutações Win32

**Correção.** `windows/controller.rs` é o dono único das mutações. Callbacks (`ScaleFactorChanged`, `Resized`), bandeja e comandos só enfileiram. A thread do controlador junta layout e leituras do cursor e posta um lote por vez com `run_on_main_thread`. Como a chamada sai dela, a execução é sempre diferida e nunca acontece dentro do callback. `set_rail_motion` virou assíncrono e aguarda a resposta. `AURORA_TRACE=1` registra jobs, espera, duração, thread e profundidade, e uma profundidade diferente de 1 ou outra thread geram log mesmo sem a variável.

**Regressões.** `controller::i05_*` (3). **Validação interativa.** 82 e 80 lotes rastreados, todos na thread dona `ThreadId(1)` e com profundidade 1. Mediana de espera de 0,11 a 0,14 ms; máximo de 78 a 180 ms, na primeira abertura a frio do WebView do resumo.

### I06 · Leituras locais limitadas

**Correção.**

- `local::read_limited`: credenciais até 256 KiB, perfil até 8 MiB, `config.toml` e preferências até 256 KiB e 64 KiB.
- `local::Budget`: entradas, arquivos, bytes, registros e prazo cumulativos.
- `read_tail_within` consome o orçamento.
- Claude: o orçamento é de 20 mil entradas, 256 arquivos, 64 MiB, 200 mil registros e 2 s; cada `message.id` fica com o maior total; a atividade é lida depois da consulta e só com sessão; `LocalActivity.partial` sinaliza um corte.
- Codex: só pastas numéricas, até 2.048 entradas por pasta e 10 dias.
- Codex respeita `auth_mode`, e o armazenamento do sistema (`keyring` ou `auto`) vira `credentialsUnavailable`.
- No Claude, credencial ausente com conta no perfil é `credentialsUnavailable`, não logout. Ilegível ou inválida é `credentialsInvalid`. Escopo ausente leva a uma tentativa, e a recusa vira `authRejected`.

**Regressões.** `local::i06_*` (2), `claude::i06_*` (4, incluindo 300 arquivos e o limite atingido), `codex::i06_declared_mode_and_system_store_are_honored`.

### I07 · Monitores, DPI e preferências

**Correção.** Os detalhes estão na [arquitetura](architecture.md#monitores-dpi-e-preferências):

- monitores por `EnumDisplayMonitors`, DPI por `GetDpiForMonitor` e identificador FNV estável;
- fallback para o principal sem apagar a preferência;
- um snapshot único de geometria, verificado a cada 2 s contra topologia e área útil e refeito em mudança de DPI;
- preferências versionadas com escrita atômica, recuperação e precedência ambiente > preferências > padrões;
- o caso limite da largura física menor corrigido em `indicator_circles`.

**Regressões.** `monitors::i07_*` (2), `position::i07_*`, `position::slot_tests`, `preferences::*` (7).

### I08 · Parsing com semântica confiável

**Correção.** Só janelas conhecidas são `eligible` para virar o indicador: o `rate_limit` principal no Codex, e `five_hour` e `seven_day*` conhecidos no Claude. Métricas secundárias ou futuras são listadas. Chaves sem semântica de janela são ignoradas. Uma resposta sem janela elegível válida é `malformed`. Percentuais fora de 0–100 (com tolerância de 0,5) são rejeitados, e o caminho do campo, nunca o valor, é registrado (`schema_anomaly`). Renovações implausíveis viram desconhecidas.

**Regressões.** `codex::i08_*`, `claude::i08_*`, `adapter::i08_*`.

### Melhorias M01–M04

- **M01:** a capability `settings` tem só três comandos tipados. `ipc_tests` usa o ACL real (`generate_context!(test = true)`) e só conta como recusa o erro "not allowed" do ACL.
- **M02:** rótulos com no máximo 48 caracteres e sem caracteres de controle.
- **M03:** sem alteração.
- **M04:** instância única via mutex de sessão antes dos workers.

## Fase 3 — Funcionalidades

| Requisito                  | Implementação                                                                                                         |
| -------------------------- | --------------------------------------------------------------------------------------------------------------------- |
| Janela Dark Tech Premium   | `settings.html`, `Settings.tsx`, estilos em `styles.css`, tema escuro nativo. Aberta por "Configurações…" na bandeja. |
| Persistência versionada    | `preferences.rs`.                                                                                                     |
| Seleção de monitor         | `windows/monitors.rs` e seletor na janela, com monitor desconectado listado.                                          |
| DPI                        | Escala efetiva por monitor no snapshot; `ScaleFactorChanged` reposiciona.                                             |
| Inicialização com Windows  | `autostart.rs`, só o valor próprio em `HKCU\…\Run`.                                                                   |
| Provedores visíveis        | `Service::set_visible`; barra com 1 ou 2 círculos.                                                                    |
| Diagnóstico do Claude Code | `claude::diagnose` com estados, rótulo mascarado e horários, sem token.                                               |
| CI                         | `.github/workflows/ci.yml`: Windows, `contents: read`, actions fixadas por SHA, sem credenciais persistidas.          |

A identidade semicircular não mudou: mesmos círculos de 52 px, arco interno e cores dos provedores. A janela de configurações usa a meia-lua com as duas cores como marca.

## Evidências de validação

### Suítes automáticas

| Verificação                                                | Resultado                                                                                                                                                        |
| ---------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `npm run format:check`, `npm run lint`, `npx tsc --noEmit` | Aprovados.                                                                                                                                                       |
| `npm test`                                                 | 26/26 em 3 arquivos (eram 15).                                                                                                                                   |
| `cargo fmt --check`                                        | Aprovado.                                                                                                                                                        |
| `cargo clippy --locked --all-targets -- -D warnings`       | Aprovado.                                                                                                                                                        |
| `cargo test --locked`                                      | 122 aprovados, 1 ignorado de propósito (eram 47).                                                                                                                |
| `cargo test -- --ignored registry_round_trip`              | Aprovado: o wrapper real do registro grava, lê e remove um valor numa subchave temporária de `HKCU\Software`, removida em seguida. A chave `Run` não foi tocada. |

### Validação interativa (build de debug, modo demo, 96 DPI)

- [`scripts/validate-overlay.ps1`](../scripts/validate-overlay.ps1), 19/19 nas duas laterais:
  - barra visível, resumo oculto no início, estilos `WS_EX_NOACTIVATE` e `WS_EX_TOOLWINDOW`;
  - hover real por `SetCursorPos`, abrindo em 336 a 1.304 ms a frio e 182 a 234 ms a quente, e fechando em 270 a 416 ms depois que o cursor sai;
  - foco preservado: o primeiro plano nunca passa às sobreposições;
  - resumo para dentro da tela e transparente a cliques, e canto da barra transparente a cliques;
  - fecha ao sair e uma varredura rápida não abre;
  - a troca de indicador mantém o cartão;
  - lotes na thread dona, sem reentrância.

  Saída completa em [evidence/cycle-02-interactive-validation.txt](evidence/cycle-02-interactive-validation.txt).

  **Intermitência observada no roteiro, não no app.** Em uma execução, "closes after leaving" falhou quando a verificação ainda usava espera fixa de 700 ms. O log daquela execução foi sobrescrito, então a causa não está provada; a hipótese é carga da primeira abertura a frio. O roteiro passou a medir a latência de fechamento, e seis execuções seguidas fecharam em 270 a 389 ms. Em outra execução, "no re-entrant batch" falhou porque a última linha do log estava truncada (`[aurora] batch ["hover"`), cortada quando o roteiro encerra o processo. Nenhum lote com profundidade diferente de 1 foi encontrado. O roteiro agora avalia só linhas completas.

- [`scripts/validate-settings.ps1`](../scripts/validate-settings.ps1), 15/15:
  - padrões sem arquivo;
  - segunda instância encerra com código 0 e a primeira continua;
  - lateral salva sobrevive ao reinício;
  - um provedor gera uma barra de 52 px;
  - monitor desconectado cai no principal e a escolha é preservada no arquivo;
  - arquivo corrompido: padrões em uso e original guardado como `preferences.corrupt-*.json`;
  - versão mais nova: arquivo intacto;
  - `AURORA_SIDE` prevalece e não é gravado.
- Captura da janela de configurações com `PrintWindow`, só da janela: layout correto em modo demo e diagnóstico real do Claude nesta máquina. O diagnóstico mostrou perfil vinculado com e-mail mascarado e "Token vazio: o CLI saiu da conta, ou o app Claude Desktop fornece a credencial", coerente com o estado conhecido do CLI aqui. Nenhum token apareceu.

### Build Windows e NSIS

| Verificação                                                | Resultado                                                                                                                                                                                                                                |
| ---------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `npm run desktop:build`                                    | Aprovado. Release em 1 min 39 s. O download do NSIS, que falhou na baseline, funcionou.                                                                                                                                                  |
| Executável `target/release/aurora-control.exe`             | 10.012.160 bytes. SHA-256 `F90446CE5AC484F49CA01FCAACDED126566A36D8DCA7382171279824F300F848`.                                                                                                                                            |
| Instalador `Aurora Control_0.2.0_x64-setup.exe`            | 2.383.680 bytes. SHA-256 `95202C8CA8C00EEF405369B860B51EB715C744F03555554D745D75F4DFFCAE06`.                                                                                                                                             |
| Instalação silenciosa (`/S /D=%TEMP%\aurora-install-test`) | Código 0. Executável presente (SHA-256 `9650594C…584C0A`, diferente do binário de `target/release` porque o empacotador o marca com o tipo de pacote), entrada de desinstalação "Aurora Control 0.2.0" em HKCU e atalho no menu Iniciar. |
| Execução do binário instalado (demo)                       | A barra abriu. Uma segunda execução encerrou com código 0.                                                                                                                                                                               |
| Desinstalação silenciosa                                   | Código 0. Executável, entrada de registro e atalho removidos. Nenhuma entrada `Run` criada.                                                                                                                                              |
| `npm audit` e `npm audit --omit=dev`                       | 0 vulnerabilidades.                                                                                                                                                                                                                      |
| `cargo audit`                                              | Não executado: `cargo-audit` não está instalado nesta máquina.                                                                                                                                                                           |
| CI                                                         | Workflow criado, ainda não executado no GitHub: a branch não foi enviada.                                                                                                                                                                |

## Testes não executados e riscos residuais

- **Vários monitores e DPI de 125% e 150% em sessão interativa:** não executados. O ambiente tem um único monitor a 100%. A geometria fracionária, a escolha e o fallback de monitor estão cobertos por testes puros (`position`, `monitors`), e a enumeração Win32 foi exercitada com um monitor. Troca de monitor principal, desconexão e reconexão com DPI misto e barra de tarefas em outras bordas ficam para a auditoria independente.
- **Interação clicando na janela de configurações:** não executada. O controle do desktop foi recusado nesta sessão. A lógica dos controles está coberta por `settings.test.tsx`. Os comandos e a capability, pela matriz IPC. A persistência, pelo teste do `Store` e por `validate-settings.ps1`. A gravação real no registro, pelo `registry_round_trip`. Falta o caminho completo clique → `set_preferences` → registro `Run` no app em execução.
- **Inicialização real no logon do Windows:** não executada (exigiria sair da sessão).
- **Quota real do Claude:** não executada. A credencial do CLI nesta máquina está vazia e o Claude Code roda pelo app Desktop. O diagnóstico mostra esse estado corretamente.
- **Endpoints internos:** continuam não documentados (I08). Agora uma mudança de formato vira `malformed` em vez de exibir dado sem semântica.
- **Identidade do Claude sem perfil:** quando `.claude.json` some, a sessão fica `Unverified` e a cota em cache é ocultada até a identidade voltar. É conservador, ao custo de esconder dado da mesma conta nesse intervalo.
- **Primeira abertura do resumo:** pode levar de 0,3 a 0,9 s enquanto o WebView inicializa, com a thread principal ocupada. As aberturas seguintes ficam em cerca de 0,2 s.
- **M03 (conteúdo longo no resumo):** fora do escopo deste ciclo.
- **`npm audit` e `cargo audit`:** ver a seção de build.

## Instruções para a auditoria independente (Codex)

1. `git log --oneline 07455f4..feat/cycle-02-daily-use`. Revise os quatro commits contra a baseline.
2. Suítes: `npm ci`, `npm run format:check`, `npm run lint`, `npx tsc --noEmit`, `npm test`; em `src-tauri`: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`. Opcional, grava e remove uma subchave temporária em HKCU: `cargo test -- --ignored registry_round_trip`.
3. Reproduza a baseline: aplique os testes de [evidence/cycle-02-baseline-repro.txt](evidence/cycle-02-baseline-repro.txt) em `07455f4` (devem falhar) e rode os equivalentes listados acima nesta branch (devem passar).
4. Interativo: `npm run desktop:debug`, depois `pwsh -File scripts/validate-overlay.ps1 -Exe src-tauri/target/debug/aurora-control.exe -Side right` (e `-Side left`) e `pwsh -File scripts/validate-settings.ps1 -Exe src-tauri/target/debug/aurora-control.exe`. O segundo faz backup das preferências e as restaura no fim.
5. Execute o que não foi possível aqui: dois ou mais monitores com DPI misto (100%, 125% e 150%), troca do monitor principal, desconexão e reconexão do monitor escolhido, barra de tarefas em cada borda. Use `AURORA_TRACE=1` com `AURORA_DEMO=73,25` para registrar thread, profundidade e latência durante hover, bandeja, minimização e mudança de DPI.
6. Configurações: abra pela bandeja e exercite lateral, monitor, provedores e "Iniciar com o Windows". Confira `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` → `Aurora Control`.
7. IPC em build de produção: confirme que `settings` não escuta eventos e que `rail` e `summary` não chamam `get_settings`, `set_preferences` nem `get_claude_diagnostics`.
8. Instale e desinstale o NSIS gerado e rode `npm audit` e `cargo audit` se disponíveis.
