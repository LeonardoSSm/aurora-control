# Continuação com Claude Code

## Ponto de partida

Trabalhar a partir de `feat/cycle-01-shell`. O [README](../README.md) reúne os comandos de verificação e de build no Windows, e a [arquitetura](architecture.md) descreve o IPC e o gerenciamento de janelas. O adaptador selecionado na inicialização é `DemoAdapter`, em `src-tauri/src/main.rs`.

## Contratos existentes

- `src-tauri/src/usage/model.rs` e `src/usage/model.ts` definem o formato de `Usage` e `Snapshot`. Os identificadores são `codex` e `claudeCode`. Cada leitura tem origem `demo` ou `live`, conta de exibição e horários em RFC 3339. Conta, janela de consumo, renovação e última atualização aceitam `null` quando desconhecidas.
- `percentUsed` representa a porcentagem consumida entre 0 e 100. `available` e `stale` incluem esse campo. `stale` preserva a última leitura válida e `unavailable` omite o percentual. A interface exibe uma leitura por provedor.
- `UsageAdapter::snapshot()` entrega o estado atual ao `Service`. A barra chama `get_usage` e o resumo recebe a mesma estrutura `Usage` pelo evento `summary-selection`. O teste `demo_serializes_the_wire_contract` fixa o formato serializado.

## Próxima etapa

1. Confirmar com o proprietário a fonte autorizada, o método de autenticação e os campos disponíveis de cada provedor antes de criar os adaptadores.
2. Implementar os adaptadores Codex e Claude Code em módulos Rust separados dentro de `src-tauri/src/usage/`. Manter a consulta autenticada no Rust. Atualizar as leituras em segundo plano para servir o último snapshot pelo IPC sem bloquear o hover.
3. Converter os dados de cada fonte para porcentagem consumida e distinguir indisponibilidade de zero. Usar `stale` quando uma atualização falhar após uma leitura válida e `unavailable` quando não houver leitura válida. O contrato aceita horários desconhecidos com `null`. Propagar alterações de formato nos dois modelos e no teste de serialização.
4. Manter o `DemoAdapter` para testes. Validar cada adaptador com dados de teste em sucesso, falha e renovação, além de retestar a bandeja e o hover após alterações no IPC.

## Auditoria da captura

A captura anterior ficou visível na saída da ferramenta de automação neste chat. Existe um registro local correspondente pelo identificador da sessão em `.codex/sessions/`. A auditoria verificou o caminho do registro sem abrir seu conteúdo, portanto a presença da imagem nesse arquivo permanece indeterminada.

No repositório de origem, Aurora Board, `git check-ignore -v` confirmou a regra de `.gitignore` para `.env`. A consulta de caminhos no índice e no histórico local disponível por referências e reflogs não encontrou o arquivo. No Aurora Control, a mesma consulta não encontrou `.env` nem arquivos de imagem da captura. A regra `/.env` agora também protege a raiz do Aurora Control. A auditoria foi por nomes de arquivos e metadados do Git.

As credenciais potencialmente expostas são uma chave de API da OpenAI, uma chave de API do Gemini e um `Client Secret` do Pluggy. O proprietário deve rotacionar os valores que ainda estiverem ativos.

- **OpenAI e Gemini.** Gerar novas chaves nos respectivos consoles, atualizar os serviços, verificar o funcionamento, revogar as chaves anteriores e revisar uso e cobrança desde a captura. [OpenAI](https://help.openai.com/en/articles/5112595-best-practices-for-api-key-safety), [Google Cloud](https://docs.cloud.google.com/docs/security/compromised-credentials)
- **Pluggy.** Preparar a atualização dos serviços antes de gerar um novo `Client Secret`. A geração invalida o segredo atual imediatamente. As chaves de API já emitidas mantêm a validade até expirar. Revisar a atividade da aplicação desde a exposição. [Pluggy](https://docs.pluggy.ai/en/docs/security/credentials)

## Validação anterior ao redesenho dos indicadores

Teste do binário instalado no Windows 11 x64, build 26300, com área útil primária de 1920 × 1032 e escala de 100%. O Chrome usou um perfil descartável e uma página local com contador de cliques. As evidências vieram das APIs Win32 e da acessibilidade da bandeja, sem novas imagens.

| Verificação           | Resultado                                                                                                                                                                                                                                                                                                                                               |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Chrome maximizado     | `IsZoomed` confirmou a maximização. A barra em `(1814, 411)` ficou acima do Chrome na ordem Z e recebeu eventos nas coordenadas dos indicadores.                                                                                                                                                                                                        |
| Foco e hover          | O mesmo identificador da janela do Chrome permaneceu em primeiro plano durante a abertura da barra e do resumo e depois de retirar o ponteiro. Codex e Claude Code abriram para dentro da tela. A barra permaneceu visível ao fechar o resumo.                                                                                                          |
| Cliques fora da barra | O Chrome recebeu cliques ao lado da barra. Com o resumo aberto, o teste de destino do ponteiro apontou para o Chrome sob o resumo. O teste reproduziu o bloqueio nos cantos transparentes da barra original. Após a correção da região nativa, os quatro cantos passaram no teste de destino e a página registrou um clique no canto superior esquerdo. |
| Bandeja               | O clique esquerdo e a ação `Mostrar / ocultar` alternaram a visibilidade. As duas ações de posição moveram a barra. Selecionar a esquerda com a barra oculta preservou o estado oculto até exibi-la novamente. `Encerrar` terminou o processo com código 0.                                                                                             |
| Esquerda e direita    | À esquerda, a barra ficou em `x=10` e o resumo abriu à direita em `x=114`. Ao voltar para a direita, a barra ficou em `x=1814`.                                                                                                                                                                                                                         |
| NSIS                  | Instalação silenciosa em diretório temporário retornou código 0 e registrou a versão 0.1.0. O executável instalado abriu a barra. A desinstalação retornou código 0 e removeu o registro e o executável de teste.                                                                                                                                       |

Os comandos `npm run format:check`, `npm run lint`, `npm test`, `cargo fmt --check`, `cargo clippy --locked --offline --all-targets -- -D warnings`, `cargo test --locked --offline` e `npm run desktop:build` passaram. Foram cinco testes de interface e seis testes Rust. O instalador foi testado na conta Windows atual, com diretório de instalação e perfil WebView2 de teste. A disponibilidade do Windows Sandbox não pôde ser confirmada sem privilégios de administrador. A escala fracionária foi coberta no teste de geometria. Ainda falta repetir a medição interativa com escala acima de 100%.

## Validação após o redesenho

No Windows 11, com o Chrome maximizado, o executável recolheu e expandiu cada indicador independentemente nos dois lados. O resumo abriu para dentro da área útil, o Chrome manteve o foco e recebeu os cliques fora dos semicírculos. A troca de lado pelo menu nativo e o encerramento passaram. A suíte atual tem seis testes de interface e sete testes Rust.
