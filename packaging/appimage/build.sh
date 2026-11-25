#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Build an AppImage bundling libreburnd and the desktop client: build.sh x86_64|aarch64
# Starting point; the Pi 5 packaging spike will harden it (PL-6).
set -euo pipefail
arch="${1:?usage: build.sh x86_64|aarch64}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
appdir="$root/build/AppDir"
rm -rf "$appdir" && mkdir -p "$appdir/usr/bin"

case "$arch" in
  x86_64) target=x86_64-unknown-linux-gnu ;;
  aarch64) target=aarch64-unknown-linux-gnu ;;
  *) echo "unknown arch $arch" >&2; exit 1 ;;
esac

cargo build --release --locked -p libreburn-service --target "$target"
install -Dm755 "$root/target/$target/release/libreburnd" "$appdir/usr/bin/libreburnd"

python3 -m venv "$appdir/usr/venv"
"$appdir/usr/venv/bin/pip" install "$root"/dist/libreburn-*.whl "$root/python/libreburn-desktop"

install -Dm644 "$root/packaging/appimage/libreburn.desktop" "$appdir/libreburn.desktop"
cat > "$appdir/AppRun" <<'RUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
"$here/usr/bin/libreburnd" &
trap 'kill $!' EXIT
"$here/usr/venv/bin/libreburn" "$@"
RUN
chmod +x "$appdir/AppRun"

curl -fsSLo appimagetool "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$(uname -m).AppImage"
chmod +x appimagetool
ARCH="$arch" ./appimagetool --appimage-extract-and-run "$appdir" "libreburn-$arch.AppImage"
