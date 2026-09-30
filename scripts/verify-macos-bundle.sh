#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
bundle_dir="$workspace_dir/target/macos/Slime.app"
executable="$bundle_dir/Contents/MacOS/Slime"
ffi_library="$bundle_dir/Contents/Frameworks/libslime_ffi.dylib"
mode="${1:---development}"

if [[ "$mode" != "--development" && "$mode" != "--release" ]]; then
  echo "usage: scripts/verify-macos-bundle.sh [--development|--release]" >&2
  exit 64
fi

test -x "$executable"
test ! -L "$executable"
test -f "$ffi_library"
test ! -L "$ffi_library"
test -f "$bundle_dir/Contents/Resources/InputMethodIcon.tiff"
test -f "$bundle_dir/Contents/Resources/MOZC_DICTIONARY_LICENSE.txt"
test -f "$bundle_dir/Contents/Resources/LICENSE.txt"
test -f "$bundle_dir/Contents/Resources/English.lproj/InfoPlist.strings"
test -f "$bundle_dir/Contents/Resources/Japanese.lproj/InfoPlist.strings"
test -x "$bundle_dir/Contents/Resources/uninstall-macos-system.sh"
test "$(wc -c < "$bundle_dir/Contents/PkgInfo" | tr -d ' ')" = "8"
test "$(< "$bundle_dir/Contents/PkgInfo")" = "APPL????"
plutil -lint "$bundle_dir/Contents/Info.plist"
codesign --verify --deep --strict "$bundle_dir"
codesign --verify --strict "$ffi_library"

neural_resource="$(plutil -extract SlimeNeuralModelResource raw -o - \
  "$bundle_dir/Contents/Info.plist" 2>/dev/null || true)"
if [[ -n "$neural_resource" ]]; then
  if [[ "$neural_resource" != "SlimeNeuralModel.gguf" ]] \
    || [[ ! -f "$bundle_dir/Contents/Resources/$neural_resource" ]]; then
    echo "neural bundle lacks its fixed local model resource" >&2
    exit 1
  fi
  neural_live_enabled="$(plutil -extract SlimeNeuralLiveRerankingEnabled raw -o - \
    "$bundle_dir/Contents/Info.plist" 2>/dev/null || true)"
  neural_live_debounce="$(plutil -extract SlimeNeuralLiveDebounceMilliseconds raw -o - \
    "$bundle_dir/Contents/Info.plist" 2>/dev/null || true)"
  neural_live_margin="$(plutil -extract SlimeNeuralLiveMinSwitchMargin raw -o - \
    "$bundle_dir/Contents/Info.plist" 2>/dev/null || true)"
  neural_live_long_margin="$(plutil -extract SlimeNeuralLiveLongReadingMinSwitchMargin raw -o - \
    "$bundle_dir/Contents/Info.plist" 2>/dev/null || true)"
  neural_live_numeric_margin="$(plutil -extract SlimeNeuralLiveNumericBaseSwitchMargin raw -o - \
    "$bundle_dir/Contents/Info.plist" 2>/dev/null || true)"
  neural_live_long_lambda="$(plutil -extract SlimeNeuralLiveLongReadingLambda raw -o - \
    "$bundle_dir/Contents/Info.plist" 2>/dev/null || true)"
  if [[ "$neural_live_enabled" != "true" && "$neural_live_enabled" != "false" ]] \
    || [[ ! "$neural_live_debounce" =~ ^[0-9]+$ ]] \
    || (( neural_live_debounce < 50 || neural_live_debounce > 1000 )) \
    || [[ ! "$neural_live_margin" =~ ^([0-9]+([.][0-9]+)?|[.][0-9]+)$ ]] \
    || [[ ! "$neural_live_long_margin" =~ ^([0-9]+([.][0-9]+)?|[.][0-9]+)$ ]] \
    || [[ ! "$neural_live_numeric_margin" =~ ^([0-9]+([.][0-9]+)?|[.][0-9]+)$ ]] \
    || [[ ! "$neural_live_long_lambda" =~ ^(0([.][0-9]+)?|1([.]0+)?)$ ]]; then
    echo "neural bundle has an invalid delayed LIVE policy" >&2
    exit 1
  fi
fi

if find "$bundle_dir" -type l -print -quit | grep -q .; then
  echo "input method bundle must not contain symbolic links" >&2
  exit 1
fi

entitlements="$(codesign -d --entitlements - "$bundle_dir" 2>/dev/null)"
if [[ "$mode" == "--development" ]]; then
  if [[ "$entitlements" != *"com.apple.security.get-task-allow"* ]]; then
    echo "development input method entitlement is missing" >&2
    exit 1
  fi
else
  source_revision="$(plutil -extract SlimeSourceRevision raw -o - \
    "$bundle_dir/Contents/Info.plist" 2>/dev/null || true)"
  if [[ ! "$source_revision" =~ ^[0-9a-f]{40}$ ]]; then
    echo "release bundle lacks a valid source revision" >&2
    exit 1
  fi
  signature="$(codesign -dv --verbose=4 "$bundle_dir" 2>&1)"
  ffi_signature="$(codesign -dv --verbose=4 "$ffi_library" 2>&1)"
  if [[ "$signature" != *"Authority=Developer ID Application:"* ]]; then
    echo "release bundle lacks a Developer ID Application authority" >&2
    exit 1
  fi
  if [[ "$signature" != *"flags="*"runtime"* ]]; then
    echo "release bundle lacks hardened runtime" >&2
    exit 1
  fi
  team_identifier="$(sed -n 's/^TeamIdentifier=//p' <<<"$signature" | head -n 1)"
  ffi_team_identifier="$(sed -n 's/^TeamIdentifier=//p' <<<"$ffi_signature" | head -n 1)"
  if [[ -z "$team_identifier" || "$team_identifier" == "not set" ]]; then
    echo "release bundle lacks a signing team identifier" >&2
    exit 1
  fi
  if [[ "$ffi_signature" != *"Authority=Developer ID Application:"* ]] \
    || [[ "$ffi_signature" != *"flags="*"runtime"* ]] \
    || [[ "$ffi_team_identifier" != "$team_identifier" ]]; then
    echo "embedded dylib does not share the bundle's Developer ID runtime signature" >&2
    exit 1
  fi
  ffi_entitlements="$(codesign -d --entitlements - "$ffi_library" 2>/dev/null || true)"
  if grep -Eq 'com[.]apple[.]security[.](get-task-allow|cs[.])' \
    <<<"$entitlements$ffi_entitlements"; then
    echo "release bundle contains a development or code-signing exception entitlement" >&2
    exit 1
  fi
fi

if ! otool -L "$executable" | grep -q '@rpath/libslime_ffi.dylib'; then
  echo "embedded Rust dylib is not linked through @rpath" >&2
  exit 1
fi

echo "macOS input method bundle verification passed"
