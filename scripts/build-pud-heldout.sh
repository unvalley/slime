#!/usr/bin/env bash
set -euo pipefail

# Builds a one-shot external held-out set from UD Japanese PUD news/wiki text.
# PUD is CC BY-SA 3.0, is distributed only as a test split, and remains an
# evaluation-only cache. Do not use this output for training or threshold
# selection after inspecting its result.

workspace_dir=$(cd "$(dirname "$0")/.." && pwd)
version="r2.18"
revision="4abd575c57bfa125dd4bc564f2ceb8973bbbf422"
expected_sha256="7ac51383189bdf5513102f9f8ac7ee1b967d5449f8c2dc1a9108fa21a8f1a688"
data_dir="${SLIME_EVALUATION_DATA_DIR:-$workspace_dir/target/evaluation}/ud-japanese-pud/$version"
source_file="$data_dir/ja_pud-ud-test.conllu"
output_file="$data_dir/balanced_test.json"
phrase_output_file="$data_dir/phrase_test.json"
source_url="https://raw.githubusercontent.com/UniversalDependencies/UD_Japanese-PUD/$revision/ja_pud-ud-test.conllu"
dictionary_file="$workspace_dir/crates/slime-converter/data/mozc-basic.tsv"

mkdir -p "$data_dir"
if [[ ! -f "$source_file" ]]; then
  temporary_file=$(mktemp "$data_dir/ja_pud-ud-test.conllu.tmp.XXXXXX")
  trap 'rm -f "$temporary_file"' EXIT
  curl --fail --location --silent --show-error "$source_url" --output "$temporary_file"
  actual_sha256=$(shasum -a 256 "$temporary_file" | awk '{print $1}')
  if [[ "$actual_sha256" != "$expected_sha256" ]]; then
    echo "PUD checksum mismatch: expected $expected_sha256, got $actual_sha256" >&2
    exit 1
  fi
  mv "$temporary_file" "$source_file"
  trap - EXIT
fi

actual_sha256=$(shasum -a 256 "$source_file" | awk '{print $1}')
if [[ "$actual_sha256" != "$expected_sha256" ]]; then
  echo "Cached PUD checksum mismatch: expected $expected_sha256, got $actual_sha256" >&2
  exit 1
fi

cargo run --release --quiet -p slime-tools --bin slime-balanced-devset -- \
  "$source_file" "$dictionary_file" "$output_file" \
  --source-split "ud-japanese-pud-$version-test"
cargo run --release --quiet -p slime-tools --bin slime-balanced-devset -- \
  "$source_file" "$dictionary_file" "$phrase_output_file" \
  --source-split "ud-japanese-pud-$version-phrase-test" --phrase-windows

echo "PUD held-out set: $output_file"
echo "PUD phrase held-out set: $phrase_output_file"
