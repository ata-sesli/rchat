#!/usr/bin/env sh
set -eu

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO_DIR=$(CDPATH='' cd -- "$SCRIPT_DIR/../.." && pwd)

case "${1:-}" in
  "") dry_run=0 ;;
  --dry-run) dry_run=1 ;;
  --help|-h)
    echo "Usage: scripts/dist/bootstrap-source.sh [--dry-run]"
    echo "Installs native dependencies, checks prerequisites, and checks the frontend, core, and TUI."
    exit 0
    ;;
  *) echo "Usage: scripts/dist/bootstrap-source.sh [--dry-run]" >&2; exit 1 ;;
esac

case "$(uname -s)" in
  Darwin) installer=install-macos-build-deps.sh ;;
  Linux)
    if [ ! -r /etc/os-release ]; then
      echo "Cannot identify Linux distribution (/etc/os-release missing)." >&2
      exit 1
    fi
    # shellcheck disable=SC1091
    . /etc/os-release
    case " ${ID:-} ${ID_LIKE:-} " in
      *" debian "*|*" ubuntu "*) installer=install-debian-build-deps.sh ;;
      *" fedora "*|*" rhel "*|*" centos "*) installer=install-fedora-build-deps.sh ;;
      *) echo "Unsupported Linux distribution: ${ID:-unknown}. Install dependencies manually." >&2; exit 1 ;;
    esac
    ;;
  *) echo "Source bootstrap supports macOS, Debian/Ubuntu, and Fedora/RHEL." >&2; exit 1 ;;
esac

if [ "$dry_run" -eq 1 ]; then
  echo "$SCRIPT_DIR/$installer"
  echo "$SCRIPT_DIR/doctor.sh --check-gui"
  echo "bun install --frozen-lockfile"
  echo "bun run check"
  echo "bun run build"
  echo "cargo check --locked --manifest-path src-tauri/Cargo.toml -p rchat-core -p rchat-tui"
  exit 0
fi

cd "$REPO_DIR"
"$SCRIPT_DIR/$installer"
"$SCRIPT_DIR/doctor.sh" --check-gui
bun install --frozen-lockfile
bun run check
bun run build
cargo check --locked --manifest-path src-tauri/Cargo.toml -p rchat-core -p rchat-tui
