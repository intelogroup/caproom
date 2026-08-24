#!/usr/bin/env node
// caproom postinstall — npm unpacks prebuilt Rust binary like esbuild, fallback to bash.
const { existsSync, copyFileSync, chmodSync, mkdirSync } = require('fs');
const { join, dirname } = require('path');
const { platform, arch } = process;

const plat = platform === 'win32' ? 'win32' : platform; // darwin, linux, win32
const a = arch === 'arm64' ? 'arm64' : arch === 'x64' ? 'x64' : arch;
const asset = `caproom-${plat}-${a}${plat === 'win32' ? '.exe' : ''}`;
const root = join(__dirname, '..');
const prebuilt = join(root, 'prebuilt', asset);
const dest = join(root, 'bin', `caproom-rs${plat === 'win32' ? '.exe' : ''}`);
const localBuild = join(root, 'target', 'release', `caproom${plat === 'win32' ? '.exe' : ''}`);

function tryCopy(src, dst) {
  try {
    if (!existsSync(src)) return false;
    mkdirSync(dirname(dst), { recursive: true });
    copyFileSync(src, dst);
    try { chmodSync(dst, 0o755); } catch {}
    console.log(`caproom: installed Rust binary ${asset} -> bin/caproom-rs`);
    return true;
  } catch (e) {
    console.warn(`caproom: copy failed ${src} -> ${dst}: ${e.message}`);
    return false;
  }
}

let installed = false;
if (existsSync(prebuilt)) {
  installed = tryCopy(prebuilt, dest);
} else if (existsSync(localBuild)) {
  installed = tryCopy(localBuild, dest);
}

if (!installed) {
  // fallback: bash watchdog remains (bin/caproom)
  console.log('caproom: Rust binary not prebuilt for this platform — using bash watchdog (bin/caproom). Run `cargo build --release` locally to enable Rust path.');
}

// never block install, hint for setup
try {
  process.stdout.write('caproom: optional next step — run "caproom setup" to bind headroom warnings to your shells (never modifies rc files on install)\n');
} catch {}
