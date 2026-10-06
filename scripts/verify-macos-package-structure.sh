#!/usr/bin/env bash
set -euo pipefail

package_path="${1:-}"
workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS package verification can only run on macOS" >&2
  exit 1
fi
if [[ -z "$package_path" || ! -f "$package_path" ]]; then
  echo "usage: scripts/verify-macos-package-structure.sh <package.pkg>" >&2
  exit 64
fi

package_path="$(cd "$(dirname "$package_path")" && pwd)/$(basename "$package_path")"
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
scripts_dir="$expanded_dir/Scripts"

if [[ ! -d "$bundle_dir" || -L "$bundle_dir" ]]; then
  echo "package does not contain the expected input method bundle" >&2
  exit 1
fi
if [[ ! -d "$scripts_dir" || -L "$scripts_dir" ]]; then
  echo "package does not contain install lifecycle scripts" >&2
  exit 1
fi

for script in preinstall postinstall; do
  script_path="$scripts_dir/$script"
  reviewed_script="$workspace_dir/platforms/macos/PackageScripts/$script"
  if [[ ! -f "$script_path" || -L "$script_path" || ! -x "$script_path" ]]; then
    echo "package lifecycle script is missing or not executable: $script" >&2
    exit 1
  fi
  if ! cmp -s "$script_path" "$reviewed_script"; then
    echo "package lifecycle script differs from the reviewed source: $script" >&2
    exit 1
  fi
  unexpected_xattrs="$(
    xattr "$script_path" 2>/dev/null \
      | grep -v '^com[.]apple[.]provenance$' || true
  )"
  if [[ -n "$unexpected_xattrs" ]]; then
    echo "package lifecycle script contains an unexpected extended attribute: $script" >&2
    exit 1
  fi
  metadata_file="$scripts_dir/._$script"
  if [[ -e "$metadata_file" ]]; then
    if [[ ! -f "$metadata_file" || -L "$metadata_file" || -x "$metadata_file" \
      || "$(file -b "$metadata_file")" != AppleDouble* \
      || "$(stat -f '%z' "$metadata_file")" -gt 4096 ]]; then
      echo "package lifecycle metadata is unexpected: ._$script" >&2
      exit 1
    fi
  fi
done

unexpected_scripts="$(
  find "$scripts_dir" -mindepth 1 -maxdepth 1 \
    ! -path "$scripts_dir/preinstall" \
    ! -path "$scripts_dir/postinstall" \
    ! -path "$scripts_dir/._preinstall" \
    ! -path "$scripts_dir/._postinstall" -print
)"
if [[ -n "$unexpected_scripts" ]]; then
  echo "package contains unexpected lifecycle scripts:" >&2
  echo "$unexpected_scripts" >&2
  exit 1
fi
if find "$payload_dir" -type l -print -quit | grep -q .; then
  echo "package payload must not contain symbolic links" >&2
  exit 1
fi
if find "$bundle_dir" \( -name '.DS_Store' -o -name '._*' \) -print -quit \
  | grep -q .; then
  echo "package bundle contains macOS metadata sidecar files" >&2
  exit 1
fi

unexpected_payload="$(
  find "$payload_dir" -mindepth 1 -maxdepth 1 ! -path "$payload_dir/Library" -print
  find "$payload_dir/Library" -mindepth 1 -maxdepth 1 \
    ! -path "$payload_dir/Library/Input Methods" -print
  find "$payload_dir/Library/Input Methods" -mindepth 1 -maxdepth 1 \
    ! -path "$bundle_dir" -print
)"
if [[ -n "$unexpected_payload" ]]; then
  echo "extracted package contains payload outside the expected bundle:" >&2
  echo "$unexpected_payload" >&2
  exit 1
fi

