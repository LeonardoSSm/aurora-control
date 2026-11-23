# Aurora Control

Barra de utilização para Windows 11 com indicadores de Codex e Claude Code. O app lê a sessão local de cada ferramenta e consulta o consumo real da conta, sem renovar tokens nem alterar arquivos dos provedores.

## Uso

1. Abra `aurora-control.exe`. Os semicírculos de Codex e Claude Code ficam presos à lateral direita da área útil do monitor principal. O arco de cada um mostra o consumo da janela mais restritiva: vazio em 0%, até o ponto mais interno em 50% e até o topo em 100%.
2. Pare o ponteiro sobre um indicador. O círculo desliza para dentro e o resumo mostra a conta, a janela principal, a renovação, as outras cotas, a atividade local e o estado da conexão. Você pode levar o cursor até o resumo sem que ele feche, e os cliques passam através dele.
3. Clique em um indicador ou use "Atualizar agora" na bandeja para forçar uma leitura. O app consulta a cada 5 minutos e respeita os limites dos provedores.
4. Clique no ícone da bandeja para mostrar ou ocultar a barra. O menu de contexto permite escolher a lateral, atualizar ou encerrar. Se o ícone estiver recolhido, abra a área de ícones ocultos da bandeja.

Requisitos das contas: Codex autenticado com ChatGPT (`codex login`) e Claude Code autenticado no terminal (`claude`). Logins por chave de API não expõem cotas de assinatura, e o app informa isso.

Variáveis opcionais: `AURORA_SIDE=left` inicia na esquerda. `AURORA_DEMO=1`, ou `AURORA_DEMO=73,25` para escolher os percentuais, usa dados fictícios sem acessar contas nem rede.

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
- [Relatório da Etapa B para auditoria](docs/audit-cycle-01-stage-b.md)
- [Histórico da Etapa A](docs/handoff-claude.md)
