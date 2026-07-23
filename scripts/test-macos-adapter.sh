#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
test_binary="$workspace_dir/target/macos/adapter-tests"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS adapter tests can only run on macOS" >&2
  exit 1
fi

cargo build --manifest-path "$workspace_dir/Cargo.toml" --release -p slime-ffi
mkdir -p "$workspace_dir/target/macos"

swiftc \
  -swift-version 5 \
  -import-objc-header "$workspace_dir/crates/slime-ffi/include/slime_ffi.h" \
  -L "$workspace_dir/target/release" \
  -lslime_ffi \
  "$workspace_dir/platforms/macos/Sources/RustEngine.swift" \
  "$workspace_dir/platforms/macos/Sources/UserDataStore.swift" \
  "$workspace_dir/platforms/macos/Sources/DictionaryImporter.swift" \
  "$workspace_dir/platforms/macos/Sources/InputPrivacy.swift" \
  "$workspace_dir/platforms/macos/Sources/KeyEventMapping.swift" \
  "$workspace_dir/platforms/macos/Sources/TextClientActions.swift" \
  "$workspace_dir/platforms/macos/Sources/CandidatePanel.swift" \
  "$workspace_dir/platforms/macos/Tests/AdapterTests.swift" \
  -o "$test_binary"

DYLD_LIBRARY_PATH="$workspace_dir/target/release" "$test_binary"
