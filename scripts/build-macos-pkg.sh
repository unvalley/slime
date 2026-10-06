#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
bundle_dir="$workspace_dir/target/macos/Slime.app"
package_dir="$workspace_dir/target/package"
package_scripts="$workspace_dir/platforms/macos/PackageScripts"
component_plist="$workspace_dir/platforms/macos/PackageComponent.plist"
installer_identity="${SLIME_INSTALLER_IDENTITY:-}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS package can only be built on macOS" >&2
  exit 1
fi
if [[ "$installer_identity" != "Developer ID Installer:"* ]]; then
  echo "SLIME_INSTALLER_IDENTITY=Developer ID Installer: ... is required" >&2
  exit 1
fi
if ! codesign -dv --verbose=4 "$bundle_dir" 2>&1 \
  | grep -q '^Authority=Developer ID Application:'; then
  echo "Slime.app must have a Developer ID Application signature" >&2
  exit 1
fi
for script in preinstall postinstall; do
  if [[ ! -x "$package_scripts/$script" ]]; then
    echo "Missing executable package script: $package_scripts/$script" >&2
    exit 1
  fi
done
if ! plutil -lint "$component_plist" >/dev/null; then
  echo "Invalid package component metadata: $component_plist" >&2
  exit 1
fi
"$workspace_dir/scripts/verify-macos-bundle.sh" --release

version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \
  "$bundle_dir/Contents/Info.plist")"
build_number="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' \
  "$bundle_dir/Contents/Info.plist")"
if [[ ! "$version" =~ ^[0-9]+([.][0-9]+){1,2}$ ]]; then
  echo "Unsupported bundle version: $version" >&2
  exit 1
fi
if [[ ! "$build_number" =~ ^[1-9][0-9]{0,17}$ ]]; then
  echo "Unsupported bundle build number: $build_number" >&2
  exit 1
fi
workspace_version="$(
  cargo metadata --manifest-path "$workspace_dir/Cargo.toml" \
    --no-deps --format-version 1 \
    | plutil -extract 'packages.0.version' raw -o - -
)"
if [[ "$version" != "$workspace_version" ]]; then
  echo "Bundle version $version does not match workspace version $workspace_version" >&2
  exit 1
fi

work_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT
staging_dir="$work_dir/root"
install_dir="$staging_dir/Library/Input Methods"
normalized_scripts="$work_dir/package-scripts"
mkdir -p "$install_dir" "$package_dir" "$normalized_scripts"
ditto "$bundle_dir" "$install_dir/Slime.app"
for script in preinstall postinstall; do
  cp "$package_scripts/$script" "$normalized_scripts/$script"
  xattr -c "$normalized_scripts/$script"
done
xattr -cr "$staging_dir"

unexpected_payload="$(
  find "$staging_dir" -mindepth 1 -maxdepth 1 \
    ! -path "$staging_dir/Library" -print
  find "$staging_dir/Library" -mindepth 1 -maxdepth 1 \
    ! -path "$staging_dir/Library/Input Methods" -print
  find "$staging_dir/Library/Input Methods" -mindepth 1 -maxdepth 1 \
    ! -path "$install_dir/Slime.app" -print
)"
if [[ -n "$unexpected_payload" ]]; then
  echo "Refusing package payload outside the expected input method bundle:" >&2
  echo "$unexpected_payload" >&2
  exit 1
fi

package_version="$version.$build_number"
output="$package_dir/Slime-$package_version-macos.pkg"
pkgbuild \
  --root "$staging_dir" \
  --identifier com.unvalley.inputmethod.Slime.pkg \
  --version "$package_version" \
  --install-location / \
  --component-plist "$component_plist" \
  --scripts "$normalized_scripts" \
  --sign "$installer_identity" \
  "$output"

app_signature="$(codesign -dv --verbose=4 "$bundle_dir" 2>&1)"
team_identifier="$(sed -n 's/^TeamIdentifier=//p' <<<"$app_signature" | head -n 1)"
package_signature="$(pkgutil --check-signature "$output")"
if [[ -z "$team_identifier" || "$team_identifier" == "not set" \
  || "$package_signature" != *"Developer ID Installer:"* \
  || "$package_signature" != *"($team_identifier)"* ]]; then
  echo "package installer signature does not match the app signing team" >&2
  exit 1
fi
printf '%s\n' "$package_signature"
shasum -a 256 "$output"
echo "Built signed package $output"
