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
release_build="${SLIME_RELEASE_BUILD:-0}"
build_number="${SLIME_BUILD_NUMBER:-11}"
dictionary_pack_verification_keys="${SLIME_DICTIONARY_PACK_VERIFICATION_KEYS:-}"
dictionary_pack_version_floors="${SLIME_DICTIONARY_PACK_VERSION_FLOORS:-}"
neural_model="${SLIME_NEURAL_MODEL:-}"
neural_model_license="${SLIME_NEURAL_MODEL_LICENSE:-}"
neural_profile="${SLIME_NEURAL_PROFILE:-}"
# shellcheck source=dictionary-pack-verification-keys.sh
source "$workspace_dir/scripts/dictionary-pack-verification-keys.sh"
# shellcheck source=dictionary-pack-version-floors.sh
source "$workspace_dir/scripts/dictionary-pack-version-floors.sh"
# shellcheck source=macos-release-source.sh
source "$workspace_dir/scripts/macos-release-source.sh"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS bundle can only be built on macOS" >&2
  exit 1
fi

case "$bundle_dir" in
  "$workspace_dir/target/macos/Slime.app") ;;
  *) echo "refusing to replace unexpected path: $bundle_dir" >&2; exit 1 ;;
esac

if [[ "$release_build" != "0" && "$release_build" != "1" ]]; then
  echo "SLIME_RELEASE_BUILD must be 0 or 1" >&2
  exit 1
fi
neural_enabled=false
if [[ -n "$neural_model" ]]; then
  if [[ ! -f "$neural_model" ]]; then
    echo "Neural model not found: $neural_model" >&2
    exit 1
  fi
  if [[ -z "$neural_model_license" || ! -f "$neural_model_license" ]]; then
    echo "SLIME_NEURAL_MODEL_LICENSE must name the model's redistributable license" >&2
    exit 1
  fi
  if [[ -z "$neural_profile" ]]; then
    neural_profile="balanced"
  fi
  case "$neural_profile" in
    balanced|high-accuracy) ;;
    *) echo "SLIME_NEURAL_PROFILE must be balanced or high-accuracy" >&2; exit 1 ;;
  esac
  neural_enabled=true
elif [[ -n "$neural_model_license" ]]; then
  echo "SLIME_NEURAL_MODEL_LICENSE requires SLIME_NEURAL_MODEL" >&2
  exit 1
elif [[ -n "$neural_profile" ]]; then
  echo "SLIME_NEURAL_PROFILE requires SLIME_NEURAL_MODEL" >&2
  exit 1
fi

if [[ -n "$dictionary_pack_verification_keys" ]]; then
  slime_validate_dictionary_pack_verification_keys "$dictionary_pack_verification_keys"
fi
if [[ -n "$dictionary_pack_version_floors" ]]; then
  slime_validate_dictionary_pack_version_floors "$dictionary_pack_version_floors"
  if [[ -z "$dictionary_pack_verification_keys" ]]; then
    echo "Dictionary pack version floors require verification keys" >&2
    exit 1
  fi
fi

workspace_version="$(
  cargo metadata --manifest-path "$workspace_dir/Cargo.toml" \
    --no-deps --format-version 1 \
    | plutil -extract 'packages.0.version' raw -o - -
)"
if [[ ! "$workspace_version" =~ ^[0-9]+([.][0-9]+){1,2}$ ]]; then
  echo "Unsupported workspace version: $workspace_version" >&2
  exit 1
fi
if [[ "$release_build" == "1" && -z "${SLIME_BUILD_NUMBER:-}" ]]; then
  echo "Release builds require a monotonically increasing SLIME_BUILD_NUMBER" >&2
  exit 1
fi
if [[ ! "$build_number" =~ ^[1-9][0-9]{0,17}$ ]]; then
  echo "SLIME_BUILD_NUMBER must be a positive integer of at most 18 digits" >&2
  exit 1
fi

if [[ "$release_build" == "1" ]]; then
  if [[ "$codesign_identity" != "Developer ID Application:"* ]]; then
    echo "Release builds require SLIME_CODESIGN_IDENTITY=Developer ID Application: ..." >&2
    exit 1
  fi
  if [[ -z "$dictionary_pack_verification_keys" ]]; then
    echo "Release builds require SLIME_DICTIONARY_PACK_VERIFICATION_KEYS" >&2
    exit 1
  fi
  if [[ -z "$dictionary_pack_version_floors" ]]; then
    echo "Release builds require SLIME_DICTIONARY_PACK_VERSION_FLOORS" >&2
    exit 1
  fi
  source_revision="$(slime_release_source_revision "$workspace_dir")"
elif [[ -z "$codesign_identity" ]]; then
  codesign_identity="$(security find-identity -v -p codesigning \
    | sed -n 's/.*"\(Apple Development:.*\)"/\1/p' \
    | head -n 1)"
