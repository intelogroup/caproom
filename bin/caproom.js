#!/usr/bin/env node
// Platform dispatch: bash script on POSIX, PowerShell script on Windows.
const { spawnSync } = require('child_process');
const { join } = require('path');

const args = process.argv.slice(2);
const isWin = process.platform === 'win32';

const result = isWin
  ? spawnSync(
      'powershell.exe',
      ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', join(__dirname, 'caproom.ps1'), ...args],
      { stdio: 'inherit' }
    )
  : spawnSync('bash', [join(__dirname, 'caproom'), ...args], { stdio: 'inherit' });

if (result.error) {
  console.error(`caproom: failed to launch backend — ${result.error.message}`);
  process.exit(1);
}
process.exit(result.status === null ? 1 : result.status);
