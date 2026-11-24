# Aurora Control 0.2.0 — Implementação do Ciclo 02

Resposta à [auditoria da baseline](audit-cycle-02-baseline.md) (`07455f4`). Branch `feat/cycle-02-daily-use`, sem merge na `main`. Ambiente: Windows 11 Pro build 26300, um monitor de 1920 × 1080 a 100%, Node 24.15.0, Rust 1.97.1.

## Commits

| Commit    | Grupo                        | Conteúdo                                                                             |
| --------- | ---------------------------- | ------------------------------------------------------------------------------------ |
| `b50f0ee` | 1. Correções bloqueantes     | B01 e B02.                                                                           |
| `ce88eb8` | 2. Estabilização             | I01–I06 e I08, M02 (rótulos).                                                        |
| `4355c0a` | 3. Configurações e monitores | I07, M01 (matriz IPC), M04 (instância única), janela de configurações, versão 0.2.0. |
| `e4fd96c` | 4. Testes, docs e entrega    | CI, scripts de validação, evidências, documentação e este relatório.                 |
| `b210572` | Revisão do Codex             | Os três achados P2 do PR #1.                                                         |
| (este)    | Fechamento                   | Revalidação, quatro defeitos residuais, validação da UI real, relatório final.       |

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
| `npm test`                                                 | 28/28 em 3 arquivos (eram 15).                                                                                                                                   |
| `npm run build`                                            | Aprovado.                                                                                                                                                        |
| `cargo fmt --check`                                        | Aprovado.                                                                                                                                                        |
| `cargo clippy --locked --all-targets -- -D warnings`       | Aprovado.                                                                                                                                                        |
| `cargo test --locked`                                      | 132 aprovados, 1 ignorado de propósito (eram 47).                                                                                                                |
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
- [`scripts/validate-settings-ui.ps1`](../scripts/validate-settings-ui.ps1), 22/22, descrito na [revalidação de fechamento](#revalidação-de-fechamento). Ele cobre salvar pela janela, a persistência após o reinício, um ou dois provedores, a inicialização com o Windows, a falha ao salvar com rollback real e a reconciliação.
- Falha de rede no app real (build de debug, adaptadores reais, só `status` e `issue.code` lidos). Com `HTTPS_PROXY` apontando para uma porta fechada local, nenhuma requisição sai da máquina. O Codex fica `stale` com `network`, porque o adaptador cai no registro local (`codex.rs:550`), sem nada apresentado como leitura ao vivo. Com a rede normal, o Codex fica `available`. O Claude Code fica `credentialsUnavailable` nos dois casos, porque a credencial do CLI está vazia aqui.
- Captura da janela de configurações com `PrintWindow`, só da janela: layout correto em modo demo e diagnóstico real do Claude nesta máquina. O diagnóstico mostrou perfil vinculado com e-mail mascarado e "Token vazio: o CLI saiu da conta, ou o app Claude Desktop fornece a credencial", coerente com o estado conhecido do CLI aqui. Nenhum token apareceu.

### Build Windows e NSIS

| Verificação                                                             | Resultado |
| ----------------------------------------------------------------------- | --------- |
| Artefatos do commit de fechamento (os de `e4fd96c` foram substituídos): |

| Verificação                                                                          | Resultado                                                                                                                                                                                                                                |
| ------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `npm run desktop:build`                                                              | Aprovado. Release em 53 s (incremental).                                                                                                                                                                                                 |
| Executável `src-tauri/target/release/aurora-control.exe`                             | 10.041.344 bytes. SHA-256 `3D31D5065CE80ED810C93C1FCA85A4817B58296F935A4970655D9A939B578674`.                                                                                                                                            |
| Instalador `src-tauri/target/release/bundle/nsis/Aurora Control_0.2.0_x64-setup.exe` | 2.384.904 bytes. SHA-256 `EA277A2381D6DB6403008B817E706DBBE282438FFFF8318C66B9B6B3EB9C12DD`.                                                                                                                                             |
| Instalação silenciosa (`/S /D=%TEMP%\aurora-install-test`)                           | Código 0. Executável presente (SHA-256 `CEC8422B…75C4A6`, diferente do binário de `target/release` porque o empacotador o marca com o tipo de pacote), entrada de desinstalação "Aurora Control 0.2.0" em HKCU e atalho no menu Iniciar. |
| Execução do binário instalado (demo)                                                 | A barra abriu. Uma segunda execução encerrou com código 0.                                                                                                                                                                               |
| Desinstalação silenciosa                                                             | Código 0. Executável, entrada de registro e atalho removidos. Nenhuma entrada `Run` nem pasta de preferências.                                                                                                                           |
| `npm audit` e `npm audit --omit=dev`                                                 | 0 vulnerabilidades.                                                                                                                                                                                                                      |
| `cargo audit`                                                                        | Não executado: `cargo-audit` não está instalado nesta máquina.                                                                                                                                                                           |
| CI                                                                                   | `Verify (Windows)` verde em `e4fd96c` e `b210572`. O resultado do commit de fechamento está na seção [Estado do PR](#estado-do-pr-1).                                                                                                    |

### Segurança

- CSP idêntica à da baseline (`default-src 'none'`, sem `unsafe-*`). Capabilities por janela: `rail` (5 comandos e escuta de eventos), `summary` (2 comandos e escuta), `settings` (3 comandos, sem eventos). A matriz IPC (`ipc_tests`) confirma as recusas.
- Nenhum segredo nos arquivos versionados. O único padrão de token encontrado é o fixture `sk-ant-test`. O diagnóstico e o IPC não carregam tokens, conteúdo de arquivos nem identificadores internos; as identidades são SipHash com chave por processo.
- `preferences.json` guarda só a lateral, o id e o nome do monitor, os provedores visíveis e a escolha de inicialização.
- A instância única e o tratamento de arquivo corrompido foram validados no app real (`validate-settings.ps1`).
- O caminho com o nome de usuário do Windows foi removido de `evidence/cycle-02-baseline-repro.txt`. Ele continua no histórico do commit `e4fd96c`, já público. Não é credencial, e reescrever o histórico exigiria force-push.

## Revisão do Codex no PR #1

Três achados P2 sobre `e4fd96c`, todos corrigidos com regressão:

| Achado                                                                                                                 | Correção                                                                                                                                                                                                                                          | Regressão                                                                          |
| ---------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| O diagnóstico lia os arquivos do Claude Code mesmo com o provedor oculto.                                              | `Service::diagnostics` devolve `None` para um provedor desativado, sem chamar o adaptador. A janela explica que o provedor está oculto e refaz o diagnóstico depois de salvar.                                                                    | `usage::a_hidden_provider_is_not_diagnosed`, `settings.test.tsx` (provedor oculto) |
| Uma transcrição cortada pelo limite de 8 MiB por arquivo não marcava `partial`.                                        | `read_tail_within` informa se o início ficou de fora (`Tail.cut`). Um corte vira `partial` quando o primeiro registro datado do trecho lido é posterior ao início da janela de 5 h. Um corte que só descarta registros antigos continua completo. | `claude::truncation_tests` (2)                                                     |
| Se gravar as preferências falhasse depois de alterar o registro, a entrada de inicialização e a preferência divergiam. | `autostart::apply_then` lê a entrada anterior, aplica a mudança e, se a gravação falhar, restaura a entrada exatamente como estava, inclusive a de outra cópia.                                                                                   | `autostart::a_failed_save_restores_the_previous_entry`                             |

### Revalidação de fechamento

As três correções foram exercitadas de novo, inclusive no app real, e a revalidação encontrou quatro defeitos residuais nas mesmas áreas. Cada um foi reproduzido antes da correção (teste temporário ou mutação) e agora passa; a saída está em [evidence/cycle-02-closing-validation.txt](evidence/cycle-02-closing-validation.txt).

| Área                       | Defeito encontrado                                                                                                                                                                                                                                                             | Correção                                                                                                                                                                                                                                                                                                                                     | Regressão                                                                                                                             |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| Achado 2 · transcrições    | `read_tail_within` media o arquivo e `read_tail` o reabria e media de novo. Uma transcrição ativa que crescia entre as duas leituras era lida a partir do meio: o primeiro registro sumia e `cut` dizia `false`. Reproduzido em `b210572`: `cut=false text="second\nthird\n"`. | Uma abertura e uma medição (`tail_of`). Bytes acrescentados depois ficam para a próxima leitura em vez de deslocar o início, e `cut` passa a ser `start > 0`, o que de fato ficou de fora.                                                                                                                                                   | `local::a_file_growing_during_the_read_keeps_its_start_and_cut_flag`                                                                  |
| Achado 2 · transcrições    | Uma transcrição recente que não abria (bloqueada, sem permissão) era pulada em silêncio, e o total aparecia como completo.                                                                                                                                                     | Marca `partial`.                                                                                                                                                                                                                                                                                                                             | `claude::an_unreadable_recent_transcript_is_partial` (falha com a correção removida)                                                  |
| Achado 1 · provedor oculto | Um provedor oculto com consulta vencida nunca inicia tentativa, então `next_wake` fica no passado e o worker acordava a cada 250 ms para sempre. Não lia nada, mas girava à toa.                                                                                               | `sleep_for`: oculto dorme o máximo (300 s); `set_visible` o acorda ao reexibir.                                                                                                                                                                                                                                                              | `usage::a_hidden_worker_with_an_overdue_refresh_sleeps_instead_of_spinning`                                                           |
| Achado 3 · inicialização   | Se a própria restauração falhasse, só havia um `eprintln` e o usuário via o erro de gravação comum, como se os estados estivessem coerentes. Ao salvar sem mudar a escolha, uma entrada sobrando com a preferência desativada também nunca era corrigida.                      | O erro passa a ser `OUT_OF_SYNC`, que diz que o registro diverge. O log registra só o tipo do erro. `startupMismatch` na visão mostra a divergência e oferece "Corrigir", e salvar reaplica a escolha quando o registro diverge. Uma entrada de outra cópia nunca é removida por isso. A janela recarrega o estado real depois de uma falha. | `autostart::a_failed_restore_is_reported_as_out_of_sync`, `autostart::disagreement_is_judged_for_this_copy_only`, `settings.test.tsx` |

Também foi acrescentado `usage::a_hidden_worker_reads_nothing_until_shown_again`: com o provedor oculto antes do worker começar, três atualizações manuais não fazem nenhuma sondagem nem consulta; ao reexibi-lo, ele sonda e consulta uma vez. O teste falha se a verificação de `enabled` for removida.

**No app real** ([`scripts/validate-settings-ui.ps1`](../scripts/validate-settings-ui.ps1), 22/22), clicando nos controles da janela pelo DevTools do WebView2:

- ligar "Iniciar com o Windows" grava em `Run` o caminho entre aspas, e a preferência concorda;
- com `preferences.json` aberto sem compartilhamento de exclusão, desligar falha com "Acesso negado", o valor em `Run` é restaurado, a preferência fica como estava, a janela mostra o estado gravado e não sobra arquivo temporário;
- com o valor removido por fora, a janela aponta a divergência, e "Corrigir" o recria;
- ocultar o Codex encolhe a barra para um círculo na hora, e reexibi-lo traz o segundo de volta.

A falha da própria restauração não foi provocada no registro real, porque isso exigiria alterar as permissões da chave `Run`. Esse caminho está coberto só pelo teste com a chave simulada.

## Testes não executados e riscos residuais

- **Vários monitores e DPI de 125% e 150% em sessão interativa:** não executados. O ambiente tem um único monitor a 100%. A geometria fracionária, a escolha e o fallback de monitor estão cobertos por testes puros (`position`, `monitors`), e a enumeração Win32 foi exercitada com um monitor. Troca de monitor principal, desconexão e reconexão com DPI misto e barra de tarefas em outras bordas ficam para a auditoria independente.
- **Bandeja do sistema:** não exercitada. O controle do desktop foi recusado nesta sessão, e o menu da bandeja não é alcançável pelo DevTools. A janela de configurações foi aberta por `AURORA_OPEN_SETTINGS=1`, um gancho só de debug, não por "Configurações…". Esquerda, direita e sair pela bandeja também não foram clicados; a troca de lateral foi validada pela janela.
- **Seleção de monitor pela janela:** só existe um monitor. A lista e o fallback de monitor desconectado foram validados (`validate-settings.ps1`, `settings.test.tsx`), mas não a troca entre dois monitores reais.
- **Reconexão da rede com o app em execução:** não executada no app real; a variável de proxy é fixa por processo. A falha de rede e a execução normal foram observadas separadamente, e a retomada é coberta por `scheduler::i01_network_failures_allow_a_manual_retry_after_the_short_gap`.
- **Falha da própria restauração do registro:** coberta só com a chave simulada, porque provocá-la no real exigiria mudar as permissões de `Run`.
- **Inicialização real no logon do Windows:** não executada (exigiria sair da sessão). O valor gravado em `Run` foi conferido.
- **Nome de usuário no histórico:** ver [Segurança](#segurança).
- **Quota real do Claude:** não executada. A credencial do CLI nesta máquina está vazia e o Claude Code roda pelo app Desktop. O diagnóstico mostra esse estado corretamente.
- **Endpoints internos:** continuam não documentados (I08). Agora uma mudança de formato vira `malformed` em vez de exibir dado sem semântica.
- **Identidade do Claude sem perfil:** quando `.claude.json` some, a sessão fica `Unverified` e a cota em cache é ocultada até a identidade voltar. É conservador, ao custo de esconder dado da mesma conta nesse intervalo.
- **Primeira abertura do resumo:** pode levar de 0,3 a 0,9 s enquanto o WebView inicializa, com a thread principal ocupada. As aberturas seguintes ficam em cerca de 0,2 s.
- **M03 (conteúdo longo no resumo):** fora do escopo deste ciclo.
- **`npm audit` e `cargo audit`:** ver a seção de build.

### Segunda revisão do Codex (`efe6f5e`)

Um achado P2: depois de um `Retry-After` longo da conta A e da troca para a conta B, a sondagem descartava a espera do servidor de A, mas o horário da próxima atualização automática (`next_due`) continuava o de A. O refresh automático de B ficava bloqueado até esse prazo, que pode chegar a 24 h. O teste de troca só verificava o clique. Reproduzido com o código de `efe6f5e`: `should_fetch(at(30), false)` era falso depois de `probe(account("B"))`.

**Correção.** `observe` informa quando descarta o estado da conta anterior. A sondagem então antecipa `next_due` para agora quando a nova sessão é uma conta. A renovação da credencial da mesma conta mantém a espera do servidor.

**Regressão.** `scheduler::b01_the_retry_state_of_one_account_does_not_bind_the_next` agora verifica o refresh automático e o manual, inclusive no caminho logout → outra conta, e confirma que a renovação da mesma conta continua bloqueada nos dois.

## Estado do PR #1

- `main` continua em `07455f4`, sem merge.
- Os três achados P2 do Codex estão corrigidos, respondidos e com threads resolvidas. A revalidação acima reforçou os três.
- CI `Verify (Windows)`: verde em `e4fd96c` e `b210572`. O resultado no commit de fechamento é informado no PR.
- Uma nova revisão do Codex foi solicitada depois do fechamento.
- Nenhum problema bloqueante conhecido. Os testes ainda não executados estão na seção anterior e dependem de hardware (dois monitores, DPI misto) ou de interação com a bandeja.

## Instruções para a auditoria independente (Codex)

1. `git log --oneline 07455f4..feat/cycle-02-daily-use`. Revise os quatro commits contra a baseline.
2. Suítes: `npm ci`, `npm run format:check`, `npm run lint`, `npx tsc --noEmit`, `npm test`; em `src-tauri`: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`. Opcional, grava e remove uma subchave temporária em HKCU: `cargo test -- --ignored registry_round_trip`.
3. Reproduza a baseline: aplique os testes de [evidence/cycle-02-baseline-repro.txt](evidence/cycle-02-baseline-repro.txt) em `07455f4` (devem falhar) e rode os equivalentes listados acima nesta branch (devem passar).
4. Interativo: `npm run desktop:debug`, depois `pwsh -File scripts/validate-overlay.ps1 -Exe src-tauri/target/debug/aurora-control.exe -Side right` (e `-Side left`), `pwsh -File scripts/validate-settings.ps1 -Exe src-tauri/target/debug/aurora-control.exe` e `pwsh -File scripts/validate-settings-ui.ps1 -Exe src-tauri/target/debug/aurora-control.exe`. Os dois últimos fazem backup das preferências e as restauram no fim. O terceiro grava e remove o valor `Aurora Control` em `HKCU\…\Run` e o deixa como estava antes; precisa do Node 22+ e abre o DevTools do WebView2 só em `127.0.0.1`.
5. Execute o que não foi possível aqui: dois ou mais monitores com DPI misto (100%, 125% e 150%), troca do monitor principal, desconexão e reconexão do monitor escolhido, barra de tarefas em cada borda. Use `AURORA_TRACE=1` com `AURORA_DEMO=73,25` para registrar thread, profundidade e latência durante hover, bandeja, minimização e mudança de DPI.
6. Configurações: abra pela bandeja e exercite lateral, monitor, provedores e "Iniciar com o Windows". Confira `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` → `Aurora Control`.
7. IPC em build de produção: confirme que `settings` não escuta eventos e que `rail` e `summary` não chamam `get_settings`, `set_preferences` nem `get_claude_diagnostics`.
8. Instale e desinstale o NSIS gerado e rode `npm audit` e `cargo audit` se disponíveis.
