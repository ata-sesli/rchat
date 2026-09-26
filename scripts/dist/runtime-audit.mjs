import { execFileSync } from 'node:child_process';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

export function run(command, args) {
  return execFileSync(command, args, { encoding: 'utf8' });
}

export function validateMacLibraries(libraries) {
  for (const library of libraries.trim().split('\n').filter(Boolean)) {
    if (!library.startsWith('/usr/lib/') && !library.startsWith('/System/Library/')
        && library !== '@rpath/libswift_Concurrency.dylib') {
      throw new Error(`Non-system macOS library must be statically linked or bundled: ${library}`);
    }
  }
}

export function validateLinuxVersions(output, maximum) {
  const limit = maximum.split('.').map(Number);
  for (const match of output.matchAll(/GLIBC_(\d+)\.(\d+)/g)) {
    if (+match[1] > limit[0] || (+match[1] === limit[0] && +match[2] > limit[1])) {
      throw new Error(`Binary requires ${match[0]}, newer than baseline GLIBC_${maximum}`);
    }
  }
}

export function auditMac(binary) {
  const libraries = run('otool', ['-L', binary]).split('\n').slice(1)
    .map(line => line.trim().split(' (')[0]).filter(Boolean).join('\n');
  validateMacLibraries(libraries);
  const commands = run('otool', ['-l', binary]);
  const paths = [...commands.matchAll(/cmd LC_RPATH\s+cmdsize \d+\s+path (.*?) \(offset/g)].map(m => m[1]);
  if (paths.some(path => path !== '/usr/lib/swift')) throw new Error(`Unexpected runtime search paths: ${paths}`);
  if (libraries.includes('@rpath/') && !paths.includes('/usr/lib/swift')) throw new Error('Missing system Swift runtime search path');
  return libraries;
}

export function auditLinux(binary, maximum) {
  const header = run('readelf', ['-h', binary]);
  const machine = { x64: 'Advanced Micro Devices X86-64', arm64: 'AArch64' }[process.arch];
  if (!machine || !header.includes(machine)) throw new Error('Package architecture must match the native build host');
  validateLinuxVersions(run('readelf', ['--version-info', binary]), maximum);
  const dynamic = run('readelf', ['-d', binary]);
  if (/\((?:RPATH|RUNPATH)\)/.test(dynamic)) throw new Error('Release binary must not contain a runtime library search path');
  return [...dynamic.matchAll(/\(NEEDED\).*?\[(.*?)\]/g)].map(m => m[1]);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [platform, binary, maximum] = process.argv.slice(2);
  if (!binary || !['macos', 'linux'].includes(platform) || (platform === 'linux' && !maximum)) {
    throw new Error('Usage: node scripts/dist/runtime-audit.mjs macos <binary> | linux <binary> <max-glibc>');
  }
  console.log(platform === 'macos' ? auditMac(resolve(binary)) : auditLinux(resolve(binary), maximum).join('\n'));
}
