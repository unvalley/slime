#!/usr/bin/env bash
set -euo pipefail

# Downloads the zenz-v3.1-xsmall GGUF used for neural N-best rescoring
# (docs/phase2-context-model-survey.md, first candidate, step 1) and rewrites
# its pre-tokenizer tag so that current llama.cpp builds can load it.
#
# The published GGUF declares the pre-tokenizer 'gpt2-small-japanese-char',
# which upstream llama.cpp does not know and refuses to load. The vocabulary
# is a plain character-level BPE, so the standard 'gpt-2' pre-tokenizer
# produces identical tokenization for Japanese text.
#
# License note: zenz-v3.1 is CC BY-SA 4.0 (Miwa-Keita/zenz-v3.1-xsmall-gguf).
# The model is used for offline evaluation only and is not bundled into any
# build artifact.

workspace_dir=$(cd "$(dirname "$0")/.." && pwd)
models_dir="${SLIME_EVALUATION_DATA_DIR:-$workspace_dir/target/evaluation}/models"
source_url="https://huggingface.co/Miwa-Keita/zenz-v3.1-xsmall-gguf/resolve/main/ggml-model-Q5_K_M.gguf"
source_sha256="189638370c43292fd54ba5e83854b24887ddd57e8914e19095514e663f60c7f5"
source_file="$models_dir/zenz-v3.1-xsmall-Q5_K_M.gguf"
target_file="$models_dir/zenz-v3.1-xsmall-Q5_K_M-fixed.gguf"

if [[ -f "$target_file" ]]; then
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

if ! command -v uvx >/dev/null; then
  echo "uvx is required to rewrite the GGUF pre-tokenizer metadata (https://docs.astral.sh/uv/)" >&2
  exit 1
fi
uvx --from gguf gguf-new-metadata --pre-tokenizer "gpt-2" "$source_file" "$target_file" >&2
echo "$target_file"
