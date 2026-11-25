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

# Pinned tool and runtime, checked against known hashes. Upstream's "continuous"
# builds move under us, so never fetch those in a release build.
appimagetool_version=1.9.1
runtime_version=20251108
declare -A appimagetool_sha256=(
  [x86_64]=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
  [aarch64]=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
)
declare -A runtime_sha256=(
  [x86_64]=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
  [aarch64]=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444
)
host="$(uname -m)"
curl -fsSLo appimagetool "https://github.com/AppImage/appimagetool/releases/download/$appimagetool_version/appimagetool-$host.AppImage"
curl -fsSLo appimage-runtime "https://github.com/AppImage/type2-runtime/releases/download/$runtime_version/runtime-$arch"
sha256sum --check --strict - <<SUMS
${appimagetool_sha256[$host]}  appimagetool
${runtime_sha256[$arch]}  appimage-runtime
SUMS
chmod +x appimagetool
# Signing happens later, in the release job, so the key never meets build scripts.
ARCH="$arch" ./appimagetool --appimage-extract-and-run --runtime-file appimage-runtime \
  "$appdir" "libreburn-$arch.AppImage"
