#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
preview_binary="$workspace_dir/target/macos/settings-preview"

swiftc \
  -swift-version 5 \
  -framework AppKit \
  -framework Security \
  -framework SwiftUI \
  "$workspace_dir/platforms/macos/Sources/UserDataStore.swift" \
  "$workspace_dir/platforms/macos/Sources/DictionaryImporter.swift" \
  "$workspace_dir/platforms/macos/Sources/InputPrivacy.swift" \
  "$workspace_dir/platforms/macos/Sources/Licensing.swift" \
  "$workspace_dir/platforms/macos/Sources/LicenseSettingsView.swift" \
  "$workspace_dir/platforms/macos/Sources/SettingsWindow.swift" \
  "$workspace_dir/platforms/macos/Tests/SettingsPreview.swift" \
  -o "$preview_binary"

exec "$preview_binary"
