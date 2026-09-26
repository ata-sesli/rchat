# Runtime packages

These scripts build local artifacts; they do not publish releases. Do not call
an artifact release-ready until the clean-machine checklist below passes.

## Initial build baselines

| Target | Build environment | Maximum GLIBC |
| --- | --- | --- |
| DEB | Ubuntu 24.04 | 2.39 |
| RPM | Fedora 44 | 2.43 |
| macOS | macOS 15, native architecture | not applicable |

These are intentionally narrow initial targets, not a promise of support for
all Debian/RHEL variants. Linux packaging rejects other hosts and binaries
requiring newer GLIBC. It also rejects embedded library search paths and a
binary architecture different from the host. Build each architecture natively.
Use versioned container images, not `latest`, for repeatable release builds.
Record the image digest and Git commit with any published artifacts.

## Linux

Install Rust and the existing native build prerequisites first. Packaging also
needs Node.js plus `dpkg-dev`/`binutils` on Ubuntu or `rpm-build`/`binutils` on
Fedora. GUI builds additionally need Bun and the GUI development libraries.

```sh
export CARGO_TARGET_DIR=/absolute/path/on/build-disk/rchat-target
# If kache is installed:
export RUSTC_WRAPPER=kache
scripts/dist/build-linux-package.sh tui /absolute/path/on/build-disk/packages
scripts/dist/build-linux-package.sh gui /absolute/path/on/build-disk/packages
```

The GUI path uses Tauri to compile the app without bundling, then packages its
binary, desktop/deep-link entry, icon and documentation. The TUI has a separate
package and does not inherit the GUI's GTK/WebKit dependencies. No Kitty
dependency is imposed on SSH servers. Install Kitty on the display/client
machine for the full graphical TUI experience.

To package an already built binary without compiling:

```sh
node scripts/dist/package-linux.mjs tui /absolute/path/rchat-tui /absolute/output
```

Use release binaries for distribution. Debug binaries may be used to test the
packaging machinery, but are not release artifacts. The RPM output path must
not contain spaces/metacharacters because rpmbuild embeds it in shell macros.

DEB dependencies come from `dpkg-shlibdeps`; RPM dependencies come from
rpmbuild's automatic ELF analysis. Each finished artifact is queried again and
gets an adjacent `.audit.json` recording its library requirements. In particular,
`libvpx.so.9` cannot be satisfied by a different libvpx SONAME. Staging trees are
retained under the output directory for inspection, not in the system temp area.

For release packaging use these scripts rather than raw `tauri build` Linux
bundles: the latter retain only a conservative Ubuntu-24.04-oriented GUI
dependency list and do not run this binary-derived metadata audit.

Install a package using:

```sh
scripts/dist/install-linux.sh /absolute/path/to/package.deb
# Or pass an RPM on Fedora.
```

The package manager installs hard library requirements. PipeWire services,
desktop portals/backends, and Avahi service setup are separate feature-level
requirements. They are suggested rather than installation-blocking checks.
Use the existing media diagnostics to investigate unavailable screen capture.
No installation/removal script deletes user data or changes system services.

## macOS

The libvpx wrapper requires static linking on macOS and fails if the static
archive is missing. Homebrew may supply build dependencies, but end users must
not need Homebrew. The libvpx redistribution notice ships with both packages.

```sh
export CARGO_TARGET_DIR=/Volumes/your-build-disk/rchat-target
export RUSTC_WRAPPER=kache
scripts/dist/build-macos-package.sh tui /Volumes/your-build-disk/packages
scripts/dist/build-macos-package.sh gui /Volumes/your-build-disk/packages
```

The TUI output is a tarball; the GUI output is a DMG. The scripts audit linked
libraries and runtime search paths before copying deliverables, and generate
SHA-256 sidecars. Only Apple system libraries/frameworks and the system Swift
concurrency runtime are allowed by the current audit. This intentionally rejects
new unhandled dynamic libraries. It is not a substitute for testing OS API
availability. Tauri signing/notarization credentials must be configured separately
for public GUI distribution; no credentials or publishing are managed here.

## Required validation before publication

- Build in the declared baseline, inspect dependency metadata, and retain audits.
- Install in a fresh environment without Rust/Bun/Clang/development packages.
- Run as an ordinary user; open/unlock GUI and TUI and verify basic messaging.
- Test real calls/screen sharing in a desktop session with permissions/portals.
- Upgrade from a previous package and uninstall; verify user data survives.
- On macOS test without Homebrew or DYLD_LIBRARY_PATH, on both advertised CPUs,
  including the minimum OS. Verify signatures/notarization where applicable.
- Windows has no new release path in this work; native GUI packaging remains
  unverified, and Kitty does not support a native Windows TUI.

Current evidence: macOS arm64 debug TUI passes the static-library audit. A Fedora
44 x86_64 debug TUI RPM was generated with automatic SONAME requirements and
installed/reinstalled/removed in a fresh Podman container without Rust/Bun or
GTK/WebKit. `rchat-tui --help` ran and a sentinel user-data file survived removal.
Ubuntu 24.04 DEB generation, dependency extraction, ordinary-user execution and
removal were tested with `/usr/bin/true` as an ELF fixture, not the RChat binary.
No real upgrade or GUI/media session validation is implied by these smoke checks.
