# Lista de verificação da release 1.0.0

Marque cada item na ordem. Itens com ✅ foram concluídos neste ciclo; os demais dependem do proprietário ou da auditoria independente.

## Código e CI

- ✅ Verificação completa local (format, lint, tipos, 51 testes de frontend, build, fmt, Clippy `-D warnings`, 217 testes Rust, build desktop, NSIS).
- ✅ PR #4 aberto, empilhado sobre o #3.
- [ ] CI do PR #4 aprovada (Windows).
- [ ] Revisão do Codex sobre `eca2a91` (PR #2), `e7c06d6` (PR #3) e o PR #4.
- [ ] Observações da revisão tratadas.

## Integração ordenada (sem force push, sem push direto na `main`)

1. [ ] Proprietário aprova e faz merge do PR #2 (`feat/cycle-03-multi-provider` → `main`).
2. [ ] No PR #3, trocar a base para `main` (o GitHub faz isso sozinho quando a base é apagada após o merge; se não, `gh pr edit 3 --base main`), conferir CI, aprovar e fazer merge.
3. [ ] No PR #4, trocar a base para `main`, conferir CI, aprovar e fazer merge.
4. [ ] Criar a tag `v1.0.0` no commit de merge do PR #4.

Como as três branches são lineares (cada uma contém a anterior), não há conflitos esperados; se o merge for por squash, os PRs seguintes mostrarão os commits anteriores até a troca de base, e um `git rebase --onto main <base-antiga>` local seguido de push normal (não forçado) de uma branch nova resolve.

## Homologação pelo proprietário

- [ ] Grok / Cursor: depois de um `/usage` no Grok Build, o percentual do indicador é igual ao da tela de uso do Cursor e ao de `grok.com/dashboard`.
- [ ] Antigravity: ativar nas configurações, aceitar o risco descrito no guia do usuário, conferir com `agy -p /usage`.
- [ ] Hover do Grok / Cursor: resumo com "Franquia semanal compartilhada", renovação e "Registro do cliente oficial".
- [ ] Notificação de teste com o "Não incomodar" desligado: aparece com o nome e o ícone do Aurora Control.
- [ ] "Encerrar" pela bandeja fecha o app e nenhum `msedgewebview2.exe` do app permanece.
- [ ] Inicialização com o Windows: ativar, reiniciar a sessão, conferir; desativar.
- [ ] Execução prolongada: deixar rodando um dia de trabalho e conferir no Gerenciador de Tarefas que a memória do `aurora-control.exe` não cresce continuamente e que não há processos `agy` remanescentes.
- [ ] Monitores: com um segundo monitor, escolher cada um e cada lateral.

## Distribuição

- ✅ Instalador candidato `Aurora Control_1.0.0_x64-setup.exe`, SHA-256 `F65999DD526F6D4F9B0EE7A3583936590468A4F417CEE2A8311E8D50D8978763`, não assinado.
- ✅ Release em rascunho no GitHub com instalador, hash e notas.
- [ ] Opcional: assinar o instalador (Authenticode) e publicar o novo hash.
- [ ] Publicar a release (tirar do rascunho) só após aprovação.

## Reprovação

Reprove a release se: a CI falhar; a revisão independente encontrar um bloqueante; o indicador do Grok / Cursor divergir da tela do Cursor sem explicação; ou o Antigravity executar uma rodada do modelo sem suspender a fonte.
