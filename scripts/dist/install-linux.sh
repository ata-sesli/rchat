#!/usr/bin/env sh
set -eu

DEB_DEPS="libwebkit2gtk-4.1-0 libgtk-3-0 libayatana-appindicator3-1 librsvg2-2 libasound2 libssl3 libopus0 libavahi-client3 libavahi-compat-libdnssd1 pipewire xdg-desktop-portal"
DEB_LIBVPX_DEPS="libvpx9 libvpx8 libvpx7"
RPM_DEPS="webkit2gtk4.1 gtk3 libayatana-appindicator-gtk3 librsvg2 alsa-lib openssl-libs opus libvpx avahi-compat-libdns_sd pipewire xdg-desktop-portal"
RPM_PORTAL_BACKENDS="xdg-desktop-portal-gnome xdg-desktop-portal-gtk xdg-desktop-portal-kde xdg-desktop-portal-wlr"

usage() {
  cat <<USAGE
Usage: scripts/dist/install-linux.sh <package-file>

Examples:
  scripts/dist/install-linux.sh src-tauri/target/release/bundle/deb/rchat_0.1.0_amd64.deb
  scripts/dist/install-linux.sh src-tauri/target/release/bundle/rpm/rchat-0.1.0-1.x86_64.rpm
USAGE
}

fail_missing_deb() {
  missing="$1"
  echo "Missing Debian/Ubuntu runtime dependencies:" >&2
  echo "  $missing" >&2
  echo "For libvpx, your distro may provide libvpx9, libvpx8, or libvpx7." >&2
  echo "Install them first:" >&2
  echo "  sudo apt-get update && sudo apt-get install -y $missing" >&2
  exit 1
}

fail_missing_rpm() {
  missing="$1"
  portal_note="${2:-}"
  echo "Missing RPM runtime dependencies:" >&2
  echo "  $missing" >&2
  if [ -n "$portal_note" ]; then
    echo "$portal_note" >&2
  fi
  echo "Install them first:" >&2
  if command -v dnf >/dev/null 2>&1; then
    echo "  sudo dnf install -y $missing" >&2
  elif command -v yum >/dev/null 2>&1; then
    echo "  sudo yum install -y $missing" >&2
  else
    echo "  Use your package manager to install the packages above." >&2
  fi
  exit 1
}

if [ "${1:-}" = "" ]; then
  usage
  exit 1
fi

PKG_FILE="$1"
if [ ! -f "$PKG_FILE" ]; then
  echo "Package file not found: $PKG_FILE" >&2
  exit 1
fi

case "$PKG_FILE" in
  *.deb)
    if ! command -v dpkg-query >/dev/null 2>&1; then
      echo "dpkg-query is required to validate .deb dependencies." >&2
      exit 1
    fi

    missing=""
    for pkg in $DEB_DEPS; do
      if ! dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null | grep -q "ok installed"; then
        missing="$missing $pkg"
      fi
    done
    libvpx_found=""
    for pkg in $DEB_LIBVPX_DEPS; do
      if dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null | grep -q "ok installed"; then
        libvpx_found="yes"
      fi
    done
    if [ -z "$libvpx_found" ]; then
      missing="$missing libvpx9"
    fi
    if [ -n "$missing" ]; then
      fail_missing_deb "$(echo "$missing" | xargs)"
    fi

    echo "Dependencies satisfied. Installing package: $PKG_FILE"
    sudo apt-get install -y "$PKG_FILE"
    ;;

  *.rpm)
    if ! command -v rpm >/dev/null 2>&1; then
      echo "rpm is required to validate .rpm dependencies." >&2
      exit 1
    fi

    missing=""
    for pkg in $RPM_DEPS; do
      if ! rpm -q "$pkg" >/dev/null 2>&1; then
        missing="$missing $pkg"
      fi
    done
    portal_backend_found=""
    for pkg in $RPM_PORTAL_BACKENDS; do
      if rpm -q "$pkg" >/dev/null 2>&1; then
        portal_backend_found="yes"
      fi
    done
    portal_note=""
    if [ -z "$portal_backend_found" ]; then
      missing="$missing xdg-desktop-portal-gtk"
      portal_note="Portal backend note: install xdg-desktop-portal-gtk for XFCE/GTK sessions, or another backend matching your desktop."
    fi
    if [ -n "$missing" ]; then
      fail_missing_rpm "$(echo "$missing" | xargs)" "$portal_note"
    fi

    echo "Dependencies satisfied. Installing package: $PKG_FILE"
    if command -v dnf >/dev/null 2>&1; then
      sudo dnf install -y "$PKG_FILE"
    elif command -v yum >/dev/null 2>&1; then
      sudo yum localinstall -y "$PKG_FILE"
    else
      echo "Neither dnf nor yum is available for installing RPM packages." >&2
      exit 1
    fi
    ;;

  *)
    echo "Unsupported package type: $PKG_FILE" >&2
    usage
    exit 1
    ;;
esac
