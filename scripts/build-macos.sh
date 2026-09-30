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
neural_lambda="${SLIME_NEURAL_LAMBDA:-0.2}"
neural_explicit_long_lambda="${SLIME_NEURAL_EXPLICIT_LONG_LAMBDA:-0.45}"
neural_explicit_medium_lambda="${SLIME_NEURAL_EXPLICIT_MEDIUM_LAMBDA:-0.3}"
neural_explicit_minimum_characters="${SLIME_NEURAL_EXPLICIT_MINIMUM_CHARACTERS:-20}"
neural_max_cost_gap="${SLIME_NEURAL_MAX_COST_GAP:-1000}"
# Preserve explicitly tuned scoring settings unless confidence is also opted in.
neural_explicit_confidence_default=1
if [[ -n "${SLIME_NEURAL_LAMBDA+x}${SLIME_NEURAL_EXPLICIT_LONG_LAMBDA+x}${SLIME_NEURAL_EXPLICIT_MEDIUM_LAMBDA+x}${SLIME_NEURAL_EXPLICIT_MINIMUM_CHARACTERS+x}${SLIME_NEURAL_MAX_COST_GAP+x}${SLIME_NEURAL_EXPLICIT_MAX_COST_GAP+x}" ]]; then
  neural_explicit_confidence_default=0
fi
neural_explicit_confidence="${SLIME_NEURAL_EXPLICIT_CONFIDENCE:-$neural_explicit_confidence_default}"
# Keep manually tuned scoring settings unchanged unless agreement is opted in.
neural_explicit_live_agreement_default="$neural_explicit_confidence_default"
if [[ -n "${SLIME_NEURAL_LIVE_MIN_SWITCH_MARGIN+x}${SLIME_NEURAL_LIVE_LONG_READING_MIN_SWITCH_MARGIN+x}${SLIME_NEURAL_LIVE_NUMERIC_BASE_SWITCH_MARGIN+x}${SLIME_NEURAL_LIVE_LONG_READING_LAMBDA+x}" ]]; then
  neural_explicit_live_agreement_default=0
fi
neural_explicit_live_agreement="${SLIME_NEURAL_EXPLICIT_LIVE_AGREEMENT:-$neural_explicit_live_agreement_default}"
neural_explicit_max_cost_gap="${SLIME_NEURAL_EXPLICIT_MAX_COST_GAP:-${SLIME_NEURAL_MAX_COST_GAP:-1500}}"
neural_live_reranking="${SLIME_NEURAL_LIVE_RERANKING:-1}"
neural_live_debounce_ms="${SLIME_NEURAL_LIVE_DEBOUNCE_MS:-180}"
neural_live_min_switch_margin="${SLIME_NEURAL_LIVE_MIN_SWITCH_MARGIN:-0.2}"
neural_live_long_reading_min_switch_margin="${SLIME_NEURAL_LIVE_LONG_READING_MIN_SWITCH_MARGIN:-0.3}"
neural_live_numeric_base_switch_margin="${SLIME_NEURAL_LIVE_NUMERIC_BASE_SWITCH_MARGIN:-0.1}"
neural_live_long_reading_lambda="${SLIME_NEURAL_LIVE_LONG_READING_LAMBDA:-0.6}"
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
if [[ -n "$neural_model" ]]; then
  if [[ ! -f "$neural_model" || ! -r "$neural_model" ]]; then
    echo "SLIME_NEURAL_MODEL must name a readable GGUF file" >&2
    exit 1
  fi
  neural_magic="$(LC_ALL=C od -An -tx1 -N4 "$neural_model" | tr -d '[:space:]')"
  if [[ "$neural_magic" != "47475546" ]]; then
    echo "SLIME_NEURAL_MODEL must start with the GGUF file signature" >&2
    exit 1
  fi
  if [[ ! "$neural_lambda" =~ ^(0([.][0-9]+)?|1([.]0+)?)$ ]]; then
    echo "SLIME_NEURAL_LAMBDA must be between 0 and 1" >&2
    exit 1
  fi
  if [[ ! "$neural_max_cost_gap" =~ ^[0-9]+$ ]] \
    || (( neural_max_cost_gap > 2147483647 )); then
    echo "SLIME_NEURAL_MAX_COST_GAP must be between 0 and 2147483647" >&2
    exit 1
  fi
  if [[ ! "$neural_explicit_max_cost_gap" =~ ^[0-9]+$ ]] \
    || (( neural_explicit_max_cost_gap > 2147483647 )); then
    echo "SLIME_NEURAL_EXPLICIT_MAX_COST_GAP must be between 0 and 2147483647" >&2
    exit 1
  fi
  if [[ "$neural_explicit_confidence" != "0" && "$neural_explicit_confidence" != "1" ]]; then
    echo "SLIME_NEURAL_EXPLICIT_CONFIDENCE must be 0 or 1" >&2
    exit 1
  fi
  if [[ "$neural_explicit_live_agreement" != "0" && "$neural_explicit_live_agreement" != "1" ]]; then
    echo "SLIME_NEURAL_EXPLICIT_LIVE_AGREEMENT must be 0 or 1" >&2
    exit 1
  fi
  if [[ "$neural_live_reranking" != "0" && "$neural_live_reranking" != "1" ]]; then
    echo "SLIME_NEURAL_LIVE_RERANKING must be 0 or 1" >&2
    exit 1
  fi
  if [[ ! "$neural_live_debounce_ms" =~ ^[0-9]+$ ]] \
    || (( neural_live_debounce_ms < 50 || neural_live_debounce_ms > 1000 )); then
    echo "SLIME_NEURAL_LIVE_DEBOUNCE_MS must be between 50 and 1000" >&2
    exit 1
  fi
  if [[ ! "$neural_live_min_switch_margin" =~ ^([0-9]+([.][0-9]+)?|[.][0-9]+)$ ]]; then
    echo "SLIME_NEURAL_LIVE_MIN_SWITCH_MARGIN must be a non-negative number" >&2
    exit 1
  fi
  if [[ ! "$neural_live_long_reading_min_switch_margin" =~ ^([0-9]+([.][0-9]+)?|[.][0-9]+)$ ]]; then
    echo "SLIME_NEURAL_LIVE_LONG_READING_MIN_SWITCH_MARGIN must be a non-negative number" >&2
    exit 1
  fi
  if [[ ! "$neural_live_numeric_base_switch_margin" =~ ^([0-9]+([.][0-9]+)?|[.][0-9]+)$ ]]; then
    echo "SLIME_NEURAL_LIVE_NUMERIC_BASE_SWITCH_MARGIN must be a non-negative number" >&2
    exit 1
  fi
  if [[ ! "$neural_live_long_reading_lambda" =~ ^(0([.][0-9]+)?|1([.]0+)?)$ ]]; then
    echo "SLIME_NEURAL_LIVE_LONG_READING_LAMBDA must be between 0 and 1" >&2
    exit 1
  fi
  if [[ "$release_build" == "1" ]]; then
    echo "Neural model redistribution is disabled until its distribution terms are recorded" >&2
    exit 1
  fi
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

