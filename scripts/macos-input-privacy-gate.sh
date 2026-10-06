#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=macos-console-state.sh
source "$workspace_dir/scripts/macos-console-state.sh"
# shellcheck source=macos-package-binding.sh
source "$workspace_dir/scripts/macos-package-binding.sh"
user_bundle="$HOME/Library/Input Methods/Slime.app"
system_bundle="/Library/Input Methods/Slime.app"
register_tool="$workspace_dir/target/macos/register-input-source"
data_dir="$HOME/Library/Application Support/Slime"
history_file="$data_dir/history.tsv"
context_history_file="$data_dir/context_history.tsv"
state_file="$workspace_dir/target/macos/input-privacy-gate.state"
probe_source="$workspace_dir/platforms/macos/Tests/SecureInputProbe.swift"
probe_binary="$workspace_dir/target/macos/secure-input-probe"
defaults_domain="com.unvalley.inputmethod.Slime"
defaults_key="inputVerificationToken"
defaults_mode_key="inputVerificationMode"
defaults_target_process_key="inputVerificationTargetProcess"
release_package="${SLIME_RELEASE_PACKAGE:-}"
command="${1:-}"

usage() {
  echo "usage: scripts/macos-input-privacy-gate.sh prepare-private|check-private|prepare-secure|check-secure|prepare-resume|check-resume|cancel" >&2
  exit 64
}

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "input privacy verification can only run on macOS" >&2
  exit 1
fi

require_unlocked_console() {
  case "$(slime_console_lock_state "$(ioreg -n Root -d1)")" in
    unlocked) return ;;
    locked)
      echo "unlock the macOS console before preparing a physical input gate" >&2
      ;;
    *)
      echo "cannot determine the macOS console lock state; physical input gate was not prepared" >&2
      ;;
  esac
  exit 1
}

snapshot() {
  local path="$1"
  if [[ -f "$path" ]]; then
    shasum -a 256 "$path" | awk '{print $1}'
  else
    echo "missing"
  fi
}

validate_snapshot() {
  local value="$1"
  [[ "$value" == "missing" || "$value" =~ ^[0-9a-f]{64}$ ]]
}

prepare_bundle_binding() {
  if [[ -n "$release_package" ]]; then
    if [[ ! -f "$release_package" || -L "$release_package" ]]; then
      echo "SLIME_RELEASE_PACKAGE must name the notarized release PKG" >&2
      exit 1
    fi
    if [[ -e "$user_bundle" ]]; then
      echo "remove the per-user input method before verifying the system release package" >&2
      exit 1
    fi
    installed_bundle="$system_bundle"
    installed_scope="system"
    "$workspace_dir/scripts/verify-macos-release.sh" "$release_package"
    slime_assert_installed_bundle_matches_package "$release_package" "$installed_bundle"
    package_sha256="$(slime_sha256_file "$release_package")"
  else
    package_sha256="development"
    if [[ -d "$user_bundle" ]]; then
      installed_bundle="$user_bundle"
      installed_scope="user"
    elif [[ -d "$system_bundle" ]]; then
      installed_bundle="$system_bundle"
      installed_scope="system"
    else
      echo "install the macOS input method before preparing the privacy gate" >&2
      exit 1
    fi
  fi

  installed_bundle_sha256="$(slime_bundle_manifest_sha256 "$installed_bundle")"
  installed_version="$(plutil -extract CFBundleShortVersionString raw -o - \
    "$installed_bundle/Contents/Info.plist")"
  installed_build="$(plutil -extract CFBundleVersion raw -o - \
    "$installed_bundle/Contents/Info.plist")"
  installed_source_revision="$(
    plutil -extract SlimeSourceRevision raw -o - \
      "$installed_bundle/Contents/Info.plist" 2>/dev/null || echo development
  )"
}

