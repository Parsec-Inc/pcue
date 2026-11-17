#!/usr/bin/env bash
# Gera os artefatos da release em dist/. Local e CI usam este mesmo script.
#   scripts/package.sh binaries | deb | rpm | msi | aur   (em ordem; rpm também gera o .src.rpm do COPR)
# VERSION vem do Cargo.toml; o CI passa a versão do bump.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}
LINUX=x86_64-unknown-linux-musl
WINDOWS=x86_64-pc-windows-gnu
LINUX_BIN="dist/pcue-$VERSION-linux-x86_64"
WINDOWS_BIN="dist/pcue-$VERSION-windows-x86_64.exe"
mkdir -p dist

need() { command -v "$1" >/dev/null || { echo "faltando '$1': $2" >&2; exit 1; }; }
built() { [ -f "$1" ] || { echo "falta $1: rode antes scripts/package.sh binaries" >&2; exit 1; }; }

binaries() {
  cargo build --locked --release --target "$LINUX"
  cargo build --locked --release --target "$WINDOWS"
  cp "target/$LINUX/release/pcue" "$LINUX_BIN"
  cp "target/$WINDOWS/release/pcue.exe" "$WINDOWS_BIN"
}

deb() {
  need cargo-deb "cargo install cargo-deb"
  built "$LINUX_BIN"
  cargo deb --no-build --no-strip --target "$LINUX" --deb-version "$VERSION" --output "dist/pcue_${VERSION}_amd64.deb"
}

# .rpm e .src.rpm (o COPR recompila o .src.rpm) com a spec e os arquivos de packaging/linux.
rpm() {
  need rpmbuild "sudo dnf install rpm-build (ou apt install rpm)"
  built "$LINUX_BIN"
  local top
  top=$(mktemp -d)
  mkdir -p "$top/SOURCES" "$top/SPECS"
  cp "$LINUX_BIN" packaging/linux/* "$top/SOURCES/"
  sed "s/@VERSION@/$VERSION/" packaging/rpm/pcue.spec > "$top/SPECS/pcue.spec"
  rpmbuild --quiet --define "_topdir $top" --define "dist %{nil}" --target x86_64 -ba "$top/SPECS/pcue.spec"
  find "$top/RPMS" "$top/SRPMS" -name '*.rpm' -exec cp {} dist/ \;
  rm -rf "$top"
}

# MSI por usuário: precisa do WiX 3 (o wixl do Linux ignora InstallPrivileges e Platform).
msi() {
  need candle "WiX 3 no Windows (choco install wixtoolset)"
  built "$WINDOWS_BIN"
  mkdir -p target/wix
  candle -nologo -arch x64 -dVersion="$VERSION" -dExe="$PWD/$WINDOWS_BIN" -out target/wix/pcue.wixobj packaging/windows/pcue.wxs
  light -nologo -spdb -out "dist/pcue-$VERSION-x86_64.msi" target/wix/pcue.wixobj
}

# PKGBUILD do AUR (pcue-bin) com a versão; as somas o deploy calcula.
aur() {
  mkdir -p dist/aur
  sed "s/@VERSION@/$VERSION/" packaging/aur/PKGBUILD > dist/aur/PKGBUILD
  cp packaging/aur/pcue.install dist/aur/
}

[ $# -ge 1 ] || { sed -n '2,4p' "$0"; exit 1; }
for cmd in "$@"; do
  case $cmd in
    binaries | deb | rpm | msi | aur) "$cmd" ;;
    *) echo "artefato desconhecido: $cmd" >&2; exit 1 ;;
  esac
done
ls -lh dist
