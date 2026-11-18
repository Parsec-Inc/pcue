## 0.8.0 (2026-11-18)

### Feat

- **phone**: espera a tela acender, foco vazio com bloqueio e Ctrl+Y copia campos da TUI
- **phone**: controle do celular via adb nas rotinas e gravação dos toques
- **daemon**: gatilho por dispositivo USB e passo shell oculto
- **daemon**: gatilho por notificação de app (Teams, WhatsApp...) com filtros por regex

## 0.7.0 (2026-11-18)

### Feat

- **tui**: revisão da gravação edita (e) e separa (s) passos, além de apagar
- parâmetros de rotina com {nome} nos passos, pcue run/call nome=valor e a TUI perguntando os valores

## 0.6.0 (2026-11-18)

### Feat

- **recorder**: trajeto completo do arrasto com via=, posição do cursor no instante do evento e atribuição mais estreita às mudanças do desktop

## 0.5.0 (2026-11-18)

### Feat

- **tui**: edições ficam pendentes até salvar com S, com descarte e confirmação ao sair ou rodar

## 0.4.0 (2026-11-18)

### Feat

- **tui**: C limpa todos os passos da rotina, com confirmação
- **steps**: hold, release, move e drag para ações simultâneas; gravação de arrastos e cliques com modificador

### Refactor

- ClickTarget como janela opcional + ponto, hold com posição igual ao click e gatilho de boot em um lugar só

## 0.3.1 (2026-11-17)

### Fix

- **build**: spec do rpm com caminhos literais (o rpm do Ubuntu não tem as macros do systemd)

## 0.3.0 (2026-11-17)

### Feat

- **update**: pede senha (pkexec/UAC) quando o executável está numa pasta do sistema

## 0.2.0 (2026-11-17)

### Feat

- **update**: atualiza pelas releases do GitHub com confirmação
- gravação inteligente com revisão
- suporte a Windows com camada de plataforma
- prefixo de boot automático ao adicionar o gatilho login
- passo workspace vai para o workspace exato
- passo wait_monitors espera os monitores das regras e reposiciona workspaces
- passo wait_layer espera superfícies do shell (barra, painéis)
- **wait_window**: nth=N espera a N-ésima janela da class
- TUI animada com mouse, histórico, gravação embutida e passos condicionais
- **click**: alvo por ordem de abertura entre janelas da mesma class
- pwboot, rotinas pós-login com TUI, daemon e gravação de ações

### Fix

- **daemon**: unlock pelo session lock do Hyprland e reinício ao atualizar
- **recorder**: grava pausas a partir de 100ms

### Refactor

- renomeia pwboot para pcue
