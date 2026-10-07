#!/bin/sh
set -eu
task_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$task_root"
cargo build --release --locked
task_arch=$(uname -m)
case "$task_arch" in arm64|x86_64) ;; *) echo 'Unsupported architecture' >&2; exit 1 ;; esac
task_dest="$task_root/target/package/claudeHistory-macos-$task_arch"
mkdir -p "$task_dest"
cp target/release/claudeHistory "$task_dest/claudeHistory"
cp scripts/install.sh README.md LICENSE "$task_dest/"
(cd "$task_dest" && shasum -a 256 claudeHistory > SHA256SUMS)
tar -czf "$task_dest.tar.gz" -C "$(dirname "$task_dest")" "$(basename "$task_dest")"
printf '%s\n' "$task_dest.tar.gz"
