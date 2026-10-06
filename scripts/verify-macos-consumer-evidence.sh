#!/usr/bin/env bash
set -euo pipefail

evidence_directory="${1:-}"
if [[ -z "$evidence_directory" || ! -d "$evidence_directory" ]]; then
  echo "usage: scripts/verify-macos-consumer-evidence.sh EVIDENCE_DIRECTORY" >&2
  exit 64
fi

required_evidence=(
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
required_success_patterns=(
  '^macOS release gate passed for .+$'
  '^macOS package install/update/uninstall gate passed for .+$'
  '^TextEdit delivered an event through the installed Slime InputMethodKit controller[.]$'
  '^TextEdit delivered arrow candidate movement and commit through the installed input method[.]$'
  '^TextEdit delivered candidate number selection and commit through the installed input method[.]$'
  '^TextEdit delivered candidate click selection and commit through the installed input method[.]$'
  '^TextEdit delivered selection reconversion, candidate display, and commit through the installed input method[.]$'
  '^Private mode handled physical conversion and left both history stores byte-for-byte unchanged[.]$'
  '^Secure Event Input handled physical conversion and both history stores stayed byte-for-byte unchanged[.]$'
  '^Normal learning resumed and changed history[.]tsv after the privacy boundary was removed[.]$'
)
binding_pattern='^\{"Schema":1,"Platform":"macOS","Version":"[0-9]+(\.[0-9]+){1,2}","Build":"[1-9][0-9]{0,17}","SourceRevision":"[0-9a-f]{40}","PackageSha256":"[0-9a-f]{64}","BundleManifestSha256":"[0-9a-f]{64}"\}$'
expected_binding=""

for evidence_index in "${!required_evidence[@]}"; do
  evidence_name="${required_evidence[$evidence_index]}"
  success_pattern="${required_success_patterns[$evidence_index]}"
  evidence_path="$evidence_directory/$evidence_name"
  if [[ ! -f "$evidence_path" || -L "$evidence_path" ]]; then
    echo "consumer evidence is missing or is not a regular file: $evidence_name" >&2
    exit 1
  fi
  success_count="$(
    /usr/bin/grep -Ec "$success_pattern" "$evidence_path" || true
  )"
  if [[ "$success_count" != "1" ]]; then
    echo "consumer evidence must contain exactly one gate success marker: $evidence_name" >&2
    exit 1
  fi
  binding_count="$(
    /usr/bin/grep -c '^Release binding JSON: ' "$evidence_path" || true
  )"
  if [[ "$binding_count" != "1" ]]; then
    echo "consumer evidence must contain exactly one release binding: $evidence_name" >&2
    exit 1
  fi
  binding="$(
    /usr/bin/sed -n 's/^Release binding JSON: //p' "$evidence_path"
  )"
  if [[ ! "$binding" =~ $binding_pattern ]]; then
    echo "consumer evidence contains a malformed release binding: $evidence_name" >&2
    exit 1
  fi
  if [[ -z "$expected_binding" ]]; then
    expected_binding="$binding"
  elif [[ "$binding" != "$expected_binding" ]]; then
    echo "consumer evidence refers to a different release: $evidence_name" >&2
    exit 1
  fi
done

echo "macOS consumer release evidence gate passed"
echo "Release binding JSON: $expected_binding"
