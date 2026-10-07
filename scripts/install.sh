#!/bin/sh
set -eu
task_source=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
test -f "$task_source/claudeHistory"
test -f "$task_source/SHA256SUMS"
task_hash=$(shasum -a 256 "$task_source/claudeHistory" | awk '{print $1}')
task_expected=$(awk 'NF == 2 && $2 == "claudeHistory" && length($1) == 64 && $1 !~ /[^0-9a-f]/ {print $1}' "$task_source/SHA256SUMS")
test "$(wc -l < "$task_source/SHA256SUMS" | tr -d ' ')" = 1
test "$task_hash" = "$task_expected"
test "$("$task_source/claudeHistory" --version)" = 'claudeHistory 0.1.0'
test "$#" -le 1
task_bin=${1:-"$HOME/.local/bin"}
case "$task_bin" in /*) ;; *) printf 'Install path must be absolute\n' >&2; exit 1 ;; esac
mkdir -p "$task_bin"
task_staging=$(mktemp "$task_bin/.claudeHistory.XXXXXX")
trap 'rm -f "$task_staging"' EXIT HUP INT TERM
cp "$task_source/claudeHistory" "$task_staging"
chmod 755 "$task_staging"
test "$("$task_staging" --version)" = 'claudeHistory 0.1.0'
mv -f "$task_staging" "$task_bin/claudeHistory"
printf 'Installed: %s/claudeHistory\n' "$task_bin"
printf 'PATHにない場合は、シェル設定に ~/.local/bin を追加してください。\n'
