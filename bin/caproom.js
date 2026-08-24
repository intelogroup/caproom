#!/usr/bin/env node
// Platform dispatch: Rust binary if present (prebuilt/cargo), else bash/PowerShell fallback.
const { spawnSync, spawn } = require('child_process');
const { existsSync } = require('fs');
const { join } = require('path');

const args = process.argv.slice(2);
const isWin = process.platform === 'win32';
const rsBin = join(__dirname, `caproom-rs${isWin ? '.exe' : ''}`);

// Prefer Rust binary when available — <10ms startup, phys_footprint, typed top --json.
if (existsSync(rsBin)) {
  const r = spawnSync(rsBin, args, { stdio: 'inherit' });
  if (!r.error) process.exit(r.status === null ? 1 : r.status);
  // fallback to bash if Rust failed to spawn
  console.warn(`caproom: Rust binary failed (${r.error?.message}), falling back to bash`);
}

const result = isWin
  ? spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', join(__dirname, 'caproom.ps1'), ...args], { stdio: 'inherit' })
  : spawnSync('bash', [join(__dirname, 'caproom'), ...args], { stdio: 'inherit' });

if (result.error) {
  console.error(`caproom: failed to launch backend — ${result.error.message}`);
  process.exit(1);
}
process.exit(result.status === null ? 1 : result.status);
