#!/usr/bin/env sh
set -eu

REQUIRED_PACKAGES="libwebkit2gtk-4.1-0 libgtk-3-0 libayatana-appindicator3-1 librsvg2-2 libasound2 libssl3 libopus0 libavahi-client3 libavahi-compat-libdnssd1 pipewire xdg-desktop-portal"
LIBVPX_PACKAGES="libvpx9 libvpx8 libvpx7"

missing=""
for pkg in $REQUIRED_PACKAGES; do
  if ! dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null | grep -q "ok installed"; then
    missing="$missing $pkg"
  fi
done

libvpx_found=""
for pkg in $LIBVPX_PACKAGES; do
  if dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null | grep -q "ok installed"; then
    libvpx_found="yes"
  fi
done

if [ -z "$libvpx_found" ]; then
  missing="$missing libvpx9"
fi

if [ -n "$missing" ]; then
  missing_trimmed=$(echo "$missing" | xargs)
  echo "rchat preinstall check failed: missing runtime dependencies." >&2
  echo "Missing packages: $missing_trimmed" >&2
  echo "For libvpx, your distro may provide libvpx9, libvpx8, or libvpx7." >&2
  echo "Install them, then retry:" >&2
  echo "  sudo apt-get update && sudo apt-get install -y $missing_trimmed" >&2
  exit 1
fi

exit 0
