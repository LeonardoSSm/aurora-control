# Grok — investigação

Data: 24/11/2026.

## Ambiente encontrado

| Ferramenta | Versão                                | Local                                                           | Sessão                                          |
| ---------- | ------------------------------------- | --------------------------------------------------------------- | ----------------------------------------------- |
| Grok Build | `grok 1.0.92 (b06756fd6256) [stable]` | `~/.grok/bin/grok.exe` (pacote npm `@xai-official/grok@1.0.92`) | `~/.grok/auth.json` (login OIDC em `auth.x.ai`) |

A sessão é detectada pelo adaptador da 0.3.0 (`auth.json` lido localmente, nada enviado).

## Fontes avaliadas, na ordem pedida

| Fonte                                                                                  | Resultado                                                                                                                                                                                                                                                                                                                                             |
| -------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `/usage` dentro do Grok Build                                                          | Mostra a franquia da conta, mas só na interface interativa. Não há modo não interativo para ele.                                                                                                                                                                                                                                                      |
| `grok usage --json` (subcomando oficial)                                               | **Indisponível nesta instalação.** Saída: "`grok usage` can't show your plan's usage here yet. Run /usage inside grok instead." A documentação local (`~/.grok/docs/user-guide/17-sessions.md`) explica que o limite do plano só é lido na "rota de daemon"; na "rota de shell", a padrão aqui, o comando falha assim "without reading your account". |
| Mesmo comando com o leader oficial ativo (`grok agent leader`)                         | Mesma resposta, em 0,1 s. O leader foi iniciado e encerrado (`grok leader kill`) durante o teste.                                                                                                                                                                                                                                                     |
| Ativar a rota de daemon                                                                | Controlada por flags de recursos remotas da xAI (`features.*`, "turned off remotely"); exigiria alterar a configuração do cliente. **Não feito.** O formato JSON dessa rota não está documentado, então não daria para implementar um parser honesto.                                                                                                 |
| `grok usage <session-id>`                                                              | Funciona: tokens e custo persistidos por sessão local. É consumo de tokens e custo (`costUsdTicks`), **não** percentual da franquia; não foi usado como cota.                                                                                                                                                                                         |
| Status line (`[ui.status_line]`)                                                       | A documentação lista os campos enviados; "a rate-limit summary" está entre os que **não** são enviados.                                                                                                                                                                                                                                               |
| OpenTelemetry externo (`GROK_EXTERNAL_OTEL`)                                           | Alfa, exporta tokens e eventos; a documentação diz que não há métrica de custo e que o nível de assinatura nunca é exportado.                                                                                                                                                                                                                         |
| Endpoint do `/usage` (`GET https://cli-chat-proxy.grok.com/v1/billing?format=credits`) | Já implementado e testado na 0.3.0 (`grok::parse_billing`), com a sessão do `auth.json`. **Continua desativado** (`LIVE_QUERY_AUTHORIZED = false`): os termos da xAI não puderam ser lidos (ver [Revisão de termos](provider-terms-review.md)).                                                                                                       |
| API xAI / Management API                                                               | Uso e limites de **equipe**, com chave de gerenciamento. Não é a franquia pessoal do SuperGrok; não apresentado como tal.                                                                                                                                                                                                                             |
| grok.com/dashboard (web)                                                               | Exigiria sessão de navegador ou cookies. **Descartado.**                                                                                                                                                                                                                                                                                              |
| Importação ou atualização manual                                                       | Avaliada, não implementada: ver abaixo.                                                                                                                                                                                                                                                                                                               |

## Grok Build e Cursor usam a mesma franquia?

Fontes oficiais lidas pela pesquisa:

- [docs.x.ai/x-subscription/overview](https://docs.x.ai/x-subscription/overview) e [cursor.com/help/the-x-subscription/overview](https://cursor.com/help/the-x-subscription/overview): "The X Subscription is one subscription that brings Grok, Cursor, and X together" e "Grok and Cursor share a weekly included usage pool". A migração para essa assinatura é opcional e permanente.
- [cursor.com/docs/models](https://cursor.com/docs/models): planos pagos do Cursor incluem "a weekly pool of usage for Cursor models, shared across Grok and Cursor"; os modelos Grok aparecem com provedor "Cursor"; a API do Grok é cobrada à parte.
- A documentação local do Grok Build menciona login com Cursor (`features.cursor_relay_login`) e o Grok Build como "Cursor private worker".

Conclusão: **depende da conta.** Para quem migrou para a X Subscription, a franquia semanal do Grok Build e a do Cursor são o mesmo pool. Para quem tem SuperGrok e Cursor separados, são sistemas distintos. O app não sabe qual é o caso do proprietário e não apresenta um como se fosse o outro. O Cursor não está instalado nesta máquina (ver [Uso do Cursor](cursor-usage-research.md)).

## Por que não há leitura manual

A atualização manual (o usuário digita o percentual que o `/usage` mostra) foi avaliada como último recurso:

- o número envelhece rápido e viraria "desatualizado" em 15 minutos pela política do app;
- seria fácil confundir um valor digitado com uma leitura da conta, justamente o que o requisito proíbe;
- não geraria alertas (dados não frescos), e o `/usage` do próprio CLI já mostra o mesmo número.

O custo de interface e persistência não compensa o valor. Fica registrado como alternativa possível.

## Condições para concluir a integração

Qualquer uma destas:

1. O proprietário (ou a xAI) confirma que os termos permitem que software de terceiros use a sessão do Grok Build para ler a franquia. Basta então `LIVE_QUERY_AUTHORIZED = true` em `grok.rs`, e a consulta real precisa ser validada contra o `/usage`.
2. A xAI libera a rota de daemon para esta conta e documenta o JSON de `grok usage --json`. Aí dá para adotar o CLI oficial, como no Antigravity.
3. A xAI publica uma API oficial de franquia pessoal.