fi
if [[ "$release_build" == "0" && -z "$codesign_identity" ]]; then
  codesign_identity="-"
  echo "No trusted code-signing identity found; building with an ad-hoc signature."
else
  echo "Signing with: $codesign_identity"
fi

build_slime_ffi() {
  MACOSX_DEPLOYMENT_TARGET=13.0 cargo build \
    --manifest-path "$workspace_dir/Cargo.toml" \
    --release \
    -p slime-ffi \
    "$@"
}

if [[ "$neural_enabled" == true ]]; then
  build_slime_ffi --features neural
else
  build_slime_ffi
fi

rm -rf "$bundle_dir"
mkdir -p "$macos_dir" "$frameworks_dir" "$resources_dir"

swiftc \
  -swift-version 5 \
  -module-name Slime \
  -import-objc-header "$workspace_dir/crates/slime-ffi/include/slime_ffi.h" \
  -framework AppKit \
  -framework InputMethodKit \
  -framework Security \
  -framework SwiftUI \
  -L "$workspace_dir/target/release" \
  -lslime_ffi \
  -Xlinker -rpath \
  -Xlinker @executable_path/../Frameworks \
  "$workspace_dir/platforms/macos/Sources/RustEngine.swift" \
  "$workspace_dir/platforms/macos/Sources/UserDataStore.swift" \
  "$workspace_dir/platforms/macos/Sources/DictionaryImporter.swift" \
  "$workspace_dir/platforms/macos/Sources/InputPrivacy.swift" \
  "$workspace_dir/platforms/macos/Sources/InputVerification.swift" \
  "$workspace_dir/platforms/macos/Sources/InputContextBoundary.swift" \
  "$workspace_dir/platforms/macos/Sources/KeyEventMapping.swift" \
  "$workspace_dir/platforms/macos/Sources/TextClientActions.swift" \
  "$workspace_dir/platforms/macos/Sources/CandidatePanel.swift" \
  "$workspace_dir/platforms/macos/Sources/SettingsWindow.swift" \
  "$workspace_dir/platforms/macos/Sources/InputController.swift" \
  "$workspace_dir/platforms/macos/Sources/main.swift" \
  -o "$executable"

cp "$workspace_dir/target/release/libslime_ffi.dylib" "$frameworks_dir/"
cp "$workspace_dir/platforms/macos/Resources/Info.plist" "$contents_dir/Info.plist"
plutil -replace CFBundleShortVersionString -string "$workspace_version" \
  "$contents_dir/Info.plist"
plutil -replace CFBundleVersion -string "$build_number" \
  "$contents_dir/Info.plist"
if [[ "$release_build" == "1" ]]; then
  plutil -insert SlimeSourceRevision -string "$source_revision" \
    "$contents_dir/Info.plist"
fi
if [[ -n "$dictionary_pack_verification_keys" ]]; then
  plutil -insert SlimeDictionaryPackVerificationKeys \
    -string "$dictionary_pack_verification_keys" "$contents_dir/Info.plist"
fi
if [[ -n "$dictionary_pack_version_floors" ]]; then
  plutil -insert SlimeDictionaryPackVersionFloors \
    -string "$dictionary_pack_version_floors" "$contents_dir/Info.plist"
fi
if [[ "$neural_enabled" == true ]]; then
  plutil -insert SlimeNeuralProfile -string "$neural_profile" "$contents_dir/Info.plist"
fi
cp "$workspace_dir/platforms/macos/Resources/PkgInfo" "$contents_dir/PkgInfo"
# Third-party dictionary notices must accompany binary redistributions.
cp "$workspace_dir/crates/slime-converter/data/MOZC_DICTIONARY_LICENSE.txt" "$resources_dir/"
cp "$workspace_dir/LICENSE" "$resources_dir/LICENSE.txt"
if [[ "$neural_enabled" == true ]]; then
  cp "$neural_model" "$resources_dir/SlimeNeuralModel.gguf"
  cp "$neural_model_license" "$resources_dir/SlimeNeuralModel-LICENSE.txt"
fi
cp "$workspace_dir/scripts/uninstall-macos-system.sh" "$resources_dir/uninstall-macos-system.sh"
chmod 755 "$resources_dir/uninstall-macos-system.sh"
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

signing_arguments=(--force --sign "$codesign_identity")
if [[ "$release_build" == "1" ]]; then
  signing_arguments+=(--timestamp --options runtime)
fi
codesign "${signing_arguments[@]}" "$frameworks_dir/libslime_ffi.dylib"
if [[ "$release_build" == "1" ]]; then
  codesign "${signing_arguments[@]}" "$bundle_dir"
else
  codesign \
    "${signing_arguments[@]}" \
    --entitlements "$workspace_dir/platforms/macos/Slime.entitlements" \
    "$bundle_dir"
fi

echo "Built $bundle_dir"
