#!/usr/bin/env bash

slime_validate_dictionary_pack_verification_keys() {
  local source="$1"
  local line key_id encoded_key seen_ids="" key_count=0
  while IFS= read -r line; do
    key_id="${line%%$'\t'*}"
    encoded_key="${line#*$'\t'}"
    if [[ "$key_id" == "$line" || "$encoded_key" == *$'\t'* \
      || ! "$key_id" =~ ^[a-z0-9-]{1,64}$ \
      || ! "$encoded_key" =~ ^[0-9a-f]{64}$ ]]; then
      echo "Dictionary pack verification keys must be key-id<TAB>64 lowercase hex" >&2
      return 1
    fi
    if [[ $'\n'"$seen_ids"$'\n' == *$'\n'"$key_id"$'\n'* ]]; then
      echo "Dictionary pack verification key ids must be unique" >&2
      return 1
    fi
    seen_ids+="${seen_ids:+$'\n'}$key_id"
    key_count=$((key_count + 1))
    if (( key_count > 16 )); then
      echo "At most 16 dictionary pack verification keys are allowed" >&2
      return 1
    fi
  done <<<"$source"
  if (( key_count == 0 )); then
    echo "At least one dictionary pack verification key is required" >&2
    return 1
  fi
}
