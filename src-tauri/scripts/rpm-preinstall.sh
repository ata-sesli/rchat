#!/usr/bin/env sh
set -eu

REQUIRED_PACKAGES="webkit2gtk4.1 gtk3 libayatana-appindicator-gtk3 librsvg2 alsa-lib openssl-libs opus libvpx avahi-compat-libdns_sd"

missing=""
for pkg in $REQUIRED_PACKAGES; do
  if ! rpm -q "$pkg" >/dev/null 2>&1; then
    missing="$missing $pkg"
  fi
done

if [ -n "$missing" ]; then
  missing_trimmed=$(echo "$missing" | xargs)
  echo "rchat preinstall check failed: missing runtime dependencies." >&2
  echo "Missing packages: $missing_trimmed" >&2
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
