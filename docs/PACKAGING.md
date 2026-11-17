# Empacotamento e releases

Tudo sai de `scripts/package.sh`, o mesmo script no seu computador e no CI.

## Como sai uma release

Os testes (`tests.yml`: clippy e `cargo test` nativos no Linux e no Windows) rodam em todo push
fora da `main`; um push novo na mesma branch cancela o teste anterior.

A release roda quando um PR é mergeado na `main` (`.github/workflows/release.yml`):

1. **tests** e **version** começam juntos. O `version` roda o commitizen, que calcula a versão
   pelos commits (`feat` sobe minor, `fix` sobe patch) e cria localmente o commit do bump
   (`Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`) e a tag, passados aos outros jobs num git bundle.
   Sem commit relevante, para aqui. Para forçar, rode o workflow à mão na `main` escolhendo o
   incremento.
2. **linux**: binário estático (musl) e `.exe` (mingw), `.deb`, `.rpm`, `.src.rpm` do COPR e
   PKGBUILD do AUR.
3. **msi**: no runner Windows, empacota o mesmo `.exe` com o WiX 3.
4. **release**: só com os testes e o linux verdes. Dá push do commit do bump e da tag na `main`,
   gera `SHA256SUMS` e cria a release. Se o MSI falhar, a release sai sem ele; o job linux é
   obrigatório porque o `pcue update` baixa os binários dele.
5. **aur** e **copr**: só rodam se as variáveis do repositório estiverem ligadas (abaixo).

## Build local

| Comando | Gera (em `dist/`) | Precisa de |
|---|---|---|
| `scripts/package.sh binaries` | binário Linux (musl) e `.exe` | `musl-tools` / `musl-gcc` e `mingw64-gcc` |
| `scripts/package.sh deb` | `.deb` | `cargo install cargo-deb` |
| `scripts/package.sh rpm` | `.rpm` e `.src.rpm` (o que vai para o COPR) | `rpm-build` |
| `scripts/package.sh msi` | `.msi` | WiX 3 (só Windows; o `wixl` ignora a instalação por usuário) |
| `scripts/package.sh aur` | `dist/aur/PKGBUILD` | |

Os comandos rodam em ordem e os pacotes usam os binários do `binaries`.

## Permissões que os pacotes configuram

| Onde | O quê | Por quê |
|---|---|---|
| Linux | `/usr/lib/udev/rules.d/70-pcue.rules` com `TAG+="uaccess"` em teclados, mouses e `uinput` | gravar e reproduzir sem grupo `input` nem logout; vale só para o usuário da sessão ativa |
| Linux | `/usr/lib/modules-load.d/pcue.conf` | carrega o `uinput` no boot |
| Linux | `postinst` (deb, rpm e AUR) | `modprobe uinput` e `udevadm trigger` para valer na hora |
| Windows | MSI por usuário em `%LOCALAPPDATA%\Programs\pcue` + `PATH` do usuário | não precisa de admin, e o `pcue update` troca o `.exe` sem UAC |

O daemon continua sendo registrado pelo próprio pcue (`pcue install` ou `I` na TUI), porque o
pacote é instalado como root e não sabe qual usuário vai usar.

## Atualização

Os dois caminhos convivem:

- **Gerenciador de pacotes** (dnf, apt, pacman): atualiza quando o canal estiver ligado.
- **`pcue update` / `U` na TUI**: baixa da release do GitHub e confere o `SHA256SUMS`. Se o binário
  estiver numa pasta sem escrita (instalado por pacote), pede a senha com `pkexec` (Linux) ou UAC
  (Windows). O gerenciador continua achando que a versão é a antiga até a próxima atualização dele.

## Canais (configuração única)

Nada disso está configurado ainda; cada canal fica desligado até a variável virar `true`
(*Settings > Secrets and variables > Actions*).

| Canal | Para o usuário | O que configurar |
|---|---|---|
| **GitHub Releases** | baixar `.rpm` / `.deb` / `.msi` / binário | nada |
| **AUR** (`pcue-bin`) | `yay -S pcue-bin` | Conta no AUR com chave SSH; secret `AUR_SSH_PRIVATE_KEY`, variável `PUBLISH_AUR=true`. O primeiro push cria o pacote. |
| **COPR** (dnf) | `sudo dnf copr enable <dono>/pcue && sudo dnf install pcue` | Crie o projeto em copr.fedorainfracloud.org (chroots Fedora x86_64). Em *API*, copie o conteúdo para o secret `COPR_CONFIG`. Variáveis `COPR_PROJECT=<dono>/pcue` e `PUBLISH_COPR=true`. |

O AUR e o COPR empacotam o binário da release (não compilam de novo), então ficam na mesma versão
da release do GitHub.

## O que não foi testado

- `.deb`, `.rpm` e o pacote do AUR foram instalados e removidos em containers (Debian, Fedora, Arch).
- A regra de udev passou no `udevadm verify`; o efeito do `uaccess` numa sessão real não foi testado.
- O MSI só roda no CI; o primeiro release é o primeiro teste.
- A troca com `pkexec`/UAC do `pcue update` não foi exercitada.
