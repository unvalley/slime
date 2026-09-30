#!/bin/bash
set -euo pipefail

repository_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repository_root"

obsolete_paths=(
  "docs/billing.md"
  "platforms/macos/Sources/LicenseSettingsView.swift"
  "platforms/macos/Sources/Licensing.swift"
  "platforms/macos/Tests/LicensingTests.swift"
)

for obsolete_path in "${obsolete_paths[@]}"; do
  if [[ -e "$obsolete_path" ]]; then
    echo "Commercial boundary check failed: obsolete public path exists: $obsolete_path" >&2
    exit 1
  fi
done

if grep -ERin \
  --include='*.swift' \
  'SlimeAccessController|LicenseSettingsView|SLIME_BILLING_|checkout|billing_environment' \
  platforms/macos/Sources; then
  echo "Commercial boundary check failed: commercial access logic entered the macOS runtime." >&2
  exit 1
fi

if grep -ERin \
  --include='*.swift' \
  'URLSession|https?://' \
  platforms/macos/Sources; then
  echo "Commercial boundary check failed: the macOS input runtime gained a network dependency." >&2
  exit 1
fi

if grep -ERin \
  --include='*.rs' --include='*.cpp' --include='*.h' \
  'SlimeAccessController|checkout_url|billing_environment|reqwest|ureq|https?://' \
  crates/slime-core/src crates/slime-ffi/src platforms/windows/native/src; then
  echo "Commercial boundary check failed: commercial or network logic entered the public core." >&2
  exit 1
fi

required_contracts=(
  "契約やネットワークの状態をキーイベント処理の前提にせず"
  "年額2,400円（税込）の自動更新契約"
  "月額契約と無料トライアルは設けない"
  "決済日から10日以内の全額返金"
  "解約は次回の自動更新を停止"
  "初回商用版では端末activationを実装しない"
  "契約終了後も入力に使え"
  '`/legal`、`/terms`、`/privacy`、`/support`'
  "残る間はcheckoutを有効にしない"
)

for required_contract in "${required_contracts[@]}"; do
  if ! grep -Fq "$required_contract" docs/commercial-distribution-boundary.md; then
    echo "Commercial boundary check failed: missing contract: $required_contract" >&2
    exit 1
  fi
done

if grep -Ein 'checkout|無料トライアル|月額|購入する' landing/src/index.html; then
  echo "Commercial boundary check failed: the public preparation page contains a purchase path." >&2
  exit 1
fi

if ! grep -Fq "公開準備中" landing/src/index.html; then
  echo "Commercial boundary check failed: the public preparation state is not explicit." >&2
  exit 1
fi

echo "Commercial boundary checks passed."
