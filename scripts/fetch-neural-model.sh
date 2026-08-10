#!/usr/bin/env bash
set -euo pipefail

# Downloads the Apache-2.0 zenz-v3.2-small GGUF used by the high-accuracy
# neural N-best profile and rewrites its pre-tokenizer tag so that current
# llama.cpp builds can load it.
#
# The published GGUF declares the pre-tokenizer 'gpt2-small-japanese-char',
# which upstream llama.cpp does not know and refuses to load. The vocabulary
# is a plain character-level BPE, so the standard 'gpt-2' pre-tokenizer
# produces identical tokenization for Japanese text.
#
# The upstream commit and both source/fixed checksums are pinned. The model is
# still optional and is never fetched by a normal build. Binary distributors
# must include crates/slime-neural/data/ZENZ_V3_2_SMALL_LICENSE.txt.

workspace_dir=$(cd "$(dirname "$0")/.." && pwd)
models_dir="${SLIME_EVALUATION_DATA_DIR:-$workspace_dir/target/evaluation}/models"
upstream_revision="c67e03e07d215c869f591b274c1631170d3e11fe"
source_url="https://huggingface.co/Miwa-Keita/zenz-v3.2-small-gguf/resolve/$upstream_revision/ggml-model-Q5_K_M.gguf"
source_sha256="29c223d4c23327b80fd13ebb5ab2555057a46317997d5da391584ffbef0db673"
fixed_sha256="b660082fcbe8e538c4ccc1044f79c2c881364a25f8c9277a8b8f1dcf680e5b84"
source_file="$models_dir/zenz-v3.2-small-Q5_K_M.gguf"
target_file="$models_dir/zenz-v3.2-small-Q5_K_M-fixed.gguf"

if [[ -f "$target_file" ]]; then
  actual_fixed_sha256=$(shasum -a 256 "$target_file" | awk '{print $1}')
  if [[ "$actual_fixed_sha256" != "$fixed_sha256" ]]; then
    echo "cached fixed zenz model checksum mismatch: expected $fixed_sha256, got $actual_fixed_sha256" >&2
    exit 1
  fi
  echo "$target_file"
  exit 0
fi

mkdir -p "$models_dir"
if [[ ! -f "$source_file" ]]; then
  temporary_file=$(mktemp "$source_file.tmp.XXXXXX")
  trap 'rm -f "$temporary_file"' EXIT
  curl --fail --location --silent --show-error "$source_url" --output "$temporary_file"
  actual_sha256=$(shasum -a 256 "$temporary_file" | awk '{print $1}')
  if [[ "$actual_sha256" != "$source_sha256" ]]; then
    echo "zenz model checksum mismatch: expected $source_sha256, got $actual_sha256" >&2
    exit 1
  fi
  mv "$temporary_file" "$source_file"
  trap - EXIT
fi
actual_source_sha256=$(shasum -a 256 "$source_file" | awk '{print $1}')
if [[ "$actual_source_sha256" != "$source_sha256" ]]; then
  echo "cached zenz model checksum mismatch: expected $source_sha256, got $actual_source_sha256" >&2
  exit 1
fi

if ! command -v uvx >/dev/null; then
  echo "uvx is required to rewrite the GGUF pre-tokenizer metadata (https://docs.astral.sh/uv/)" >&2
  exit 1
fi
uvx --from gguf gguf-new-metadata --pre-tokenizer "gpt-2" "$source_file" "$target_file" >&2
actual_fixed_sha256=$(shasum -a 256 "$target_file" | awk '{print $1}')
if [[ "$actual_fixed_sha256" != "$fixed_sha256" ]]; then
  echo "fixed zenz model checksum mismatch: expected $fixed_sha256, got $actual_fixed_sha256" >&2
  exit 1
fi
echo "$target_file"
