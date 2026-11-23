# Ciclo 01, Etapa B: relatório técnico para auditoria

Branch `feat/cycle-01-refinement`, criada a partir de `de4dac4`. O primeiro commit da branch preserva o redesenho que o Codex deixou sem commit na árvore de trabalho. Antes da importação, os 6 testes de interface, os 7 testes Rust, o ESLint, o Prettier e o rustfmt passaram. Não houve merge na `main`.

## Escopo entregue

| Item                         | Estado                                                                 | Onde                                                                   |
| ---------------------------- | ---------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| Indicadores semicirculares   | Entregue e validado                                                    | `railGeometry.ts`, `UsageRing.tsx`, `position.rs`                      |
| Hover e resumos              | Entregue e validado                                                    | `windows/hover.rs`, `windows/mod.rs`, `UsageSummary.tsx`, `styles.css` |
| Adaptador Codex              | Entregue e validado com dados reais                                    | `usage/codex.rs`                                                       |
| Adaptador Claude Code        | Entregue. Dados reais **não** validados nesta máquina (ver limitações) | `usage/claude.rs`                                                      |
| Atualização, cache e backoff | Entregue e coberto por testes                                          | `usage/scheduler.rs`, `usage/mod.rs`                                   |
| Executável e instalador NSIS | Gerados e testados                                                     | `src-tauri/target/release/`                                            |

## Decisões que merecem revisão

1. **Métrica principal = janela mais restritiva.** O círculo mostra a janela com maior consumo, e o rótulo (`5 h`/`7 d` no círculo, nome no resumo) a identifica. Motivo: com 0% em 5 h e 73% na semanal, os dados reais desta máquina, mostrar 0% seria enganoso. A alternativa, sempre a janela de 5 h, é uma linha em `adapter::primary_index`.
2. **Endpoints internos.** Não existe API documentada para cotas de assinatura de nenhum dos dois provedores. Os endpoints usados são os mesmos dos clientes oficiais, e o schema fica isolado em `parse_usage`. Ver "Riscos de compatibilidade" em [architecture.md](architecture.md).
3. **Sem renovação de token.** O Codenotch renova a credencial do Claude executando o binário `claude`. Essa abordagem foi rejeitada de propósito, porque viola as restrições da etapa: sem renovação própria e sem execução de comandos.
4. **Resumo click-through.** O cartão ignora o ponteiro, como na base do Codex. O "repouso" do cursor sobre ele é detectado pela posição real do cursor, não por eventos do WebView. Assim o cartão nunca bloqueia cliques.
5. **Estilo `WS_EX_TOOLWINDOW`.** Foi adicionado às duas janelas, e `WS_EX_APPWINDOW` foi removido. Também foi adicionada restauração sem ativação quando algo externo minimiza a barra (ver defeito D3).
6. **E-mail mascarado** na interface, porque a janela fica sempre visível e aparece em compartilhamento de tela.
7. **Dependências novas:** `ureq` 3 (com `native-tls`/Schannel, sem `ring` nem `aws-lc`), além de `chrono` e `base64`, que já estavam no lockfile pelo Tauri, e `serde_json` promovido a dependência normal.

## Defeitos encontrados durante a validação real e corrigidos

| ID  | Defeito                                                                                                  | Evidência                                                                           | Correção                                                                                                      |
| --- | -------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| D1  | Varredura vertical pela borda a 358–491 px/s abria resumos, e a troca imediata "arrastava" a abertura.   | Medido com `SetCursorPos` em processo único e controle positivo.                    | Intenção de 150 ms para abrir e de 120 ms para trocar. Depois: 0 aberturas a 358 e 491 px/s.                  |
| D2  | O número "100" recolhido encostava no arco, e o vermelho crítico se confundia com o terracota do Claude. | Captura ampliada 4×.                                                                | Fonte de 8,5 px para três dígitos. O crítico passou para o número e o aro, e o arco mantém a cor do provedor. |
| D3  | Barra minimizada por agente externo: `SWP_SHOWWINDOW` não restaurava, e a barra sumia.                   | `GetWindowRect` = (−32000, −32000) após a ferramenta de captura minimizar a janela. | `WS_EX_TOOLWINDOW`, `SW_SHOWNOACTIVATE` em `place()` e restauração automática no `Resized`.                   |
| D4  | `formatSpan(30 s)` exibia "1 min".                                                                       | Teste unitário.                                                                     | "menos de 1 min".                                                                                             |

Um aviso sobre o meu próprio processo de teste: a primeira bateria de varreduras usou `FindWindowW` sem `CharSet.Unicode`, recebeu um handle nulo e relatou "0 aberturas" falsamente. O controle positivo, adicionado depois, expôs o erro. Os números de D1 vêm da bateria corrigida.

## Validação no Windows

Ambiente: Windows 11 Pro 26300, monitor primário 1920 × 1080 com área útil 1920 × 1032, escala de 100% e um segundo monitor acima. Evidências obtidas por APIs Win32 (`GetWindowRect`, `GetWindowRgnBox`, `WindowFromPoint`, `GetForegroundWindow`, `IsZoomed`). As capturas foram feitas só da região da barra e do cartão, em modo demonstração (`AURORA_DEMO`), e revisadas antes do uso. A leitura real mostrou apenas o e-mail mascarado.

