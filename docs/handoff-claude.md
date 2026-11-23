# Continuação com Claude Code

## Ponto de partida

Trabalhar a partir de `feat/cycle-01-shell`. O [README](../README.md) contém os comandos de verificação e de build no Windows, e a [arquitetura](architecture.md) descreve o contrato dado ao frontend. O adaptador selecionado na inicialização é `DemoAdapter`, em `src-tauri/src/main.rs`.

## Próxima etapa

1. Aguardar a definição da próxima etapa e das fontes autorizadas para Codex e Claude Code. A disponibilidade de percentual, janela e renovação depende da fonte aprovada para cada provedor.
2. Implementar adaptadores Rust separados em `src-tauri/src/usage/`. Manter as consultas fora do WebView e a leitura de `snapshot()` rápida. Caso haja atualização assíncrona, atualizar o estado do serviço em segundo plano e entregar ao IPC o snapshot mais recente.
3. Preservar a semântica de percentual utilizado e os estados `available`, `stale` e `unavailable`. Alinhar os contratos Rust e TypeScript e o teste de serialização ao alterar o formato.
4. Manter o adaptador fictício disponível para testes e validar o fluxo da bandeja e do hover após qualquer alteração na comunicação entre janelas.

## Validação visual

No Windows 11 x64 (build 26300), o executável exibiu os dois círculos na direita. Os dois resumos abriram à esquerda da barra, trocaram de conteúdo conforme o indicador e fecharam ao sair dele, com a barra ainda visível. A barra apareceu sobre o VS Code maximizado e foi enumerada acima da janela do Chrome depois de ativá-lo.

Ainda falta confirmar manualmente a preservação do foco ao iniciar e ao interagir com a barra, bem como testar as ações da bandeja e o resumo com a barra na esquerda. A ferramenta de automação usada não ofereceu um alvo para a bandeja. Também vale repetir o teste de sobreposição com o Chrome maximizado, verificar a geometria com escala acima de 100% e executar o instalador em um perfil Windows de teste.
