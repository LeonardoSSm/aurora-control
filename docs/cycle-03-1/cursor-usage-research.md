# Cursor — fontes de uso

Data: 24/11/2026.

## Ambiente

O Cursor não está instalado nesta máquina: nada em `%LOCALAPPDATA%\Programs`, `%APPDATA%`, `%LOCALAPPDATA%` ou `~/.cursor`, e nenhum `cursor` no `PATH`. Não houve validação real.

## Fontes oficiais

| Fonte                                                        | O que oferece                                                                                                                                                                                                                   | Adequada ao Aurora Control?                                              |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| Configurações do editor e painel web                         | [cursor.com/docs/models](https://cursor.com/docs/models): "Your remaining usage and next reset date are shown in your editor settings and under Usage at grok.com/dashboard."                                                   | Não: interface do usuário, sem API.                                      |
| [Admin API](https://cursor.com/docs/account/teams/admin-api) | "lets you programmatically access your team's data": `POST /teams/daily-usage-data`, `POST /teams/spend`, `POST /teams/filtered-usage-events`, `GET /v2/teams/billing-items`. Chave de equipe com escopo `read:*` ou `admin:*`. | Só para equipes, com chave de administrador. Não para uma conta pessoal. |

A documentação geral da API (`cursor.com/docs/api.md`) respondeu 403; não há confirmação de acesso para contas pessoais.

## Relação com o Grok

- Os modelos Grok escolhidos dentro do Cursor consomem a franquia do **Cursor** ("weekly pool of usage for Cursor models, shared across Grok and Cursor"), não uma cota da API da xAI.
- Com a X Subscription, Grok (incluindo o Grok Build) e Cursor dividem o mesmo pool semanal. Sem ela, SuperGrok e Cursor são assinaturas separadas.
- O painel de uso apontado pelo Cursor é o `grok.com/dashboard`, que é web e exige sessão de navegador.

## Conclusão

Não existe fonte autorizada e programática para o uso pessoal do Cursor. Por isso o Cursor não foi adicionado como provedor (o catálogo deste ciclo é de quatro provedores). O app não mostra o consumo do Cursor como se fosse do Grok, nem o contrário. Se o proprietário estiver na X Subscription, a franquia que o Grok Build mostraria (se liberado) seria a mesma do Cursor, e isso será documentado quando houver validação real.
