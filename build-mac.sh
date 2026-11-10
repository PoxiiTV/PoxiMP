#!/usr/bin/env bash
# PoxiMP for macOS (Apple Silicon): signed update bundle, dmg and latest.json in deploy-hosting/.
#   ./build-mac.sh          build only
#   ./build-mac.sh --subir  build and upload to the GitHub release of this version (made from Windows first)
# The update signing key: TAURI_SIGNING_PRIVATE_KEY (path or contents), by default ~/.tauri/poximp.key.
set -euo pipefail
cd "$(dirname "$0")"

echo
echo "  =========================================================="
echo "    PoxiMP  -  Mac (Apple Silicon): dmg + actualización"
echo "  =========================================================="
echo

for tool in node npm cargo rustup cmake; do
  command -v "$tool" >/dev/null || { echo "  [X] Falta $tool (cmake: brew install cmake)."; exit 1; }
done
rustup target list --installed | grep -q aarch64-apple-darwin || rustup target add aarch64-apple-darwin

export TAURI_SIGNING_PRIVATE_KEY="${TAURI_SIGNING_PRIVATE_KEY:-$HOME/.tauri/poximp.key}"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"
if [ ! -f "$TAURI_SIGNING_PRIVATE_KEY" ] && [[ "$TAURI_SIGNING_PRIVATE_KEY" == /* ]]; then
  echo "  [X] No está la clave de firma en $TAURI_SIGNING_PRIVATE_KEY."
  echo "      Cópiala ahí (poximp.key) o pon su ruta en TAURI_SIGNING_PRIVATE_KEY."
  exit 1
fi

[ -d node_modules ] || { echo "  [1/3] Instalando dependencias de Node..."; npm ci; }

VER=$(node -p "require('./src-tauri/tauri.conf.json').version")
echo "  [2/3] Compilando PoxiMP $VER (aarch64-apple-darwin)..."
npx tauri build --target aarch64-apple-darwin

B=target/aarch64-apple-darwin/release/bundle
OUT=deploy-hosting
NAME="PoxiMP-$VER-mac-arm64"
echo
echo "  [3/3] Preparando $OUT..."
rm -rf "$OUT" && mkdir "$OUT"
cp "$B/dmg/PoxiMP_${VER}_aarch64.dmg" "$OUT/$NAME.dmg"
cp "$B/macos/PoxiMP.app.tar.gz" "$OUT/$NAME.app.tar.gz"
cp "$B/macos/PoxiMP.app.tar.gz.sig" "$OUT/$NAME.app.tar.gz.sig"

# latest.json is shared with Windows: start from the release's one so its entry is kept.
if command -v gh >/dev/null && gh release view "v$VER" >/dev/null 2>&1; then
  gh release download "v$VER" -p latest.json -D "$OUT" --clobber
else
  echo "  (Sin release v$VER en GitHub todavía: latest.json solo llevará Mac.)"
fi
node scripts/latest-json.mjs

if [ "${1:-}" = "--subir" ]; then
  gh release upload "v$VER" "$OUT/$NAME.dmg" "$OUT/$NAME.app.tar.gz" "$OUT/latest.json" --clobber
  echo "  Subido a la release v$VER."
else
  echo
  echo "  Para subirlo:  ./build-mac.sh --subir"
  echo "  (o a mano: $NAME.dmg, $NAME.app.tar.gz y latest.json a la release v$VER)"
fi
echo
ls -lh "$OUT"
