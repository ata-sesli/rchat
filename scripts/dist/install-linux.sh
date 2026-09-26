#!/usr/bin/env sh
set -eu

if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
  echo "Usage: scripts/dist/install-linux.sh <existing .deb or .rpm package>" >&2
  exit 1
fi

# An absolute path prevents apt treating a bare filename as a repository name.
package=$(CDPATH='' cd -- "$(dirname -- "$1")" && pwd)/$(basename -- "$1")
case "$package" in
  *.deb) set -- apt-get install -y "$package" ;;
  *.rpm)
    if command -v dnf >/dev/null 2>&1; then
      set -- dnf install -y "$package"
    else
      set -- yum localinstall -y "$package"
    fi
    ;;
  *) echo "Only .deb and .rpm packages are supported" >&2; exit 1 ;;
esac
command -v "$1" >/dev/null 2>&1 || { echo "Required package manager not found: $1" >&2; exit 1; }
if [ "$(id -u)" -eq 0 ]; then
  exec "$@"
else
  exec sudo "$@"
fi
