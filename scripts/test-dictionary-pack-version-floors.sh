#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=dictionary-pack-version-floors.sh
source "$workspace_dir/scripts/dictionary-pack-version-floors.sh"

valid="sample-general"$'\t'"2026.08.1"$'\n'"sample-context"$'\t'"2026.09.0"
slime_validate_dictionary_pack_version_floors "$valid"

if slime_validate_dictionary_pack_version_floors "" 2>/dev/null; then
  echo "empty dictionary pack version floor set was accepted" >&2
  exit 1
fi
if slime_validate_dictionary_pack_version_floors \
  "sample-general"$'\t'"2026.08" 2>/dev/null; then
  echo "incomplete dictionary pack version floor was accepted" >&2
  exit 1
fi
if slime_validate_dictionary_pack_version_floors \
  "sample-general"$'\t'"123456789012345678.123456789012.1" 2>/dev/null; then
  echo "oversized dictionary pack version floor was accepted" >&2
  exit 1
fi
if slime_validate_dictionary_pack_version_floors \
  "Sample"$'\t'"2026.08.1" 2>/dev/null; then
  echo "invalid dictionary pack id was accepted in a version floor" >&2
  exit 1
fi
if slime_validate_dictionary_pack_version_floors \
  "sample-general"$'\t'"2026.08.1"$'\n'"sample-general"$'\t'"2026.09.0" \
  2>/dev/null; then
  echo "duplicate dictionary pack version floor id was accepted" >&2
  exit 1
fi

too_many=""
for index in {1..65}; do
  too_many+="${too_many:+$'\n'}sample-$index"$'\t'"2026.08.1"
done
if slime_validate_dictionary_pack_version_floors "$too_many" 2>/dev/null; then
  echo "too many dictionary pack version floors were accepted" >&2
  exit 1
fi

echo "Dictionary pack version floor policy test passed."
