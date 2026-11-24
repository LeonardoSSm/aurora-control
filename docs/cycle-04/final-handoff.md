# Passagem de manutenção — Aurora Control 1.0.0

Este documento basta para assumir o projeto. Leia-o junto com o [Guia de manutenção](../maintenance-guide.md).

## O que é

Barra de utilização para Windows 11 (Tauri 2 + React + Rust). Quatro semicírculos na lateral da tela mostram o consumo das cotas de Codex, Claude Code, Antigravity e Grok / Cursor, com resumo no hover, configurações, alertas nativos e instalador NSIS por usuário.

## Estado de cada funcionalidade

| Funcionalidade                    | Estado                                                                                                                                               |
| --------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| Codex                             | Operacional. Endpoint interno do cliente oficial; fallback no rollout local                                                                          |
| Claude Code                       | Operacional. Endpoint interno do cliente oficial; atividade local de 5 h                                                                             |
| Antigravity                       | Operacional, desativado por padrão. CLI oficial `agy -p /usage`; Job Object; disjuntor persistente; sem identidade de conta (sem alertas de consumo) |
| Grok / Cursor                     | Operacional, desativado por padrão. Registro local do Grok Build; conta verificada; tão recente quanto o último uso do Grok Build                    |
| Indicadores, hover, resumo        | Operacional; validado por scripts (0 a 4 indicadores, duas laterais)                                                                                 |
| Configurações e preferências (v3) | Operacional; migração v1/v2 → v3 em memória, gravação só ao salvar                                                                                   |
| Monitores e lateral               | Operacional; monitor desconectado cai para o principal e volta                                                                                       |
| Inicialização com o Windows       | Operacional (valor `Run` em HKCU), restauração em falha                                                                                              |
| Alertas e horário silencioso      | Operacional; só leituras recentes de contas verificadas                                                                                              |
| Diagnósticos                      | Operacional; somente leitura, sem valores sensíveis                                                                                                  |
| Instalador                        | NSIS por usuário, **sem assinatura**                                                                                                                 |
| CI                                | GitHub Actions, Windows, somente leitura, inclui o instalador                                                                                        |

## Fontes de métricas

| Provedor      | Fonte                                                                                          | Identidade                                          | Rede feita pelo app |
| ------------- | ---------------------------------------------------------------------------------------------- | --------------------------------------------------- | ------------------- |
| Codex         | `GET chatgpt.com/backend-api/wham/usage`                                                       | `account_id` + `sub` do `id_token`                  | Sim                 |
| Claude Code   | `GET api.anthropic.com/api/oauth/usage`                                                        | `accountUuid` + `organizationUuid`                  | Sim                 |
| Antigravity   | `%LOCALAPPDATA%\agy\bin\agy.exe -p /usage --output-format json` (+ `--settings`, `--log-file`) | Nenhuma                                             | Não (o CLI faz)     |
| Grok / Cursor | `$GROK_HOME/logs/unified.jsonl`, evento `billing: fetched credits config`                      | `user_id` do mesmo `pid` = `user_id` do `auth.json` | Não                 |

## Decisões que não devem ser desfeitas sem nova análise

1. Nenhum dado ausente vira 0%.
2. Leituras ficam presas à identidade da conta; sem identidade, não há alertas de consumo.
3. `grok::LIVE_QUERY_AUTHORIZED = false` até os termos da xAI serem lidos e permitirem.
4. O disjuntor do Antigravity dispara para qualquer resposta que não prove zero rodadas e zero tokens, e só o usuário o reativa.
5. Nada de cookies, interceptação, tokens de terceiros, extensões privadas do ACP ou scraping.
6. Preferências reais nunca são escritas por testes (`AURORA_CONFIG_DIR` em depuração).

Detalhes: [Decisões do Ciclo 04](architecture-decisions.md), [Ciclo 03.1](../cycle-03-1/architecture-decisions.md), [Arquitetura](../architecture.md).

## Riscos residuais

Ver [Auditoria de segurança](security-audit.md): instalador sem assinatura; termos do Antigravity (seção 6); interfaces não documentadas (endpoints, log do Grok, JSON do `agy`); revisão independente pendente; janela de microssegundos na associação ao Job; Antigravity sem identidade; `chrono` sem manutenção.

## Git e PRs

- `main` ← PR #2 (`feat/cycle-03-multi-provider`, 0.3.0) ← PR #3 (`feat/cycle-03-1-provider-usage`, 0.3.1) ← PR #4 (`feat/cycle-04-final-release`, 1.0.0). Cada branch contém a anterior; integrar na ordem descrita na [lista de verificação](release-checklist.md#integração-ordenada-sem-force-push-sem-push-direto-na-main).
- Nenhum merge foi feito neste ciclo. Nenhum push na `main`, nenhum force push.
- Release `v1.0.0` em **rascunho** no GitHub, com o instalador e o hash.

## Máquina do proprietário (24/11/2026)

- 1.0.0 instalada em `%LOCALAPPDATA%\Aurora Control`, em execução, sem inicialização com o Windows.
- Preferências reais em `%APPDATA%\app.auroracontrol.desktop\preferences.json`, ainda no formato v2 (Codex, Claude Code e Grok ativos; Gemini → Antigravity desativado na migração). Serão regravadas em v3 na próxima vez que o proprietário salvar algo.
- Grok Build 1.0.92 com cobrança unificada (SuperGrok Lite); Antigravity CLI instalado; Cursor não instalado.
- O "Não incomodar" do Windows estava ativo durante a validação.

## Primeiros passos de quem assume

1. `npm ci`, depois a verificação completa do guia de manutenção.
2. Ler as observações pendentes do Codex nos PRs #2–#4.
3. Rodar `live_grok_log` e `live_antigravity` (opcionais) para conferir que os formatos não mudaram.
4. Seguir a [lista de verificação da release](release-checklist.md).

## Não iniciar

O Aurora Lens é o próximo projeto e **não** faz parte deste repositório nem deste ciclo.
