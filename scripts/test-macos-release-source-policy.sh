#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=macos-release-source.sh
source "$workspace_dir/scripts/macos-release-source.sh"

test_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$test_dir"
}
trap cleanup EXIT

repository="$test_dir/repository"
mkdir -p "$repository"
git -C "$repository" init --quiet
git -C "$repository" config user.name "Release Policy Fixture"
git -C "$repository" config user.email "release-policy@example.invalid"
printf 'committed\n' >"$repository/source.txt"
git -C "$repository" add source.txt
git -C "$repository" commit --quiet -m "test: add release source fixture"

expected_revision="$(git -C "$repository" rev-parse HEAD)"
actual_revision="$(slime_release_source_revision "$repository")"
if [[ "$actual_revision" != "$expected_revision" ]]; then
  echo "clean release source returned the wrong revision" >&2
  exit 1
fi

printf 'changed\n' >>"$repository/source.txt"
if slime_release_source_revision "$repository" >/dev/null 2>&1; then
  echo "release source policy accepted a tracked modification" >&2
  exit 1
fi
git -C "$repository" restore source.txt

printf 'untracked\n' >"$repository/untracked.txt"
if slime_release_source_revision "$repository" >/dev/null 2>&1; then
  echo "release source policy accepted an untracked file" >&2
  exit 1
fi
rm "$repository/untracked.txt"

mkdir "$repository/nested"
if slime_release_source_revision "$repository/nested" >/dev/null 2>&1; then
  echo "release source policy accepted a nested build directory" >&2
  exit 1
fi
rmdir "$repository/nested"

echo "macOS release source policy test passed"
