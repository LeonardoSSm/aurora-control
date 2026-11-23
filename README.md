# Aurora Control

Barra de utilização para Windows 11 com indicadores de Codex e Claude Code. A versão 0.1.0 usa dados fictícios, sinalizados na barra e nos resumos.

## Uso

1. Abra `aurora-control.exe`. A barra inicia na lateral direita da área útil do monitor principal.
2. Passe o ponteiro sobre um indicador para ver a conta, a janela de consumo, a renovação e a última atualização. O resumo abre para dentro da tela.
3. Clique no ícone da bandeja para mostrar ou ocultar a barra. O menu de contexto permite escolher uma lateral ou encerrar o app. Se o ícone estiver recolhido, abra a área de ícones ocultos da bandeja.

A lateral escolhida vale durante a execução. Cada nova execução começa na direita.

## Compilação no Windows

O build foi preparado para Windows 11 x64 com WebView2, Visual Studio Build Tools 2022 com MSVC e Windows SDK, Node.js 24.15.0, npm 11.12.1 e Rust 1.97.1 (`x86_64-pc-windows-msvc`). Os lockfiles fixam as dependências. No primeiro build, o Tauri pode baixar as ferramentas do instalador NSIS.

```powershell
npm ci
npm run format:check
npm run lint
npm test
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml
npm run desktop:build
```

O executável fica em `src-tauri/target/release/aurora-control.exe` e o instalador NSIS em `src-tauri/target/release/bundle/nsis/`. O pacote de teste é gerado sem assinatura de código e instala para o usuário atual. Se necessário, o instalador obtém o WebView2 pelo bootstrapper da Microsoft.

Use `npm run desktop:debug` para compilar o binário de depuração em `src-tauri/target/debug/aurora-control.exe`. Os builds usam os recursos produzidos em `dist/` e empacotados pelo Tauri.

- [Decisões de arquitetura](docs/architecture.md)
- [Continuação com Claude Code](docs/handoff-claude.md)
