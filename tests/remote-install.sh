#!/bin/sh
set -eu
task_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
task_test=$(mktemp -d)
trap 'rm -rf "$task_test"' EXIT HUP INT TERM
mkdir -p "$task_test/tools" "$task_test/fixture" "$task_test/home"
cp "$task_root/target/package/claudeHistory-macos-arm64.tar.gz" "$task_test/fixture/claudeHistory-macos-arm64.tar.gz"
(cd "$task_test/fixture" && shasum -a 256 claudeHistory-macos-arm64.tar.gz > SHA256SUMS)
cat > "$task_test/tools/curl" <<'MOCK'
#!/bin/sh
set -eu
if [ "${CLAUDE_HISTORY_TEST_DOWNLOAD_FAIL:-0}" = 1 ]; then exit 22; fi
out=
while [ "$#" -gt 0 ]; do
 case "$1" in -o) out=$2; shift 2 ;; *) url=$1; shift ;; esac
done
cp "$CLAUDE_HISTORY_TEST_ASSETS/${url##*/}" "$out"
MOCK
chmod +x "$task_test/tools/curl"
export CLAUDE_HISTORY_TEST_ASSETS="$task_test/fixture"
export CLAUDE_HISTORY_INSTALL_NO_PATH=1
export BIN_DIR="$task_test/home/bin with ' quote \$literal"
export HOME="$task_test/home" SHELL=/bin/zsh
PATH="$task_test/tools:$PATH"; export PATH
sh "$task_root/install.sh"
test "$("$BIN_DIR/claudeHistory" --version)" = 'claudeHistory 0.5.1'
task_good=$(shasum -a 256 "$BIN_DIR/claudeHistory" | awk '{print $1}')
printf '%064d  claudeHistory-macos-arm64.tar.gz\n' 0 > "$task_test/fixture/SHA256SUMS"
if sh "$task_root/install.sh"; then exit 1; fi
test "$task_good" = "$(shasum -a 256 "$BIN_DIR/claudeHistory" | awk '{print $1}')"
(cd "$task_test/fixture" && shasum -a 256 claudeHistory-macos-arm64.tar.gz > SHA256SUMS)
cat "$task_test/fixture/SHA256SUMS" > "$task_test/duplicate"
cat "$task_test/duplicate" >> "$task_test/fixture/SHA256SUMS"
if sh "$task_root/install.sh"; then exit 1; fi
test "$task_good" = "$(shasum -a 256 "$BIN_DIR/claudeHistory" | awk '{print $1}')"
(cd "$task_test/fixture" && shasum -a 256 claudeHistory-macos-arm64.tar.gz > SHA256SUMS)
export CLAUDE_HISTORY_TEST_DOWNLOAD_FAIL=1
if sh "$task_root/install.sh"; then exit 1; fi
unset CLAUDE_HISTORY_TEST_DOWNLOAD_FAIL
test "$task_good" = "$(shasum -a 256 "$BIN_DIR/claudeHistory" | awk '{print $1}')"
mkdir "$task_test/inner"
tar -xzf "$task_test/fixture/claudeHistory-macos-arm64.tar.gz" -C "$task_test/inner"
printf '%064d  claudeHistory\n' 0 > "$task_test/inner/claudeHistory-macos-arm64/SHA256SUMS"
tar -czf "$task_test/fixture/claudeHistory-macos-arm64.tar.gz" -C "$task_test/inner" claudeHistory-macos-arm64
(cd "$task_test/fixture" && shasum -a 256 claudeHistory-macos-arm64.tar.gz > SHA256SUMS)
if sh "$task_root/install.sh"; then exit 1; fi
test "$task_good" = "$(shasum -a 256 "$BIN_DIR/claudeHistory" | awk '{print $1}')"
cp "$task_root/target/package/claudeHistory-macos-arm64.tar.gz" "$task_test/fixture/claudeHistory-macos-arm64.tar.gz"
(cd "$task_test/fixture" && shasum -a 256 claudeHistory-macos-arm64.tar.gz > SHA256SUMS)
unset CLAUDE_HISTORY_INSTALL_NO_PATH
sh "$task_root/install.sh"
sh "$task_root/install.sh"
test "$(wc -l < "$HOME/.zshrc" | tr -d ' ')" = 2
task_found=$(PATH=/usr/bin:/bin /bin/zsh -f -c '. "$HOME/.zshrc"; command -v claudeHistory')
test "$task_found" = "$BIN_DIR/claudeHistory"
printf 'Remote installer checksum/preservation/PATH quoting/idempotence passed\n'