ensure_installed_source() {
  local installed_identifier current_source
  prepare_bundle_binding
  if [[ ! -x "$register_tool" ]]; then
    echo "input source diagnostic tool is missing" >&2
    exit 1
  fi
  installed_identifier="$(
    plutil -extract CFBundleIdentifier raw -o - \
      "$installed_bundle/Contents/Info.plist" 2>/dev/null || true
  )"
  if [[ "$installed_identifier" != "com.unvalley.inputmethod.Slime" ]]; then
    echo "installed input method has an unexpected bundle identifier" >&2
    exit 1
  fi
  current_source="$($register_tool --current)"
  if ! grep -q '^  source id: com[.]unvalley[.]inputmethod[.]Slime[.]Japanese$' \
    <<<"$current_source"; then
    echo "Slime is not the current input source" >&2
    echo "$current_source" >&2
    exit 1
  fi
  if [[ "$(defaults read "$defaults_domain" historyLearning 2>/dev/null || echo 0)" != "1" ]]; then
    echo "history learning must be enabled before preparing the privacy gate" >&2
    exit 1
  fi
}

clear_pending_token() {
  defaults delete "$defaults_domain" "$defaults_key" >/dev/null 2>&1 || true
  defaults delete "$defaults_domain" "$defaults_mode_key" >/dev/null 2>&1 || true
  defaults delete "$defaults_domain" "$defaults_target_process_key" >/dev/null 2>&1 || true
}

require_process_id() {
  local process_id="$1"
  local purpose="$2"
  if [[ ! "$process_id" =~ ^[1-9][0-9]{0,9}$ ]] \
    || ! kill -0 "$process_id" 2>/dev/null; then
    echo "$purpose process is not running" >&2
    exit 1
  fi
}

textedit_process_id() {
  local process_id
  process_id="$(pgrep -n -x TextEdit || true)"
  require_process_id "$process_id" "TextEdit"
  printf '%s\n' "$process_id"
}

frontmost_process_id() {
  local application process_id
  application="$(lsappinfo front 2>/dev/null || true)"
  process_id="$(
    lsappinfo info -only pid "$application" 2>/dev/null \
      | sed -n 's/^"pid"=\([1-9][0-9]*\)$/\1/p'
  )"
  require_process_id "$process_id" "Frontmost secure-input host"
  printf '%s\n' "$process_id"
}

secure_target_process_id() {
  if [[ -n "${SLIME_PRIVACY_TARGET_PROCESS_ID:-}" ]]; then
    require_process_id "$SLIME_PRIVACY_TARGET_PROCESS_ID" \
      "Configured secure-input host"
    printf '%s\n' "$SLIME_PRIVACY_TARGET_PROCESS_ID"
  else
    frontmost_process_id
  fi
}

ensure_secure_input_probe() {
  mkdir -p "$(dirname "$probe_binary")"
  if [[ ! -x "$probe_binary" || "$probe_source" -nt "$probe_binary" ]]; then
    xcrun --sdk macosx swiftc -O "$probe_source" -framework Carbon -o "$probe_binary"
  fi
}

require_secure_input() {
  ensure_secure_input_probe
  if ! "$probe_binary"; then
    echo "Secure Event Input is not enabled; enable it in the focused host application first" >&2
    exit 1
  fi
}

write_state() {
  local mode="$1"
  local verification_mode="$2"
  local target_process_id="$3"
  local target_process_identity
  target_process_identity="$(
    slime_process_identity_sha256 "$target_process_id"
  )" || {
    echo "privacy verification host identity could not be recorded" >&2
    exit 1
  }
  mkdir -p "$(dirname "$state_file")"
  umask 077
  clear_pending_token
  token="$(uuidgen | tr '[:upper:]' '[:lower:]')"
  started_at="$(date +%s)"
  printf '%s\n' \
    "mode=$mode" \
    "verification_mode=$verification_mode" \
    "token=$token" \
    "started_at=$started_at" \
    "target_process_id=$target_process_id" \
    "target_process_identity=$target_process_identity" \
    "history=$(snapshot "$history_file")" \
    "context=$(snapshot "$context_history_file")" \
    "installed_scope=$installed_scope" \
    "bundle_sha256=$installed_bundle_sha256" \
    "package_sha256=$package_sha256" \
    "version=$installed_version" \
    "build=$installed_build" \
    "source_revision=$installed_source_revision" >"$state_file"
  defaults write "$defaults_domain" "$defaults_key" -string "$token"
  defaults write "$defaults_domain" "$defaults_mode_key" -string "$verification_mode"
  defaults write "$defaults_domain" "$defaults_target_process_key" \
    -int "$target_process_id"
  pkill -x Slime >/dev/null 2>&1 || true
}

