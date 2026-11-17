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
