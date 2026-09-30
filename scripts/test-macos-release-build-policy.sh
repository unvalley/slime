#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "$0")/.." && pwd)"
build_script="$workspace_dir/scripts/build-macos.sh"
fixture_identity="Developer ID Application: Fixture Only"
fixture_key="$(printf '01%.0s' {1..32})"
fixture_model="$(mktemp "${TMPDIR:-/tmp}/slime-neural-policy.XXXXXX")"
trap 'rm -f "$fixture_model"' EXIT
printf 'GGUF' >"$fixture_model"

if SLIME_NEURAL_MODEL="$fixture_model" \
  SLIME_NEURAL_MAX_COST_GAP=-1 \
  "$build_script" >/dev/null 2>&1; then
  echo "macOS build accepted a negative neural cost-gap threshold" >&2
  exit 1
fi

if SLIME_NEURAL_MODEL="$fixture_model" \
  SLIME_NEURAL_LIVE_RERANKING=yes \
  "$build_script" >/dev/null 2>&1; then
  echo "macOS build accepted an invalid neural LIVE enable flag" >&2
  exit 1
fi

if SLIME_NEURAL_MODEL="$fixture_model" \
  SLIME_NEURAL_LIVE_DEBOUNCE_MS=49 \
  "$build_script" >/dev/null 2>&1; then
  echo "macOS build accepted an unsafe neural LIVE debounce" >&2
  exit 1
fi

if SLIME_NEURAL_MODEL="$fixture_model" \
  SLIME_NEURAL_LIVE_MIN_SWITCH_MARGIN=-0.5 \
  "$build_script" >/dev/null 2>&1; then
  echo "macOS build accepted a negative neural LIVE switch margin" >&2
  exit 1
fi

if SLIME_NEURAL_MODEL="$fixture_model" \
  SLIME_NEURAL_LIVE_LONG_READING_MIN_SWITCH_MARGIN=-0.5 \
  "$build_script" >/dev/null 2>&1; then
  echo "macOS build accepted a negative long-reading neural LIVE switch margin" >&2
  exit 1
fi

if SLIME_NEURAL_MODEL="$fixture_model" \
  SLIME_NEURAL_LIVE_LONG_READING_LAMBDA=1.1 \
  "$build_script" >/dev/null 2>&1; then
  echo "macOS build accepted an invalid long-reading neural LIVE lambda" >&2
  exit 1
fi

if SLIME_NEURAL_MODEL="$fixture_model" \
  SLIME_NEURAL_LIVE_NUMERIC_BASE_SWITCH_MARGIN=-0.1 \
  "$build_script" >/dev/null 2>&1; then
  echo "macOS build accepted a negative numeric-base LIVE switch margin" >&2
  exit 1
fi

if SLIME_RELEASE_BUILD=1 \
  SLIME_BUILD_NUMBER=999999 \
  SLIME_CODESIGN_IDENTITY="$fixture_identity" \
  "$build_script" >/dev/null 2>&1; then
  echo "macOS release build accepted missing dictionary pack verification keys" >&2
  exit 1
fi

if SLIME_RELEASE_BUILD=1 \
  SLIME_BUILD_NUMBER=999999 \
  SLIME_CODESIGN_IDENTITY="$fixture_identity" \
  SLIME_DICTIONARY_PACK_VERIFICATION_KEYS="fixture"$'\t'"$fixture_key" \
  SLIME_DICTIONARY_PACK_VERSION_FLOORS=$'sample-general\t2026.08.1' \
  SLIME_NEURAL_MODEL="$fixture_model" \
  "$build_script" >/dev/null 2>&1; then
  echo "macOS release build accepted a neural model without cleared redistribution terms" >&2
  exit 1
fi

if SLIME_RELEASE_BUILD=1 \
  SLIME_BUILD_NUMBER=999999 \
  SLIME_CODESIGN_IDENTITY="$fixture_identity" \
  SLIME_DICTIONARY_PACK_VERIFICATION_KEYS="fixture"$'\t'"$fixture_key" \
  "$build_script" >/dev/null 2>&1; then
  echo "macOS release build accepted missing dictionary pack version floors" >&2
  exit 1
fi

if SLIME_RELEASE_BUILD=1 \
  SLIME_BUILD_NUMBER=999999 \
  SLIME_CODESIGN_IDENTITY="$fixture_identity" \
  SLIME_DICTIONARY_PACK_VERIFICATION_KEYS="fixture"$'\t'"$fixture_key" \
  SLIME_DICTIONARY_PACK_VERSION_FLOORS=$'sample-general\t2026.08' \
  "$build_script" >/dev/null 2>&1; then
  echo "macOS release build accepted invalid dictionary pack version floors" >&2
  exit 1
fi

if SLIME_RELEASE_BUILD=1 \
  SLIME_BUILD_NUMBER=999999 \
  SLIME_CODESIGN_IDENTITY="$fixture_identity" \
  SLIME_DICTIONARY_PACK_VERIFICATION_KEYS=$'fixture\tinvalid' \
  "$build_script" >/dev/null 2>&1; then
  echo "macOS release build accepted invalid dictionary pack verification keys" >&2
  exit 1
fi

echo "macOS release build policy test passed."
