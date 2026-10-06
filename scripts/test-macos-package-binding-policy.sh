#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=macos-package-binding.sh
source "$script_dir/macos-package-binding.sh"

revision="0123456789abcdef0123456789abcdef01234567"
package_sha256="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
manifest_sha256="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
expected='{"Schema":1,"Platform":"macOS","Version":"1.2.3","Build":"42","SourceRevision":"0123456789abcdef0123456789abcdef01234567","PackageSha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","BundleManifestSha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}'

actual="$(
  slime_macos_release_binding_json \
    1.2.3 42 "$revision" "$package_sha256" "$manifest_sha256"
)"
if [[ "$actual" != "$expected" ]]; then
  echo "macOS release binding is not canonical" >&2
  exit 1
fi

changed="$(
  slime_macos_release_binding_json \
    1.2.3 43 "$revision" "$package_sha256" "$manifest_sha256"
)"
if [[ "$changed" == "$actual" ]]; then
  echo "macOS release binding did not change with its build" >&2
  exit 1
fi

if slime_macos_release_binding_json \
  1.2.3 42 dirty "$package_sha256" "$manifest_sha256" >/dev/null 2>&1; then
  echo "macOS release binding accepted an invalid source revision" >&2
  exit 1
fi

echo "macOS package binding policy test passed"
