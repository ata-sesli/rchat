#!/usr/bin/env sh
set -eu
ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
case "${1:-}" in gui|tui) kind=$1 ;; *) echo 'Usage: build-macos-package.sh <gui|tui> <output-directory>' >&2; exit 1 ;; esac
[ "$#" -eq 2 ] || exit 1
[ "$(uname -s)" = Darwin ] || { echo 'Build on macOS' >&2; exit 1; }
: "${CARGO_TARGET_DIR:?Set CARGO_TARGET_DIR explicitly to an external build disk directory}"
case "$CARGO_TARGET_DIR" in /*) ;; *) echo 'CARGO_TARGET_DIR must be absolute' >&2; exit 1 ;; esac
mkdir -p "$2"
output=$(CDPATH='' cd -- "$2" && pwd)
cd "$ROOT"
# Initial packaging baseline; validate on clean macOS 15 before publication.
export MACOSX_DEPLOYMENT_TARGET=15.0
version=$(node -p 'JSON.parse(require("fs").readFileSync("src-tauri/tauri.conf.json")).version')
if [ "$kind" = tui ]; then
  cargo build --locked --release --manifest-path src-tauri/Cargo.toml -p rchat-tui
  binary="$CARGO_TARGET_DIR/release/rchat-tui"
  node scripts/dist/runtime-audit.mjs macos "$binary"
  stage=$(mktemp -d "$output/rchat-tui-stage.XXXXXX")
  cp "$binary" LICENSE "$stage/"
  cp src-tauri/licenses/libvpx-LICENSE "$stage/"
  printf '%s\n' 'Install Kitty on the display/client Mac. SSH servers do not need Kitty.' > "$stage/README.txt"
  archive="$output/rchat-tui-$version-macos-$(uname -m).tar.gz"
  tar -czf "$archive" -C "$stage" .
  shasum -a 256 "$archive" > "$archive.sha256"
else
  bun install --frozen-lockfile
  bun run tauri build --bundles app,dmg --config '{"bundle":{"macOS":{"minimumSystemVersion":"15.0"}}}' -- --locked
  node scripts/dist/runtime-audit.mjs macos "$CARGO_TARGET_DIR/release/bundle/macos/rchat.app/Contents/MacOS/rchat"
  found=0
  for dmg in "$CARGO_TARGET_DIR"/release/bundle/dmg/*.dmg; do
    [ -f "$dmg" ] || continue
    cp "$dmg" "$output/"
    shasum -a 256 "$output/$(basename "$dmg")" > "$output/$(basename "$dmg").sha256"
    found=1
  done
  [ "$found" -eq 1 ] || { echo 'No DMG produced' >&2; exit 1; }
fi
echo 'Artifacts require clean-machine launch/media verification before publication.'
