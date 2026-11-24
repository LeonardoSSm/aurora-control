# Aurora Control

Barra de utilização para Windows 11 com indicadores semicirculares de Codex, Claude Code, Antigravity e Grok / Cursor, e alertas nativos de consumo, renovação e sessão. O app lê a sessão local de cada ferramenta e, onde existe uma fonte autorizada, mostra o consumo real da conta, sem renovar tokens nem alterar arquivos dos provedores.

| Provedor      | Situação na 1.0.0                                                                                                                                                                              |
| ------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Codex         | Cotas reais da conta ChatGPT (5 h, semanal, créditos).                                                                                                                                         |
| Claude Code   | Cotas reais da assinatura (5 h, semanal, por modelo, uso extra) e atividade local.                                                                                                             |
| Antigravity   | Cotas reais (5 h e semanal por grupo de modelos) pelo CLI oficial `agy`, em modo somente leitura, com disjuntor de segurança persistente. Sem identidade de conta: sem alertas de consumo.     |
| Grok / Cursor | Franquia semanal compartilhada por Grok e Cursor, lida do registro que o próprio Grok Build grava a cada consulta dele, com a conta verificada. Tão recente quanto o último uso do Grok Build. |

Guias: [Guia do usuário](docs/user-guide.md) · [Guia de manutenção](docs/maintenance-guide.md) · [Changelog](CHANGELOG.md).

Detalhes em [Compatibilidade dos provedores](docs/provider-compatibility.md).

## Uso

1. Abra `aurora-control.exe`. Os semicírculos dos provedores ativos ficam presos à lateral da área útil do monitor escolhido (por padrão, o principal à direita), centralizados e na ordem Codex, Claude Code, Antigravity, Grok / Cursor. O arco de cada um mostra o consumo da métrica escolhida (por padrão, a janela mais restritiva): vazio em 0%, até o ponto mais interno em 50% e até o topo em 100%. Uma janela que renovou desde a última leitura aparece como "—", aguardando nova medição. Um ponto discreto indica um provedor que precisa de atenção.
2. Pare o ponteiro sobre um indicador. O círculo desliza para dentro e o resumo mostra a conta, a métrica, a renovação, as outras cotas, a atividade local e o estado da conexão. Você pode levar o cursor até o resumo sem que ele feche, e os cliques passam através dele.
3. Clique em um indicador ou use "Atualizar agora" na bandeja para forçar uma leitura. O app consulta a cada 5 minutos e respeita os limites dos provedores: depois de uma recusa de credencial ou de um `Retry-After`, o clique espera o mesmo prazo, a menos que você entre novamente na conta.
4. Clique no ícone da bandeja para mostrar ou ocultar a barra (a escolha é lembrada). O menu de contexto permite atualizar, escolher a lateral, abrir as configurações ou encerrar. Se o ícone estiver recolhido, abra a área de ícones ocultos da bandeja.
5. Em "Configurações…":
   - **Geral:** inicialização com o Windows, visibilidade da barra e versão instalada.
   - **Aparência e posicionamento:** lateral e monitor.
   - **Provedores:** ative cada provedor (Antigravity e Grok / Cursor começam desativados), veja o estado da conexão, a conta, a última leitura e o diagnóstico de autenticação, e escolha a métrica do indicador (automática, janela de 5 h, semanal, cota de um modelo…).
   - **Notificações:** ative os alertas, escolha os limites (70, 85, 95, 100%), alertas de renovação, de sessão e de reconexão, alertas por provedor, horário silencioso e uma notificação de teste.

   As escolhas ficam em `%APPDATA%\app.auroracontrol.desktop\preferences.json`. Preferências da 0.2.0, 0.3.x e anteriores são migradas sem perder nada. Com nenhum provedor ativo, a barra fica oculta, e a bandeja continua disponível. Se o monitor escolhido for desconectado, a barra usa o principal e volta a ele quando for reconectado.

Os alertas só são emitidos quando uma leitura nova cruza um limite, ou quando uma nova consulta confirma a renovação de uma cota. Não há repetição a cada atualização nem depois de reiniciar o app. Regras em [Motor de notificações](docs/notification-engine.md).

Uma segunda execução encerra sem abrir outra barra: só uma instância roda por sessão do Windows.

