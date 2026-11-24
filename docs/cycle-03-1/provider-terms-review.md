# Revisão de termos

Data: 24/11/2026. Este documento registra o que foi lido e como cada trecho afetou as decisões. Não é aconselhamento jurídico: a decisão final sobre cada integração é do proprietário.

## Google Antigravity

**Termos adicionais** ([antigravity.google/terms](https://antigravity.google/terms)), seção 6:

> "Using third party software, tools, or services to access the Service (e.g. using OpenClaw with Antigravity OAuth) is a breach of this Agreement."

e, na mesma seção, que isso "may be grounds for suspension or termination of your Antigravity and/or Gemini CLI accounts".

**Documentação oficial do modo headless** ([antigravity.google/docs/cli/headless](https://antigravity.google/docs/cli/headless)): modo de prompt único pensado para scripts e automação, que "uses your cached credentials". O changelog do CLI descreve `-p "/usage"` como resposta estruturada "without starting an agent turn, spending quota, or leaving a conversation behind", e menciona compatibilidade com "existing scripts".

**Interpretação adotada.** O trecho proíbe software de terceiros que **acessa o serviço**, e o exemplo é o uso do OAuth do Antigravity por outro cliente. O Aurora Control não acessa o serviço: não lê, copia nem envia a credencial e não chama nenhum endpoint do Google. Ele executa o cliente oficial no modo que a própria documentação oferece para scripts, e é o `agy` que acessa o serviço com o login dele. É assim que um script de CI ou um "eval harness" usaria o CLI.

**Risco residual.** O texto não diz explicitamente que um programa pessoal que dispara o `agy` está fora do alcance da seção 6. Mitigações: o provedor fica desativado até o usuário ativá-lo, a fonte é rotulada "CLI oficial", nenhuma credencial é tocada e a cadência é baixa (10 min). Se o proprietário preferir não correr esse risco, basta manter o Antigravity desativado.

**Gemini CLI.** Os termos do Gemini CLI ([tos-privacy.md](https://github.com/google-gemini/gemini-cli/blob/main/docs/resources/tos-privacy.md)) têm a mesma proibição para o OAuth do Gemini CLI, e o próprio Google encerrou esse login para Gemini Code Assist for individuals. O adaptador de detecção do Gemini da 0.3.0 foi removido.

## xAI (Grok)

Páginas tentadas e resultado:

| URL                                                | Resultado                |
| -------------------------------------------------- | ------------------------ |
| `https://x.ai/legal/terms-of-service`              | HTTP 403                 |
| `https://x.ai/legal/acceptable-use-policy`         | HTTP 403                 |
| `https://x.ai/legal/consumer-terms`                | HTTP 403                 |
| `https://grok.com/terms`, `https://grok.com/legal` | HTTP 404                 |
| `https://docs.x.ai/docs/terms`                     | HTTP 404                 |
| `https://x.com/en/tos`                             | HTTP 402                 |
| `https://help.x.ai`                                | falha de DNS             |
| cópia no web.archive.org                           | recusada pela ferramenta |

Contornar bloqueios de acesso (por exemplo, falsificando o User-Agent) foi considerado inadequado e não foi feito.

A documentação do Grok Build trata `~/.grok/auth.json` como credencial a proteger ("Anyone with filesystem access to those paths can use the credentials") e não oferece leitura por terceiros. **Sem texto dos termos, não há autorização demonstrada**, e a ausência de uma proibição lida não foi tratada como permissão. A consulta direta continua desativada.

## Cursor

A Admin API é documentada para equipes, com chave de administrador. Nada no material lido autoriza ler o uso de uma conta pessoal por outro meio. Nenhuma integração foi feita.

## OpenAI (Codex) e Anthropic (Claude Code)

Inalterados neste ciclo. Ambos usam o endpoint interno que o próprio cliente usa, decisão aceita pelo proprietário desde o Ciclo 01, e documentada em [Compatibilidade dos provedores](../provider-compatibility.md).
