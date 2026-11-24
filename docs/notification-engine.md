# Motor de notificações

Módulo `src-tauri/src/alerts/`, versão 0.3.0.

| Arquivo    | Papel                                                                                                        |
| ---------- | ------------------------------------------------------------------------------------------------------------ |
| `rules.rs` | Motor de regras puro: transforma observações em alertas. Estado por provedor e por conta, deduplicação.      |
| `quiet.rs` | Horário silencioso em hora local.                                                                            |
| `mod.rs`   | Integração: roda após cada publicação de uso, aplica as preferências e entrega pelo plugin oficial do Tauri. |

## Fonte dos dados

O motor consome `Service::observations()`: o mesmo estado dos `Tracker` que a barra exibe, mais a habilitação do provedor e a identidade da conta verificada (`Fingerprint`, válida só neste processo). As observações usam a **métrica automática** (`Tracker::automatic_usage`): a métrica escolhida para o indicador é só apresentação, então uma métrica escolhida ausente ou renovada não silencia os alertas das outras janelas medidas. O motor **nunca consulta um provedor**: não há requisição criada para gerar alertas. A avaliação roda no callback dos workers, depois de `usage-updated`, e também ao aplicar preferências. Um provedor desativado não tem worker publicando, então é assim que o motor fica sabendo da desativação. O lock do motor é mantido durante a leitura das observações, então publicações de threads diferentes são avaliadas uma de cada vez, sempre com o estado mais novo.

## Regras

### Linha de base

A primeira leitura válida de uma conta só registra os níveis: na inicialização, ao habilitar um provedor e após uma troca de conta. Nada acumulado antes é anunciado. Consequências:

- reiniciar o app não repete alertas;
- habilitar um provedor em 96% não dispara 70, 85 e 95;
- um problema de sessão já presente na inicialização não gera alerta.

### Consumo

Limites configuráveis: 70%, 85%, 95% e 100% (padrão: 85, 95 e 100).

- Para cada janela medida de uma leitura **nova** (`available`, sem `issue`, observada depois da última processada), o nível é o maior limite configurado menor ou igual ao percentual.
- Se o nível subiu acima do último anunciado, há cruzamento. Exemplo: 82% → 87% com limite de 85% gera "Codex — 85% da cota".
- Vários limites cruzados de uma vez, em uma ou mais janelas do provedor, viram **uma** notificação no nível mais alto ("Cota esgotada" em 100%).
- Permanecer acima, oscilar para baixo e voltar sem renovação confirmada: nada.
- Janelas consideradas: a métrica do indicador e as demais janelas elegíveis (semântica conhecida). Créditos e janelas renovadas sem leitura nunca disparam.

### Renovação

Três estágios, tratados de forma diferente:

1. **Renovação prevista:** o resumo mostra "Renova em … · hora".
2. **Horário atingido:** a janela vira "Renovada · aguardando nova leitura" e o percentual deixa de ser leitura (correção I02 do Ciclo 02). Isso **não** gera notificação: a passagem do relógio não confirma nada.
3. **Renovação confirmada:** uma leitura nova, depois do horário de renovação conhecido (ou, sem horário conhecido, quando o consumo simplesmente volta a cair), mostra a janela abaixo do nível que ela tinha atingido. Notifica "Codex — Limite renovado / A nova consulta confirmou a recuperação da sua cota (…)." e rearma os limites daquela janela.

Só janelas que chegaram a um limite configurado são anunciadas na renovação; uma janela de 5 h que renova a 10% não gera aviso.

### Autenticação

| Estado da última tentativa                | Alerta                                                    |
| ----------------------------------------- | --------------------------------------------------------- |
| `authExpired`                             | "Sessão expirada" + procedimento de reconexão do catálogo |
| `authMissing` ou `credentialsUnavailable` | "Credencial indisponível" + procedimento                  |
| `authRejected`                            | "Autenticação rejeitada" + procedimento                   |
| `credentialsInvalid`                      | Nenhum: costuma ser o provedor regravando o arquivo       |
| Leitura nova depois de um problema        | "Monitoramento restaurado" (opcional, padrão desligado)   |

