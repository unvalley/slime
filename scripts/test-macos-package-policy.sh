#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
bundle_dir="$workspace_dir/target/macos/Slime.app"
component_plist="$workspace_dir/platforms/macos/PackageComponent.plist"
reviewed_scripts="$workspace_dir/platforms/macos/PackageScripts"
verifier="$workspace_dir/scripts/verify-macos-package-structure.sh"
# shellcheck source=macos-package-binding.sh
source "$workspace_dir/scripts/macos-package-binding.sh"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS package policy testing can only run on macOS" >&2
  exit 1
fi
if [[ ! -d "$bundle_dir" ]]; then
  echo "build the macOS bundle before testing package policy" >&2
  exit 1
fi

version="$(plutil -extract CFBundleShortVersionString raw -o - \
  "$bundle_dir/Contents/Info.plist")"
build_number="$(plutil -extract CFBundleVersion raw -o - \
  "$bundle_dir/Contents/Info.plist")"
package_version="$version.$build_number"
work_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT

prepare_root() {
  local name="$1"
  local root="$work_dir/$name-root"
  mkdir -p "$root/Library/Input Methods"
  ditto "$bundle_dir" "$root/Library/Input Methods/Slime.app"
  xattr -cr "$root"
  echo "$root"
}

prepare_scripts() {
  local name="$1"
  local scripts="$work_dir/$name-scripts"
  mkdir -p "$scripts"
  cp "$reviewed_scripts/preinstall" "$reviewed_scripts/postinstall" "$scripts/"
  xattr -c "$scripts/preinstall" "$scripts/postinstall"
  echo "$scripts"
}

build_package() {
  local root="$1"
  local scripts="$2"
  local version_override="$3"
  local output="$4"
  pkgbuild \
    --root "$root" \
    --identifier com.unvalley.inputmethod.Slime.pkg \
    --version "$version_override" \
    --install-location / \
    --component-plist "$component_plist" \
    --scripts "$scripts" \
    "$output" >/dev/null
}

assert_rejected() {
  local package="$1"
  local expected_message="$2"
  local output
  if output="$("$verifier" "$package" 2>&1)"; then
    echo "package policy unexpectedly accepted: $package" >&2
    exit 1
  fi
  if ! grep -q "$expected_message" <<<"$output"; then
    echo "package policy failed for an unexpected reason:" >&2
    echo "$output" >&2
    exit 1
  fi
}

baseline_root="$(prepare_root baseline)"
baseline_scripts="$(prepare_scripts baseline)"
baseline_package="$work_dir/baseline.pkg"
build_package "$baseline_root" "$baseline_scripts" "$package_version" "$baseline_package"
"$verifier" "$baseline_package"
slime_assert_installed_bundle_matches_package "$baseline_package" "$bundle_dir"

changed_bundle="$work_dir/changed-Slime.app"
ditto "$bundle_dir" "$changed_bundle"
printf '\nchanged fixture\n' >>"$changed_bundle/Contents/Resources/LICENSE.txt"
if binding_output="$(
  slime_assert_installed_bundle_matches_package "$baseline_package" "$changed_bundle" 2>&1
)"; then
  echo "package binding unexpectedly accepted a changed installed bundle" >&2
  exit 1
fi
if ! grep -q 'does not match the release package payload' <<<"$binding_output"; then
  echo "package binding failed for an unexpected reason:" >&2
  echo "$binding_output" >&2
  exit 1
fi

modified_scripts="$(prepare_scripts modified)"
printf '\n# modified fixture\n' >>"$modified_scripts/postinstall"
modified_package="$work_dir/modified-script.pkg"
build_package "$baseline_root" "$modified_scripts" "$package_version" "$modified_package"
assert_rejected "$modified_package" 'differs from the reviewed source'

extra_root="$(prepare_root extra)"
printf 'unexpected\n' >"$extra_root/Library/unexpected-payload"
extra_package="$work_dir/extra-payload.pkg"
build_package "$extra_root" "$baseline_scripts" "$package_version" "$extra_package"
assert_rejected "$extra_package" 'payload outside the expected bundle'

symlink_root="$(prepare_root symlink)"
ln -s ../Info.plist \
  "$symlink_root/Library/Input Methods/Slime.app/Contents/Resources/InfoLink"
symlink_package="$work_dir/symlink.pkg"
build_package "$symlink_root" "$baseline_scripts" "$package_version" "$symlink_package"
assert_rejected "$symlink_package" 'must not contain symbolic links'

wrong_version_package="$work_dir/wrong-version.pkg"
build_package "$baseline_root" "$baseline_scripts" "$version.999999" \
  "$wrong_version_package"
assert_rejected "$wrong_version_package" 'does not match'

echo "macOS package policy regression tests passed"
