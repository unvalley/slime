#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=macos-console-state.sh
source "$script_dir/macos-console-state.sh"

assert_state() {
  local expected="$1"
  local fixture="$2"
  local actual
  actual="$(slime_console_lock_state "$fixture")"
  if [[ "$actual" != "$expected" ]]; then
    echo "console state mismatch: expected=$expected actual=$actual fixture=$fixture" >&2
    exit 1
  fi
}

assert_state locked '"IOConsoleLocked" = Yes'
assert_state unlocked '"IOConsoleLocked" = No'
assert_state locked '"CGSSessionScreenIsLocked"=Yes'
assert_state unlocked '"CGSSessionScreenIsLocked"=No'
assert_state locked $'"CGSSessionScreenIsLocked" = <true>\n"IOConsoleLocked" = No'
assert_state unlocked '"CGSSessionScreenIsLocked" = <false>'
assert_state unknown '"UnrelatedProperty"=Yes'

slime_gate_started_at_is_active 1786154400 1786161600
slime_gate_started_at_is_active 1786154400 1786161599
if slime_gate_started_at_is_active 1786154400 1786161601; then
  echo "physical gate accepted state older than two hours" >&2
  exit 1
fi
if slime_gate_started_at_is_active 1786161601 1786161600; then
  echo "physical gate accepted state prepared in the future" >&2
  exit 1
fi
if slime_gate_started_at_is_active invalid 1786161600; then
  echo "physical gate accepted malformed start time" >&2
  exit 1
fi

process_identity="$(slime_process_identity_sha256 "$$")"
if [[ ! "$process_identity" =~ ^[0-9a-f]{64}$ ]]; then
  echo "physical gate did not produce a bounded process identity" >&2
  exit 1
fi
slime_process_identity_matches "$$" "$process_identity"
if slime_process_identity_matches "$$" \
  aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa; then
  echo "physical gate accepted a different process identity" >&2
  exit 1
fi
if slime_process_identity_sha256 9999999999 >/dev/null 2>&1; then
  echo "physical gate accepted a process that is not running" >&2
  exit 1
fi

echo "macOS console state policy test passed"
