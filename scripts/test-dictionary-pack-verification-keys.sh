#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=dictionary-pack-verification-keys.sh
source "$workspace_dir/scripts/dictionary-pack-verification-keys.sh"

key_a="$(printf '01%.0s' {1..32})"
key_b="$(printf '02%.0s' {1..32})"
uppercase_key="$(printf 'ab%.0s' {1..32} | tr '[:lower:]' '[:upper:]')"
valid="release-a"$'\t'"$key_a"$'\n'"release-b"$'\t'"$key_b"
slime_validate_dictionary_pack_verification_keys "$valid"

if slime_validate_dictionary_pack_verification_keys "" 2>/dev/null; then
  echo "empty dictionary pack key set was accepted" >&2
  exit 1
fi
if slime_validate_dictionary_pack_verification_keys \
  "release-a"$'\t'"$uppercase_key" 2>/dev/null; then
  echo "uppercase dictionary pack key was accepted" >&2
  exit 1
fi
if slime_validate_dictionary_pack_verification_keys \
  "release-a"$'\t'"$key_a"$'\n'"release-a"$'\t'"$key_b" 2>/dev/null; then
  echo "duplicate dictionary pack key id was accepted" >&2
  exit 1
fi
if slime_validate_dictionary_pack_verification_keys \
  "release_a"$'\t'"$key_a" 2>/dev/null; then
  echo "invalid dictionary pack key id was accepted" >&2
  exit 1
fi

too_many=""
for index in {1..17}; do
  too_many+="${too_many:+$'\n'}release-$index"$'\t'"$key_a"
done
if slime_validate_dictionary_pack_verification_keys "$too_many" 2>/dev/null; then
  echo "too many dictionary pack keys were accepted" >&2
  exit 1
fi

echo "Dictionary pack verification key policy test passed."
