set shell := ["bash", "-euo", "pipefail", "-c"]

# 利用可能なコマンドを表示する
default:
    @just --list

# 開発環境のバージョンを確認する
doctor:
    rustc --version
    cargo --version
    just --version
    cc --version | head -n 1

# Rustコードを整形する
fmt:
    cargo fmt --all

# コードが整形済みか確認する
fmt-check:
    cargo fmt --all -- --check

# Clippyで静的解析する
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Rustの全テストを実行する
test:
    cargo test --workspace

# CからRust FFIを呼べることを確認する
test-ffi:
    scripts/test-c-ffi.sh

# 公開入力経路へ商用認証・ネットワーク依存が戻っていないことを確認する
test-commercial-boundary:
    scripts/test-commercial-boundary.sh

# 辞書パック公開鍵のbuild-time形式・重複・上限を検証する
test-dictionary-pack-verification-keys:
    scripts/test-dictionary-pack-verification-keys.sh

# 署名済み辞書パックのrollback下限形式・重複・上限を検証する
test-dictionary-pack-version-floors:
    scripts/test-dictionary-pack-version-floors.sh

# private辞書候補の生成・評価・no-clobberを架空データで検証する
test-private-dictionary-candidate:
    scripts/test-private-dictionary-candidate.sh

# macOS release buildがcleanなGit revisionだけを受け入れることを検証する
test-macos-release-source-policy:
    scripts/test-macos-release-source-policy.sh

# macOS release evidenceのcanonical artifact bindingを検証する
test-macos-package-binding-policy:
    scripts/test-macos-package-binding-policy.sh

# macOS consumer sessionの全証拠が同一releaseへbindされることを検証する
test-macos-consumer-evidence-policy:
    scripts/test-macos-consumer-evidence-policy.sh

# format、lint、Rustテスト、C ABI、商用境界テストをまとめて実行する
check: fmt-check lint test test-ffi test-commercial-boundary test-dictionary-pack-verification-keys test-dictionary-pack-version-floors test-private-dictionary-candidate test-macos-release-source-policy test-macos-package-binding-policy test-macos-consumer-evidence-policy
    @echo "All checks passed."

# 端末内文脈学習と入力ミス訂正の改善・副作用を固定データで評価する
evaluate-adaptation:
    scripts/evaluate-adaptation.sh

# 外部TSVの候補recallと、追加辞書による回収・回帰件数を分類する
evaluate-recall input *args:
    cargo run --release --quiet -p slime-tools --bin slime-recall -- --input "{{input}}" {{args}}

# 外部fixtureで入力ミス訂正の回収・誤訂正・遅延を集計する
evaluate-typos positive negative *args:
    cargo run --release --quiet -p slime-tools --bin slime-typo-evaluate -- --positive "{{positive}}" --negative "{{negative}}" {{args}}

# 非公開fixtureを出力せず、追加辞書・文脈ルールの品質差を集計する
evaluate-context-pack data_dir input *args:
    cargo run --release --quiet -p slime-tools --bin slime-context-pack-evaluate -- --data-dir "{{data_dir}}" --input "{{input}}" {{args}}

# 辞書packを独立processで反復読込し、起動時間と最大RSSを集計する
evaluate-pack-startup data_dir *args:
    cargo run --release --quiet -p slime-tools --bin slime-pack-startup-evaluate -- --data-dir "{{data_dir}}" {{args}}

# かなprefixを逐次再生し、LIVE表示の巻き戻し・かな戻り・数字混入を集計する
evaluate-live-transitions input *args:
    cargo run --release --quiet -p slime-tools --bin slime-live-transition-evaluate -- --input "{{input}}" {{args}}

# 実際の非同期LIVE neural taskをかな入力の最終停止点で評価する
evaluate-live-neural model input *args:
    cargo run --release --quiet -p slime-ffi --features neural --example live_neural_evaluate -- "{{model}}" --input "{{input}}" {{args}}

# debugビルドする
build:
    cargo build --workspace

