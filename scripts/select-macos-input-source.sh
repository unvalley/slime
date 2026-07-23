#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
user_bundle="$HOME/Library/Input Methods/Slime.app"
system_bundle="/Library/Input Methods/Slime.app"

if [[ -d "$user_bundle" ]]; then
  destination="$user_bundle"
elif [[ -d "$system_bundle" ]]; then
  destination="$system_bundle"
else
  echo "Slime is not installed. Run: just install-macos" >&2
  exit 1
fi

if [[ ! -x "$workspace_dir/target/macos/register-input-source" ]]; then
  "$workspace_dir/scripts/build-macos.sh"
fi

"$workspace_dir/target/macos/register-input-source" \
  "$destination" \
  com.unvalley.inputmethod.Slime \
  --select >/dev/null 2>&1 || true

if "$workspace_dir/target/macos/register-input-source" \
  "$destination" \
  com.unvalley.inputmethod.Slime \
  --select-id com.unvalley.inputmethod.Slime.Japanese; then
  exit 0
fi

"$workspace_dir/target/macos/register-input-source" \
  "$destination" \
  com.unvalley.inputmethod.Slime \
  --diagnose >&2 || true
echo "Slime was found, but macOS refused to select it." >&2
codesign -dv --verbose=2 "$destination" 2>&1 \
  | grep -E '^(Identifier|Authority|TeamIdentifier|Signature)=' >&2 || true
exit 1