Requisitos das contas: Codex autenticado com ChatGPT (`codex login`) e Claude Code autenticado no terminal (`claude`). Logins por chave de API não expõem cotas de assinatura, e o app informa isso. Se o Claude Code roda dentro do app Claude Desktop, a credencial não fica no arquivo do CLI e o diagnóstico mostra esse estado. Para o Antigravity, instale o Antigravity CLI (`agy`) e entre com a conta Google; o app executa só `agy -p /usage`, que não gasta cota, e se suspende sozinho se isso mudar. Para o Grok / Cursor, entre no Grok Build (`grok login`) e use-o normalmente: a franquia aparece depois que o Grok Build a consultar (ao abrir, a cada prompt ou com `/usage`).

Variáveis opcionais: `GROK_HOME` muda a pasta do Grok Build, como no próprio CLI. `AURORA_SIDE=left` inicia na esquerda nesta sessão, acima da preferência salva. `AURORA_DEMO=1`, ou `AURORA_DEMO=73,25,82,21` para escolher os percentuais de Codex, Claude Code, Antigravity e Grok, usa dados fictícios sem acessar contas nem rede; dados de demonstração nunca geram alertas. `AURORA_TRACE=1` registra no console os lotes do controlador de janelas. Em builds de depuração, `AURORA_OPEN_SETTINGS=1` abre as configurações ao iniciar e `AURORA_CONFIG_DIR` troca a pasta de preferências e `AURORA_INSTANCE` troca o nome da instância única (os scripts de validação usam isso para não tocar nas suas preferências nem na cópia instalada em execução).

## Compilação no Windows

O build foi preparado para Windows 11 x64 com WebView2, Visual Studio Build Tools 2022 com MSVC e Windows SDK, Node.js 24.15.0, npm 11.12.1 e Rust 1.97.1 (`x86_64-pc-windows-msvc`). Os lockfiles fixam as dependências. No primeiro build, o Tauri pode baixar as ferramentas do instalador NSIS.

```powershell
npm ci
npm run format:check
npm run lint
npm test
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml
npm run desktop:build
```

A CI (`.github/workflows/ci.yml`) roda essas mesmas verificações em cada Pull Request, num runner Windows, com permissão só de leitura e sem credenciais, incluindo o build de produção com o instalador NSIS. Validações interativas, que exigem uma sessão de desktop:

```powershell
pwsh -File scripts/validate-overlay.ps1 -Exe src-tauri/target/debug/aurora-control.exe -Side right
pwsh -File scripts/validate-providers.ps1 -Exe src-tauri/target/debug/aurora-control.exe -Side right
pwsh -File scripts/validate-settings.ps1 -Exe src-tauri/target/debug/aurora-control.exe
pwsh -File scripts/validate-settings-ui.ps1 -Exe src-tauri/target/debug/aurora-control.exe
pwsh -File scripts/capture-toast.ps1 -Exe src-tauri/target/debug/aurora-control.exe -Out toast.png
```

O executável fica em `src-tauri/target/release/aurora-control.exe` e o instalador NSIS em `src-tauri/target/release/bundle/nsis/`. O instalador é gerado **sem assinatura de código** (o Windows SmartScreen pode alertar) e instala para o usuário atual; confira o SHA-256 publicado com a versão. Se necessário, o instalador obtém o WebView2 pelo bootstrapper da Microsoft. As notificações aparecem com o nome do app só na cópia instalada, porque o atalho do instalador registra a identidade `app.auroracontrol.desktop`.

Use `npm run desktop:debug` para compilar o binário de depuração em `src-tauri/target/debug/aurora-control.exe`. Os builds usam os recursos produzidos em `dist/` e empacotados pelo Tauri.

- [Decisões de arquitetura](docs/architecture.md)
- [Compatibilidade dos provedores](docs/provider-compatibility.md)
- [Motor de notificações](docs/notification-engine.md)
- [Ciclo 04: implementação](docs/cycle-04/implementation-report.md), [validação](docs/cycle-04/validation-report.md), [auditoria de segurança](docs/cycle-04/security-audit.md) e [passagem de manutenção](docs/cycle-04/final-handoff.md)
- [Ciclo 03.1: implementação](docs/cycle-03-1/implementation-report.md) e [validação](docs/cycle-03-1/validation-report.md)
- [Implementação do Ciclo 03 para auditoria](docs/audit-cycle-03-implementation.md)
- [Implementação do Ciclo 02 para auditoria](docs/audit-cycle-02-implementation.md)
- [Auditoria da baseline do Ciclo 02](docs/audit-cycle-02-baseline.md)
- [Relatório da Etapa B para auditoria](docs/audit-cycle-01-stage-b.md)
- [Histórico da Etapa A](docs/handoff-claude.md)