load_state() {
  local expected_mode="$1"
  local expected_verification_mode="$2"
  if [[ ! -f "$state_file" ]]; then
    echo "no prepared input privacy gate was found" >&2
    exit 1
  fi
  mode="$(sed -n 's/^mode=//p' "$state_file")"
  verification_mode="$(sed -n 's/^verification_mode=//p' "$state_file")"
  token="$(sed -n 's/^token=//p' "$state_file")"
  started_at="$(sed -n 's/^started_at=//p' "$state_file")"
  target_process_id="$(sed -n 's/^target_process_id=//p' "$state_file")"
  target_process_identity="$(
    sed -n 's/^target_process_identity=//p' "$state_file"
  )"
  history_before="$(sed -n 's/^history=//p' "$state_file")"
  context_before="$(sed -n 's/^context=//p' "$state_file")"
  installed_scope="$(sed -n 's/^installed_scope=//p' "$state_file")"
  bundle_sha256="$(sed -n 's/^bundle_sha256=//p' "$state_file")"
  package_sha256="$(sed -n 's/^package_sha256=//p' "$state_file")"
  installed_version="$(sed -n 's/^version=//p' "$state_file")"
  installed_build="$(sed -n 's/^build=//p' "$state_file")"
  installed_source_revision="$(sed -n 's/^source_revision=//p' "$state_file")"
  if [[ "$mode" != "$expected_mode" ]] \
    || [[ "$verification_mode" != "$expected_verification_mode" ]] \
    || [[ ! "$token" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$ ]] \
    || [[ ! "$started_at" =~ ^[0-9]{10}$ ]] \
    || [[ ! "$target_process_id" =~ ^[1-9][0-9]{0,9}$ ]] \
    || [[ ! "$target_process_identity" =~ ^[0-9a-f]{64}$ ]] \
    || [[ ! "$bundle_sha256" =~ ^[0-9a-f]{64}$ ]] \
    || [[ ! "$package_sha256" =~ ^([0-9a-f]{64}|development)$ ]] \
    || [[ ! "$installed_version" =~ ^[0-9]+([.][0-9]+){1,2}$ ]] \
    || [[ ! "$installed_build" =~ ^[1-9][0-9]{0,17}$ ]] \
    || [[ ! "$installed_source_revision" =~ ^([0-9a-f]{40}|development)$ ]] \
    || ! validate_snapshot "$history_before" \
    || ! validate_snapshot "$context_before"; then
    echo "input privacy gate state is malformed or has the wrong mode" >&2
    exit 1
  fi
  now="$(date +%s)"
  if ! slime_gate_started_at_is_active "$started_at" "$now"; then
    clear_pending_token
    echo "input privacy gate expired; prepare a new physical input session" >&2
    exit 1
  fi
  require_process_id "$target_process_id" "Bound privacy verification host"
  if ! slime_process_identity_matches \
    "$target_process_id" "$target_process_identity"; then
    clear_pending_token
    echo "bound privacy verification host was restarted or replaced" >&2
    exit 1
  fi
  case "$installed_scope" in
    user) installed_bundle="$user_bundle" ;;
    system) installed_bundle="$system_bundle" ;;
    *)
      echo "input privacy gate has an invalid installed scope" >&2
      exit 1
      ;;
  esac
  if [[ "$(slime_bundle_manifest_sha256 "$installed_bundle")" != "$bundle_sha256" ]]; then
    echo "installed input method changed after the privacy gate was prepared" >&2
    exit 1
  fi
  if [[ "$(plutil -extract CFBundleShortVersionString raw -o - \
      "$installed_bundle/Contents/Info.plist")" != "$installed_version" \
    || "$(plutil -extract CFBundleVersion raw -o - \
      "$installed_bundle/Contents/Info.plist")" != "$installed_build" ]]; then
    echo "installed input method version changed after the privacy gate was prepared" >&2
    exit 1
  fi
  current_source_revision="$(
    plutil -extract SlimeSourceRevision raw -o - \
      "$installed_bundle/Contents/Info.plist" 2>/dev/null || echo development
  )"
  if [[ "$current_source_revision" != "$installed_source_revision" ]]; then
    echo "installed input method source revision changed after the privacy gate was prepared" >&2
    exit 1
  fi
  started_text="$(date -r "$started_at" '+%Y-%m-%d %H:%M:%S')"
}

