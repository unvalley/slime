#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
temporary_directory="$(mktemp -d)"
cleanup() {
  if [[ -n "$temporary_directory" && -d "$temporary_directory" ]]; then
    /usr/bin/trash "$temporary_directory"
  fi
}
trap cleanup EXIT

evidence_names=(
  release-verifier.log
  package-lifecycle.log
  textedit-character.log
  textedit-candidates.log
  textedit-number.log
  textedit-click.log
  textedit-reconversion.log
  privacy-private.log
  privacy-secure.log
  privacy-resume.log
)
success_markers=(
  'macOS release gate passed for /fixture/release.pkg'
  'macOS package install/update/uninstall gate passed for 1.2.3 (42)'
  'TextEdit delivered an event through the installed Slime InputMethodKit controller.'
  'TextEdit delivered arrow candidate movement and commit through the installed input method.'
  'TextEdit delivered candidate number selection and commit through the installed input method.'
  'TextEdit delivered candidate click selection and commit through the installed input method.'
  'TextEdit delivered selection reconversion, candidate display, and commit through the installed input method.'
  'Private mode handled physical conversion and left both history stores byte-for-byte unchanged.'
  'Secure Event Input handled physical conversion and both history stores stayed byte-for-byte unchanged.'
  'Normal learning resumed and changed history.tsv after the privacy boundary was removed.'
)
binding='{"Schema":1,"Platform":"macOS","Version":"1.2.3","Build":"42","SourceRevision":"0123456789abcdef0123456789abcdef01234567","PackageSha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","BundleManifestSha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}'

for evidence_index in "${!evidence_names[@]}"; do
  evidence_name="${evidence_names[$evidence_index]}"
  success_marker="${success_markers[$evidence_index]}"
  printf '%s\nRelease binding JSON: %s\n' "$success_marker" "$binding" \
    >"$temporary_directory/$evidence_name"
done

"$script_dir/verify-macos-consumer-evidence.sh" \
  "$temporary_directory" >/dev/null

changed_binding="${binding/\"Build\":\"42\"/\"Build\":\"43\"}"
printf '%s\nRelease binding JSON: %s\n' \
  "${success_markers[8]}" "$changed_binding" \
  >"$temporary_directory/privacy-secure.log"
if "$script_dir/verify-macos-consumer-evidence.sh" \
  "$temporary_directory" >/dev/null 2>&1; then
  echo "consumer evidence gate accepted a different release" >&2
  exit 1
fi

printf '%s\nRelease binding JSON: %s\nRelease binding JSON: %s\n' \
  "${success_markers[8]}" "$binding" "$binding" \
  >"$temporary_directory/privacy-secure.log"
if "$script_dir/verify-macos-consumer-evidence.sh" \
  "$temporary_directory" >/dev/null 2>&1; then
  echo "consumer evidence gate accepted duplicate release bindings" >&2
  exit 1
fi

printf 'Release binding JSON: %s\n' "$binding" \
  >"$temporary_directory/privacy-secure.log"
if "$script_dir/verify-macos-consumer-evidence.sh" \
  "$temporary_directory" >/dev/null 2>&1; then
  echo "consumer evidence gate accepted a log without gate success" >&2
  exit 1
fi

echo "macOS consumer evidence policy test passed"
