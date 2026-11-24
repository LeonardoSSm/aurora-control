# Changelog

Formato baseado em [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/). Versões seguem [SemVer](https://semver.org/lang/pt-BR/).

## [1.0.0] — candidata, 24/11/2026

Primeira versão estável proposta. Pendente de aprovação do proprietário e da auditoria independente.

### Adicionado

- **Grok / Cursor:** indicador da franquia semanal compartilhada por Grok e Cursor, lido do registro que o próprio Grok Build grava a cada consulta (`$GROK_HOME/logs/unified.jsonl`). Somente leitura, sem token, sem rede. A conta do registro precisa ser a do login atual; o rótulo "compartilhada" só aparece com cobrança unificada confirmada.
- Origem `clientLog` ("registro do cliente oficial") e estado `awaitingClient` ("abra o Grok Build para atualizar"), com marcador neutro no indicador.
- **Antigravity:** disjuntor de segurança persistente entre reinicializações, com reativação explícita em duas etapas nas configurações (`reset_provider_safety`, só na janela de configurações).
- Execução de CLI com Job Object do Windows: toda a árvore de processos termina com a consulta, no cancelamento, no timeout e no encerramento do app.
- `CHANGELOG.md`, guia do usuário, guia de manutenção e documentação do Ciclo 04.

### Alterado

- "Grok" passa a se chamar "Grok / Cursor" (o identificador `grok` e as preferências são preservados). Cadência de leitura local: 60 s, com nova varredura só quando o arquivo muda.
- O disjuntor do Antigravity também dispara quando o CLI sai com sucesso sem provar que não executou rodada do modelo (sem JSON, sem contadores ou outro comando).
- Desativar o Antigravity invalida a tentativa em andamento mesmo que ele seja reativado logo em seguida.

### Removido

- Estado `sourceBlocked: termsUnverified` do Grok (a consulta direta continua implementada e desligada, `LIVE_QUERY_AUTHORIZED = false`).

### Corrigido

- Saída do `agy` sem contadores era tratada como zero rodadas.
- Um descendente do CLI que herdasse a saída podia prender a consulta até terminar.
- Scripts de validação: versão esperada lida do `package.json`; caminho absoluto em `capture-toast.ps1`.

### Segurança

- `npm audit`: 0 vulnerabilidades. `cargo audit`: ver [auditoria](docs/cycle-04/security-audit.md).
- Instalador sem assinatura de código; SHA-256 publicado com a versão.

## [0.3.1] — 24/11/2026

- Antigravity pelo CLI oficial (`agy -p /usage`), substituindo o Gemini; disjuntor em memória; leituras sem identidade expiram após falha.
- Investigação de Grok e Cursor; preferências v3.

## [0.3.0] — 24/11/2026

- Catálogo de provedores, zero a quatro indicadores, escolha de métrica, motor de notificações com horário silencioso, preferências v2, CI com instalador NSIS.

## [0.2.0] — 23/11/2026

- Janela de configurações, preferências persistentes, múltiplos monitores, correções da auditoria do Ciclo 01, CI.

## [0.1.0] — 23/11/2026

- Barra semicircular, adaptadores reais de Codex e Claude Code, agendamento com backoff e `Retry-After`.