# releaseビルドする
build-release:
    cargo build --workspace --release

# Swiftなどから接続するmacOS向けdylibを生成する
build-ffi:
    cargo build --release -p slime-ffi
    @echo "Generated target/release/libslime_ffi.dylib"

# 全micro benchmarkを実行する
bench:
    cargo bench -p slime-romaji --bench romaji
    cargo bench -p slime-converter --bench converter
    cargo bench -p slime-core --bench engine

# 反復回数を減らした短時間のmicro benchmarkを実行する
bench-smoke:
    SLIME_BENCH_ITERATIONS=10000 cargo bench -p slime-romaji --bench romaji
    SLIME_BENCH_ITERATIONS=10000 cargo bench -p slime-converter --bench converter
    SLIME_BENCH_ITERATIONS=10000 cargo bench -p slime-core --bench engine

# benchmarkを実行せず、コンパイルだけ確認する
bench-build:
    cargo bench --workspace --no-run

# AJIMEE-Benchでかな漢字変換の難例精度を評価する（held-out。調整には使わない）
evaluate-ajimee *args:
    scripts/evaluate-ajimee.sh {{args}}

# JWTD v2 trainから開発セットを生成する
build-devset:
    scripts/build-devset.sh

# JWTD trainの固定dev非重複部分から文脈モデル評価用の注釈コーパスを生成する
build-jwtd-context-corpus:
    scripts/build-jwtd-context-corpus.sh

# 開発セットで変換品質を評価する（コスト・モデル調整はこちらで行う）
evaluate-dev *args:
    scripts/evaluate-dev.sh {{args}}

# UD Japanese GSD (news/blog) から外部ドメイン開発・最終testセットを生成する
build-balanced-devset:
    scripts/build-balanced-devset.sh

# 外部ドメインdevで同音異義語の文脈順位を評価する（testはモデル凍結後だけ使う）
evaluate-balanced-dev *args:
    scripts/evaluate-balanced-dev.sh {{args}}

# ニューラルrescoring評価用のzenz GGUFモデルを取得する
fetch-neural-model:
    scripts/fetch-neural-model.sh

# CI相当の検証をローカルで実行する
ci: check bench-build

# macOS Swiftアダプターのテストを実行する
test-macos:
    scripts/test-macos-adapter.sh

# macOS Swiftアダプターのmicro benchmarkを実行する
bench-macos:
    bash scripts/benchmark-macos-adapter.sh

# macOS入力メソッドbundleをビルドする
build-macos:
    scripts/build-macos.sh

# macOS入力メソッドbundleの構造、署名、リンクを検証する
verify-macos: build-macos
    scripts/verify-macos-bundle.sh

# 未署名PKGでもpayload・version・lifecycle scriptの構造policyを回帰検証する
test-macos-package-policy: build-macos
    scripts/test-macos-package-policy.sh

# TextEditの物理キーがInputMethodKitへ届いたことを内容非記録で確認する
macos-textedit-input-gate command:
    scripts/macos-textedit-input-gate.sh "{{command}}"

# private/secure input中の履歴不変と解除後の学習再開を内容非記録で確認する
macos-input-privacy-gate command:
    scripts/macos-input-privacy-gate.sh "{{command}}"

# 物理入力gateのshell構文を対話sessionなしで検証する
test-macos-input-gate-syntax:
    bash -n scripts/dictionary-pack-verification-keys.sh scripts/dictionary-pack-version-floors.sh scripts/macos-package-binding.sh scripts/macos-release-source.sh scripts/build-macos.sh scripts/verify-macos-bundle.sh scripts/verify-macos-release.sh scripts/test-macos-package-lifecycle.sh scripts/verify-macos-consumer-evidence.sh scripts/macos-console-state.sh scripts/macos-textedit-input-gate.sh scripts/macos-input-privacy-gate.sh scripts/test-macos-input-privacy-gate-policy.sh scripts/test-macos-textedit-input-gate-policy.sh scripts/test-macos-console-state-policy.sh scripts/test-macos-package-binding-policy.sh scripts/test-macos-consumer-evidence-policy.sh scripts/test-dictionary-pack-verification-keys.sh scripts/test-dictionary-pack-version-floors.sh scripts/test-macos-release-build-policy.sh scripts/test-macos-release-source-policy.sh
    scripts/test-macos-console-state-policy.sh
    scripts/test-macos-release-build-policy.sh
    scripts/test-macos-input-privacy-gate-policy.sh
    scripts/test-macos-textedit-input-gate-policy.sh

