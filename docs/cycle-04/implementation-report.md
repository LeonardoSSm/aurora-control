# Relatório de implementação do Ciclo 04

Versão 1.0.0. Branch `feat/cycle-04-final-release`, criada a partir de `feat/cycle-03-1-provider-usage` (`e7c06d6`), empilhada sobre os PRs #2 e #3, ainda abertos. PR deste ciclo: #4, com base em `feat/cycle-03-1-provider-usage`.

## Estado de partida (verificado em 24/11/2026)

- PR #2 (`feat/cycle-03-multi-provider` → `main`): aberto, mergeável, CI aprovada. A revisão do Codex é do commit `142f3b9`; as duas observações foram corrigidas em `eca2a91` e respondidas no PR, **sem nova revisão do Codex sobre `eca2a91`**.
- PR #3 (`feat/cycle-03-1-provider-usage` → `feat/cycle-03-multi-provider`): aberto, mergeável, CI aprovada. A revisão do Codex é do commit `5a7d2b4`; as duas observações foram corrigidas em `e7c06d6`, **sem nova revisão do Codex sobre `e7c06d6`**.
- Testes na partida: 190 Rust e 50 de frontend.

## Entregas

### Grok / Cursor

- `src-tauri/src/usage/grok_log.rs` (novo): leitor do `unified.jsonl` (ver [decisões D1–D3](architecture-decisions.md)).
- `src-tauri/src/usage/grok.rs`: a fonte padrão passa a ser o registro local; `parse_billing` virou `parse_response` (aceita o objeto de resposta, informa `isUnifiedBillingUser`, plano e fim do período, mede renovação contra o horário da observação); identidade pela comparação do `user_id` do registro com o do `auth.json`; diagnósticos com fonte, último registro, franquia e estado da consulta direta; teste opcional `live_grok_log` com os arquivos reais.
- Catálogo: nome "Grok / Cursor", fornecedor "xAI · Cursor", integração `clientLog`, cadência de 60 s (leitura local com cache por tamanho e data).
- Contrato: `Origin::ClientLog`, `Issue::AwaitingClient`, `FetchError::AwaitingClient`; `BlockReason::TermsUnverified` removido.
- Interface: textos de origem e de estado, conexão "Registro do cliente oficial · há X", marcador neutro `waiting` no indicador recolhido.

### Antigravity

- `src-tauri/src/usage/process.rs` (novo): execução controlada com época, Job Object e leitores com tolerância (ver [D6](architecture-decisions.md)).
- `src-tauri/src/usage/safety.rs` (novo): disjuntor persistente (ver [D7](architecture-decisions.md)).
- `src-tauri/src/usage/antigravity.rs`: usa os dois módulos; `parse_usage` exige contadores explícitos e distingue `Turn`, `Unconfirmed` e `Error`; diagnóstico mostra a causa e o horário da suspensão.
- IPC: `reset_provider_safety`, só na capability `settings` (manifesto em `build.rs`, teste de ACL em `main.rs` atualizado).
- Configurações: painel "Reativar consultas…" em duas etapas; o chip mostra "Suspenso por segurança".
- `Service::from_environment` recebe a pasta de configuração; `Service::reset_safety` reativa e pede uma leitura imediata se o provedor estiver ativo.

### Entrega

- Versão 1.0.0 em `package.json`, `package-lock.json`, `Cargo.toml`, `Cargo.lock` e `tauri.conf.json` (a janela de configurações lê a versão do app).
- `scripts/validate-settings-ui.ps1` lê a versão esperada do `package.json`; `scripts/capture-toast.ps1` aceita caminho absoluto em `-Out`.
- README, arquitetura, compatibilidade, `CHANGELOG.md`, guias de usuário e manutenção, e esta pasta.

## Defeitos encontrados e corrigidos

| Defeito                                                                                                                           | Origem     | Correção                                                                |
| --------------------------------------------------------------------------------------------------------------------------------- | ---------- | ----------------------------------------------------------------------- |
| Saída do `agy` sem `num_turns`/`usage.total_tokens` era tratada como zero rodada (`unwrap_or(0)`)                                 | Ciclo 03.1 | Contadores obrigatórios; ausência dispara o disjuntor (`Unconfirmed`)   |
| Saída em texto puro com código 0 (prompt respondido sem envelope) virava só `malformed` e o CLI voltava a rodar no ciclo seguinte | Ciclo 03.1 | Dispara o disjuntor                                                     |
| Disjuntor vivia só em memória: reiniciar o app (ou iniciar com o Windows) voltava a executar o CLI                                | Ciclo 03.1 | Persistência em `safety-antigravity.json`                               |
| Descendente do CLI herdando stdout bloqueava o `join` do leitor até ele terminar                                                  | Ciclo 03.1 | Job Object encerrado após cada execução; leitores com tolerância de 2 s |
| Desativar e reativar em menos de 50 ms deixava a execução em andamento continuar e publicar o resultado                           | Ciclo 03.1 | Época: tentativa cancelada é descartada                                 |
| Processos do CLI podiam sobreviver ao encerramento do app                                                                         | Ciclo 03.1 | `KILL_ON_JOB_CLOSE`                                                     |
| Grok sem fonte de cotas                                                                                                           | Ciclo 03   | Registro local do cliente oficial                                       |
| `validate-settings-ui.ps1` com versão fixa; `capture-toast.ps1` concatenava a raiz do repositório a um `-Out` absoluto            | Scripts    | Corrigidos                                                              |

## Fora do escopo, deliberadamente

- Consulta direta à xAI, extensão ACP, cookies, `state.vscdb` do Cursor, `cursor.com/api/*`.
- Indicador separado para o Cursor.
- Assinatura de código do instalador (requer certificado do proprietário).
