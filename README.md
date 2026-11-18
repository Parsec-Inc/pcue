<div align="center">

<img src="assets/screenshots/mascot.gif" width="128" alt="mascote do pcue" />

# pcue

**Rotinas dos parças para o desktop, no Linux (Hyprland) e no Windows.**
Abre seus apps nos workspaces certos, grava o que você faz com teclado e mouse
e repete tudo no login, no desbloqueio, num horário ou quando você mandar.

![Rust](https://img.shields.io/badge/Rust-2024-orange?logo=rust)
![Hyprland](https://img.shields.io/badge/Hyprland-0.56-58e1ff)
![Windows](https://img.shields.io/badge/Windows-10%2F11-0078d4)
![TUI](https://img.shields.io/badge/TUI-ratatui-cyan)
![Pixel art](https://img.shields.io/badge/pixel%20art-Aseprite-7d929e)

<img src="assets/screenshots/main.png" alt="tela principal do pcue" />

</div>

---

## O que é

O pcue (de *cue*, a deixa do teatro) organiza **rotinas**: sequências de passos
como abrir um app num workspace, esperar a janela aparecer, apertar um atalho,
digitar um texto ou clicar num ponto de uma janela.

- **Rotinas livres** rodam quando você chama: `Enter` na TUI, duplo clique,
  `pcue run nome` ou um atalho do Hyprland.
- **Rotinas agendadas** rodam sozinhas pelo daemon: login, bloqueio,
  desbloqueio, volta da suspensão, monitor conectado/desconectado, eventos
  próprios, horário diário ou intervalo.

Tudo é gerenciado numa TUI animada, com mouse, mascote em pixel art e
acompanhamento ao vivo das execuções.

## Prints

<table>
<tr>
<td width="50%"><img src="assets/screenshots/running.png" alt="rotina em execução" /><br/><sub><b>Execução ao vivo</b>: spinner, barra de progresso e passo atual destacado.</sub></td>
<td width="50%"><img src="assets/screenshots/history.png" alt="histórico de execuções" /><br/><sub><b>Histórico</b>: quando, gatilho, duração, passo que falhou e erro.</sub></td>
</tr>
<tr>
<td colspan="2"><img src="assets/screenshots/recording.png" alt="tela de gravação" /><br/><sub><b>Gravação</b>: contagem, REC pulsando e os passos aparecendo enquanto você age.</sub></td>
</tr>
</table>

## Recursos

| | |
|---|---|
| 🚀 **Abrir apps no workspace certo** | `launch` abre (ou reaproveita) o app, espera a janela e move para o workspace se precisar |
| ⏺️ **Gravação inteligente** | percebe app abrindo, foco e troca de workspace: grava `launch`, `focus` e `workspace` em vez das teclas, e você revisa cada trecho antes de salvar |
| 🖱️ **Cliques relativos** | cliques ficam relativos à janela, mesmo com várias janelas da mesma class |
| 🔒 **Senhas fora da gravação** | `SUPER+F9` pausa e retoma, `SUPER+F10` encerra |
| ⚡ **Gatilhos** | `login`, `lock`, `unlock`, `resume`, `monitor_added`, `monitor_removed`, eventos próprios via `pcue emit`, notificação de app (Linux), dispositivo USB conectado ou desconectado, horário diário e intervalo |
| 🪟 **Linux e Windows** | mesma config e mesma TUI; no Windows `workspace=N` é o monitor N |
| 🧠 **Passos espertos** | `optional`, `repeat=N`, `if_window=`, `unless_window=` e `call outra_rotina` |
| 🐞 **Depuração** | rodar só um passo, rodar a partir de um passo, acompanhar e interromper execuções |
| 📜 **Histórico detalhado** | todas as execuções, com a linha que falhou e o erro; `Enter` leva direto ao passo |
| 🗂️ **Organização** | duplicar, exportar e importar rotinas em `.toml`, busca na lista |
| 🖱️ **Mouse** | clique seleciona, duplo clique roda/edita, roda rola, botões do rodapé são clicáveis |
| 🎨 **Animações** | seleção que desliza, janelas que crescem, pulso do daemon, brilho ao salvar, contagem até o próximo disparo |

## Instalação

### Pacotes

Cada release em [Releases](https://github.com/Parsec-Inc/pcue/releases) traz:

| Sistema | Arquivo | Instalação |
|---|---|---|
| Fedora / openSUSE | `pcue-X.Y.Z-1.x86_64.rpm` | `sudo dnf install ./pcue-*.rpm` (ou pelo COPR, quando ligado) |
| Debian / Ubuntu | `pcue_X.Y.Z_amd64.deb` | `sudo apt install ./pcue_*.deb` |
| Arch | AUR `pcue-bin` | `yay -S pcue-bin` (quando ligado) |
| Windows | `pcue-X.Y.Z-x86_64.msi` | duplo clique; instala só para o seu usuário, sem admin |
| Qualquer Linux | `pcue-X.Y.Z-linux-x86_64` | binário estático; copie para o `PATH` |

Os pacotes de Linux já instalam a regra de udev que dá ao usuário da sessão
acesso ao teclado, mouse e `/dev/uinput` (sem grupo `input` nem logout) e
carregam o módulo `uinput` no boot. Depois de instalar, rode `pcue install`
(ou aperte `I` na TUI) para registrar o daemon no autostart.

O `pcue update` funciona em qualquer forma de instalação: se o binário estiver
numa pasta do sistema, ele pede a senha (pkexec no Linux, UAC no Windows).

### Linux pelo código

Requisitos:

- Rust estável (edition 2024)
- Hyprland 0.56+ para workspaces, janelas e cliques (sem Hyprland os apps abrem, mas sem posicionamento)
- Acesso a `/dev/input` (gravar) e `/dev/uinput` (reproduzir)

```sh
git clone https://github.com/Parsec-Inc/pcue.git
cd pcue
cargo install --path .

# acesso a teclado, mouse e uinput para o usuário da sessão (a mesma regra dos pacotes)
sudo install -Dm644 packaging/linux/70-pcue.rules /etc/udev/rules.d/70-pcue.rules
sudo install -Dm644 packaging/linux/modules-load.conf /etc/modules-load.d/pcue.conf
sudo sh packaging/linux/postinst

# registra o daemon no autostart da sessão (ou aperte I na TUI)
pcue install
```

O daemon se reinicia sozinho quando o binário muda, então depois de um
`cargo install --path .` não precisa fazer nada.

### Windows pelo código

Requisitos: Windows 10 ou 11 e Rust estável. Não precisa de permissão de
administrador.

```powershell
git clone https://github.com/Parsec-Inc/pcue.git
cd pcue
cargo install --path .
pcue install   # registra o daemon na inicialização do usuário (HKCU\...\Run)
```

Diferenças no Windows:

- `workspace=N` é o **monitor N**, contado da esquerda para a direita;
  `workspace N` foca a janela mais recente desse monitor.
- `class` é o nome do executável sem `.exe` (ex.: `Code`, `firefox`); a class
  Win32 da janela também é aceita.
- `wait_layer` não existe; `wait_monitors` espera a barra de tarefas e a
  quantidade de monitores parar de mudar.
- `type` digita em Unicode, então funciona com qualquer layout de teclado.
- O Windows não deixa sobrescrever um `.exe` em uso, mas deixa renomear: o
  `pcue update` afasta o antigo (`pcue.old.exe`) e grava o novo no lugar.
- Config em `%APPDATA%\pcue`, log e histórico em `%LOCALAPPDATA%\pcue`.

## Uso

```sh
pcue                 # abre a TUI
pcue run trabalho    # roda uma rotina agora
pcue run trabalho --from 4   # a partir da linha 4
pcue run trabalho --only 4   # só a linha 4
pcue emit cafe       # dispara o evento personalizado "cafe"
pcue record trabalho # grava pelo terminal e acrescenta à rotina
pcue capture -w 2    # gera passos launch das janelas abertas no workspace 2
pcue list            # lista as rotinas
pcue paths           # mostra config, log e histórico
pcue update --check  # mostra se há versão nova no GitHub
pcue update          # baixa a última release, confere o SHA-256 e troca o binário
```

A TUI procura versão nova ao abrir e avisa na barra de status; `U` pergunta
antes de atualizar. Nada é instalado sem confirmação, e o daemon se reinicia
sozinho quando o binário muda.

### Atalhos da TUI

Nada vai para o disco até você salvar: as edições ficam pendentes (o topo
mostra "não salvo"), `S` grava e `u` volta ao que estava salvo. Sair ou rodar
com alterações pendentes pede confirmação, e o daemon só enxerga o que foi
salvo.

| Onde | Teclas |
|---|---|
| Geral | `S`/`Ctrl+S` salva · `u` descarta as alterações · `Tab`/`←`/`→` troca painel · `H` histórico · `L` log · `I` autostart · `U` atualiza · `?` ajuda · `q` sai |
| Rotinas | `Enter` roda · `s` para · `n` nova · `D` duplica · `r` renomeia · `d` apaga · `espaço` ativa/desativa · `t` livre/agendada · `/` busca · `X` exporta · `i` importa |
| Gatilhos | `a` adiciona · `Enter` edita · `d` apaga |
| Passos | `a` adiciona · `Enter` edita · `x` roda só este · `f` roda daqui · `J`/`K` move · `C` limpa todos (pede confirmação) · `R` grava · `c` captura janelas · `E` abre no `$EDITOR` |

Com o mouse: clique seleciona, duplo clique roda (rotina) ou edita (gatilho e
passo), a roda rola as listas e os botões do rodapé e das janelas respondem ao
clique.

## Passos

Cada rotina é uma lista de passos, um por linha:

```text
launch "firefox --new-window" workspace=2 class=firefox [focus] [always]
wait_window class=REGEX [nth=N] [timeout=30s]
focus class=REGEX [nth=N] [timeout=30s]
wait_layer namespace=REGEX [timeout=30s]
wait_monitors [timeout=30s]
workspace N
sleep 500ms
key SUPER+2
type "texto"
click [left|right|middle] [hold=CTRL] X,Y
click window=REGEX [nth=N] at=X,Y
drag [left|right|middle] [hold=SUPER] [window=REGEX] from=X,Y [via=X,Y@120ms;X,Y@300ms] to=X,Y [duration=300ms]
hold SUPER                      # ou mouse_left|mouse_right|mouse_middle [X,Y]
move X,Y [duration=300ms]       # ou move window=REGEX at=X,Y
release SUPER
scroll -3
call outra_rotina [parâmetro=valor]
shell "comando" [background] [timeout=5m]
phone tap X,Y [duration=800ms]  # celular: toque (com duration, toque longo)
phone tap text=REGEX [id=REGEX] [timeout=30s]   # toca no elemento pelo texto
phone wait text=REGEX [id=REGEX] [timeout=30s]  # espera o elemento aparecer
phone swipe from=X,Y [via=X,Y@120ms;X,Y@300ms] to=X,Y [duration=300ms]
phone open br.com.brainweb.ifood [timeout=30s]   # abre o app e espera ele aparecer
phone key BACK                  # HOME, APP_SWITCH, ENTER, WAKEUP, VOLUME_UP...
phone text "texto"
# comentário
```

`shell` roda o comando sem janela (`sh -c` no Linux, `cmd /C` no Windows).
Por padrão espera ele terminar, até o `timeout` (5 min): código de saída
diferente de zero falha o passo, com as últimas linhas do erro na mensagem.
Com `background` ele segue rodando sozinho, mesmo depois que a rotina acaba.
Os parâmetros da rotina chegam como variáveis de ambiente (`-` vira `_`), e
não por `{nome}`, para um valor como `; rm -rf ~` nunca virar comando:

```text
shell "adb pull /sdcard/DCIM ~/Fotos/celular" timeout=30m
shell "echo \"$titulo: $texto\" >> ~/mensagens.txt"
```

Qualquer passo aceita modificadores:

| Modificador | Efeito |
|---|---|
| `optional` | se falhar, registra o aviso e a rotina segue |
| `repeat=N` | executa o passo N vezes |
| `if_window=REGEX` | só executa se existir janela com essa class |
| `unless_window=REGEX` | só executa se **não** existir janela com essa class |

### Parâmetros

Uma rotina pode receber valores na hora de rodar. Declare os parâmetros com
`P` na TUI (`pessoa, mensagem=safeeerrr`: vírgulas separam, `=` dá um padrão)
e use `{pessoa}` em qualquer passo. Para mandar mensagem a qualquer pessoa no
Teams:

```text
focus class=teams
key CTRL+ALT+E
type {pessoa}
key RETURN
type {mensagem}
key RETURN
```

Ao rodar pela TUI, ela pergunta cada valor (lembrando o último usado). Fora
dela: `pcue run teams-msg pessoa=erick` (bom para um atalho do Hyprland) ou,
dentro de outra rotina, `call teams-msg pessoa=erick`. Parâmetros sem padrão
são obrigatórios; rotina agendada precisa de padrão em todos, porque o daemon
roda sem perguntar (exceto os que o gatilho preenche, como os da
notificação).

### Gatilho por notificação

No Linux, uma rotina pode disparar quando um app notifica, por exemplo ao
chegar mensagem no Teams. Em `a` (gatilho) escolha **notificação** e informe
os filtros: `app=teams, titulo=erick, texto=urgente`. Cada filtro é uma regex
sem diferenciar maiúsculas, e filtro vazio aceita tudo. `Ctrl+G` no campo
captura a próxima notificação que chegar e preenche app e título.

| Filtro | O que é | No Teams |
|---|---|---|
| `app` | nome do app ou o `desktop-entry` do flatpak | `com.github.IsmaelMartinez.teams_for_linux` |
| `titulo` | título da notificação | quem mandou (ou o nome do grupo) |
| `texto` | corpo da notificação | a mensagem |

A rotina recebe os valores nos parâmetros `app`, `titulo` e `texto`, se os
declarar (com `P`). Eles não precisam de padrão, porque o gatilho os preenche:

```toml
[[routine]]
name = "responde-erick"
params = [{ name = "titulo" }]
triggers = [{ type = "notification", app = "teams", title = "erick" }]
steps = ["focus class=teams", "key CTRL+ALT+E", "type {titulo}", "key RETURN", "type \"já vejo!\"", "key RETURN"]
```

O daemon lê as notificações pelo D-Bus da sessão e não grava o conteúdo no
log. Notificações do próprio pcue são ignoradas, e a mesma notificação
repetida em menos de 2 s dispara uma vez só. Se o Teams estiver configurado
para esconder a prévia da mensagem, `texto` chega vazio.

### Celular (adb)

Os passos `phone` controlam um Android pelo `adb` (pacote `android-tools`),
com a **Depuração USB** ligada nas Opções do desenvolvedor do celular. As
posições são pixels da tela dele, na vertical. O primeiro passo `phone` de
cada execução espera o celular aparecer no adb, até o timeout da config, e
acende a tela se ela estiver apagada (a gravação também). Com bloqueio por
padrão, grave o desenho logo depois.

Para gravar, use `R` → **celular** na TUI (ou `pcue record --phone ROTINA`) e
mexa no celular: toque vira `phone tap`, toque parado por meio segundo vira
toque longo, arrasto vira `phone swipe` com o caminho inteiro em `via` (dá
para gravar o padrão de desbloqueio sem tirar o dedo), e os botões físicos (voltar, home,
volume, power) viram `phone key`, com as pausas entre eles. `SUPER+F9` pausa
e `SUPER+F10` encerra, no teclado do PC. A revisão sempre abre no fim: troque
a sequência de toques no teclado virtual por um `phone text "..."` (com `e`
num trecho). `phone text` só digita ASCII; acentos não passam pelo adb.

`phone wait` e `phone tap text=` leem a tela pelo `uiautomator` (cerca de 3 s
por leitura) até achar um elemento cujo texto ou descrição case com `text=`
(regex sem diferenciar maiúsculas) e cujo id case com `id=`. Servem para
esperar o app terminar de carregar e para tocar num botão onde quer que ele
esteja. Telas protegidas (senha, alguns bancos e jogos) não são lidas.

Durante a gravação a tela é lida sem parar: um toque num elemento com texto,
descrição ou id que o identifica sozinho vira `phone tap text=...`, com a
posição como alternativa na revisão. Toques muito seguidos (antes de dar
tempo de ler a tela) ficam pela posição.

Quando um app abre a partir da tela inicial, os toques feitos nela até ali
viram um único `phone open PACOTE` (a forma gravada fica como alternativa na
revisão, com `←`/`→`). O padrão de desbloqueio gravado fica em texto puro na
config: quem ler o arquivo consegue desbloquear o celular.

Só o primeiro dedo é gravado (pinça vira um arrasto), e a tela é tratada na
vertical. Os toques são repetidos nas mesmas posições, então um app que mudou
de layout ou a tela girada fazem a rotina errar, como os cliques no PC.

### Gatilho por dispositivo

Dispara quando um dispositivo USB é conectado (ou desconectado), como um
celular ou um pendrive. Em `a` escolha **dispositivo conectado** ou
**dispositivo desconectado** e informe uma regex do nome ou do id
`fabricante:produto` (`pixel`, `18d1:4ee7`); vazio aceita qualquer um, e
`Ctrl+G` captura o próximo que você plugar. Hubs USB ficam de fora.

```toml
[[routine]]
name = "backup-celular"
params = [{ name = "dispositivo" }]
triggers = [{ type = "device", name = "pixel" }]   # removed = true: ao desconectar
steps = ["shell \"adb wait-for-device && adb pull /sdcard/DCIM ~/Fotos\" timeout=30m"]
```

A rotina recebe `dispositivo` (fabricante e produto) e `id`. No Linux o
daemon lê os eventos do kernel; no Windows o nome não vem, então
`dispositivo` é o próprio id, e a captura com `Ctrl+G` ainda não funciona.

`focus` espera a janela existir e foca nela, trocando de workspace se
precisar. É mais robusto que `key SUPER+N`, que depende de onde o foco está.

### Gravação

`R` na TUI (ou `pcue record rotina`) grava teclado e mouse. Enquanto grava, o
pcue também observa o desktop e junta as ações com o que elas causaram:

| Você fez | O pcue sugere | Alternativas |
|---|---|---|
| abriu um app (launcher, atalho, clique) | `launch "cmd" workspace=N class=X focus` | teclas originais + `wait_window` |
| trocou de janela por atalho (`SUPER+N`, `Alt+Tab`) | `focus class=X` | `workspace N`, teclas originais |
| foi para um workspace vazio | `workspace N` | teclas originais |
| uma janela nova de um app já aberto apareceu | teclas originais + `wait_window class=X nth=N` | |

Ao encerrar, a tela de revisão mostra cada trecho com a forma escolhida:
`Tab`/`←`/`→` troca a forma, `p` liga ou desliga a pausa antes do trecho, `e`
edita o passo, `s` separa um trecho de vários passos em um por passo, `d`
apaga e `⏎` salva. Editar um trecho de vários passos separa ele antes e abre
o primeiro. Pausas longas (de quando você pensou) ficam visíveis para
desligar. `SUPER+F9` pausa a gravação (para senhas) e `SUPER+F10` encerra.

### Rotinas de boot

Ao adicionar o gatilho `login`, o pcue põe no início da rotina os passos que
esperam o desktop ficar pronto, conforme o que detecta na hora:

```text
# pcue: espera o desktop carregar (boot)
wait_layer namespace=quickshell:bar timeout=3m   # se houver barra conhecida (Quickshell, Waybar)
wait_monitors timeout=3m                          # Hyprland: monitores das workspace rules + workspaces no lugar
sleep 1500ms
```

O comentário marca o prefixo, então ele não é repetido. Pode editar ou apagar
os passos à vontade. No Windows o prefixo é só `wait_monitors` e o `sleep`.

`drag` segura as teclas de `hold=` e o botão em `from`, arrasta até `to` e
solta tudo. É o caso comum de `SUPER` + botão direito para redimensionar uma
janela: `drag right hold=SUPER from=800,400 to=800,700`. Para outras
combinações, `hold` aperta uma tecla ou botão e mantém até o `release`; o que
ficar apertado é solto no fim da rotina, mesmo se ela falhar. `via=` lista
pontos intermediários e quando passar por cada um, contando do início do
arrasto; o cursor anda entre eles no tempo gravado. Na gravação, arrastar com
o mouse vira `drag` com o trajeto inteiro (idas e vindas incluídas), e um
clique com modificador segurado vira `click hold=CTRL ...`. A posição de cada
clique é a do instante em que o botão desceu, mesmo em sequências rápidas.

`click window=... nth=2` mira a segunda janela da class na ordem em que foram
abertas, então dá para gravar cliques em duas instâncias do mesmo app.

## Configuração

Tudo fica em `~/.config/pcue/config.toml` (modo `0600`; no Windows em
`%APPDATA%\pcue\config.toml`). A TUI edita esse
arquivo e o daemon recarrega sozinho quando ele muda.

```toml
[settings]
timeout_secs = 30
startup_delay_ms = 1500

[[routine]]
name = "trabalho"
triggers = [
    { type = "event", event = "login" },
    { type = "daily", at = "09:00", days = ["mon", "tue", "wed", "thu", "fri"] },
]
steps = [
    "launch firefox workspace=1 class=firefox",
    "launch kitty workspace=2 class=kitty",
    "wait_window class=firefox timeout=20s",
    "key SUPER+1",
    "call slack",
]

[[routine]]
name = "slack"
steps = ["launch slack workspace=4 class=Slack unless_window=Slack"]

[[routine]]
name = "pausa_cafe"
triggers = [{ type = "interval", every = "2h" }]
steps = ["type \"bora um café?\""]
```

Arquivos:

| Caminho | Conteúdo |
|---|---|
| `~/.config/pcue/config.toml` | rotinas e ajustes |
| `~/.local/state/pcue/pcue.log` | log das execuções (texto digitado não é registrado) |
| `~/.local/state/pcue/history.jsonl` | histórico |
| `$XDG_RUNTIME_DIR/pcue/` | socket do daemon, progresso ao vivo e próximos disparos |

## Como funciona

```text
            ┌──────────────┐  eventos (logind/Hyprland ou sessão do Windows, emit)
 login ───► │ pcue daemon  │◄────────────────────────────────────────────
            │  agenda e    │  horários e intervalos
            │  dispara     │──► pcue run <rotina>  ──► player
            └──────────────┘                            │
                   ▲                                     ├─ Hyprland IPC: exec, mover, focar, cursor
                   │ next.json / progresso               └─ uinput: teclas, texto, cliques, scroll
            ┌──────┴───────┐
            │   pcue TUI   │  edita config, roda, grava (evdev), mostra histórico
            └──────────────┘
```

- **Plataforma**: `src/platform/` tem uma implementação para Linux (Hyprland,
  uinput, evdev, logind) e outra para Windows (Win32, SendInput, hooks de
  teclado e mouse, eventos de sessão); o resto do código é igual nos dois.
- **Hyprland**: o pcue fala direto com o socket IPC e detecta se o Hyprland usa a configuração em Lua (0.56+) ou a sintaxe antiga.
- **Gravação**: lê `/dev/input` com evdev (no Windows, hooks globais), agrupa texto, rolagens e atalhos, observa janelas e foco e propõe `launch`/`focus`/`workspace`.
- **Reprodução**: teclado/mouse virtual com uinput (no Windows, `SendInput`).
- **Execuções**: cada rotina roda num processo próprio, com trava por rotina, histórico e notificação em caso de falha.

## Pixel art

O mascote e os ícones foram desenhados no [Aseprite](https://www.aseprite.org/)
e ficam em `assets/sprites/`:

<p>
<img src="assets/screenshots/mascot.gif" width="96" alt="mascote" />
<img src="assets/screenshots/mascot_cheer.gif" width="72" alt="mascote comemorando" />
</p>

- `*.aseprite`: fontes editáveis, com a duração de cada quadro
- `*.png` + `*.json`: faixa horizontal exportada, embutida no binário
- `src/*.lua`: scripts que geram os sprites (`assets/sprites/src/build.sh`)

Depois de editar um sprite no Aseprite:

```sh
assets/export.sh
cargo install --path .
```

Na TUI cada célula do terminal mostra dois pixels (meio-bloco), então os
sprites têm altura par.

## Desenvolvimento

```sh
cargo test
cargo clippy
cargo run            # TUI
cargo run -- run nome --only 3
scripts/package.sh binaries deb rpm   # binários, .deb e .rpm em dist/ (ver docs/PACKAGING.md)

# Windows a partir do Linux (precisa do mingw-w64; testes rodam no wine)
cargo check --target x86_64-pc-windows-gnu
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine cargo test --target x86_64-pc-windows-gnu
```