# 署名状態に依存しないmacOS PKGの構造policyを検証する
verify-macos-package package:
    scripts/verify-macos-package-structure.sh "{{package}}"

# Developer ID署名済みbundleからInstaller署名済みpkgを作る
build-macos-release-pkg build:
    SLIME_RELEASE_BUILD=1 SLIME_BUILD_NUMBER="{{build}}" scripts/build-macos.sh
    scripts/build-macos-pkg.sh

# 公証・staple済みpkgの商用配布gateを検証する
verify-macos-release package:
    scripts/verify-macos-release.sh "{{package}}"

# 使い捨てmacOS VMで署名済みPKGのinstall/update/uninstallを検証する
test-macos-package-lifecycle current previous="":
    sudo -E scripts/test-macos-package-lifecycle.sh --current "{{current}}" {{ if previous != "" { "--previous '" + previous + "'" } else { "" } }}

# システム領域のmacOS版を削除し、ユーザーデータは保持する
uninstall-macos-system:
    scripts/uninstall-macos-system.sh

# macOS版をまとめて検証する
check-macos: check test-macos verify-macos test-macos-package-policy test-macos-input-gate-syntax

# Windows TSFアダプターをx64/x86/ARM64向けに型検査する
check-windows:
    scripts/check-windows.sh

# Slime専用Landingを生成し、価格・trial・ホスト境界を検証する
check-landing:
    cd landing && pnpm build && pnpm check

# slime.unvalley.meへ静的Landingをdeployする
deploy-landing:
    cd landing && pnpm run deploy

# Git管理外の開発用追加辞書をApplication Supportへ配置する
install-local-dictionary-packs:
    scripts/install-local-dictionary-packs.sh

# Git管理外の開発用追加辞書を形式検証する
validate-local-dictionary-packs:
    cargo run -q -p slime-tools --bin slime-dictionary-pack -- validate .slime-private/dictionary-packs/*.slime-dict

# 署名・rollback下限・期待件数を語彙非出力で最終検証する
verify-signed-dictionary-packs data_dir keys floors expected *args:
    cargo run -q -p slime-tools --bin slime-dictionary-pack -- verify-signed --data-dir "{{data_dir}}" --verification-keys "{{keys}}" --version-floors "{{floors}}" --expected-packs "{{expected}}" {{args}}

# 非公開の注釈corpusから保守的な左文脈ルールTSVを生成する
generate-context-rules input output *args:
    cargo run -q -p slime-tools --bin slime-context-rules -- --input "{{input}}" --output "{{output}}" {{args}}

# 非公開の注釈corpusから生成欠落している固有語・複合語TSVを作る
generate-term-dictionary input output *args:
    cargo run -q -p slime-tools --bin slime-term-dictionary -- --input "{{input}}" --output "{{output}}" {{args}}

# private語彙・文脈を生成し、複数splitのgateを通ったunsigned候補だけを公開する
prepare-private-dictionary-candidate *args:
    scripts/prepare-private-dictionary-candidate.sh {{args}}

# macOS版をユーザー領域へインストールして選択する
install-macos: check-macos
    scripts/install-macos.sh

# macOS版をシステム領域へ管理者インストールして選択する
install-macos-system: check-macos
    scripts/install-macos-system.sh

# インストール済みmacOS版へ入力ソースを切り替える
select-macos:
    scripts/select-macos-input-source.sh

# Cargoの生成物を削除する
clean:
    cargo clean
