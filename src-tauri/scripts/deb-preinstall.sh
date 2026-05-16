#!/usr/bin/env sh
set -eu

REQUIRED_PACKAGES="libwebkit2gtk-4.1-0 libgtk-3-0 libayatana-appindicator3-1 librsvg2-2 libasound2 libssl3 libavahi-client3 libavahi-compat-libdnssd1"

missing=""
for pkg in $REQUIRED_PACKAGES; do
  if ! dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null | grep -q "ok installed"; then
    missing="$missing $pkg"
  fi
done

if [ -n "$missing" ]; then
  missing_trimmed=$(echo "$missing" | xargs)
  echo "rchat preinstall check failed: missing runtime dependencies." >&2
  echo "Missing packages: $missing_trimmed" >&2
  echo "Install them, then retry:" >&2
  echo "  sudo apt-get update && sudo apt-get install -y $missing_trimmed" >&2
  exit 1
fi

exit 0
