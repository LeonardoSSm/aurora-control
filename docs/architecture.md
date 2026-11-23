# Arquitetura do Aurora Control

## Divisão de responsabilidades

| Módulo                                               | Responsabilidade                                                   |
| ---------------------------------------------------- | ------------------------------------------------------------------ |
| `src/components/`, `src/rail.tsx`, `src/summary.tsx` | Barra, círculos, rótulos e resumo em React com Tailwind CSS.       |
| `src/usage/`                                         | Contrato de utilização em TypeScript e funções de apresentação.    |
| `src/desktop/commands.ts`                            | Ponte tipada entre React e o IPC do Tauri.                         |
| `src-tauri/src/usage/`                               | Contrato serializado em Rust, serviço e adaptador de demonstração. |
| `src-tauri/src/windows/`                             | Ciclo do resumo, posicionamento e operações nativas Win32.         |
| `src-tauri/src/tray.rs`                              | Ícone e ações nativas da bandeja.                                  |

## Janelas e foco

O Tauri mantém duas janelas sem decoração, `rail` e `summary`. A janela da barra mede até 64 × 164 pixels lógicos e fica presa à borda da área útil do monitor principal. Cada indicador expõe um semicírculo de 38 × 76 pixels lógicos em repouso. Ao receber o ponteiro, apenas esse indicador cresce para dentro da tela. O resumo tem até 274 × 192 pixels lógicos e abre ao lado do indicador. `railGeometry.ts` e `windows/position.rs` compartilham as dimensões lógicas. A posição se ajusta aos dois lados e à escala do monitor.

As duas janelas são inicialmente ocultas e não focalizáveis. O código nativo aplica `WS_EX_NOACTIVATE` e usa `SetWindowPos` com `HWND_TOPMOST` e `SWP_NOACTIVATE` para posicionar e revelar cada janela. A região Win32 da barra é a união dos dois indicadores recortada à área da janela. A animação atualiza essa região junto com o desenho, deixando livres o espaço entre os indicadores e os cantos transparentes. O resumo ignora o ponteiro. A posição pertence ao estado em memória e começa na direita. [SetWindowPos](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowpos), [CombineRgn](https://learn.microsoft.com/en-us/windows/win32/gdi/combining-regions), [SetWindowRgn](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowrgn)

No início do hover, `rail` envia o provedor ao Rust. Depois de registrar o listener, `summary` sinaliza que está pronto. O Rust envia o dado e a revisão da seleção apenas a esse WebView. O React confirma a renderização antes de o Rust mostrar a janela. Sair do indicador, ocultar a barra ou trocar de lateral invalida a seleção e fecha o resumo. Uma confirmação atrasada de uma seleção invalidada não reabre a janela.

O evento `rail-placement` atualiza o lado e a geração da posição. Quadros de expansão de gerações anteriores são descartados. O frontend aplica apenas uma largura confirmada pelo Rust e mantém uma atualização nativa em andamento por vez. A expansão e a animação do resumo respeitam a preferência de movimento reduzido.

## Contrato de utilização

O `Usage` em `src/usage/model.ts` espelha `src-tauri/src/usage/model.rs`. O Rust serializa os campos em `camelCase`.

| Campo         | Semântica                                                                     |
| ------------- | ----------------------------------------------------------------------------- |
| `provider`    | `codex` ou `claudeCode`.                                                      |
| `account`     | Rótulo de conta próprio para exibição, ou `null`.                             |
| `source`      | `demo` ou `live`. A interface sinaliza a origem fictícia.                     |
| `status`      | `available`, `stale` ou `unavailable`.                                        |
| `percentUsed` | Percentual **consumido**, entre 0 e 100. Presente em `available` e `stale`.   |
| `window`      | `durationMinutes` e `resetsAt` quando houver uma janela conhecida, ou `null`. |
| `updatedAt`   | Horário da leitura bem-sucedida mais recente, ou `null`.                      |

Datas no IPC seguem RFC 3339 com deslocamento UTC. O frontend as apresenta em português no horário local do Windows. A Etapa A exibe uma leitura por provedor. `UsageAdapter::snapshot()` entrega a leitura atual, e `Service` concentra o acesso ao adaptador. O `DemoAdapter` fornece 64% e 37% fixos, sem acesso externo. `stale` mantém a última leitura disponível. A interface representa dados indisponíveis por um travessão.

## Segurança e empacotamento

Vite gera duas páginas estáticas. O Tauri as empacota e serve pelo protocolo interno do WebView2. `tauri.localhost` e `ipc.localhost` são origens virtuais do WebView2. A aplicação e o fluxo documentado de desenvolvimento usam recursos empacotados sem servidor HTTP local.

A CSP parte de `default-src 'none'` e libera scripts, estilos e imagens da própria origem, além do IPC. Cada janela aceita navegação somente para a própria página empacotada. `withGlobalTauri` permanece desativado. O manifesto mapeia comandos a permissões nominadas, e as capabilities separam as permissões de `rail` e `summary`. Credenciais, hooks de outras ferramentas e atualização automática ficam fora da Etapa A. [Capabilities do Tauri](https://v2.tauri.app/security/capabilities/)
