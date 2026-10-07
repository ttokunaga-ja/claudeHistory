#!/bin/sh
# curl -fsSL https://raw.githubusercontent.com/ttokunaga-ja/claudeHistory/main/install.sh | sh
# BIN_DIR overrides the destination; CLAUDE_HISTORY_INSTALL_NO_PATH=1 skips profile edits.
set -eu
if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
  echo 'This installer requires macOS Apple Silicon. Windows: use install.ps1.' >&2
  exit 1
fi
task_bin=${BIN_DIR:-"$HOME/.local/bin"}
case "$task_bin" in /*) ;; *) echo 'BIN_DIR must be absolute' >&2; exit 1 ;; esac
case "$task_bin" in *:*|*'
'*) echo 'BIN_DIR must not contain colons or newlines' >&2; exit 1 ;; esac
if [ -L "$task_bin/claudeHistory" ] || { [ -e "$task_bin/claudeHistory" ] && [ ! -f "$task_bin/claudeHistory" ]; }; then
  echo 'Destination must be a regular file; existing install preserved' >&2
  exit 1
fi
task_base=https://github.com/ttokunaga-ja/claudeHistory/releases/latest/download
task_asset=claudeHistory-macos-arm64.tar.gz
task_temp=$(mktemp -d)
trap 'rm -rf "$task_temp"' EXIT
trap 'exit 1' HUP INT TERM
curl --proto '=https' --tlsv1.2 -fsSL -o "$task_temp/$task_asset" "$task_base/$task_asset"
curl --proto '=https' --tlsv1.2 -fsSL -o "$task_temp/SHA256SUMS" "$task_base/SHA256SUMS"
awk '$2 == "claudeHistory-macos-arm64.tar.gz" || $2 == "*claudeHistory-macos-arm64.tar.gz" {print}' "$task_temp/SHA256SUMS" > "$task_temp/match"
if [ "$(wc -l < "$task_temp/match" | tr -d ' ')" != 1 ] ||
   ! LC_ALL=C grep -Eq '^[0-9a-fA-F]{64} [ *]claudeHistory-macos-arm64\.tar\.gz$' "$task_temp/match" ||
   ! (cd "$task_temp" && shasum -a 256 -c match >/dev/null); then
  echo 'Invalid/duplicate checksum entry or SHA-256 mismatch; existing install preserved' >&2
  exit 1
fi
tar -xzf "$task_temp/$task_asset" -C "$task_temp"
sh "$task_temp/claudeHistory-macos-arm64/install.sh" "$task_bin"
if [ "${CLAUDE_HISTORY_INSTALL_NO_PATH:-0}" != 1 ]; then
  case "${SHELL:-}" in
    */zsh) task_profile=${ZDOTDIR:-"$HOME"}/.zshrc ;;
    */bash)
      if [ -f "$HOME/.bash_profile" ]; then task_profile=$HOME/.bash_profile
      elif [ -f "$HOME/.bash_login" ]; then task_profile=$HOME/.bash_login
      elif [ -f "$HOME/.profile" ]; then task_profile=$HOME/.profile
      else task_profile=$HOME/.bash_profile; fi ;;
    *) task_profile=$HOME/.profile ;;
  esac
  task_quoted=$(printf '%s' "$task_bin" | sed "s/'/'\\\\''/g")
  task_entry="case :\$PATH: in *:'$task_quoted':*) ;; *) export PATH='$task_quoted':\$PATH ;; esac # claudeHistory installer"
  if [ ! -f "$task_profile" ] || ! grep -Fqx "$task_entry" "$task_profile"; then
    mkdir -p "$(dirname "$task_profile")"
    printf '\n%s\n' "$task_entry" >> "$task_profile"
    printf 'PATH updated: %s\n' "$task_profile"
  fi
  printf 'Open a new terminal to use claudeHistory.\n'
fi
