#!/usr/bin/env sh
set -eu

usage() {
  echo "Usage: scripts/dist/doctor.sh [--check-core-tui|--check-gui]"
  echo "Checks source-build tools and native libraries; --check-gui also checks Bun and Linux WebKitGTK."
}

mode=${1:---check-core-tui}
case "$mode" in
  --check-core-tui|--check-gui) ;;
  --help|-h) usage; exit 0 ;;
  *) usage >&2; exit 1 ;;
esac

failed=0
require_tool() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing tool: $1 ($2)" >&2
    failed=1
  fi
}

require_module() {
  if command -v pkg-config >/dev/null 2>&1 && ! pkg-config --exists "$1"; then
    echo "Missing native library metadata: $1 ($2)" >&2
    failed=1
  fi
}

require_tool cargo "install Rust from https://rustup.rs/"
require_tool rustc "install Rust from https://rustup.rs/"
require_tool clang "install Clang or Xcode Command Line Tools"
require_tool pkg-config "run the platform native-dependency installer"
require_module opus "libopus development package"
require_module vpx "libvpx development package"

case "$(uname -s)" in
  Linux)
    require_module libpipewire-0.3 "PipeWire development package"
    if [ "$mode" = --check-gui ]; then
      require_module webkit2gtk-4.1 "WebKitGTK 4.1 development package"
      require_module gtk+-3.0 "GTK 3 development package"
    fi
    ;;
  Darwin)
    require_tool xcode-select "install Xcode Command Line Tools with xcode-select --install"
    if command -v xcode-select >/dev/null 2>&1 && ! xcode-select -p >/dev/null 2>&1; then
      echo "Xcode Command Line Tools are not selected." >&2
      failed=1
    fi
    ;;
  *)
    echo "This doctor supports Linux and macOS source builds." >&2
    exit 1
    ;;
esac

if [ "$mode" = --check-gui ]; then
  require_tool bun "install Bun from https://bun.sh/"
fi

if [ "$failed" -ne 0 ]; then
  echo "Install missing prerequisites, then rerun this check." >&2
  exit 1
fi
echo "RChat source-build prerequisites are available ($mode)."
