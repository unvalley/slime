#!/usr/bin/env bash
set -euo pipefail

# Builds the kana-kanji conversion development set from the JWTD v2 train
# split. AJIMEE-Bench derives from the JWTD test split, so all cost and model
# tuning must use this set and keep AJIMEE-Bench held out for reporting.

workspace_dir=$(cd "$(dirname "$0")/.." && pwd)
version="2.0"
expected_sha256="2c6d864905adbac75a2c0181aa8796c81d3f86df41aa8bf79b37d47ee8ec5025"
data_dir="${SLIME_EVALUATION_DATA_DIR:-$workspace_dir/target/evaluation}/jwtd/$version"
archive_file="$data_dir/jwtd_v2.0.tar.gz"
train_file="$data_dir/jwtd_v2.0/train.jsonl"
output_file="$data_dir/dev_items.json"
source_url="https://nlp.ist.i.kyoto-u.ac.jp/nl-resource/JWTD/jwtd_v2.0.tar.gz"
item_count="${SLIME_DEVSET_COUNT:-400}"

mkdir -p "$data_dir"
if [[ ! -f "$archive_file" ]]; then
  temporary_file=$(mktemp "$data_dir/jwtd.tar.gz.tmp.XXXXXX")
  trap 'rm -f "$temporary_file"' EXIT
  curl --fail --location --silent --show-error "$source_url" --output "$temporary_file"
  actual_sha256=$(shasum -a 256 "$temporary_file" | awk '{print $1}')
  if [[ "$actual_sha256" != "$expected_sha256" ]]; then
    echo "JWTD v2 checksum mismatch: expected $expected_sha256, got $actual_sha256" >&2
    exit 1
  fi
  mv "$temporary_file" "$archive_file"
  trap - EXIT
fi

actual_sha256=$(shasum -a 256 "$archive_file" | awk '{print $1}')
if [[ "$actual_sha256" != "$expected_sha256" ]]; then
  echo "Cached JWTD v2 checksum mismatch: expected $expected_sha256, got $actual_sha256" >&2
  exit 1
fi

if [[ ! -f "$train_file" ]]; then
  tar -xzf "$archive_file" -C "$data_dir" jwtd_v2.0/train.jsonl
fi

cargo run --release --quiet -p slime-tools --bin slime-devset -- \
  "$train_file" "$workspace_dir/crates/slime-converter/data/mozc-basic.tsv" \
  "$output_file" --count "$item_count"
echo "Development set: $output_file"
