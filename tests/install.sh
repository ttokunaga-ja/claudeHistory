#!/bin/sh
set -eu
task_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
task_test=$(mktemp -d)
trap 'rm -rf "$task_test"' EXIT HUP INT TERM
mkdir "$task_test/package" "$task_test/bin"
cp "$task_root/target/release/claudeHistory" "$task_test/package/claudeHistory"
cp "$task_root/scripts/install.sh" "$task_test/package/install.sh"
(cd "$task_test/package" && shasum -a 256 claudeHistory > SHA256SUMS)
sh "$task_test/package/install.sh" "$task_test/bin"
test "$("$task_test/bin/claudeHistory" --version)" = 'claudeHistory 0.2.0'
task_good=$(shasum -a 256 "$task_test/bin/claudeHistory" | awk '{print $1}')
printf '%064d  claudeHistory\n' 0 > "$task_test/package/SHA256SUMS"
if sh "$task_test/package/install.sh" "$task_test/bin"; then exit 1; fi
test "$task_good" = "$(shasum -a 256 "$task_test/bin/claudeHistory" | awk '{print $1}')"
(cd "$task_test/package" && shasum -a 256 claudeHistory > SHA256SUMS)
cat "$task_test/package/SHA256SUMS" >> "$task_test/package/duplicate"
cat "$task_test/package/duplicate" >> "$task_test/package/SHA256SUMS"
if sh "$task_test/package/install.sh" "$task_test/bin"; then exit 1; fi
test "$task_good" = "$(shasum -a 256 "$task_test/bin/claudeHistory" | awk '{print $1}')"
printf 'Installer valid checksum / corrupt checksum / duplicate manifest: passed\n'
