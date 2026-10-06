#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=macos-console-state.sh
source "$workspace_dir/scripts/macos-console-state.sh"
# shellcheck source=macos-package-binding.sh
source "$workspace_dir/scripts/macos-package-binding.sh"
user_bundle="$HOME/Library/Input Methods/Slime.app"
system_bundle="/Library/Input Methods/Slime.app"
built_bundle="$workspace_dir/target/macos/Slime.app"
register_tool="$workspace_dir/target/macos/register-input-source"
state_file="$workspace_dir/target/macos/textedit-input-gate.state"
defaults_domain="com.unvalley.inputmethod.Slime"
defaults_key="inputVerificationToken"
defaults_mode_key="inputVerificationMode"
defaults_target_process_key="inputVerificationTargetProcess"
release_package="${SLIME_RELEASE_PACKAGE:-}"
command="${1:-}"

usage() {
  echo "usage: scripts/macos-textedit-input-gate.sh prepare|check|prepare-candidates|check-candidates|prepare-number|check-number|prepare-click|check-click|prepare-reconversion|check-reconversion|cancel" >&2
  exit 64
}

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "TextEdit input verification can only run on macOS" >&2
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

clear_pending_token() {
  defaults delete "$defaults_domain" "$defaults_key" >/dev/null 2>&1 || true
  defaults delete "$defaults_domain" "$defaults_mode_key" >/dev/null 2>&1 || true
  defaults delete "$defaults_domain" "$defaults_target_process_key" >/dev/null 2>&1 || true
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
    installed_bundle="$user_bundle"
    installed_scope="user"
    package_sha256="development"
    if [[ ! -d "$installed_bundle" || ! -d "$built_bundle" ]]; then
      echo "build and install the macOS input method before preparing the gate" >&2
      exit 1
    fi
    if [[ "$(slime_bundle_manifest_sha256 "$installed_bundle")" != \
      "$(slime_bundle_manifest_sha256 "$built_bundle")" ]]; then
      echo "installed input method does not match the current build" >&2
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

write_gate_state() {
  local mode="$1"
  local target_process_id="$2"
  local target_process_identity
  target_process_identity="$(
    slime_process_identity_sha256 "$target_process_id"
  )" || {
    echo "TextEdit process identity could not be recorded" >&2
    exit 1
  }
  mkdir -p "$(dirname "$state_file")"
  umask 077
  token="$(uuidgen | tr '[:upper:]' '[:lower:]')"
  started_at="$(date +%s)"
  printf '%s\n' \
    "token=$token" \
    "started_at=$started_at" \
    "mode=$mode" \
    "target_process_id=$target_process_id" \
    "target_process_identity=$target_process_identity" \
    "installed_scope=$installed_scope" \
    "bundle_sha256=$installed_bundle_sha256" \
    "package_sha256=$package_sha256" \
    "version=$installed_version" \
    "build=$installed_build" \
    "source_revision=$installed_source_revision" >"$state_file"
  defaults write "$defaults_domain" "$defaults_key" -string "$token"
  defaults write "$defaults_domain" "$defaults_mode_key" -string "$mode"
  defaults write "$defaults_domain" "$defaults_target_process_key" \
    -int "$target_process_id"
  pkill -x Slime >/dev/null 2>&1 || true
}

