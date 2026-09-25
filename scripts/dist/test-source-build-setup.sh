#!/usr/bin/env sh
set -eu

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)

debian_packages=$("$SCRIPT_DIR/install-debian-build-deps.sh" --print-packages)
for package in libwebkit2gtk-4.1-dev libayatana-appindicator3-dev libasound2-dev libopus-dev libvpx-dev libpipewire-0.3-dev libavahi-compat-libdnssd-dev clang libclang-dev pkg-config; do
  case " $debian_packages " in
    *" $package "*) ;;
    *) echo "Missing Debian build dependency: $package" >&2; exit 1 ;;
  esac
done
"$SCRIPT_DIR/install-debian-build-deps.sh" --dry-run | grep -q 'apt-get install -y'
core_packages=$("$SCRIPT_DIR/install-debian-build-deps.sh" --print-core-tui-packages)
for package in libpipewire-0.3-dev libvpx-dev; do
  case " $core_packages " in
    *" $package "*) ;;
    *) echo "Core/TUI package list is missing $package" >&2; exit 1 ;;
  esac
done
case " $core_packages " in
  *" libwebkit2gtk-4.1-dev "*) echo "Core/TUI package list should not install WebKitGTK" >&2; exit 1 ;;
esac

macos_packages=$("$SCRIPT_DIR/install-macos-build-deps.sh" --print-packages)
for package in pkgconf opus libvpx; do
  case " $macos_packages " in
    *" $package "*) ;;
    *) echo "Missing macOS build dependency: $package" >&2; exit 1 ;;
  esac
done
"$SCRIPT_DIR/install-macos-build-deps.sh" --dry-run | grep -q 'brew install'

"$SCRIPT_DIR/doctor.sh" --help | grep -q 'check-core-tui'

bootstrap_dry_run=$("$SCRIPT_DIR/bootstrap-source.sh" --dry-run)
echo "$bootstrap_dry_run" | grep -q 'install-macos-build-deps.sh\|install-debian-build-deps.sh\|install-fedora-build-deps.sh'
echo "$bootstrap_dry_run" | grep -q 'doctor.sh --check-gui'
echo "$bootstrap_dry_run" | grep -q 'bun install --frozen-lockfile'
echo "$bootstrap_dry_run" | grep -q 'cargo check --locked --manifest-path src-tauri/Cargo.toml -p rchat-core -p rchat-tui'

workflow="$SCRIPT_DIR/../../.github/workflows/ci.yml"
grep -q '^channel = "1.98.0"$' "$SCRIPT_DIR/../../rust-toolchain.toml"
grep -q '^  push:' "$workflow"
grep -q '^  pull_request:' "$workflow"
grep -q 'install-debian-build-deps.sh' "$workflow"
grep -q 'install-macos-build-deps.sh' "$workflow"
grep -q 'cargo nextest run' "$workflow"