| Verificação                     | Resultado                                                                                                                                                                        |
| ------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Recolhido, direita              | Barra em (1860, 457) 60 × 118. Região nos dois semicírculos (x 33–60). Arco de 73% e 25% visualmente correto.                                                                    |
| Expandido, direita              | Região do círculo inteiro (x 1–60). Resumo de 296 × 245 centralizado no indicador.                                                                                               |
| Recolhido e expandido, esquerda | Barra em x = 0, região recolhida em 0–26 e resumo em x = 68. Extremos 0% e 100% espelhados.                                                                                      |
| Percentuais                     | 0, 25, 73, 92 e 100% capturados. Tom crítico em 92% e 100%.                                                                                                                      |
| Hover para o cartão             | Travessia lenta (~720 ms) mantém o cartão aberto, assim como o cursor parado nele. Ainda visível aos 150 ms após sair, fechado aos 650 ms.                                       |
| Clique-através                  | `WindowFromPoint` sob o cartão, nos cantos da barra e no vão entre indicadores devolve o app de trás.                                                                            |
| Foco                            | O mesmo HWND em primeiro plano antes, durante e depois do hover, inclusive sobre o Bloco de notas maximizado (`IsZoomed`).                                                       |
| Always-on-top                   | Sobre o app maximizado, o ponto do semicírculo pertence ao Aurora Control.                                                                                                       |
| Bandeja                         | Clique esquerdo oculta, mantendo a posição, e mostra de novo, com o hover funcionando em seguida.                                                                                |
| Minimização externa             | `SW_MINIMIZE` externo restaurado em < 30 ms, sem mudança de foco.                                                                                                                |
| Codex real                      | 73% semanal como principal e 0% na janela de 5 h, renovações reais, "Conectado". Plano Plus e e-mail mascarado.                                                                  |
| Claude Code real                | `authMissing` correto: o arquivo do CLI tem `accessToken` vazio e `expiresAt` 0, porque o app desktop encerrou a sessão do CLI. Atividade local: 751,8 mil tokens em 2 sessões.  |
| NSIS                            | Instalação silenciosa com código 0 e registro da versão 0.1.0. O binário instalado abriu e o hover funcionou. Desinstalação com código 0, sem executável nem registro restantes. |

Comandos aprovados: `npm run format:check`, `npm run lint`, `npm test` (15 testes), `npm run build`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (47 testes) e `npm run desktop:build`.

Artefatos: `src-tauri/target/release/aurora-control.exe` (9,3 MB) e `src-tauri/target/release/bundle/nsis/Aurora Control_0.1.0_x64-setup.exe` (2,2 MB, SHA-256 `F3B36890198860D0037A2B3D25578B4DD7DF1701E403ADE210E8B40C654DEB2C`). Ambos sem assinatura de código.

## Não validado (pendências honestas)

- **Cotas reais do Claude Code.** Nesta máquina, o Claude Code roda dentro do app desktop, que injeta a credencial no processo e zerou o arquivo do CLI. O parser foi validado com fixtures do formato publicado e com o 401 real. O caminho de sucesso com dados reais precisa de uma máquina com `claude` autenticado no terminal.
- **Troca de lateral pela bandeja em execução.** O menu de contexto exige botão direito no Explorer, que a ferramenta de automação concede apenas no nível "clique". A geometria da esquerda foi validada com `AURORA_SIDE=left`. O código de `set_side` não mudou, exceto pelo fechamento do hover.
- **Escala de DPI acima de 100%.** Alterar a escala do sistema está fora do que posso fazer. A cobertura vem de testes de geometria a 125% e 150%.
- **Movimento reduzido.** O caminho de código existe (CSS e `matchMedia`) e não foi exercitado ao vivo.
- **Chrome e VS Code maximizados.** Foram testados indiretamente: o Bloco de notas maximizado e o app em primeiro plano cobrem o mesmo mecanismo (`HWND_TOPMOST` e `WS_EX_NOACTIVATE`). O Chrome não foi aberto, para não tocar no perfil do usuário.
- **Commits intermediários.** Os commits de backend e frontend foram separados por tema, mas só o último commit da série compila e passa em todos os testes. Os anteriores dependem de comandos IPC que mudaram juntos.

## Pontos sugeridos para auditoria

1. `usage/scheduler.rs`: limites de backoff, `Retry-After` aplicado à atualização manual e regra de dados desatualizados.
2. `windows/mod.rs`: todas as operações de janela ocorrem na thread principal, e o watchdog só agenda. Revisar a possibilidade de deadlock entre o mutex `State` e `run_on_main_thread`.
3. `codex.rs` e `claude.rs`: os tokens nunca entram em `Outcome`, `Fetched`, `Usage` nem em logs. As mensagens de erro não incluem corpo de resposta.
4. Capabilities: `rail` ganhou `refresh-usage` e `rail-pointer` e perdeu `set-hover` e `set-rail-expansion`.