prepare_gate() {
  local mode="$1"
  require_unlocked_console
  prepare_bundle_binding
  if [[ ! -x "$register_tool" ]]; then
    echo "input source diagnostic tool is missing" >&2
    exit 1
  fi
  current_source="$("$register_tool" --current)"
  if ! grep -q '^  source id: com[.]unvalley[.]inputmethod[.]Slime[.]Japanese$' \
    <<<"$current_source"; then
    echo "Slime is not the current input source" >&2
    echo "$current_source" >&2
    exit 1
  fi
  target_process_id="$(pgrep -n -x TextEdit || true)"
  if [[ ! "$target_process_id" =~ ^[1-9][0-9]{0,9}$ ]]; then
    echo "TextEdit must be running with an editable document" >&2
    exit 1
  fi
  write_gate_state "$mode" "$target_process_id"
  echo "Only HID system source events count; synthesized private/session events are ignored."

  case "$mode" in
    candidate-selection)
      echo "TextEdit arrow candidate gate prepared."
      echo "With an empty TextEdit document focused and Slime selected, physically type nihon."
      echo "Press Space, press Down Arrow once, then press Enter."
      echo "Then run: scripts/macos-textedit-input-gate.sh check-candidates"
      ;;
    candidate-number)
      echo "TextEdit candidate number gate prepared."
      echo "With an empty TextEdit document focused and Slime selected, physically type nihon."
      echo "Press Space, then press the 2 key to commit the second candidate."
      echo "Then run: scripts/macos-textedit-input-gate.sh check-number"
      ;;
    candidate-click)
      echo "TextEdit candidate click gate prepared."
      echo "With an empty TextEdit document focused and Slime selected, physically type nihon."
      echo "Press Space, then click the second candidate row."
      echo "Then run: scripts/macos-textedit-input-gate.sh check-click"
      ;;
    reconversion)
      echo "TextEdit reconversion gate prepared."
      echo "In TextEdit, select one previously committed Japanese word."
      echo "Physically press Control+Shift+R, then press Enter to commit a candidate."
      echo "Then run: scripts/macos-textedit-input-gate.sh check-reconversion"
      ;;
    *)
      echo "TextEdit input gate prepared."
      echo "With TextEdit focused and Slime selected, press one physical A-Z letter key."
      echo "Then run: scripts/macos-textedit-input-gate.sh check"
      ;;
  esac
}

load_gate_state() {
  local expected_mode="$1"
  if [[ ! -f "$state_file" ]]; then
    echo "no prepared TextEdit input gate was found" >&2
    exit 1
  fi
  token="$(sed -n 's/^token=//p' "$state_file")"
  started_at="$(sed -n 's/^started_at=//p' "$state_file")"
  mode="$(sed -n 's/^mode=//p' "$state_file")"
  target_process_id="$(sed -n 's/^target_process_id=//p' "$state_file")"
  target_process_identity="$(
    sed -n 's/^target_process_identity=//p' "$state_file"
  )"
  installed_scope="$(sed -n 's/^installed_scope=//p' "$state_file")"
  bundle_sha256="$(sed -n 's/^bundle_sha256=//p' "$state_file")"
  package_sha256="$(sed -n 's/^package_sha256=//p' "$state_file")"
  installed_version="$(sed -n 's/^version=//p' "$state_file")"
  installed_build="$(sed -n 's/^build=//p' "$state_file")"
  installed_source_revision="$(sed -n 's/^source_revision=//p' "$state_file")"
  if [[ ! "$token" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$ \
    || ! "$started_at" =~ ^[0-9]{10}$ \
    || "$mode" != "$expected_mode" \
    || ! "$target_process_id" =~ ^[1-9][0-9]{0,9}$ \
    || ! "$target_process_identity" =~ ^[0-9a-f]{64}$ \
    || ! "$bundle_sha256" =~ ^[0-9a-f]{64}$ \
    || ! "$package_sha256" =~ ^([0-9a-f]{64}|development)$ \
    || ! "$installed_version" =~ ^[0-9]+([.][0-9]+){1,2}$ \
    || ! "$installed_build" =~ ^[1-9][0-9]{0,17}$ \
    || ! "$installed_source_revision" =~ ^([0-9a-f]{40}|development)$ ]]; then
    echo "TextEdit input gate state is malformed or has the wrong mode" >&2
    exit 1
  fi
  if ! slime_process_identity_matches \
    "$target_process_id" "$target_process_identity"; then
    clear_pending_token
    echo "bound TextEdit process was restarted or replaced" >&2
    exit 1
  fi
  now="$(date +%s)"
  if ! slime_gate_started_at_is_active "$started_at" "$now"; then
    clear_pending_token
    echo "TextEdit input gate expired; prepare a new physical input session" >&2
    exit 1
  fi
  case "$installed_scope" in
    user) installed_bundle="$user_bundle" ;;
    system) installed_bundle="$system_bundle" ;;
    *)
      echo "TextEdit input gate has an invalid installed scope" >&2
      exit 1
      ;;
  esac
  if [[ "$(slime_bundle_manifest_sha256 "$installed_bundle")" != "$bundle_sha256" ]]; then
    echo "installed input method changed after the TextEdit gate was prepared" >&2
    exit 1
  fi
  if [[ "$(plutil -extract CFBundleShortVersionString raw -o - \
      "$installed_bundle/Contents/Info.plist")" != "$installed_version" \
    || "$(plutil -extract CFBundleVersion raw -o - \
      "$installed_bundle/Contents/Info.plist")" != "$installed_build" ]]; then
    echo "installed input method version changed after the TextEdit gate was prepared" >&2
    exit 1
  fi
  current_source_revision="$(
    plutil -extract SlimeSourceRevision raw -o - \
      "$installed_bundle/Contents/Info.plist" 2>/dev/null || echo development
  )"
  if [[ "$current_source_revision" != "$installed_source_revision" ]]; then
    echo "installed input method source revision changed after the TextEdit gate was prepared" >&2
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

