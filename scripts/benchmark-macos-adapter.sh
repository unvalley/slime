#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
benchmark_binary="$workspace_dir/target/macos/adapter-performance-benchmarks"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS adapter benchmarks can only run on macOS" >&2
  exit 1
fi

cargo build --manifest-path "$workspace_dir/Cargo.toml" --release -p ime-ffi
mkdir -p "$workspace_dir/target/macos"

swiftc \
  -O \
  -swift-version 5 \
  -import-objc-header "$workspace_dir/crates/ime-ffi/include/ime_ffi.h" \
  -L "$workspace_dir/target/release" \
  -lime_ffi \
  "$workspace_dir/platforms/macos/Sources/RustEngine.swift" \
  "$workspace_dir/platforms/macos/Tests/AdapterPerformanceBenchmarks.swift" \
  -o "$benchmark_binary"

DYLD_LIBRARY_PATH="$workspace_dir/target/release" "$benchmark_binary"
