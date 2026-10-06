#!/usr/bin/env bash

slime_validate_dictionary_pack_version_floors() {
  local source="$1"
  local line pack_id minimum_version seen_ids="" floor_count=0
  while IFS= read -r line; do
    pack_id="${line%%$'\t'*}"
    minimum_version="${line#*$'\t'}"
    if [[ "$pack_id" == "$line" || "$minimum_version" == *$'\t'* \
      || ! "$pack_id" =~ ^[a-z0-9][a-z0-9._-]{0,63}$ \
      || ${#minimum_version} -gt 32 \
      || ! "$minimum_version" =~ ^[0-9]{1,18}[.][0-9]{1,18}[.][0-9]{1,18}$ ]]; then
      echo "Dictionary pack version floors must be pack-id<TAB>MAJOR.MINOR.PATCH" >&2
      return 1
    fi
    if [[ $'\n'"$seen_ids"$'\n' == *$'\n'"$pack_id"$'\n'* ]]; then
      echo "Dictionary pack version floor ids must be unique" >&2
      return 1
    fi
    seen_ids+="${seen_ids:+$'\n'}$pack_id"
    floor_count=$((floor_count + 1))
    if (( floor_count > 64 )); then
      echo "At most 64 dictionary pack version floors are allowed" >&2
      return 1
    fi
  done <<<"$source"
  if (( floor_count == 0 )); then
    echo "At least one dictionary pack version floor is required" >&2
    return 1
  fi
}
