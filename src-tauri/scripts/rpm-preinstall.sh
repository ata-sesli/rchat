#!/usr/bin/env sh
set -eu

REQUIRED_PACKAGES="webkit2gtk4.1 gtk3 libayatana-appindicator-gtk3 librsvg2 alsa-lib openssl-libs opus libvpx avahi-compat-libdns_sd pipewire xdg-desktop-portal"
PORTAL_BACKENDS="xdg-desktop-portal-gnome xdg-desktop-portal-gtk xdg-desktop-portal-kde xdg-desktop-portal-wlr"

missing=""
for pkg in $REQUIRED_PACKAGES; do
  if ! rpm -q "$pkg" >/dev/null 2>&1; then
    missing="$missing $pkg"
  fi
done

portal_backend_found=""
for pkg in $PORTAL_BACKENDS; do
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
  missing_trimmed=$(echo "$missing" | xargs)
  echo "rchat preinstall check failed: missing runtime dependencies." >&2
  echo "Missing packages: $missing_trimmed" >&2
  if [ -n "$portal_note" ]; then
    echo "$portal_note" >&2
  fi
  echo "Install them, then retry:" >&2
  if command -v dnf >/dev/null 2>&1; then
    echo "  sudo dnf install -y $missing_trimmed" >&2
  elif command -v yum >/dev/null 2>&1; then
    echo "  sudo yum install -y $missing_trimmed" >&2
  else
    echo "  Use your package manager to install the packages above." >&2
  fi
  exit 1
fi

exit 0
