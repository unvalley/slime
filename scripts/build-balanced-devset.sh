#!/usr/bin/env bash
set -euo pipefail

# Builds external-domain ranking sets from UD Japanese GSD news/blog text.
# The pinned source is CC BY-SA 4.0 and remains an evaluation-only cache.

workspace_dir=$(cd "$(dirname "$0")/.." && pwd)
version="r2.18"
revision="33e7310b58308e85fd2b33a2fc3ef3e434f821c7"
data_dir="${SLIME_EVALUATION_DATA_DIR:-$workspace_dir/target/evaluation}/ud-japanese-gsd/$version"
source_base="https://raw.githubusercontent.com/UniversalDependencies/UD_Japanese-GSD/$revision"
dictionary_file="$workspace_dir/crates/slime-converter/data/mozc-basic.tsv"

mkdir -p "$data_dir"

download() {
  name=$1
  expected_sha256=$2
  path="$data_dir/$name"
  if [[ ! -f "$path" ]]; then
    temporary_file=$(mktemp "$data_dir/$name.tmp.XXXXXX")
    trap 'rm -f "$temporary_file"' EXIT
    curl --fail --location --silent --show-error "$source_base/$name" --output "$temporary_file"
    actual_sha256=$(shasum -a 256 "$temporary_file" | awk '{print $1}')
    if [[ "$actual_sha256" != "$expected_sha256" ]]; then
      echo "$name checksum mismatch: expected $expected_sha256, got $actual_sha256" >&2
      exit 1
    fi
    mv "$temporary_file" "$path"
    trap - EXIT
  fi
  actual_sha256=$(shasum -a 256 "$path" | awk '{print $1}')
  if [[ "$actual_sha256" != "$expected_sha256" ]]; then
    echo "Cached $name checksum mismatch: expected $expected_sha256, got $actual_sha256" >&2
    exit 1
  fi
}

download "ja_gsd-ud-dev.conllu" "18d266e29336ef619787a928d89d20f2e64188be70d5b37829e3aadc5bb6b841"
download "ja_gsd-ud-test.conllu" "5f50ee6ed45c7ebda3787e593eaf5e9a225f25e53f6b4b32df778031862c843f"
download "ja_gsd-ud-train.conllu" "99f67fd88257e7cfe81d81c4b8ee98aff85fc22bb525d907475a2856c8cfa9f3"

cargo run --release --quiet -p slime-tools --bin slime-balanced-devset -- \
  "$data_dir/ja_gsd-ud-train.conllu" "$dictionary_file" \
  "$data_dir/balanced_train.json" --source-split "ud-japanese-gsd-$version-train" \
  --annotated-output "$data_dir/annotated_train.txt"

for split in dev test; do
  cargo run --release --quiet -p slime-tools --bin slime-balanced-devset -- \
    "$data_dir/ja_gsd-ud-$split.conllu" "$dictionary_file" \
    "$data_dir/balanced_$split.json" --source-split "ud-japanese-gsd-$version-$split"
done

echo "Balanced dev set: $data_dir/balanced_dev.json"
echo "Balanced test set: $data_dir/balanced_test.json"