info_plist="$bundle_dir/Contents/Info.plist"
main_executable="$bundle_dir/Contents/MacOS/Slime"
ffi_library="$bundle_dir/Contents/Frameworks/libslime_ffi.dylib"
uninstaller="$bundle_dir/Contents/Resources/uninstall-macos-system.sh"
for regular_file in "$info_plist" "$main_executable" "$ffi_library" "$uninstaller"; do
  if [[ ! -f "$regular_file" || -L "$regular_file" ]]; then
    echo "package bundle is missing a required regular file: $regular_file" >&2
    exit 1
  fi
done
if [[ ! -x "$main_executable" || ! -x "$uninstaller" ]]; then
  echo "package bundle contains a non-executable program or uninstaller" >&2
  exit 1
fi
if [[ "$(< "$bundle_dir/Contents/PkgInfo")" != "APPL????" ]]; then
  echo "package bundle contains invalid PkgInfo" >&2
  exit 1
fi
for resource in \
  InputMethodIcon.tiff \
  LICENSE.txt \
  MOZC_DICTIONARY_LICENSE.txt \
  English.lproj/InfoPlist.strings \
  Japanese.lproj/InfoPlist.strings; do
  if [[ ! -f "$bundle_dir/Contents/Resources/$resource" ]]; then
    echo "package bundle is missing a required resource: $resource" >&2
    exit 1
  fi
done
plutil -lint "$info_plist" >/dev/null

package_identifier="$(
  xmllint --xpath 'string(/pkg-info/@identifier)' "$expanded_dir/PackageInfo"
)"
package_version="$(
  xmllint --xpath 'string(/pkg-info/@version)' "$expanded_dir/PackageInfo"
)"
package_relocatable="$(
  xmllint --xpath 'string(/pkg-info/@relocatable)' "$expanded_dir/PackageInfo"
)"
bundle_identifier="$(plutil -extract CFBundleIdentifier raw -o - "$info_plist")"
bundle_version="$(plutil -extract CFBundleShortVersionString raw -o - "$info_plist")"
bundle_build_number="$(plutil -extract CFBundleVersion raw -o - "$info_plist")"

if [[ "$package_identifier" != "com.unvalley.inputmethod.Slime.pkg" ]]; then
  echo "unexpected package identifier: $package_identifier" >&2
  exit 1
fi
if [[ "$bundle_identifier" != "com.unvalley.inputmethod.Slime" ]]; then
  echo "unexpected bundle identifier: $bundle_identifier" >&2
  exit 1
fi
if [[ ! "$bundle_version" =~ ^[0-9]+([.][0-9]+){1,2}$ ]]; then
  echo "invalid bundle version: $bundle_version" >&2
  exit 1
fi
if [[ ! "$bundle_build_number" =~ ^[1-9][0-9]{0,17}$ ]]; then
  echo "invalid bundle build number: $bundle_build_number" >&2
  exit 1
fi
expected_package_version="$bundle_version.$bundle_build_number"
if [[ "$package_version" != "$expected_package_version" ]]; then
  echo "package version $package_version does not match $expected_package_version" >&2
  exit 1
fi
if [[ "$package_relocatable" != "false" ]]; then
  echo "package payload must not be relocatable" >&2
  exit 1
fi
if [[ "$(
  xmllint --xpath \
    'count(/pkg-info/strict-identifier/bundle[@id="com.unvalley.inputmethod.Slime"])' \
    "$expanded_dir/PackageInfo"
)" != "1" ]]; then
  echo "package does not enforce the input method bundle identifier" >&2
  exit 1
fi
if [[ "$(
  xmllint --xpath \
    'count(/pkg-info/upgrade-bundle/bundle[@id="com.unvalley.inputmethod.Slime"])' \
    "$expanded_dir/PackageInfo"
)" != "1" ]]; then
  echo "package does not atomically replace an older bundle" >&2
  exit 1
fi
if ! pkgutil --payload-files "$package_path" \
  | grep -q '^./Library/Input Methods/Slime.app/Contents/MacOS/Slime$'; then
  echo "package does not contain the input method executable at the expected path" >&2
  exit 1
fi

echo "macOS package structure verification passed"
