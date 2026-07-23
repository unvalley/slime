#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
source_bundle="$workspace_dir/target/macos/Slime.app"
user_bundle="$HOME/Library/Input Methods/Slime.app"
system_bundle="/Library/Input Methods/Slime.app"
user_backup="$workspace_dir/target/macos/Slime.user-install-backup.$$.app"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS input method can only be installed on macOS" >&2
  exit 1
fi

pkill -x Slime 2>/dev/null || true

osascript \
  "$workspace_dir/platforms/macos/InstallSystem.applescript" \
  "$source_bundle" \
  "$system_bundle"

if [[ -d "$user_bundle" ]]; then
  mv "$user_bundle" "$user_backup"
  echo "Moved the previous user install to $user_backup"
fi

"$workspace_dir/target/macos/register-input-source" \
  "$system_bundle" \
  com.unvalley.inputmethod.Slime \
  --register

"$workspace_dir/target/macos/register-input-source" \
  "$system_bundle" \
  com.unvalley.inputmethod.Slime \
  --select >/dev/null 2>&1 || true

if "$workspace_dir/target/macos/register-input-source" \
  "$system_bundle" \
  com.unvalley.inputmethod.Slime \
  --select-id com.unvalley.inputmethod.Slime.Japanese; then
  echo "Installed and selected $system_bundle"
else
  echo "Installed $system_bundle"
  echo "macOS refused to enable the input source."
  codesign -dv --verbose=2 "$system_bundle" 2>&1 \
    | grep -E '^(Identifier|Authority|TeamIdentifier|Signature)=' || true
fi