report_binding() {
  echo "Verified installed bundle $installed_version ($installed_build), manifest $bundle_sha256."
  if [[ "$package_sha256" != "development" ]]; then
    echo "Verified release package SHA-256: $package_sha256"
    echo "Release binding JSON: $(
      slime_macos_release_binding_json \
        "$installed_version" \
        "$installed_build" \
        "$installed_source_revision" \
        "$package_sha256" \
        "$bundle_sha256"
    )"
  fi
}

check_privacy_evidence() {
  evidence="$(
    log show \
      --start "$started_text" \
      --style compact \
      --predicate \
      "process == \"Slime\" AND eventMessage CONTAINS \"InputMethodPrivacyEvent token=$token\"" \
      2>/dev/null
  )"
  for expected_step in character candidate-shown privacy-committed; do
    expected_event="InputMethodPrivacyEvent token=$token mode=$verification_mode step=$expected_step"
    if [[ "$evidence" != *"$expected_event"* ]]; then
      echo "privacy verification step was not recorded: $expected_step" >&2
      exit 1
    fi
  done
  if defaults read "$defaults_domain" "$defaults_key" >/dev/null 2>&1; then
    echo "privacy verification token was not consumed by Slime" >&2
    exit 1
  fi
}

check_unchanged() {
  local history_after context_after
  history_after="$(snapshot "$history_file")"
  context_after="$(snapshot "$context_history_file")"
  if [[ "$history_after" != "$history_before" ]]; then
    echo "history.tsv changed while the privacy gate was active" >&2
    exit 1
  fi
  if [[ "$context_after" != "$context_before" ]]; then
    echo "context_history.tsv changed while the privacy gate was active" >&2
    exit 1
  fi
}

main() {
  case "$command" in
    prepare-private)
    require_unlocked_console
    ensure_installed_source
    target_process_id="$(textedit_process_id)"
    write_state private privacy-private "$target_process_id"
    echo "Private-mode privacy gate prepared."
    echo "Enable private mode, confirm learned ranking is absent, then physically type nihon, Space, Enter."
    echo "Keep private mode enabled until check-private finishes."
    echo "Then run: scripts/macos-input-privacy-gate.sh check-private"
      ;;
    check-private)
    load_state private privacy-private
    check_privacy_evidence
    check_unchanged
    rm -f "$state_file"
    echo "Private mode handled physical conversion and left both history stores byte-for-byte unchanged."
    report_binding
      ;;
    prepare-secure)
    require_unlocked_console
    ensure_installed_source
    require_secure_input
    target_process_id="$(secure_target_process_id)"
    write_state secure privacy-secure "$target_process_id"
    echo "Secure-input privacy gate prepared."
    echo "Confirm learned ranking is absent, then physically type nihon, Space, Enter."
    echo "Keep Secure Event Input enabled until check-secure finishes."
      ;;
    check-secure)
    load_state secure privacy-secure
    require_secure_input
    check_privacy_evidence
    check_unchanged
    rm -f "$state_file"
    echo "Secure Event Input handled physical conversion and both history stores stayed byte-for-byte unchanged."
    report_binding
      ;;
    prepare-resume)
    require_unlocked_console
    ensure_installed_source
    ensure_secure_input_probe
    if "$probe_binary"; then
      echo "disable Secure Event Input before checking normal learning" >&2
      exit 1
    fi
    target_process_id="$(textedit_process_id)"
    write_state resume privacy-resume "$target_process_id"
    echo "Normal-learning resume gate prepared."
    echo "With private mode off, physically type nihon, Space, Enter."
    echo "Then run: scripts/macos-input-privacy-gate.sh check-resume"
      ;;
    check-resume)
    load_state resume privacy-resume
    check_privacy_evidence
    history_after="$(snapshot "$history_file")"
    if [[ "$history_after" == "$history_before" ]]; then
      echo "history.tsv did not change after normal learning resumed" >&2
      exit 1
    fi
    rm -f "$state_file"
    echo "Normal learning resumed and changed history.tsv after the privacy boundary was removed."
    report_binding
      ;;
    cancel)
    clear_pending_token
    rm -f "$state_file"
    pkill -x Slime >/dev/null 2>&1 || true
    echo "input privacy gate cancelled"
      ;;
    *) usage ;;
  esac
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main
fi
