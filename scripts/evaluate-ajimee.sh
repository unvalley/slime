#!/usr/bin/env bash
set -euo pipefail

workspace_dir=$(cd "$(dirname "$0")/.." && pwd)
revision="401666cd56d1a570c2021798b64b6da4396bfd45"
expected_sha256="e9eb668fd6aa14b1e26436f429b5550108af0a1dfd443b8cea0bcb3ab3028fca"
data_dir="${SLIME_EVALUATION_DATA_DIR:-$workspace_dir/target/evaluation}/ajimee-bench/$revision"
data_file="$data_dir/evaluation_items.json"
source_url="https://raw.githubusercontent.com/azooKey/AJIMEE-Bench/$revision/JWTD_v2/v1/evaluation_items.json"

mkdir -p "$data_dir"
if [[ ! -f "$data_file" ]]; then
  temporary_file=$(mktemp "$data_dir/evaluation_items.json.tmp.XXXXXX")
  trap 'rm -f "$temporary_file"' EXIT
  curl --fail --location --silent --show-error "$source_url" --output "$temporary_file"
  actual_sha256=$(shasum -a 256 "$temporary_file" | awk '{print $1}')
  if [[ "$actual_sha256" != "$expected_sha256" ]]; then
    echo "AJIMEE-Bench checksum mismatch: expected $expected_sha256, got $actual_sha256" >&2
    exit 1
  fi
  mv "$temporary_file" "$data_file"
  trap - EXIT
fi

actual_sha256=$(shasum -a 256 "$data_file" | awk '{print $1}')
if [[ "$actual_sha256" != "$expected_sha256" ]]; then
  echo "Cached AJIMEE-Bench checksum mismatch: expected $expected_sha256, got $actual_sha256" >&2
  exit 1
fi

features=()
for argument in "$@"; do
  if [[ "$argument" == "--neural-model" ]]; then
    features=(--features neural)
  fi
done

AJIMEE_BENCH_REVISION="$revision" AJIMEE_BENCH_SHA256="$actual_sha256" \
  cargo run --release --quiet -p slime-tools "${features[@]}" --bin slime-evaluate -- \
  ajimee "$data_file" "$@"
