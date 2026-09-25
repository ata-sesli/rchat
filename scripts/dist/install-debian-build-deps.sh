#!/usr/bin/env sh
set -eu

# Core/TUI deps are kept separate so CI need not install the GUI toolchain.
CORE_TUI_DEPS="libasound2-dev libssl-dev libopus-dev libvpx-dev libpipewire-0.3-dev libavahi-compat-libdnssd-dev clang libclang-dev pkg-config build-essential shellcheck"
GUI_DEPS="libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev patchelf curl wget file"
DEBIAN_BUILD_DEPS="$CORE_TUI_DEPS $GUI_DEPS"

usage() {
  echo "Usage: scripts/dist/install-debian-build-deps.sh [--core-tui|--dry-run|--print-packages|--print-core-tui-packages|--check]"
}

run_apt() {
  if [ "$(id -u)" = 0 ]; then
    apt-get "$@"
  else
    sudo apt-get "$@"
  fi
}

case "${1:-}" in
  "")
    command -v apt-get >/dev/null 2>&1 || { echo "apt-get is required (Debian/Ubuntu)." >&2; exit 1; }
    run_apt update
    # Word splitting is intentional for this fixed package list.
    # shellcheck disable=SC2086
    run_apt install -y $DEBIAN_BUILD_DEPS
    ;;
  --core-tui)
    command -v apt-get >/dev/null 2>&1 || { echo "apt-get is required (Debian/Ubuntu)." >&2; exit 1; }
    run_apt update
    # shellcheck disable=SC2086
    run_apt install -y $CORE_TUI_DEPS
    ;;
  --dry-run)
    if [ "$(id -u)" = 0 ]; then
      echo "apt-get update"
      echo "apt-get install -y $DEBIAN_BUILD_DEPS"
    else
      echo "sudo apt-get update"
      echo "sudo apt-get install -y $DEBIAN_BUILD_DEPS"
    fi
    ;;
  --print-packages)
    echo "$DEBIAN_BUILD_DEPS"
    ;;
  --print-core-tui-packages)
    echo "$CORE_TUI_DEPS"
    ;;
  --check)
    command -v dpkg-query >/dev/null 2>&1 || { echo "dpkg-query is required (Debian/Ubuntu)." >&2; exit 1; }
    missing=""
    for package in $DEBIAN_BUILD_DEPS; do
      if ! dpkg-query -W -f='${Status}' "$package" 2>/dev/null | grep -q 'ok installed'; then
        missing="$missing $package"
      fi
    done
    if [ -n "$missing" ]; then
      echo "Missing Debian/Ubuntu build dependencies:$missing" >&2
      exit 1
    fi
    echo "Debian/Ubuntu build dependencies are installed."
    ;;
  --help|-h)
    usage
    ;;
  *)
    usage >&2
    exit 1
    ;;
esac
