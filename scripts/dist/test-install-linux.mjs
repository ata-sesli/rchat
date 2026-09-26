import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, chmodSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

const dir = mkdtempSync(join(tmpdir(), 'rchat-install-test-'));
try {
  const log = join(dir, 'calls');
  for (const name of ['sudo', 'apt-get', 'dnf', 'dpkg-query', 'rpm']) {
    const path = join(dir, name);
    writeFileSync(path, name === 'sudo'
      ? '#!/bin/sh\nexec "$@"\n'
      : `#!/bin/sh\nprintf '%s\\n' '${name}' "$@" >> "$LOG"\nexit ${['rpm', 'dpkg-query'].includes(name) ? 1 : 0}\n`);
    chmodSync(path, 0o755);
  }
  for (const ext of ['deb', 'rpm']) {
    const pkg = join(dir, `package with spaces.${ext}`);
    writeFileSync(pkg, 'fixture');
    writeFileSync(log, '');
    const result = spawnSync('sh', [resolve('scripts/dist/install-linux.sh'), pkg], {
      env: { ...process.env, PATH: `${dir}:${process.env.PATH}`, LOG: log }, encoding: 'utf8',
    });
    assert.equal(result.status, 0, result.stderr);
    const calls = readFileSync(log, 'utf8');
    assert.match(calls, ext === 'deb' ? /apt-get\ninstall\n-y\n/ : /dnf\ninstall\n-y\n/);
    assert.ok(calls.includes(pkg));
    assert.doesNotMatch(calls, /dpkg-query|^rpm$/m);
  }
  const bad = spawnSync('sh', ['scripts/dist/install-linux.sh', join(dir, 'missing.deb')]);
  assert.notEqual(bad.status, 0);
  console.log('Linux installer delegation tests passed');
} finally {
  rmSync(dir, { recursive: true, force: true });
}
