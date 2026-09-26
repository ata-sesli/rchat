import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { auditLinux } from './runtime-audit.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
export function baselineFor(id, version) {
  if (id === 'ubuntu' && version === '24.04') return { format: 'deb', glibc: '2.39', tag: 'ubuntu24.04' };
  if (id === 'fedora' && version === '44') return { format: 'rpm', glibc: '2.43', tag: 'fedora44' };
  throw new Error(`Unsupported release build host: ${id} ${version}. Use Ubuntu 24.04 or Fedora 44.`);
}
export function packageName(kind) {
  if (kind === 'tui') return 'rchat-tui';
  if (kind === 'gui') return 'rchat';
  throw new Error('Package kind must be gui or tui');
}
function command(name, args, cwd) {
  console.log(`Running ${name} ${args.join(' ')}`);
  return execFileSync(name, args, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] }).trim();
}
function put(path, content) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
}
function normalizePermissions(directory) {
  chmodSync(directory, 0o755);
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) normalizePermissions(path);
    else chmodSync(path, directory.endsWith('/usr/bin') ? 0o755 : 0o644);
  }
}
function packageLinux(kind, binary, output) {
  const name = packageName(kind);
  const os = Object.fromEntries(readFileSync('/etc/os-release', 'utf8').split('\n')
    .filter(line => /^[A-Z_]+=/.test(line)).map(line => {
      const pos = line.indexOf('=');
      return [line.slice(0, pos), line.slice(pos + 1).replace(/^"|"$/g, '')];
    }));
  const baseline = baselineFor(os.ID, os.VERSION_ID);
  const needed = auditLinux(binary, baseline.glibc);
  const version = JSON.parse(readFileSync(join(root, 'src-tauri/tauri.conf.json'), 'utf8')).version;
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error('Packaging currently requires a numeric major.minor.patch version');
  mkdirSync(output, { recursive: true });
  const work = mkdtempSync(join(output, `${name}-work-`));
  // Retain staging and dependency evidence beside artifacts for inspection.
  const payload = join(work, 'payload');
  const executable = join(payload, 'usr/bin', name);
  mkdirSync(dirname(executable), { recursive: true });
  copyFileSync(binary, executable);
  chmodSync(executable, 0o755);
  put(join(payload, 'usr/share/doc', name, 'README'), kind === 'tui'
    ? 'RChat TUI. Install Kitty on the display/client machine for graphics. SSH servers do not need Kitty. Run rchat-tui --no-host to use the current terminal. User data is preserved on uninstall.\n'
    : 'RChat GUI. Screen sharing needs PipeWire, xdg-desktop-portal and a portal backend matching your desktop. Local discovery needs an active Avahi service. User data is preserved on uninstall.\n');
  copyFileSync(join(root, 'LICENSE'), join(payload, 'usr/share/doc', name, 'copyright'));
  if (kind === 'gui') {
    put(join(payload, 'usr/share/applications/rchat.desktop'), '[Desktop Entry]\nType=Application\nName=RChat\nExec=rchat %u\nIcon=rchat\nTerminal=false\nCategories=Network;Chat;\nMimeType=x-scheme-handler/rchat;\n');
    const icon = join(payload, 'usr/share/icons/hicolor/128x128/apps/rchat.png');
    mkdirSync(dirname(icon), { recursive: true });
    copyFileSync(join(root, 'src-tauri/icons/128x128.png'), icon);
  }
  normalizePermissions(payload);
  let artifact;
  let dependencies;
  if (baseline.format === 'deb') {
    const arch = command('dpkg', ['--print-architecture']);
    put(join(work, 'debian/control'), `Source: ${name}\nMaintainer: RChat contributors <noreply@github.com>\n\nPackage: ${name}\nArchitecture: any\nDescription: RChat ${kind}\n`);
    const result = command('dpkg-shlibdeps', ['-O', `-e${executable}`], work);
    dependencies = result.split('\n').find(line => line.startsWith('shlibs:Depends='))?.slice('shlibs:Depends='.length);
    if (!dependencies) throw new Error('dpkg-shlibdeps produced no dependency metadata');
    put(join(payload, 'DEBIAN/control'), `Package: ${name}\nVersion: ${version}-1\nArchitecture: ${arch}\nMaintainer: RChat contributors <noreply@github.com>\nDepends: ${dependencies}\nSuggests: pipewire, xdg-desktop-portal, avahi-daemon\nDescription: RChat ${kind}\n`);
    normalizePermissions(payload);
    artifact = join(output, `${name}_${version}-1_${baseline.tag}_${arch}.deb`);
    command('dpkg-deb', ['--build', '--root-owner-group', payload, artifact]);
    if (command('dpkg-deb', ['-f', artifact, 'Depends']) !== dependencies) throw new Error('DEB dependencies differ from binary-derived dependencies');
  } else {
    // rpmbuild shell macros cannot safely quote arbitrary staging paths.
    if (!/^[A-Za-z0-9_./-]+$/.test(work)) throw new Error('RPM output path must not contain spaces or shell metacharacters');
    const files = kind === 'gui'
      ? '/usr/bin/rchat\n/usr/share/applications/rchat.desktop\n/usr/share/icons/hicolor/128x128/apps/rchat.png\n/usr/share/doc/rchat'
      : '/usr/bin/rchat-tui\n/usr/share/doc/rchat-tui';
    const spec = join(work, `${name}.spec`);
    put(spec, `Name: ${name}\nVersion: ${version}\nRelease: 1.fc44\nSummary: RChat ${kind}\nLicense: AGPL-3.0-only\nAutoReqProv: yes\nSuggests: pipewire\nSuggests: xdg-desktop-portal\nSuggests: avahi\n%description\nRChat ${kind}.\n%prep\n%build\n%install\nmkdir -p %{buildroot}\ncp -a ${payload}/. %{buildroot}/\n%files\n${files}\n`);
    command('rpmbuild', ['-bb', '--define', `_topdir ${work}`, '--define', '_build_id_links none', '--define', 'debug_package %{nil}', spec]);
    const arch = command('rpm', ['--eval', '%{_arch}']);
    const rpmDir = join(work, 'RPMS', arch);
    const packages = readdirSync(rpmDir).filter(file => file.endsWith('.rpm'));
    if (packages.length !== 1) throw new Error('Expected exactly one RPM');
    artifact = join(output, packages[0]);
    copyFileSync(join(rpmDir, packages[0]), artifact);
    dependencies = command('rpm', ['-qp', '--requires', artifact]);
    for (const library of needed) {
      if (!dependencies.includes(library)) throw new Error(`RPM is missing automatic requirement ${library}`);
    }
  }
  put(`${artifact}.audit.json`, JSON.stringify({ baseline, needed, dependencies, binary, version }, null, 2) + '\n');
  console.log(`Created ${artifact}\nAudit: ${artifact}.audit.json`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [kind, binary, output] = process.argv.slice(2);
  if (!output || process.argv.length !== 5) throw new Error('Usage: node scripts/dist/package-linux.mjs <gui|tui> <binary> <output-directory>');
  packageLinux(kind, resolve(binary), resolve(output));
}
