#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=macos-package-binding.sh
source "$workspace_dir/scripts/macos-package-binding.sh"
current_package=""
previous_package=""

usage() {
  echo "usage: sudo -E scripts/test-macos-package-lifecycle.sh --current CURRENT.pkg [--previous PREVIOUS.pkg]" >&2
  exit 64
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --current)
      [[ "$#" -ge 2 ]] || usage
      current_package="$2"
      shift 2
      ;;
    --previous)
      [[ "$#" -ge 2 ]] || usage
      previous_package="$2"
      shift 2
      ;;
    *) usage ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS package lifecycle testing requires macOS" >&2
  exit 1
fi
if [[ "${SLIME_RELEASE_TEST_VM:-}" != "1" ]]; then
  echo "refusing to modify the system outside an explicit disposable release-test VM" >&2
  echo "set SLIME_RELEASE_TEST_VM=1 only inside that VM" >&2
  exit 77
fi
if [[ "$EUID" -ne 0 ]]; then
  echo "package lifecycle testing must run as root through sudo -E" >&2
  exit 77
fi
if [[ -z "$current_package" || ! -f "$current_package" ]]; then
  usage
fi
if [[ -n "$previous_package" && ! -f "$previous_package" ]]; then
  usage
fi

current_package="$(cd "$(dirname "$current_package")" && pwd)/$(basename "$current_package")"
if [[ -n "$previous_package" ]]; then
  previous_package="$(cd "$(dirname "$previous_package")" && pwd)/$(basename "$previous_package")"
fi

test_user="${SLIME_RELEASE_TEST_USER:-${SUDO_USER:-}}"
if [[ -z "$test_user" || "$test_user" == "root" ]] || ! /usr/bin/id "$test_user" >/dev/null 2>&1; then
  echo "SLIME_RELEASE_TEST_USER must name the disposable VM's non-root test user" >&2
  exit 77
fi
test_home="$(/usr/bin/dscl . -read "/Users/$test_user" NFSHomeDirectory | /usr/bin/awk '{print $2}')"
if [[ -z "$test_home" || "$test_home" == "/var/root" || ! -d "$test_home" ]]; then
  echo "could not resolve a safe home directory for $test_user" >&2
  exit 77
fi
test_group="$(/usr/bin/id -gn "$test_user")"
system_bundle="/Library/Input Methods/Slime.app"
user_bundle="$test_home/Library/Input Methods/Slime.app"
package_identifier="com.unvalley.inputmethod.Slime.pkg"
user_data_dir="$test_home/Library/Application Support/Slime"
sentinel="$user_data_dir/release-lifecycle-sentinel"
stale_file="$system_bundle/Contents/Resources/release-lifecycle-stale-file"

if [[ -e "$system_bundle" || -e "$user_bundle" ]]; then
  echo "VM is not clean: an input method bundle is already installed" >&2
  exit 77
fi
if /usr/sbin/pkgutil --pkg-info "$package_identifier" >/dev/null 2>&1; then
  echo "VM is not clean: package receipt already exists" >&2
  exit 77
fi
if [[ -e "$sentinel" ]]; then
  echo "VM is not clean: lifecycle sentinel already exists" >&2
  exit 77
fi

inspection_dir="$(mktemp -d)"
created_user_data=0
cleanup() {
  "$workspace_dir/scripts/uninstall-macos-system.sh" >/dev/null 2>&1 || true
  if [[ -f "$sentinel" ]]; then
    /bin/rm -f "$sentinel"
  fi
  if [[ "$created_user_data" -eq 1 && -d "$user_data_dir" ]]; then
    /usr/bin/rmdir "$user_data_dir" >/dev/null 2>&1 || true
  fi
  /bin/rm -rf "$inspection_dir"
}
trap cleanup EXIT

extract_bundle() {
  local package="$1"
  local name="$2"
  local expanded="$inspection_dir/$name-expanded"
  local payload="$inspection_dir/$name-payload"
  /usr/sbin/pkgutil --expand "$package" "$expanded"
  /bin/mkdir -p "$payload"
  /usr/bin/ditto -x "$expanded/Payload" "$payload"
  echo "$payload/Library/Input Methods/Slime.app"
}

