import assert from 'node:assert/strict';
import { validateMacLibraries, validateLinuxVersions } from './runtime-audit.mjs';

assert.throws(() => validateMacLibraries('/opt/homebrew/opt/libvpx/lib/libvpx.12.dylib'));
assert.throws(() => validateMacLibraries('/Users/builder/lib/libopus.dylib'));
assert.throws(() => validateMacLibraries('@rpath/unknown.dylib'));
assert.doesNotThrow(() => validateMacLibraries('/usr/lib/libSystem.B.dylib\n/System/Library/Frameworks/AppKit.framework/AppKit\n@rpath/libswift_Concurrency.dylib'));
assert.throws(() => validateLinuxVersions('GLIBC_2.43', '2.39'));
assert.doesNotThrow(() => validateLinuxVersions('GLIBC_2.17 GLIBC_2.39', '2.39'));
console.log('Runtime audit boundary tests passed');
