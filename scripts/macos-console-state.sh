#!/usr/bin/env bash

slime_console_lock_state() {
  local compact_state
  compact_state="$(printf '%s' "$1" | LC_ALL=C tr -d '[:space:]')"

  if [[ "$compact_state" == *'"IOConsoleLocked"=Yes'* \
    || "$compact_state" == *'"CGSSessionScreenIsLocked"=Yes'* \
    || "$compact_state" == *'"IOConsoleLocked"=<true>'* \
    || "$compact_state" == *'"CGSSessionScreenIsLocked"=<true>'* ]]; then
    echo "locked"
    return
  fi
  if [[ "$compact_state" == *'"IOConsoleLocked"=No'* \
    || "$compact_state" == *'"CGSSessionScreenIsLocked"=No'* \
    || "$compact_state" == *'"IOConsoleLocked"=<false>'* \
    || "$compact_state" == *'"CGSSessionScreenIsLocked"=<false>'* ]]; then
    echo "unlocked"
    return
  fi
  echo "unknown"
}

slime_gate_started_at_is_active() {
  local started_at="$1"
  local now="$2"
  local maximum_age="${3:-7200}"

  [[ "$started_at" =~ ^[0-9]{10}$ \
    && "$now" =~ ^[0-9]{10}$ \
    && "$maximum_age" =~ ^[1-9][0-9]{0,8}$ ]] || return 1
  (( started_at <= now && now - started_at <= maximum_age ))
}

slime_process_identity_sha256() {
  local process_id="$1"
  local process_record

  [[ "$process_id" =~ ^[1-9][0-9]{0,9}$ ]] || return 1
  kill -0 "$process_id" 2>/dev/null || return 1
  process_record="$(
    LC_ALL=C /bin/ps -p "$process_id" -o lstart= -o comm= 2>/dev/null
  )" || return 1
  [[ -n "${process_record//[[:space:]]/}" ]] || return 1
  /usr/bin/printf '%s\n' "$process_record" \
    | /usr/bin/shasum -a 256 \
    | /usr/bin/awk '{print $1}'
}

slime_process_identity_matches() {
  local process_id="$1"
  local expected_sha256="$2"
  local actual_sha256

  [[ "$expected_sha256" =~ ^[0-9a-f]{64}$ ]] || return 1
  actual_sha256="$(slime_process_identity_sha256 "$process_id")" || return 1
  [[ "$actual_sha256" == "$expected_sha256" ]]
}
