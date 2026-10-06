#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
temporary_directory="$(mktemp -d "${TMPDIR:-/tmp}/slime-private-candidate-test.XXXXXX")"
cleanup() {
  rm -rf "$temporary_directory"
}
trap cleanup EXIT
umask 077

training="$temporary_directory/train.annotated"
exclusion="$temporary_directory/exclude.annotated"
recall_target="$temporary_directory/recall-target.tsv"
recall_held_out="$temporary_directory/recall-held-out.tsv"
context_target="$temporary_directory/context-target.tsv"
context_held_out="$temporary_directory/context-held-out.tsv"
license_ledger="$temporary_directory/license-ledger.txt"
candidate="$temporary_directory/candidate"
log="$temporary_directory/run.log"

sha256_file() {
  if command -v shasum >/dev/null; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    sha256sum "$1" | awk '{print $1}'
  fi
}

hex_file() {
  od -An -tx1 "$1" | tr -d ' \n'
}

sign_pack() {
  local pack="$1"
  local signature="$temporary_directory/signature.bin"
  openssl pkeyutl -sign -rawin -inkey "$signing_key" -in "$pack" -out "$signature"
  printf '%s\n' \
    '# slime-dictionary-signature-v1' \
    '# key-id: fixture-release-key' \
    "# signature-ed25519: $(hex_file "$signature")" \
    >"$pack.sig"
}

printf '%s\n' \
  '甲/こう 文章/ぶんしょう 漢字/かんじ 蒼峰/そうほう' \
  '乙/おつ 文章/ぶんしょう 漢字/かんじ 蒼峰/そうほう' \
  '丙/へい 文章/ぶんしょう 漢字/かんじ 蒼峰/そうほう' \
  >"$training"
printf '%s\n' '丁/てい 別文/べつぶん 対象/たいしょう' >"$exclusion"
printf '%s\n' $'そうほう\t蒼峰' >"$recall_target"
printf '%s\n' $'にほん\t日本' >"$recall_held_out"
printf '%s\n' $'文章\tかんじ\t漢字' >"$context_target"
printf '%s\n' $'文章\tにほん\t日本' >"$context_held_out"
printf '%s\n' 'fixture data is approved only for this local test' >"$license_ledger"

cd "$repo_dir"
if ! scripts/prepare-private-dictionary-candidate.sh \
  --train "$training" \
  --exclude "$exclusion" \
  --recall-target "$recall_target" \
  --recall-held-out "$recall_held_out" \
  --context-target "$context_target" \
  --context-held-out "$context_held_out" \
  --license-ledger "$license_ledger" \
  --output "$candidate" \
  --term-pack-id fixture-terms \
  --context-pack-id fixture-context \
  --version 2026.08.1 \
  --minimum-slime-version 0.1.0 \
  --published-at 2026-08-08 \
  --license Example-Test-Only \
  --provenance fixture/private-candidate \
  >"$log" 2>&1; then
  cat "$log" >&2
  exit 1
fi

for required in \
  READY UNSIGNED \
  generated/entries.tsv generated/context-rules.tsv \
  packs/terms.slime-dict packs/context.slime-dict \
  reports/term-generation.json reports/context-generation.json \
  reports/recall-target-0.json reports/recall-held-out-0.json \
  reports/context-target-0.json reports/context-held-out-0.json \
  reports/process-startup.json \
  reports/license-ledger.sha256; do
  if [[ ! -f "$candidate/$required" ]]; then
    echo "private candidate pipeline omitted a required artifact" >&2
    exit 1
  fi
done

if grep -q '蒼峰\|そうほう' "$log" "$candidate"/reports/*; then
  echo "private vocabulary leaked into aggregate pipeline output" >&2
  exit 1
fi

if ! command -v openssl >/dev/null; then
  echo "OpenSSL is required for the signed private candidate test" >&2
  exit 1
fi
signing_key="$temporary_directory/signing-key.pem"
public_key_der="$temporary_directory/public-key.der"
verification_keys="$temporary_directory/verification-keys.tsv"
version_floors="$temporary_directory/version-floors.tsv"
signed_baseline="$temporary_directory/signed-baseline"
signed_candidate="$temporary_directory/signed-candidate"
mkdir -p \
  "$signed_baseline/dictionary-packs" \
  "$signed_candidate/dictionary-packs"
openssl genpkey -algorithm ED25519 -out "$signing_key" 2>>"$log"
openssl pkey -in "$signing_key" -pubout -outform DER -out "$public_key_der" 2>>"$log"
public_key_hex="$(tail -c 32 "$public_key_der" | od -An -tx1 | tr -d ' \n')"
printf 'fixture-release-key\t%s\n' "$public_key_hex" >"$verification_keys"
printf '%s\n' \
  $'fixture-terms\t2026.08.1' \
  $'fixture-context\t2026.08.1' \
  >"$version_floors"

cp "$candidate/packs/terms.slime-dict" "$signed_baseline/dictionary-packs/"
cp "$candidate/packs/terms.slime-dict" "$signed_candidate/dictionary-packs/"
cp "$candidate/packs/context.slime-dict" "$signed_candidate/dictionary-packs/"
sign_pack "$signed_baseline/dictionary-packs/terms.slime-dict"
sign_pack "$signed_candidate/dictionary-packs/terms.slime-dict"
sign_pack "$signed_candidate/dictionary-packs/context.slime-dict"

cargo run --quiet -p slime-tools --bin slime-dictionary-pack -- verify-signed \
  --data-dir "$signed_candidate" \
  --verification-keys "$verification_keys" \
  --version-floors "$version_floors" \
  --expected-packs 2 \
  --json >"$temporary_directory/signed-verification.json"
cargo run --release --quiet -p slime-tools --bin slime-pack-startup-evaluate -- \
  --baseline-data-dir "$signed_baseline" \
  --data-dir "$signed_candidate" \
  --verification-keys "$verification_keys" \
  --version-floors "$version_floors" \
  --baseline-expected-packs 1 \
  --expected-packs 2 \
  --iterations 3 \
  --max-median-ms 250 \
  --max-p95-ms 500 \
  --max-rss-bytes 134217728 \
  --max-median-delta-ms 50 \
  --max-p95-delta-ms 100 \
  --max-rss-delta-bytes 8388608 \
  --json >"$temporary_directory/signed-startup.json"
if grep -q '蒼峰\|そうほう' \
  "$temporary_directory/signed-verification.json" \
  "$temporary_directory/signed-startup.json"; then
  echo "private vocabulary leaked into signed aggregate output" >&2
  exit 1
fi

ready_sha256="$(sha256_file "$candidate/READY")"
if scripts/prepare-private-dictionary-candidate.sh \
  --train "$training" \
  --exclude "$exclusion" \
  --recall-target "$recall_target" \
  --context-target "$context_target" \
  --license-ledger "$license_ledger" \
  --output "$candidate" \
  --term-pack-id fixture-terms \
  --context-pack-id fixture-context \
  --version 2026.08.1 \
  --minimum-slime-version 0.1.0 \
  --published-at 2026-08-08 \
  --license Example-Test-Only \
  --provenance fixture/private-candidate \
  >>"$log" 2>&1; then
  echo "private candidate pipeline replaced an existing output" >&2
  exit 1
fi
if [[ "$(sha256_file "$candidate/READY")" != "$ready_sha256" ]]; then
  echo "existing private candidate output changed after a rejected rerun" >&2
  exit 1
fi

echo "Private dictionary candidate pipeline test passed"
