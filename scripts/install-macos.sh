#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
source_bundle="$workspace_dir/target/macos/Slime.app"
input_methods_dir="$HOME/Library/Input Methods"
destination="$input_methods_dir/Slime.app"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS input method can only be installed on macOS" >&2
  exit 1
fi

if [[ "$destination" != "$HOME/Library/Input Methods/Slime.app" ]]; then
  echo "refusing to install to unexpected path: $destination" >&2
  exit 1
fi

bundle_manifest() {
  local bundle="$1"
  (
    cd "$bundle"
    find . -type f -print | LC_ALL=C sort | while IFS= read -r relative_path; do
      shasum -a 256 "$relative_path"
    done
  )
}

pkill -x Slime 2>/dev/null || true
mkdir -p "$input_methods_dir"
if [[ -e "$destination" ]]; then
  if [[ -L "$destination" || ! -d "$destination" ]]; then
    echo "refusing to replace a non-bundle path: $destination" >&2
    exit 1
  fi
  installed_identifier="$(
    plutil -extract CFBundleIdentifier raw -o - \
      "$destination/Contents/Info.plist" 2>/dev/null || true
  )"
  if [[ "$installed_identifier" != "com.unvalley.inputmethod.Slime" ]]; then
    echo "refusing to replace a bundle with identifier: $installed_identifier" >&2
    exit 1
  fi
  rm -rf "$destination"
fi
ditto "$source_bundle" "$destination"
if find "$destination" -type l -print -quit | grep -q .; then
  echo "installed input method unexpectedly contains a symbolic link" >&2
  exit 1
fi
codesign --verify --deep --strict "$destination"
expected_manifest="$(bundle_manifest "$source_bundle")"
installed_manifest="$(bundle_manifest "$destination")"
if [[ "$installed_manifest" != "$expected_manifest" ]]; then
  echo "installed input method differs from the built bundle" >&2
  diff -u <(printf '%s\n' "$expected_manifest") \
    <(printf '%s\n' "$installed_manifest") >&2 || true
  exit 1
fi

"$workspace_dir/target/macos/register-input-source" \
  "$destination" \
  com.unvalley.inputmethod.Slime \
  --register

open "$destination" || true
sleep 1
pkill -x Slime 2>/dev/null || true

"$workspace_dir/target/macos/register-input-source" \
  "$destination" \
  com.unvalley.inputmethod.Slime \
  --select >/dev/null 2>&1 || true

if "$workspace_dir/target/macos/register-input-source" \
  "$destination" \
  com.unvalley.inputmethod.Slime \
  --select-id com.unvalley.inputmethod.Slime.Japanese; then
  current_source="$("$workspace_dir/target/macos/register-input-source" --current)"
  if ! grep -q '^  source id: com[.]unvalley[.]inputmethod[.]Slime[.]Japanese$' \
    <<<"$current_source"; then
    echo "input source selection did not become current" >&2
    echo "$current_source" >&2
    exit 1
  fi
  echo "Installed and selected $destination"
else
  echo "Installed $destination"
  echo "First install: add Hiragana (Slime) from Keyboard > Input Sources."
  if [[ "${SLIME_OPEN_INPUT_SETTINGS:-0}" == "1" ]]; then
    open 'x-apple.systempreferences:com.apple.Keyboard-Settings.extension'
  fi
fi
