#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=macos-textedit-input-gate.sh
source "$workspace_dir/scripts/macos-textedit-input-gate.sh"

temporary_directory="$(mktemp -d)"
cleanup() {
  if [[ -n "$temporary_directory" && -d "$temporary_directory" ]]; then
    /usr/bin/trash "$temporary_directory"
  fi
}
trap cleanup EXIT

state_file="$temporary_directory/textedit-input-gate.state"
defaults_calls="$temporary_directory/defaults.calls"
installed_scope="user"
installed_bundle_sha256="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
package_sha256="development"
installed_version="0.1.0"
installed_build="1"
installed_source_revision="development"

defaults() {
  printf '%s\t' "$@" >>"$defaults_calls"
  printf '\n' >>"$defaults_calls"
}

pkill() {
  :
}

uuidgen() {
  printf '%s\n' "12345678-1234-4123-8123-123456789abc"
}

write_gate_state candidate-selection "$$"

grep -Fxq "target_process_id=$$" "$state_file"
process_identity="$(slime_process_identity_sha256 "$$")"
grep -Fxq "target_process_identity=$process_identity" "$state_file"
grep -Fq $'write\tcom.unvalley.inputmethod.Slime\tinputVerificationTargetProcess\t-int\t'"$$" "$defaults_calls"

/usr/bin/sed -i '' \
  's/^target_process_identity=.*/target_process_identity=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/' \
  "$state_file"
if (load_gate_state candidate-selection >/dev/null 2>&1); then
  echo "TextEdit gate accepted a replaced target process" >&2
  exit 1
fi

clear_pending_token
grep -Fq $'delete\tcom.unvalley.inputmethod.Slime\tinputVerificationTargetProcess\t' "$defaults_calls"

echo "macOS TextEdit input gate process-identity policy passed"
