#!/usr/bin/env sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
INSTALLER="$SCRIPT_DIR/install-fedora-build-deps.sh"

packages="$("$INSTALLER" --print-packages)"

assert_contains() {
  package="$1"
  case " $packages " in
    *" $package "*) ;;
    *)
      echo "Expected Fedora build dependency package missing: $package" >&2
      echo "Packages: $packages" >&2
      exit 1
      ;;
  esac
}

assert_contains webkit2gtk4.1-devel
assert_contains gtk3-devel
assert_contains libayatana-appindicator-gtk3-devel
assert_contains librsvg2-devel
assert_contains alsa-lib-devel
assert_contains openssl-devel
assert_contains opus-devel
assert_contains libvpx-devel
assert_contains pipewire-devel
assert_contains avahi-compat-libdns_sd-devel
assert_contains clang
assert_contains clang-devel
assert_contains pkgconf-pkg-config
assert_contains gcc
assert_contains gcc-c++
assert_contains make
assert_contains patchelf
assert_contains rpm-build
assert_contains git
assert_contains pipewire
assert_contains xdg-desktop-portal
assert_contains xdg-desktop-portal-gtk

if ! grep -q "libpipewire-0.3" "$INSTALLER"; then
  echo "Expected Fedora helper to verify pkg-config module: libpipewire-0.3" >&2
  exit 1
fi

dry_run="$("$INSTALLER" --dry-run)"
case "$dry_run" in
  *"dnf install -y"*|*"yum install -y"*) ;;
  *)
    echo "Dry run did not show a dnf/yum install command:" >&2
    echo "$dry_run" >&2
    exit 1
    ;;
esac
