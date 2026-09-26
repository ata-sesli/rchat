#!/usr/bin/env sh
set -eu
ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
case "${1:-}" in gui|tui) kind=$1 ;; *) echo 'Usage: build-linux-package.sh <gui|tui> <output-directory>' >&2; exit 1 ;; esac
[ "$#" -eq 2 ] || exit 1
mkdir -p "$2"
output=$(CDPATH='' cd -- "$2" && pwd)
cd "$ROOT"
# Fail before compiling when the host is not a supported packaging baseline.
# shellcheck disable=SC2016
node --input-type=module -e 'import {baselineFor} from "./scripts/dist/package-linux.mjs"; import {readFileSync} from "node:fs"; const s=readFileSync("/etc/os-release","utf8"); const get=k=>s.match(new RegExp(`^${k}="?([^"\\n]+)`,"m"))?.[1]; baselineFor(get("ID"),get("VERSION_ID"));'
: "${CARGO_TARGET_DIR:?Set CARGO_TARGET_DIR explicitly to a build disk directory}"
case "$CARGO_TARGET_DIR" in /*) ;; *) echo 'CARGO_TARGET_DIR must be absolute' >&2; exit 1 ;; esac
if [ "$kind" = tui ]; then
  cargo build --locked --release --manifest-path src-tauri/Cargo.toml -p rchat-tui
  binary=rchat-tui
else
  bun install --frozen-lockfile
  bun run tauri build --no-bundle -- --locked
  binary=rchat
fi
node scripts/dist/package-linux.mjs "$kind" "$CARGO_TARGET_DIR/release/$binary" "$output"
