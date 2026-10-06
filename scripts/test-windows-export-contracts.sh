#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
manifest_dir="$workspace_dir/platforms/windows/arm64x"
ffi_manifest="$manifest_dir/slime-ffi-exports.txt"
ime_manifest="$manifest_dir/slime-ime-exports.txt"
ffi_header="$workspace_dir/crates/slime-ffi/include/slime_ffi.h"
ime_definition="$workspace_dir/platforms/windows/native/src/SlimeIME.def"
register_helper="$workspace_dir/platforms/windows/native/src/RegisterIME.cpp"

validate_manifest() {
  local path="$1"
  local invalid duplicate

  invalid="$(sed '/^[[:space:]]*$/d' "$path" \
    | grep -Ev '^[A-Za-z_][A-Za-z0-9_]*$' || true)"
  duplicate="$(sed '/^[[:space:]]*$/d' "$path" | LC_ALL=C sort | uniq -d)"
  if [[ -n "$invalid" || -n "$duplicate" ]]; then
    echo "invalid or duplicate Windows export manifest: $path" >&2
    exit 1
  fi
}

compare_exports() {
  local description="$1"
  local expected="$2"
  local actual="$3"

  if ! diff -u <(printf '%s\n' "$expected") <(printf '%s\n' "$actual"); then
    echo "$description export manifest is stale" >&2
    exit 1
  fi
}

validate_manifest "$ffi_manifest"
validate_manifest "$ime_manifest"

ffi_expected="$(sed '/^[[:space:]]*$/d' "$ffi_manifest" | LC_ALL=C sort -u)"
# POSIX grep and sed only: the Windows runner's Git Bash has no ripgrep.
ffi_actual="$(grep -oE '(^|[^A-Za-z0-9_])slime_[a-z0-9_]+[[:space:]]*\(' "$ffi_header" \
  | sed -E 's/^[^A-Za-z0-9_]//; s/[[:space:]]*\($//' \
  | LC_ALL=C sort -u)"
compare_exports "Rust C ABI" "$ffi_expected" "$ffi_actual"

ime_expected="$(sed '/^[[:space:]]*$/d' "$ime_manifest" | LC_ALL=C sort -u)"
ime_actual="$(awk '
  /^[[:space:]]*(LIBRARY|EXPORTS)([[:space:]]|$)/ { next }
  NF > 0 { print $1 }
' "$ime_definition" | LC_ALL=C sort -u)"
compare_exports "TSF COM" "$ime_expected" "$ime_actual"

probe_block="$(sed -n '/requiredExports\[\][[:space:]]*=/,/};/p' "$register_helper")"
if [[ -z "$probe_block" ]]; then
  echo "Windows load-probe export block is missing" >&2
  exit 1
fi
probe_actual="$(printf '%s\n' "$probe_block" \
  | grep -oE '"[A-Za-z_][A-Za-z0-9_]*"' \
  | tr -d '"' \
  | LC_ALL=C sort -u)"
compare_exports "TSF load probe" "$ime_expected" "$probe_actual"

echo "Windows export contract test passed"