check_candidate_gate() {
  local expected_mode="$1"
  local interaction_step="$2"
  local success_message="$3"
  load_gate_state "$expected_mode"
  evidence="$(
    log show \
      --start "$started_text" \
      --style compact \
      --predicate \
      "process == \"Slime\" AND eventMessage CONTAINS \"InputMethodCandidateEvent token=$token\"" \
      2>/dev/null
  )"
  for expected_step in \
    character candidate-shown "$interaction_step" candidate-moved candidate-committed; do
    if [[ "$evidence" != *"InputMethodCandidateEvent token=$token step=$expected_step"* ]]; then
      echo "candidate verification step was not recorded: $expected_step" >&2
      exit 1
    fi
  done
  if defaults read "$defaults_domain" "$defaults_key" >/dev/null 2>&1; then
    echo "candidate verification token was not consumed by Slime" >&2
    exit 1
  fi
  rm -f "$state_file"
  echo "$success_message"
  report_binding
  echo "The diagnostic recorded only step names and a random one-time token, not keys or document text."
}

check_reconversion_gate() {
  load_gate_state reconversion
  evidence="$(
    log show \
      --start "$started_text" \
      --style compact \
      --predicate \
      "process == \"Slime\" AND eventMessage CONTAINS \"InputMethodReconversionEvent token=$token\"" \
      2>/dev/null
  )"
  for expected_step in \
    reconversion-requested reconversion-started candidate-shown reconversion-committed; do
    if [[ "$evidence" != *"InputMethodReconversionEvent token=$token step=$expected_step"* ]]; then
      echo "reconversion verification step was not recorded: $expected_step" >&2
      exit 1
    fi
  done
  if defaults read "$defaults_domain" "$defaults_key" >/dev/null 2>&1; then
    echo "reconversion verification token was not consumed by Slime" >&2
    exit 1
  fi
  rm -f "$state_file"
  echo "TextEdit delivered selection reconversion, candidate display, and commit through the installed input method."
  report_binding
  echo "The diagnostic recorded only step names and a random one-time token, not selection or document text."
}

main() {
  case "$command" in
    prepare)
    prepare_gate character
      ;;
    check)
    load_gate_state character
    evidence="$(
      log show \
        --start "$started_text" \
        --style compact \
        --predicate \
        "process == \"Slime\" AND eventMessage CONTAINS \"InputMethodCharacterEvent token=$token\"" \
        2>/dev/null
    )"
    if [[ "$evidence" != *"InputMethodCharacterEvent token=$token"* ]]; then
      echo "no InputMethodKit event has been recorded for the prepared gate" >&2
      exit 1
    fi
    if defaults read "$defaults_domain" "$defaults_key" >/dev/null 2>&1; then
      echo "input verification token was not consumed by Slime" >&2
      exit 1
    fi
    rm -f "$state_file"
    echo "TextEdit delivered an event through the installed Slime InputMethodKit controller."
    report_binding
    echo "The diagnostic recorded only a random one-time token, not the key or document text."
      ;;
    prepare-candidates)
    prepare_gate candidate-selection
      ;;
    check-candidates)
    check_candidate_gate \
      candidate-selection \
      candidate-arrow \
      "TextEdit delivered arrow candidate movement and commit through the installed input method."
      ;;
    prepare-number)
    prepare_gate candidate-number
      ;;
    check-number)
    check_candidate_gate \
      candidate-number \
      candidate-number \
      "TextEdit delivered candidate number selection and commit through the installed input method."
      ;;
    prepare-click)
    prepare_gate candidate-click
      ;;
    check-click)
    check_candidate_gate \
      candidate-click \
      candidate-click \
      "TextEdit delivered candidate click selection and commit through the installed input method."
      ;;
    prepare-reconversion)
    prepare_gate reconversion
      ;;
    check-reconversion)
    check_reconversion_gate
      ;;
    cancel)
    clear_pending_token
    rm -f "$state_file"
    pkill -x Slime >/dev/null 2>&1 || true
    echo "TextEdit input gate cancelled"
      ;;
    *) usage ;;
  esac
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main
fi
