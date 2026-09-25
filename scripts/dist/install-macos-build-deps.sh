#!/usr/bin/env sh
set -eu

MACOS_BUILD_DEPS="pkgconf opus libvpx"

usage() {
  echo "Usage: scripts/dist/install-macos-build-deps.sh [--dry-run|--print-packages|--check]"
}

case "${1:-}" in
  "")
    command -v brew >/dev/null 2>&1 || { echo "Homebrew is required: https://brew.sh/" >&2; exit 1; }
    # Word splitting is intentional for this fixed package list.
    # shellcheck disable=SC2086
    brew install $MACOS_BUILD_DEPS
    ;;
  --dry-run)
    echo "brew install $MACOS_BUILD_DEPS"
    ;;
  --print-packages)
    echo "$MACOS_BUILD_DEPS"
    ;;
  --check)
    command -v brew >/dev/null 2>&1 || { echo "Homebrew is required: https://brew.sh/" >&2; exit 1; }
    missing=""
    for package in $MACOS_BUILD_DEPS; do
      if ! brew list --versions "$package" >/dev/null 2>&1; then
        missing="$missing $package"
      fi
    done
    if [ -n "$missing" ]; then
      echo "Missing Homebrew build dependencies:$missing" >&2
      exit 1
    fi
    echo "macOS build dependencies are installed."
    ;;
  --help|-h)
    usage
    ;;
  *)
    usage >&2
    exit 1
    ;;
esac
