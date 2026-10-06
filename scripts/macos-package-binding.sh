#!/usr/bin/env bash

slime_sha256_file() {
  /usr/bin/shasum -a 256 "$1" | /usr/bin/awk '{print $1}'
}

slime_bundle_manifest() {
  local bundle="$1"

  if [[ ! -d "$bundle" ]]; then
    echo "input method bundle is missing: $bundle" >&2
    return 1
  fi
  if /usr/bin/find "$bundle" -type l -print -quit | /usr/bin/grep -q .; then
    echo "input method bundle contains a symbolic link: $bundle" >&2
    return 1
  fi

  (
    cd "$bundle"
    /usr/bin/find . -type f -print | LC_ALL=C /usr/bin/sort \
      | while IFS= read -r relative_path; do
          /usr/bin/shasum -a 256 "$relative_path"
        done
  )
}

slime_bundle_manifest_sha256() {
  slime_bundle_manifest "$1" | /usr/bin/shasum -a 256 | /usr/bin/awk '{print $1}'
}

slime_package_bundle_manifest_sha256() (
  set -euo pipefail

  local package_path="$1"
  local inspection_dir expanded_dir payload_dir bundle_dir

  if [[ ! -f "$package_path" || -L "$package_path" ]]; then
    echo "release package must be a regular file: $package_path" >&2
    exit 1
  fi

  inspection_dir="$(/usr/bin/mktemp -d)"
  cleanup() {
    /bin/rm -rf "$inspection_dir"
  }
  trap cleanup EXIT

  expanded_dir="$inspection_dir/expanded"
  payload_dir="$inspection_dir/payload"
  /usr/sbin/pkgutil --expand "$package_path" "$expanded_dir"
  /bin/mkdir -p "$payload_dir"
  /usr/bin/ditto -x "$expanded_dir/Payload" "$payload_dir"
  bundle_dir="$payload_dir/Library/Input Methods/Slime.app"
  slime_bundle_manifest_sha256 "$bundle_dir"
)

slime_assert_installed_bundle_matches_package() {
  local package_path="$1"
  local installed_bundle="$2"
  local package_manifest_sha256 installed_manifest_sha256

  package_manifest_sha256="$(slime_package_bundle_manifest_sha256 "$package_path")"
  installed_manifest_sha256="$(slime_bundle_manifest_sha256 "$installed_bundle")"
  if [[ "$package_manifest_sha256" != "$installed_manifest_sha256" ]]; then
    echo "installed input method does not match the release package payload" >&2
    return 1
  fi
}

slime_macos_release_binding_json() {
  local version="$1"
  local build="$2"
  local source_revision="$3"
  local package_sha256="$4"
  local bundle_manifest_sha256="$5"

  [[ "$version" =~ ^[0-9]+([.][0-9]+){1,2}$ ]] || return 1
  [[ "$build" =~ ^[1-9][0-9]{0,17}$ ]] || return 1
  [[ "$source_revision" =~ ^[0-9a-f]{40}$ ]] || return 1
  [[ "$package_sha256" =~ ^[0-9a-f]{64}$ ]] || return 1
  [[ "$bundle_manifest_sha256" =~ ^[0-9a-f]{64}$ ]] || return 1

  /usr/bin/printf \
    '{"Schema":1,"Platform":"macOS","Version":"%s","Build":"%s","SourceRevision":"%s","PackageSha256":"%s","BundleManifestSha256":"%s"}\n' \
    "$version" \
    "$build" \
    "$source_revision" \
    "$package_sha256" \
    "$bundle_manifest_sha256"
}