bundle_version() {
  /usr/bin/plutil -extract CFBundleShortVersionString raw -o - \
    "$1/Contents/Info.plist"
}

bundle_build_number() {
  /usr/bin/plutil -extract CFBundleVersion raw -o - \
    "$1/Contents/Info.plist"
}

bundle_source_revision() {
  /usr/bin/plutil -extract SlimeSourceRevision raw -o - \
    "$1/Contents/Info.plist"
}

version_is_at_least() {
  local newer="$1"
  local older="$2"
  local newer_major newer_minor newer_patch
  local older_major older_minor older_patch
  IFS=. read -r newer_major newer_minor newer_patch <<<"$newer"
  IFS=. read -r older_major older_minor older_patch <<<"$older"
  newer_patch="${newer_patch:-0}"
  older_patch="${older_patch:-0}"
  ((
    newer_major > older_major
      || (newer_major == older_major && newer_minor > older_minor)
      || (newer_major == older_major && newer_minor == older_minor
        && newer_patch >= older_patch)
  ))
}

bundle_manifest() {
  local bundle="$1"
  (
    cd "$bundle"
    find . -type f -print | LC_ALL=C sort | while IFS= read -r relative_path; do
      /usr/bin/shasum -a 256 "$relative_path"
    done
  )
}

assert_installed_matches() {
  local expected_bundle="$1"
  local expected_version="$2"
  local expected_build_number="$3"
  local expected_source_revision="$4"
  local installed_version installed_build_number installed_source_revision
  local expected_manifest installed_manifest
  installed_version="$(bundle_version "$system_bundle")"
  if [[ "$installed_version" != "$expected_version" ]]; then
    echo "installed version $installed_version does not match $expected_version" >&2
    exit 1
  fi
  installed_build_number="$(bundle_build_number "$system_bundle")"
  if [[ "$installed_build_number" != "$expected_build_number" ]]; then
    echo "installed build $installed_build_number does not match $expected_build_number" >&2
    exit 1
  fi
  installed_source_revision="$(bundle_source_revision "$system_bundle")"
  if [[ "$installed_source_revision" != "$expected_source_revision" ]]; then
    echo "installed source revision does not match the package payload" >&2
    exit 1
  fi
  if find "$expected_bundle" "$system_bundle" -type l -print -quit | grep -q .; then
    echo "expected or installed bundle contains a symbolic link" >&2
    exit 1
  fi
  /usr/bin/codesign --verify --deep --strict --verbose=2 "$system_bundle"
  expected_manifest="$(bundle_manifest "$expected_bundle")"
  installed_manifest="$(bundle_manifest "$system_bundle")"
  if [[ "$expected_manifest" != "$installed_manifest" ]]; then
    echo "installed bundle file manifest differs from package payload" >&2
    diff -u <(printf '%s\n' "$expected_manifest") \
      <(printf '%s\n' "$installed_manifest") >&2 || true
    exit 1
  fi
  if [[ "$(/usr/bin/stat -f '%Su:%Sg' "$system_bundle")" != "root:wheel" ]]; then
    echo "installed bundle is not owned by root:wheel" >&2
    exit 1
  fi
  receipt_version="$(
    /usr/sbin/pkgutil --pkg-info-plist "$package_identifier" \
      | /usr/bin/plutil -extract pkg-version raw -o - -
  )"
  expected_package_version="$expected_version.$expected_build_number"
  if [[ "$receipt_version" != "$expected_package_version" ]]; then
    echo "package receipt version $receipt_version does not match $expected_package_version" >&2
    exit 1
  fi
}

"$workspace_dir/scripts/verify-macos-release.sh" "$current_package"
current_bundle="$(extract_bundle "$current_package" current)"
current_version="$(bundle_version "$current_bundle")"
current_build_number="$(bundle_build_number "$current_bundle")"
current_source_revision="$(bundle_source_revision "$current_bundle")"
current_package_sha256="$(/usr/bin/shasum -a 256 "$current_package" | /usr/bin/awk '{print $1}')"

if [[ ! -d "$user_data_dir" ]]; then
  /usr/bin/install -d -o "$test_user" -g "$test_group" "$user_data_dir"
  created_user_data=1
fi
/usr/bin/install -o "$test_user" -g "$test_group" -m 600 /dev/null "$sentinel"
/usr/bin/printf '%s\n' preserved >"$sentinel"

