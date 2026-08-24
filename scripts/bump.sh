#!/usr/bin/env bash
# bump version in Cargo.toml + package.json (keeps 0.7.6 sync)
set -euo pipefail
if [[ $# -ne 1 ]]; then echo "usage: $0 <version>  e.g. $0 0.8.0" >&2; exit 1; fi
VER="$1"
if ! [[ "$VER" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then echo "bad version $VER" >&2; exit 1; fi
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Cargo workspace version
sed -i '' -E "s/^version = \".*\"/version = \"$VER\"/" "$ROOT/Cargo.toml" 2>/dev/null || sed -i -E "s/^version = \".*\"/version = \"$VER\"/" "$ROOT/Cargo.toml"
# package.json
node -e "const fs=require('fs'); const p=JSON.parse(fs.readFileSync('$ROOT/package.json','utf8')); p.version='$VER'; fs.writeFileSync('$ROOT/package.json', JSON.stringify(p,null,2)+'\n');"
echo "bumped to $VER — Cargo.toml + package.json"
echo "next: git commit -am \"chore: bump $VER\" && git tag v$VER && git push --tags"
