# Guia do usuário — Aurora Control 1.0

O Aurora Control mostra, na lateral da tela, quanto da cota de cada ferramenta de IA você já usou: Codex, Claude Code, Antigravity e Grok / Cursor. Ele fica discreto (um semicírculo por provedor), não pede senhas e não altera nada nas ferramentas.

## Instalação

1. Baixe `Aurora Control_1.0.0_x64-setup.exe` da página da versão e confira o SHA-256 publicado:

   ```powershell
   Get-FileHash ".\Aurora Control_1.0.0_x64-setup.exe" -Algorithm SHA256
   ```

2. Execute o instalador. Ele instala só para o seu usuário (sem administrador), em `%LOCALAPPDATA%\Aurora Control`, e cria atalhos no Menu Iniciar e na Área de Trabalho. O instalador não é assinado digitalmente: o Windows SmartScreen pode pedir "Mais informações → Executar assim mesmo". Faça isso só se o hash conferir.
3. Abra "Aurora Control" pelo Menu Iniciar.

**Atualização:** execute o instalador da versão nova. Ele fecha a versão em execução, substitui o programa e mantém as preferências (`%APPDATA%\app.auroracontrol.desktop\preferences.json`). Preferências de versões anteriores são migradas na leitura, e o arquivo só é regravado quando você salvar alguma configuração.

**Desinstalação:** "Configurações do Windows → Aplicativos → Aurora Control → Desinstalar". Suas preferências só são apagadas se você marcar essa opção no desinstalador.

## O que cada provedor precisa

| Provedor      | Faça isto uma vez                                          | De onde vem o número                                                                                   |
| ------------- | ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| Codex         | `codex login` com a conta ChatGPT                          | Consulta à sua conta, a mesma que o Codex faz                                                          |
| Claude Code   | `claude` no terminal, com login da assinatura              | Consulta à sua conta, a mesma que o Claude Code faz                                                    |
| Antigravity   | Instale o Antigravity CLI e entre com `agy`                | O próprio `agy` responde `/usage` (sem gastar cota)                                                    |
| Grok / Cursor | `grok login` no Grok Build, e use o Grok Build normalmente | O registro que o Grok Build grava quando consulta a franquia (ao abrir, a cada prompt ou com `/usage`) |

Antigravity e Grok / Cursor começam desativados: ative-os em **Configurações → Provedores**.

## Lendo a barra

- O arco mostra o consumo da métrica escolhida (por padrão, a janela mais consumida). Abaixo do número aparece a janela (`5 h`, `7 d`).
- Pare o mouse sobre um semicírculo para ver o resumo: conta (e-mail mascarado), plano, renovação, outras cotas e de onde veio o número.
- **Cinza / "Desatualizado"**: a última leitura tem mais de 15 minutos. No Grok / Cursor isso é normal quando você não usa o Grok Build há algum tempo: o número é o da última vez que o Grok Build consultou.
- **"—"**: não há leitura confiável (janela renovada aguardando nova medição, métrica escolhida ausente, ou nenhum registro ainda). O app nunca mostra 0% por falta de dado.
- **Ponto âmbar**: o provedor precisa de atenção (sessão expirada, erro, fonte suspensa).
- **Anel cinza vazado**: o Grok / Cursor está esperando um registro do Grok Build para a conta atual. Abra o Grok Build ou use `/usage` nele.
- Clique num semicírculo para atualizar agora (respeitando os limites do provedor).

## Grok / Cursor: franquia compartilhada

Com a X Subscription, Grok e Cursor usam a mesma franquia semanal. O indicador mostra essa franquia uma vez só (nada é somado) e só a chama de "Franquia semanal compartilhada" quando o Grok Build informa que a conta está na cobrança unificada. O consumo feito só no Cursor ou no app Grok aparece quando o Grok Build consultar de novo. Créditos pré-pagos e uso sob demanda não entram no indicador.

Se você trocar de conta no Grok Build, o número da conta anterior some até o Grok Build gravar um registro da conta nova.

## Antigravity: suspensão de segurança

O app executa apenas `agy -p /usage`, que não deveria gastar cota. Se o CLI alguma vez executar uma rodada do modelo, ou responder de um jeito que não prove que não executou, o Aurora Control **suspende** o Antigravity, inclusive depois de reiniciar, e mostra "Suspenso por segurança". Para voltar:

1. Atualize o Antigravity CLI.
2. Em **Configurações → Provedores → Antigravity**, clique em **Reativar consultas…**, leia o aviso e confirme em **Reativar mesmo assim**.

O Antigravity não informa qual conta respondeu, por isso não gera alertas de consumo.

## Notificações

Em **Configurações → Notificações**: limites (70, 85, 95, 100%), renovação confirmada, problemas de sessão, reconexão, alertas por provedor, horário silencioso e uma notificação de teste. Alertas só saem de leituras recentes e de contas verificadas; não se repetem a cada atualização nem após reiniciar. Se o "Não incomodar" do Windows estiver ativo, as notificações vão direto para a central, sem banner.

## Privacidade

- Nada sai do seu computador além das consultas do Codex e do Claude Code às próprias contas, e das que o `agy` faz por conta própria.
- Credenciais nunca são copiadas, gravadas nem mostradas; e-mails aparecem mascarados (`d•••@dominio`).
- O registro do Grok Build é lido localmente; só o percentual e as datas chegam à interface.

## Problemas comuns

| Sintoma                                  | O que fazer                                                                                            |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| A barra não aparece                      | Ícone da bandeja → mostrar a barra; confira se há algum provedor ativo                                 |
| "Sessão expirada"                        | Abra a ferramenta correspondente; ela renova a sessão sozinha                                          |
| Grok / Cursor sempre desatualizado       | Esperado sem uso do Grok Build; abra-o ou use `/usage` para atualizar                                  |
| "Registro mais recente é de outra conta" | Abra o Grok Build com a conta atual                                                                    |
| Segunda janela não abre                  | Só uma instância roda por sessão do Windows; use o ícone da bandeja                                    |
| Diagnóstico                              | Configurações → Provedores → Diagnóstico (somente leitura; não mostra tokens nem conteúdo de arquivos) |
