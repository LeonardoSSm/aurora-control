# Aurora Control 0.3.0 — Implementação do Ciclo 03

Monitoramento multiprovedor e alertas inteligentes. Branch `feat/cycle-03-multi-provider`, criada a partir da `main` em `df47f01` (merge do PR #1). Sem merge na `main`. Ambiente: Windows 11 Pro build 26300, um monitor de 1920 × 1080 a 100%, Node 24.15.0, Rust 1.97.1.

## Commits

| Commit    | Fase                         | Conteúdo                                                                                                       |
| --------- | ---------------------------- | -------------------------------------------------------------------------------------------------------------- |
| `bc156f8` | 1–3. Provedores              | Catálogo compartilhado, contrato com quatro provedores, Gemini, Grok, métrica escolhida, preferências v2.      |
| `a77cd85` | 4. Barra (nativo)            | Zero a quatro indicadores, geometria e região Win32 dinâmicas, capacidade da área útil, visibilidade da barra. |
| `666e805` | 5–6. Alertas e configurações | Motor de alertas, horário silencioso, entrega nativa, comandos e capability da janela de configurações.        |
| `3144f04` | 4–6. Interface               | Quatro indicadores, identidades visuais, sinalizador de atenção, janela de configurações reorganizada.         |
| (este)    | 7–9. Validação e entrega     | Scripts de validação isolados, CI com instalador, documentação, evidências e versão 0.3.0.                     |

Cada commit de backend foi compilado e testado isoladamente antes de ser criado (stash do restante da árvore).

## Requisitos

Legenda: **V** implementado e verificado (teste automatizado ou validação no Windows), **T** implementado e coberto só por testes, **N** não verificado, com o motivo.

### Fase 1 — Arquitetura multiprovedor

| Requisito                                                              | Estado | Evidência                                                                                                                                               |
| ---------------------------------------------------------------------- | ------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Quantidade variável de provedores, sem pressupor dois                  | V      | `Provider::ALL`, `MAX_INDICATORS`, `Shifts`; validação 0–4 no Windows                                                                                   |
| Identificador, nome, identidade, cor, adaptador, capacidades, política | V      | `catalog.json` + `catalog.rs`/`catalog.ts`; `Policy::for_provider`; cores em `styles.css`                                                               |
| Catálogo centralizado sem listas duplicadas                            | V      | `catalog::tests::the_shared_catalog_matches_the_provider_enum_in_order`, `providers.test.tsx` (catálogo e CSS)                                          |
| Contratos tipados Rust/TS                                              | V      | `model.rs`/`model.ts`, `Providers` como struct (não mapa), testes de serialização                                                                       |
| Sem código arbitrário ou plugins executáveis                           | V      | Catálogo é JSON de dados embutido em tempo de compilação                                                                                                |
| Migração versionada das preferências 0.2.0                             | V      | `preferences::migrate`; `a_version_1_file_is_migrated_keeping_every_choice`; `validate-settings.ps1` com arquivos v1 (15/15)                            |
| Novos provedores desabilitados; nada lido quando desabilitados         | V      | `only_the_providers_of_the_previous_version_start_enabled`, `a_hidden_worker_reads_nothing_until_shown_again`; `Service` nasce com o padrão do catálogo |

### Fases 2–3 — Gemini e Grok

| Item                                      | Gemini                                                           | Grok                                                                  |
| ----------------------------------------- | ---------------------------------------------------------------- | --------------------------------------------------------------------- |
| Adaptador implementado                    | V (`usage/gemini.rs`, 9 testes)                                  | V (`usage/grok.rs`, 8 testes)                                         |
| Autenticação detectada                    | T (fixtures); nesta máquina não há login: `authMissing` real (V) | T (fixtures); nesta máquina não há Grok Build: `authMissing` real (V) |
| Consulta real validada                    | Não aplicável: nenhuma consulta é permitida                      | **N**: consulta desligada até a verificação dos termos                |
| Cotas de assinatura disponíveis           | Não                                                              | Parser pronto e testado; fonte desligada                              |
| Métricas parciais disponíveis             | Não                                                              | Não                                                                   |
| Integração bloqueada por fonte autorizada | **Sim**: termos do Gemini CLI                                    | **Sim**: termos da xAI não verificados (HTTP 403)                     |

Investigação, modalidades, riscos e alternativas: [Compatibilidade dos provedores](provider-compatibility.md). Testes cobrem autenticação presente, ausente, expirada (Grok), conta incompatível (chave de API, Vertex AI), resposta válida, incompleta, malformada, percentuais inválidos, renovação, múltiplas cotas, rede, timeout, 401, 429 e 503 com `Retry-After`, troca de conta e renovação da mesma conta, e ausência de credenciais nas saídas. "Provedor desabilitado" e "isolamento de cache" são cobertos no `Service` e no `Tracker`, que são comuns a todos os adaptadores.

Identidade visual: Gemini com gradiente azul → violeta no arco; Grok monocromático, com aro prateado mesmo recolhido; estados de erro usam um sinalizador próprio, independente da cor do provedor (V, `providers.test.tsx` e inspeção do DOM real).

### Fase 4 — Interface com quatro provedores

| Requisito                                                              | Estado            | Evidência                                                                                                                                                                                                                            |
| ---------------------------------------------------------------------- | ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 0 a 4 visíveis, centralização, espaçamento, sem buracos                | V                 | `validate-providers.ps1` (56/56 em cada lateral): alturas 52/118/184/250, centrado ±1 px, `[codex, grok]` sem buraco                                                                                                                 |
| Recorte Win32 correspondente                                           | V                 | Hit test: centros dos círculos atingem a barra; intervalos entre círculos deixam o clique passar                                                                                                                                     |
| Duas laterais                                                          | V                 | Validações direita e esquerda                                                                                                                                                                                                        |
| Múltiplos monitores, origens negativas, DPI 100/125/150%               | T                 | `secondary_monitors_with_negative_origins_keep_the_rail_on_screen`, `one_to_four_indicators_are_centered_and_evenly_spaced` (3 escalas × 2 lados × 4 quantidades). **N** no hardware: só há um monitor a 100%                        |
| Expansão independente; hover por slot                                  | V                 | `each_indicator_slides_on_its_own`; hover de cada slot abre o resumo alinhado ao slot                                                                                                                                                |
| Reorganização segura ao habilitar/desabilitar, inclusive durante hover | V                 | Cenário "live change": Gemini desativado com o resumo dele aberto → cartão fecha, barra passa a 3, slot 2 (Grok) abre                                                                                                                |
| Zero provedores: barra oculta, bandeja e configurações acessíveis      | V                 | "0 · rail hidden", "process alive"; `validate-settings-ui`: "zero hides the rail" e "shown again from zero". Clicar na bandeja nesse estado abre as configurações (código em `toggle_now`; o clique na bandeja não foi automatizado) |
| Área útil insuficiente                                                 | T                 | `position::capacity` (primeiros do catálogo; aviso nas configurações), `a_short_work_area_shows_only_the_indicators_that_fit`                                                                                                        |
| Always-on-top, sem roubo de foco, click-through, eventos antigos       | V                 | `validate-overlay.ps1` 19/19 (regressão do Ciclo 02); "focus unchanged" em todos os cenários de 1–4                                                                                                                                  |
| Controlador Win32 centralizado; sem threads novas                      | V                 | Nenhuma thread nova; "single owner thread", "no re-entrant batch" no rastreamento                                                                                                                                                    |
| Movimento reduzido                                                     | Preservado, **N** | `useRailMotion` mantém a checagem de `prefers-reduced-motion`; estilos novos respeitam a preferência; não revalidado no Windows                                                                                                      |
| Métrica principal por provedor, identificadores estáveis               | V                 | `Tracker::select_metric`; validação real: Claude Code passou a semanal (33%) na hora                                                                                                                                                 |
| Métrica ausente: estado próprio e retorno ao automático                | V                 | Validação real com `codex-gone`: "—", sinalizador, `metricMissing`; voltar ao automático restaurou 76%                                                                                                                               |

### Fase 5 — Alertas

| Requisito                                                                                 | Estado  | Evidência                                                                                                                     |
| ----------------------------------------------------------------------------------------- | ------- | ----------------------------------------------------------------------------------------------------------------------------- |
| Módulo independente; consome snapshots; nenhuma consulta extra                            | V       | `alerts/`; `Service::observations`; nenhum caminho de rede novo                                                               |
| Limites 70/85/95/100 configuráveis, cruzamento, consolidação, sem repetição               | V (T)   | `alerts::rules::tests` (12 testes)                                                                                            |
| Renovação prevista × atingida × confirmada                                                | T       | `a_renewal_is_announced_only_when_a_new_reading_confirms_it`                                                                  |
| Sessão expirada, indisponível, rejeitada, reconexão opcional                              | T       | `sign_in_problems_alert_once_per_transition_and_reconnection_is_optional`                                                     |
| Deduplicação, isolamento por conta, ordem, demo, desabilitado, reinício, nova habilitação | T       | testes de regras e `alerts::tests::a_restart_starts_from_a_baseline`                                                          |
| Nenhum token ou identificador persistido                                                  | V       | Nada é persistido (política conservadora; ver custo em [Motor de notificações](notification-engine.md#persistência))          |
| Notificações nativas (plugin oficial)                                                     | Parcial | Build de depuração: `send_test_notification` aceito pelo Windows. **N**: exibição do banner no app instalado (ver Limitações) |
| Identificação do app no Windows                                                           | V       | Atalho do instalador com `System.AppUserModel.ID = app.auroracontrol.desktop`                                                 |

### Fase 6 — Configurações

| Requisito                                                          | Estado | Evidência                                                           |
| ------------------------------------------------------------------ | ------ | ------------------------------------------------------------------- |
| Seções Geral, Aparência e posicionamento, Provedores, Notificações | V      | `settings.test.tsx`, `validate-settings-ui.ps1` (23/23)             |
| Estado, diagnóstico, métrica e última atualização por provedor     | V      | `ProviderView`; diagnóstico de qualquer provedor ativo              |
| Horário silencioso (meia-noite, hora local, mudança de fuso)       | T      | `alerts::quiet::tests`; leitura de `chrono::Local` a cada avaliação |
| Nada reproduzido depois do silêncio                                | T      | `quiet_hours_drop_alerts_without_replaying_them_later`              |
| Notificação de teste                                               | V      | Comando `send_test_notification`, só na capability `settings`       |

### Fase 7 — Segurança e confiabilidade

- CSP inalterada; capabilities por janela; `settings` ganhou `get-diagnostics` e `send-test-notification` e perdeu `get-claude-diagnostics`. O teste de ACL real também prova que nenhuma janela alcança `plugin:notification|notify` ou `request_permission` (V). No app instalado, a janela da barra recebeu "not allowed by ACL" ao tentar `send_test_notification` (V).
- Credenciais só no backend; os testes de Gemini e Grok verificam que nenhum valor de token aparece no `Debug` dos resultados nem nos diagnósticos.
- Leituras limitadas: `settings.json` e `oauth_creds.json` até 256 KiB, `google_accounts.json` até 64 KiB, `auth.json` do Grok até 256 KiB. HTTPS só para destinos fixos no código; o do Grok fica desligado.
- Nenhuma execução de comandos, servidor local, escrita em arquivos de provedores, cookies, renovação de token ou acesso a processos.
- Isolamento multiprovedor: workers, trackers, políticas e captura de pânico independentes (inalterados desde o Ciclo 02, agora com quatro slots).

## Testes e qualidade

| Verificação                                                | Resultado                            |
| ---------------------------------------------------------- | ------------------------------------ |
| `npm run format:check`, `npm run lint`, `npx tsc --noEmit` | Aprovados                            |
| `npm test`                                                 | 49 testes, 4 arquivos (eram 28)      |
| `npm run build`                                            | Aprovado                             |
| `cargo fmt --check`                                        | Aprovado                             |
| `cargo clippy --locked --all-targets -- -D warnings`       | Aprovado                             |
| `cargo test --locked`                                      | 184 aprovados, 1 ignorado (eram 132) |
| `npm run desktop:build` (release + NSIS)                   | `Aurora Control_0.3.0_x64-setup.exe` |

## Validação no Windows

Evidências em `docs/evidence/cycle-03-*.txt`, todas em modo demo (sem dados de conta) e com preferências isoladas em `AURORA_CONFIG_DIR`.

| Script                               | Resultado                                                  |
| ------------------------------------ | ---------------------------------------------------------- |
| `validate-providers.ps1 -Side right` | 56/56                                                      |
| `validate-providers.ps1 -Side left`  | 56/56                                                      |
| `validate-overlay.ps1 -Side right`   | 19/19 (também 19/19 à esquerda, fora do arquivo)           |
| `validate-settings.ps1`              | 15/15 (arquivos v1 → migração no app real)                 |
| `validate-settings-ui.ps1`           | 23/23 (valor `Run` restaurado ao estado original: ausente) |

Dados reais, sem modo demo, build de depuração e app instalado:

| Provedor    | Resultado                                                                |
| ----------- | ------------------------------------------------------------------------ |
| Codex       | `available`, origem `api`, semanal 76% e 5 h 4%, plano Plus              |
| Claude Code | `available`, origem `api`, 5 h 41–47% e semanal 32–33%, plano Pro        |
| Gemini      | `authMissing` (sem login do Gemini CLI nesta máquina): estado verdadeiro |
| Grok        | `authMissing` (sem Grok Build nesta máquina): estado verdadeiro          |

Instalador: instalação silenciosa (`/S /D=…\target\validation-cycle-03\install`) com saída 0, binário com `ProductVersion 0.3.0`, atalho no Menu Iniciar com o AppUserModelID do app, entrada "Aurora Control 0.3.0" em Programas. O app instalado iniciou, leu as preferências reais e exibiu Codex e Claude Code com dados reais. A cópia instalada continua em `src-tauri\target\validation-cycle-03\install` para o proprietário testar; ela substituiu o atalho da cópia de validação do Ciclo 02.

## Defeitos encontrados na validação e corrigidos

1. **A troca de métrica não chegava à barra.** O backend aplicava a métrica, mas o snapshot só era republicado na consulta seguinte, em até 5 min. `apply_preferences` agora republica o uso na hora (verificado com dados reais).
2. **Os scripts de validação não isolavam as preferências.** O Tauri resolve a pasta pela API de pastas conhecidas do Windows e ignora `APPDATA`, então definir essa variável não isolava nada (isso já valia para o `validate-overlay.ps1` do Ciclo 02, que só lia). Correção: `AURORA_CONFIG_DIR`, aceita só em builds de depuração, e os quatro scripts passaram a usá-la. `validate-providers.ps1` também verifica, ao final, que o arquivo real não mudou.
3. **Script:** `WindowFromPoint` devolve a janela filha do WebView2; o teste de região agora compara a janela raiz (`GetAncestor`).

## Incidentes e observações

- **Arquivo real de preferências reescrito.** Antes da correção 2, um cenário do `validate-providers.ps1` chamou `set_preferences` no app de depuração e gravou `%APPDATA%\app.auroracontrol.desktop\preferences.json` real no formato v2, com os mesmos valores que já estavam lá (Codex e Claude Code ativos, lateral direita, monitor principal, sem inicialização com o Windows; Gemini e Grok desativados). Tentei regravá-lo no formato v1, mas a ação foi bloqueada pela política de permissões, e não a contornei. Efeito prático: o 0.3.0 lê esse arquivo normalmente. Um retorno ao 0.2.0 o trataria como "versão mais nova", usaria os padrões (que são exatamente esses valores) e não o alteraria. Se o proprietário quiser o formato antigo, basta apagar o arquivo ou salvar pelo 0.2.0.
- **Instância antiga em execução.** Uma cópia do Ciclo 02 (`target\validation-step-a\…\install\aurora-control.exe`) segurava o mutex de instância única, e o binário novo encerrava em silêncio. Ela foi encerrada para a validação.
- **Primeira entrada sintética após mudança de região.** Logo depois de a barra mudar de tamanho com o cursor dentro dela, o WebView2 descarta o primeiro `SetCursorPos` isolado. Com movimento contínuo, como o de um mouse real, o resumo abre na primeira aproximação (2/2). O script usa aproximação em passos nesse cenário. Não foi observado com mouse físico.

## Limitações

- **Gemini e Grok não estão completamente integrados**: nenhuma consulta real de consumo foi validada, porque nenhuma é permitida (Gemini) ou autorizada (Grok). A ativação do Grok depende de uma decisão do proprietário após ler os termos da xAI (`grok::LIVE_QUERY_AUTHORIZED`).
- **Banner de notificação no app instalado não verificado.** O Não incomodar do Windows estava ativo nesta máquina, e disparar o teste no app instalado exige clicar na bandeja (o modo demo não gera alertas, por desenho). Sugestão para o proprietário: instalar, desligar o Não incomodar, abrir Configurações → Notificações → "Enviar notificação de teste".
- O plugin oficial descarta o resultado do WinRT (`show()` assíncrono). Uma recusa do Windows não chega ao app; só falhas anteriores ao envio são reportadas.
- Um limite cruzado com o app fechado não é anunciado depois (política conservadora, sem persistência).
- DPI de 125/150%, monitor secundário e origens negativas: só testes de geometria, pois o hardware disponível tem um único monitor a 100%.
- Builds de desenvolvimento mostram as notificações com a identidade do PowerShell (comportamento do plugin fora da cópia instalada).

## Roteiro de auditoria para o Codex

1. `git diff df47f01..HEAD --stat` e leitura por commit (tabela acima).
2. Rodar as verificações de qualidade da seção "Testes e qualidade".
3. Revisar especialmente:
   - `usage/gemini.rs`: confirmar que nenhum caminho faz rede ou guarda o token; conferir a leitura de `tos-privacy.md` do Gemini CLI.
   - `usage/grok.rs`: `LIVE_QUERY_AUTHORIZED = false` em produção; `parse_billing` contra o `billing.rs` do Grok Build; cabeçalhos enviados (nenhum `x-grok-client-version` falso).
   - `alerts/rules.rs`: linha de base, isolamento por conta, renovação só com leitura nova, ausência de persistência.
   - `preferences.rs`: `migrate` v1 → v2; `debug_config_dir` inerte em release.
   - `windows/mod.rs`: `place_rail` com zero slots, `apply_preferences` (geração nova antes de qualquer hover), `set_rail_motion` com até 4 deslocamentos.
   - `capabilities/settings.json` e `ipc_tests`.
4. No Windows: `npm run desktop:debug` e os scripts de validação da seção anterior.
5. Decisão pendente do proprietário: termos da xAI para liberar a consulta do Grok.