Um alerta por transição. O mesmo problema em cada nova tentativa não notifica de novo. Exemplo: "Claude Code — Sessão expirada / Autentique-se novamente pelo Claude Code CLI para restaurar o monitoramento." Nenhuma mensagem pede token, cookie ou senha.

### Isolamento e ordem

- Uma identidade de conta diferente descarta todo o estado da anterior: níveis, renovações e estado de sessão.
- Leituras sem conta verificada (sessão `Unverified`, registro local do Codex) não disparam alertas de consumo.
- Leituras desatualizadas (`stale`), com `issue`, repetidas ou mais antigas que a última processada são ignoradas.
- Dados de demonstração (`source: demo`) nunca geram alertas. A notificação de teste é explicitamente identificada como teste.
- Provedor desabilitado: estado descartado (a aplicação das preferências avalia o motor na hora) e nenhum alerta. Ao reabilitar, recomeça pela linha de base.
- Notificações desligadas (globalmente ou no provedor): o estado continua avançando, para nada ser reproduzido depois.

## Persistência

Nada é persistido. As identidades são `Fingerprint` com chave aleatória por processo e não podem ser gravadas com sentido, e gravar identificadores de conta contraria a política de privacidade do app. Por isso a política é a conservadora prevista no requisito: após reiniciar, a primeira leitura é linha de base.

**Custo conhecido:** um limite cruzado enquanto o app estava fechado não é anunciado (por exemplo, 80% ao fechar e 90% ao abrir). O próximo cruzamento, ou a renovação, voltam a ser anunciados normalmente.

## Horário silencioso

- Período diário em minutos locais desde a meia-noite; `início > fim` atravessa a meia-noite (padrão 22:00–07:00, desligado).
- A hora local é lida a cada avaliação (`chrono::Local`), então mudanças de relógio, horário de verão e fuso valem imediatamente.
- Durante o período, nada é exibido e nada fica em fila: o motor já avançou o estado, e o alerta é descartado. Às 07:00 não chega nenhuma rajada.
- A atualização de consumo continua normalmente.

## Entrega nativa

- `tauri-plugin-notification` 2.4 (plugin oficial), chamado só pelo Rust. Nenhuma janela recebe permissão do plugin; o teste de ACL em `main.rs` comprova isso.
- Toasts do Windows não roubam foco e respeitam o Não incomodar e as preferências de notificação do sistema.
- O plugin só define o AppUserModelID (`app.auroracontrol.desktop`) quando o executável **não** está em `target\debug` ou `target\release`. A identidade "Aurora Control" vale, portanto, no app instalado: o instalador NSIS cria o atalho do Menu Iniciar com esse AUMID (verificado). Builds de desenvolvimento aparecem como PowerShell.
- `show()` do plugin dispara o toast numa tarefa assíncrona e descarta o resultado do WinRT. Uma falha do Windows não chega ao app: só falhas anteriores ao envio são reportadas. Em qualquer falha reportada, o app registra no log e não tenta de novo.
- "Enviar notificação de teste" (configurações) ignora o horário silencioso, para o usuário ver como o Windows apresenta o app.

## Preferências

`notifications` em `preferences.json` (versão 2): `enabled`, `thresholds` (subconjunto ordenado de 70/85/95/100), `renewal`, `auth`, `reconnect`, `quiet { enabled, start, end }`. Por provedor: `providers.<id>.alerts`. Valores inválidos são recusados na gravação; limites fora de ordem são normalizados.

## Testes

- `alerts::rules::tests` (12): cruzamento único, permanência acima, vários limites, primeira leitura, oscilação sem renovação, renovação prevista versus confirmada, dados desatualizados e fora de ordem, troca de conta, provedor desabilitado e dados demo, notificações desligadas, sessão (transição, repetição, arquivo transitório, rejeição, reconexão opcional), estado na inicialização, janelas secundárias e créditos.
- `alerts::quiet::tests` (3): período diurno, período atravessando a meia-noite, desligado ou vazio.
- `alerts::tests` (4): chave por provedor, horário silencioso sem reprodução posterior, falha de entrega sem nova tentativa, reinicialização.