if [[ -n "$neural_model" ]]; then
  MACOSX_DEPLOYMENT_TARGET=13.0 cargo build \
    --manifest-path "$workspace_dir/Cargo.toml" \
    --release -p slime-ffi --features neural
else
  MACOSX_DEPLOYMENT_TARGET=13.0 cargo build \
    --manifest-path "$workspace_dir/Cargo.toml" \
    --release -p slime-ffi
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
  "$workspace_dir/platforms/macos/Sources/LiveNeuralScheduling.swift" \
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
if [[ -n "$neural_model" ]]; then
  neural_resource="SlimeNeuralModel.gguf"
  cp "$neural_model" "$resources_dir/$neural_resource"
  plutil -insert SlimeNeuralModelResource -string "$neural_resource" \
    "$contents_dir/Info.plist"
  plutil -insert SlimeNeuralLambda -float "$neural_lambda" \
    "$contents_dir/Info.plist"
  plutil -insert SlimeNeuralExplicitLongLambda -float "$neural_explicit_long_lambda" \
    "$contents_dir/Info.plist"
  plutil -insert SlimeNeuralExplicitMediumLambda -float "$neural_explicit_medium_lambda" \
    "$contents_dir/Info.plist"
  plutil -insert SlimeNeuralExplicitMinimumCharacters -integer "$neural_explicit_minimum_characters" \
    "$contents_dir/Info.plist"
  plutil -insert SlimeNeuralMaxCostGap -integer "$neural_max_cost_gap" \
    "$contents_dir/Info.plist"
  plutil -insert SlimeNeuralExplicitMaxCostGap -integer "$neural_explicit_max_cost_gap" \
    "$contents_dir/Info.plist"
  if [[ "$neural_explicit_confidence" == "1" ]]; then
    plutil -insert SlimeNeuralExplicitConfidenceEnabled -bool YES \
      "$contents_dir/Info.plist"
  else
    plutil -insert SlimeNeuralExplicitConfidenceEnabled -bool NO \
      "$contents_dir/Info.plist"
  fi
  if [[ "$neural_explicit_live_agreement" == "1" ]]; then
    plutil -insert SlimeNeuralExplicitLiveAgreementEnabled -bool YES \
      "$contents_dir/Info.plist"
  else
    plutil -insert SlimeNeuralExplicitLiveAgreementEnabled -bool NO \
      "$contents_dir/Info.plist"
  fi
  if [[ "$neural_live_reranking" == "1" ]]; then
    plutil -insert SlimeNeuralLiveRerankingEnabled -bool YES \
      "$contents_dir/Info.plist"
  else
    plutil -insert SlimeNeuralLiveRerankingEnabled -bool NO \
      "$contents_dir/Info.plist"
  fi
  plutil -insert SlimeNeuralLiveDebounceMilliseconds -integer "$neural_live_debounce_ms" \
    "$contents_dir/Info.plist"
  plutil -insert SlimeNeuralLiveMinSwitchMargin -float "$neural_live_min_switch_margin" \
    "$contents_dir/Info.plist"
  plutil -insert SlimeNeuralLiveLongReadingMinSwitchMargin -float \
    "$neural_live_long_reading_min_switch_margin" "$contents_dir/Info.plist"
  plutil -insert SlimeNeuralLiveNumericBaseSwitchMargin -float \
    "$neural_live_numeric_base_switch_margin" "$contents_dir/Info.plist"
  plutil -insert SlimeNeuralLiveLongReadingLambda -float "$neural_live_long_reading_lambda" \
    "$contents_dir/Info.plist"
fi
cp "$workspace_dir/platforms/macos/Resources/PkgInfo" "$contents_dir/PkgInfo"
# Third-party dictionary notices must accompany binary redistributions.
cp "$workspace_dir/crates/slime-converter/data/MOZC_DICTIONARY_LICENSE.txt" "$resources_dir/"
cp "$workspace_dir/LICENSE" "$resources_dir/LICENSE.txt"
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
