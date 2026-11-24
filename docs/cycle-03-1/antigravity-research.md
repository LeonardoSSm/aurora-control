# Antigravity — investigação

Data: 24/11/2026. Máquina do proprietário, Windows 11 Pro build 26300.

## Ambiente encontrado

| Ferramenta        | Versão              | Local                                                                                          | Dados                                                     |
| ----------------- | ------------------- | ---------------------------------------------------------------------------------------------- | --------------------------------------------------------- |
| Antigravity CLI   | `agy` 1.5.16        | `%LOCALAPPDATA%\agy\bin\agy.exe`                                                               | `~/.gemini/antigravity-cli/`                              |
| Antigravity (app) | não lida (Electron) | `%LOCALAPPDATA%\Programs\antigravity\Antigravity.exe`, com `resources\bin\language_server.exe` | `~/.gemini/antigravity/`, `%APPDATA%\Antigravity`         |
| Gemini CLI        | 0.71.1 (npm global) | `C:\nvm4w\nodejs\gemini`                                                                       | `~/.gemini/` (`oauth_creds.json`, `google_accounts.json`) |

WSL: só a distribuição `docker-desktop`, sem ferramentas de IA.

Ao autenticar o Gemini CLI, o Google respondeu que o cliente não é mais suportado para Gemini Code Assist for individuals e indicou o Antigravity. O indicador "Gemini" do 0.3.0 dependia justamente desse login e foi substituído pelo Antigravity (ver [Decisões de arquitetura](architecture-decisions.md)).

## Fontes avaliadas

| Fonte                                                       | Resultado                                                                                                                                                                                                                                       |
| ----------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `agy -p "/usage" --output-format json` (alias `/quota`)     | **Adotada.** Resposta estruturada do próprio CLI, `num_turns: 0`, `total_tokens: 0`, `conversation_id` vazio, em ~4 s.                                                                                                                          |
| `agy -p "/credits" --output-format json`                    | Funciona (`remaining_credits`, `upgrade_uri`). É saldo de créditos de IA, não percentual de cota: não usado no indicador.                                                                                                                       |
| `agy -p "/help" --output-format json`                       | Confirma os comandos que o modo `-p` responde sem rodada do agente: `usage` (alias `quota`), `credits`, `model`, `effort`, `skills`, `help`, `config` e outros.                                                                                 |
| `statusLine` do CLI                                         | Recebe JSON com `quota` (`remaining_fraction`, `reset_time`, `reset_in_seconds`), `plan_tier` e `email`, mas só roda em sessões interativas (TUI) e exige alterar `settings.json` do usuário. Não adotado; possível fonte futura de identidade. |
| `language_server` local da IDE                              | Não documentado para terceiros. Extensões da comunidade o consultam lendo um token CSRF da linha de comando do processo, o que é extração de credencial de outro processo. **Descartado.**                                                      |
| API pública do Google para cotas do Antigravity             | Não encontrada.                                                                                                                                                                                                                                 |
| `retrieveUserQuota` (`cloudcode-pa`, usado pelo Gemini CLI) | Exige o OAuth do cliente; proibido para software de terceiros pelos termos do Gemini CLI e do Antigravity. **Descartado.**                                                                                                                      |

## Evidências do modo não interativo

- Changelog do `agy` (comando `agy changelog`), versão 1.1.11: "Added non-interactive answers for the read-only slash commands in print mode, so `-p "/usage"`, `/quota`, `/credits` … emit … a structured payload under `--output-format json` … without starting an agent turn, spending quota, or leaving a conversation behind."
- Changelog, versão posterior: "`-p "/usage"` … piped and `--json` output still use UTC timestamps, so existing scripts are unaffected."
- [Documentação do modo headless](https://antigravity.google/docs/cli/headless): modo de prompt único para scripts; "Slash commands that the CLI can respond to directly (such as `/model` and `/usage`) produce a text report rather than a standard event stream"; "Headless mode uses your cached credentials."
- [Documentação do `/usage`](https://antigravity.google/docs/cli/commands/usage): "The models in a group share a weekly limit and a five-hour limit."

Estrutura real devolvida (descrições omitidas):

```json
{
  "conversation_id": "",
  "status": "SUCCESS",
  "num_turns": 0,
  "usage": { "total_tokens": 0, "...": 0 },
  "command": {
    "name": "usage",
    "data": {
      "groups": [
        {
          "name": "Gemini Models",
          "buckets": [
            {
              "id": "gemini-weekly",
              "window": "weekly",
              "remaining_fraction": 0.84,
              "reset_time": "2026-12-01T04:20:44Z"
            },
            {
              "id": "gemini-5h",
              "window": "5h",
              "remaining_fraction": 0.42,
              "reset_time": "2026-11-24T09:20:44Z"
            }
          ]
        },
        { "name": "Claude and GPT models", "buckets": ["3p-weekly", "3p-5h"] }
      ]
    }
  }
}
```

## Disponibilidade das métricas

| Métrica                      | Disponível                                                         |
| ---------------------------- | ------------------------------------------------------------------ |
| Cota de 5 horas              | Sim, por grupo de modelos (`gemini-5h`, `3p-5h`)                   |
| Cota semanal                 | Sim, por grupo (`gemini-weekly`, `3p-weekly`)                      |
| Limites por modelo ou grupo  | Por grupo: "Gemini Models" e "Claude and GPT models"               |
| Percentual usado e restante  | `remaining_fraction` (0–1); o app mostra o usado (`1 − fração`)    |
| Renovação                    | `reset_time` (UTC)                                                 |
| Estado da assinatura (plano) | Não no `/usage`; o `plan_tier` só aparece no `statusLine`          |
| Conta                        | Não no `/usage` nem no `/config`; o login fica no cofre do sistema |

## CLI e IDE compartilham a cota?

A [página de preços](https://antigravity.google/pricing) atribui as cotas do plano ao uso "across the app, CLI, and IDE Extensions", e a documentação do `/usage` diz que os modelos de um grupo compartilham os limites. Não há frase explícita de que existe um único pool. **Não verificado na prática:** a IDE não foi usada durante a validação, então não comparei o painel dela com o CLI. O app rotula a fonte como "CLI oficial" e não afirma que o número inclui o uso da IDE.

## Incidente durante a investigação

A primeira tentativa de `agy -p "/quota"` foi feita pelo Git Bash, que converte argumentos iniciados por `/` em caminhos do Windows (`/quota` virou `C:/Program Files/Git/quota`). O CLI recebeu isso como um prompt comum e **executou uma rodada do modelo** (~22 mil tokens de entrada, ~1,1 mil de saída), deixando uma conversa. Foi um erro do ambiente de teste, não do CLI. As execuções seguintes foram feitas pelo PowerShell, e o app chama o processo diretamente pelo Rust, sem essa conversão. Esse incidente motivou o disjuntor descrito no [relatório de implementação](implementation-report.md).

## Efeitos colaterais medidos

Cada execução de `/usage` atualiza `conversation_summaries.db` e `cache/onboarding.json` do CLI, cria um log de falha vazio (`crashes/crash_<pid>_<uuid>.log`, 0 bytes) e, sem `--log-file`, um log de ~14 KB em `log/`. Com `--log-file` apontando para um arquivo próprio do app (sobrescrito a cada execução), o log deixa de ir para a pasta do CLI. O log de falha vazio continua: com a cadência de 10 minutos, são até 144 arquivos vazios por dia. Isso é um efeito colateral conhecido e fica documentado.
