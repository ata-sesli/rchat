#!/usr/bin/env sh
set -eu

FEDORA_BUILD_DEPS="webkit2gtk4.1-devel gtk3-devel libayatana-appindicator-gtk3-devel librsvg2-devel alsa-lib-devel openssl-devel opus-devel libvpx-devel pipewire-devel avahi-compat-libdns_sd-devel clang clang-devel pkgconf-pkg-config gcc gcc-c++ make patchelf rpm-build git"

usage() {
  cat <<USAGE
Usage: scripts/dist/install-fedora-build-deps.sh [--dry-run|--print-packages|--check]

Installs Fedora/RHEL native dependencies needed to build RChat from source.

Options:
  --dry-run         Print the install command without running it
  --print-packages  Print the package list only
  --check           Check installed RPM packages and print missing packages
USAGE
}

package_manager() {
  if command -v dnf >/dev/null 2>&1; then
    echo "dnf"
    return
  fi
  if command -v yum >/dev/null 2>&1; then
    echo "yum"
    return
  fi
  echo "dnf"
}

sudo_prefix() {
  if [ "$(id -u)" = "0" ]; then
    echo ""
  else
    echo "sudo"
  fi
}

print_tool_notes() {
  missing_tools=""
  for tool in cargo rustc bun; do
    if ! command -v "$tool" >/dev/null 2>&1; then
      missing_tools="$missing_tools $tool"
    fi
  done

  if [ -n "$missing_tools" ]; then
    echo "Note: missing source-build tools:$(echo "$missing_tools" | xargs)" >&2
    echo "Install Rust from https://rustup.rs/ and Bun from https://bun.sh/ before building RChat." >&2
  fi
}

check_packages() {
  if ! command -v rpm >/dev/null 2>&1; then
    echo "rpm is required to check Fedora/RHEL dependencies." >&2
    exit 1
  fi

  missing=""
  for pkg in $FEDORA_BUILD_DEPS; do
    if ! rpm -q "$pkg" >/dev/null 2>&1; then
      missing="$missing $pkg"
    fi
  done

  if [ -n "$missing" ]; then
    missing_trimmed=$(echo "$missing" | xargs)
    echo "Missing Fedora/RHEL source-build dependencies:" >&2
    echo "  $missing_trimmed" >&2
    echo "Install them with:" >&2
    echo "  $(sudo_prefix) $(package_manager) install -y $missing_trimmed" >&2
    exit 1
  fi

  print_tool_notes
  echo "Fedora/RHEL native source-build dependencies are installed."
}

case "${1:-}" in
  "")
    manager=$(package_manager)
    sudo_cmd=$(sudo_prefix)
    if [ -n "$sudo_cmd" ]; then
      $sudo_cmd "$manager" install -y $FEDORA_BUILD_DEPS
    else
      "$manager" install -y $FEDORA_BUILD_DEPS
    fi
    print_tool_notes
    ;;
  --dry-run)
    sudo_cmd=$(sudo_prefix)
    if [ -n "$sudo_cmd" ]; then
      echo "$sudo_cmd $(package_manager) install -y $FEDORA_BUILD_DEPS"
    else
      echo "$(package_manager) install -y $FEDORA_BUILD_DEPS"
    fi
    ;;
  --print-packages)
    echo "$FEDORA_BUILD_DEPS"
    ;;
  --check)
    check_packages
    ;;
  --help|-h)
    usage
    ;;
  *)
    usage >&2
    exit 1
    ;;
esac
