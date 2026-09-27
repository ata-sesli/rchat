import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const cwd = fileURLToPath(new URL('../', import.meta.url));
function check(name) {
  const result = spawnSync('badbox', ['check', `.badbox/fixtures/${name}.rs`], {
    cwd, encoding: 'utf8',
  });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, result.stderr || result.stdout);
  assert.match(result.stdout, /0 diagnostics/);
  return result.stdout;
}

const bad = check('bad');
for (const [rule, count] of [
  ['unbounded-queue', 2],
  ['std-unbounded-channel', 2],
  ['blocking-in-awaiting-callable', 1],
  ['unfinished-code', 2],
]) {
  assert.equal(bad.split(`rchat/${rule}:`).length - 1, count, bad);
}
assert.match(bad, /7 findings/);
assert.match(check('good'), /0 findings/);
console.log('Badbox positive and negative fixtures passed.');
