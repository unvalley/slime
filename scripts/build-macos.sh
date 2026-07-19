#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
bundle_dir="$workspace_dir/target/macos/UnvalleyIME.app"
contents_dir="$bundle_dir/Contents"
macos_dir="$contents_dir/MacOS"
frameworks_dir="$contents_dir/Frameworks"
resources_dir="$contents_dir/Resources"
executable="$macos_dir/Unvalley"
codesign_identity="${IME_CODESIGN_IDENTITY:-}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS bundle can only be built on macOS" >&2
  exit 1
fi

case "$bundle_dir" in
  "$workspace_dir/target/macos/UnvalleyIME.app") ;;
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

MACOSX_DEPLOYMENT_TARGET=13.0 cargo build --manifest-path "$workspace_dir/Cargo.toml" --release -p ime-ffi

rm -rf "$bundle_dir"
mkdir -p "$macos_dir" "$frameworks_dir" "$resources_dir"

swiftc \
  -swift-version 5 \
  -module-name Unvalley \
  -import-objc-header "$workspace_dir/crates/ime-ffi/include/ime_ffi.h" \
  -framework AppKit \
  -framework InputMethodKit \
  -L "$workspace_dir/target/release" \
  -lime_ffi \
  -Xlinker -rpath \
  -Xlinker @executable_path/../Frameworks \
  "$workspace_dir/platforms/macos/Sources/RustEngine.swift" \
  "$workspace_dir/platforms/macos/Sources/KeyEventMapping.swift" \
  "$workspace_dir/platforms/macos/Sources/InputController.swift" \
  "$workspace_dir/platforms/macos/Sources/main.swift" \
  -o "$executable"

cp "$workspace_dir/target/release/libime_ffi.dylib" "$frameworks_dir/"
cp "$workspace_dir/platforms/macos/Resources/Info.plist" "$contents_dir/Info.plist"
cp "$workspace_dir/platforms/macos/Resources/PkgInfo" "$contents_dir/PkgInfo"
swift "$workspace_dir/platforms/macos/GenerateIcon.swift" "$resources_dir/InputMethodIcon.tiff"
for localization_dir in "$workspace_dir"/platforms/macos/Resources/*.lproj; do
  cp -R "$localization_dir" "$resources_dir/"
done

original_dylib_id="$(otool -D "$frameworks_dir/libime_ffi.dylib" | tail -n 1)"
install_name_tool \
  -id @rpath/libime_ffi.dylib \
  "$frameworks_dir/libime_ffi.dylib"
install_name_tool \
  -change "$original_dylib_id" @rpath/libime_ffi.dylib \
  "$executable"

cc \
  -framework Carbon \
  "$workspace_dir/platforms/macos/RegisterInputSource.c" \
  -o "$workspace_dir/target/macos/register-input-source"

codesign --force --sign "$codesign_identity" "$frameworks_dir/libime_ffi.dylib"
codesign \
  --force \
  --deep \
  --sign "$codesign_identity" \
  --entitlements "$workspace_dir/platforms/macos/UnvalleyIME.entitlements" \
  "$bundle_dir"

echo "Built $bundle_dir"
