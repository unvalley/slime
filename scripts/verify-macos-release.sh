#!/usr/bin/env bash
set -euo pipefail

package_path="${1:-}"
workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=dictionary-pack-verification-keys.sh
source "$workspace_dir/scripts/dictionary-pack-verification-keys.sh"
# shellcheck source=dictionary-pack-version-floors.sh
source "$workspace_dir/scripts/dictionary-pack-version-floors.sh"
# shellcheck source=macos-package-binding.sh
source "$workspace_dir/scripts/macos-package-binding.sh"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS release verification can only run on macOS" >&2
  exit 1
fi
if [[ -z "$package_path" || ! -f "$package_path" ]]; then
  echo "usage: scripts/verify-macos-release.sh <notarized.pkg>" >&2
  exit 1
fi

package_path="$(cd "$(dirname "$package_path")" && pwd)/$(basename "$package_path")"
"$workspace_dir/scripts/verify-macos-package-structure.sh" "$package_path"
inspection_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$inspection_dir"
}
trap cleanup EXIT
expanded_dir="$inspection_dir/expanded"
payload_dir="$inspection_dir/payload"
pkgutil --expand "$package_path" "$expanded_dir"
mkdir -p "$payload_dir"
ditto -x "$expanded_dir/Payload" "$payload_dir"
bundle_dir="$payload_dir/Library/Input Methods/Slime.app"
version="$(plutil -extract CFBundleShortVersionString raw -o - \
  "$bundle_dir/Contents/Info.plist")"
build="$(plutil -extract CFBundleVersion raw -o - \
  "$bundle_dir/Contents/Info.plist")"
source_revision="$(plutil -extract SlimeSourceRevision raw -o - \
  "$bundle_dir/Contents/Info.plist" 2>/dev/null || true)"
if [[ ! "$source_revision" =~ ^[0-9a-f]{40}$ ]]; then
  echo "release package lacks a valid source revision" >&2
  exit 1
fi
codesign --verify --deep --strict --verbose=2 "$bundle_dir"
signature="$(codesign -dv --verbose=4 "$bundle_dir" 2>&1)"
if [[ "$signature" != *"Authority=Developer ID Application:"* ]]; then
  echo "Developer ID Application authority is missing" >&2
  exit 1
fi
if [[ "$signature" != *"flags="*"runtime"* ]]; then
  echo "hardened runtime is missing" >&2
  exit 1
fi
team_identifier="$(sed -n 's/^TeamIdentifier=//p' <<<"$signature" | head -n 1)"
if [[ -z "$team_identifier" || "$team_identifier" == "not set" ]]; then
  echo "signing team identifier is missing" >&2
  exit 1
fi
dictionary_pack_verification_keys="$(
  plutil -extract SlimeDictionaryPackVerificationKeys raw -o - \
    "$bundle_dir/Contents/Info.plist" 2>/dev/null || true
)"
if [[ -z "$dictionary_pack_verification_keys" ]]; then
  echo "signed dictionary pack verification keys are missing" >&2
  exit 1
fi
if ! slime_validate_dictionary_pack_verification_keys \
  "$dictionary_pack_verification_keys"; then
  echo "signed dictionary pack verification keys are malformed" >&2
  exit 1
fi
dictionary_pack_version_floors="$(
  plutil -extract SlimeDictionaryPackVersionFloors raw -o - \
    "$bundle_dir/Contents/Info.plist" 2>/dev/null || true
)"
if [[ -z "$dictionary_pack_version_floors" ]]; then
  echo "dictionary pack version floors are missing" >&2
  exit 1
fi
if ! slime_validate_dictionary_pack_version_floors \
  "$dictionary_pack_version_floors"; then
  echo "dictionary pack version floors are malformed" >&2
  exit 1
fi
ffi_library="$bundle_dir/Contents/Frameworks/libslime_ffi.dylib"
if [[ ! -f "$ffi_library" ]]; then
  echo "signed FFI library is missing from package payload" >&2
  exit 1
fi
codesign --verify --strict --verbose=2 "$ffi_library"
ffi_signature="$(codesign -dv --verbose=4 "$ffi_library" 2>&1)"
ffi_team_identifier="$(sed -n 's/^TeamIdentifier=//p' <<<"$ffi_signature" | head -n 1)"
if [[ "$ffi_signature" != *"Authority=Developer ID Application:"* \
  || "$ffi_signature" != *"flags="*"runtime"* \
  || "$ffi_team_identifier" != "$team_identifier" ]]; then
  echo "embedded dylib does not share the bundle's Developer ID runtime signature" >&2
  exit 1
fi
uninstaller="$bundle_dir/Contents/Resources/uninstall-macos-system.sh"
if [[ ! -x "$uninstaller" ]]; then
  echo "bundled system uninstaller is missing or not executable" >&2
  exit 1
fi
entitlements="$(codesign -d --entitlements - "$bundle_dir" 2>/dev/null || true)"
ffi_entitlements="$(codesign -d --entitlements - "$ffi_library" 2>/dev/null || true)"
if grep -Eq 'com[.]apple[.]security[.](get-task-allow|cs[.])' \
  <<<"$entitlements$ffi_entitlements"; then
  echo "release bundle contains a development or code-signing exception entitlement" >&2
  exit 1
fi

package_signature="$(pkgutil --check-signature "$package_path")"
if [[ "$package_signature" != *"Developer ID Installer:"* \
  || "$package_signature" != *"($team_identifier)"* ]]; then
  echo "package does not have a Developer ID Installer signature from $team_identifier" >&2
  exit 1
fi
spctl --assess --type execute --verbose=4 "$bundle_dir"
xcrun stapler validate "$package_path"
spctl --assess --type install --verbose=4 "$package_path"
package_sha256="$(slime_sha256_file "$package_path")"
bundle_manifest_sha256="$(slime_bundle_manifest_sha256 "$bundle_dir")"
echo "$package_sha256  $package_path"
echo "Source revision: $source_revision"
echo "Release binding JSON: $(
  slime_macos_release_binding_json \
    "$version" \
    "$build" \
    "$source_revision" \
    "$package_sha256" \
    "$bundle_manifest_sha256"
)"
echo "macOS release gate passed for $package_path"
