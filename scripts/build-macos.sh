#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
bundle_dir="$workspace_dir/target/macos/Slime.app"
contents_dir="$bundle_dir/Contents"
macos_dir="$contents_dir/MacOS"
frameworks_dir="$contents_dir/Frameworks"
resources_dir="$contents_dir/Resources"
executable="$macos_dir/Slime"
codesign_identity="${SLIME_CODESIGN_IDENTITY:-}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS bundle can only be built on macOS" >&2
  exit 1
fi

case "$bundle_dir" in
  "$workspace_dir/target/macos/Slime.app") ;;
  *) echo "refusing to replace unexpected path: $bundle_dir" >&2; exit 1 ;;
esac

if [[ -z "$codesign_identity" ]]; then
  codesign_identity="$(security find-identity -v -p codesigning \
    | sed -n 's/.*"\(Apple Development:.*\)"/\1/p' \
    | head -n 1)"
fi
if [[ -z "$codesign_identity" ]]; then
  codesign_identity="-"
  echo "No trusted code-signing identity found; building with an ad-hoc signature."
else
  echo "Signing with: $codesign_identity"
fi

MACOSX_DEPLOYMENT_TARGET=13.0 cargo build --manifest-path "$workspace_dir/Cargo.toml" --release -p slime-ffi

rm -rf "$bundle_dir"
mkdir -p "$macos_dir" "$frameworks_dir" "$resources_dir"

swiftc \
  -swift-version 5 \
  -module-name Slime \
  -import-objc-header "$workspace_dir/crates/slime-ffi/include/slime_ffi.h" \
  -framework AppKit \
  -framework InputMethodKit \
  -framework SwiftUI \
  -L "$workspace_dir/target/release" \
  -lslime_ffi \
  -Xlinker -rpath \
  -Xlinker @executable_path/../Frameworks \
  "$workspace_dir/platforms/macos/Sources/RustEngine.swift" \
  "$workspace_dir/platforms/macos/Sources/UserDataStore.swift" \
  "$workspace_dir/platforms/macos/Sources/DictionaryImporter.swift" \
  "$workspace_dir/platforms/macos/Sources/InputPrivacy.swift" \
  "$workspace_dir/platforms/macos/Sources/KeyEventMapping.swift" \
  "$workspace_dir/platforms/macos/Sources/TextClientActions.swift" \
  "$workspace_dir/platforms/macos/Sources/CandidatePanel.swift" \
  "$workspace_dir/platforms/macos/Sources/SettingsWindow.swift" \
  "$workspace_dir/platforms/macos/Sources/InputController.swift" \
  "$workspace_dir/platforms/macos/Sources/main.swift" \
  -o "$executable"

cp "$workspace_dir/target/release/libslime_ffi.dylib" "$frameworks_dir/"
cp "$workspace_dir/platforms/macos/Resources/Info.plist" "$contents_dir/Info.plist"
cp "$workspace_dir/platforms/macos/Resources/PkgInfo" "$contents_dir/PkgInfo"
# The Mozc-derived dictionary notices must accompany binary redistributions.
cp "$workspace_dir/crates/slime-converter/data/MOZC_DICTIONARY_LICENSE.txt" "$resources_dir/"
cp "$workspace_dir/LICENSE" "$resources_dir/LICENSE.txt"
swift "$workspace_dir/platforms/macos/GenerateIcon.swift" "$resources_dir/InputMethodIcon.tiff"
for localization_dir in "$workspace_dir"/platforms/macos/Resources/*.lproj; do
  cp -R "$localization_dir" "$resources_dir/"
done

original_dylib_id="$(otool -D "$frameworks_dir/libslime_ffi.dylib" | tail -n 1)"
install_name_tool \
  -id @rpath/libslime_ffi.dylib \
  "$frameworks_dir/libslime_ffi.dylib"
install_name_tool \
  -change "$original_dylib_id" @rpath/libslime_ffi.dylib \
  "$executable"

cc \
  -framework Carbon \
  "$workspace_dir/platforms/macos/RegisterInputSource.c" \
  -o "$workspace_dir/target/macos/register-input-source"

codesign --force --sign "$codesign_identity" "$frameworks_dir/libslime_ffi.dylib"
codesign \
  --force \
  --deep \
  --sign "$codesign_identity" \
  --entitlements "$workspace_dir/platforms/macos/Slime.entitlements" \
  "$bundle_dir"

echo "Built $bundle_dir"