if [[ -n "$previous_package" ]]; then
  "$workspace_dir/scripts/verify-macos-release.sh" "$previous_package"
  previous_bundle="$(extract_bundle "$previous_package" previous)"
  previous_version="$(bundle_version "$previous_bundle")"
  previous_build_number="$(bundle_build_number "$previous_bundle")"
  previous_source_revision="$(bundle_source_revision "$previous_bundle")"
  if ! version_is_at_least "$current_version" "$previous_version"; then
    echo "current version $current_version must not be older than $previous_version" >&2
    exit 1
  fi
  if ((current_build_number <= previous_build_number)); then
    echo "current build $current_build_number must be newer than $previous_build_number" >&2
    exit 1
  fi
  /usr/sbin/installer -pkg "$previous_package" -target /
  assert_installed_matches \
    "$previous_bundle" "$previous_version" "$previous_build_number" \
    "$previous_source_revision"
  /usr/bin/touch "$stale_file"
fi

/usr/sbin/installer -pkg "$current_package" -target /
assert_installed_matches \
  "$current_bundle" "$current_version" "$current_build_number" \
  "$current_source_revision"
if [[ -e "$stale_file" ]]; then
  echo "a stale file survived the package update" >&2
  exit 1
fi
if [[ "$(/bin/cat "$sentinel")" != "preserved" ]]; then
  echo "user data changed during install or update" >&2
  exit 1
fi
current_installed_manifest_sha256="$(
  slime_bundle_manifest_sha256 "$system_bundle"
)"

# Opening the same downloaded release twice must be idempotent. Re-running the
# current installer may replace the bundle, but it must preserve the exact
# installed manifest and user data.
/usr/sbin/installer -pkg "$current_package" -target /
assert_installed_matches \
  "$current_bundle" "$current_version" "$current_build_number" \
  "$current_source_revision"
if [[ "$(slime_bundle_manifest_sha256 "$system_bundle")" != \
  "$current_installed_manifest_sha256" ]]; then
  echo "same-version reinstall changed the installed bundle manifest" >&2
  exit 1
fi
if [[ "$(/bin/cat "$sentinel")" != "preserved" ]]; then
  echo "user data changed during same-version reinstall" >&2
  exit 1
fi

if [[ -n "$previous_package" ]]; then
  # A user can keep an older package and open it after updating. The package
  # metadata should refuse or no-op that downgrade; either installer exit
  # status is acceptable, but the installed bundle, receipt, and user data
  # must remain at the current release.
  set +e
  /usr/sbin/installer -pkg "$previous_package" -target /
  downgrade_status="$?"
  set -e
  assert_installed_matches \
    "$current_bundle" "$current_version" "$current_build_number" \
    "$current_source_revision"
  if [[ "$(/bin/cat "$sentinel")" != "preserved" ]]; then
    echo "user data changed during a rejected downgrade" >&2
    exit 1
  fi
  echo "macOS downgrade resistance passed (installer status $downgrade_status)"
fi

"$workspace_dir/scripts/uninstall-macos-system.sh"
if [[ -e "$system_bundle" ]]; then
  echo "system bundle remains after uninstall" >&2
  exit 1
fi
if /usr/sbin/pkgutil --pkg-info "$package_identifier" >/dev/null 2>&1; then
  echo "package receipt remains after uninstall" >&2
  exit 1
fi
if [[ "$(/bin/cat "$sentinel")" != "preserved" ]]; then
  echo "user data was not preserved by uninstall" >&2
  exit 1
fi
if [[ "$(/usr/bin/stat -f '%Su' "$sentinel")" != "$test_user" ]]; then
  echo "user data ownership changed during lifecycle test" >&2
  exit 1
fi

echo "macOS package install/update/uninstall gate passed for $current_version ($current_build_number)"
echo "Source revision: $current_source_revision"
echo "Release package SHA-256: $current_package_sha256"
echo "Release binding JSON: $(
  slime_macos_release_binding_json \
    "$current_version" \
    "$current_build_number" \
    "$current_source_revision" \
    "$current_package_sha256" \
    "$current_installed_manifest_sha256"
)"
echo "A post-login input-source menu check remains a separate consumer gate."
