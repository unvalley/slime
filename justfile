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

# format、lint、Rustテスト、C ABIテストをまとめて実行する
check: fmt-check lint test test-ffi
    @echo "All checks passed."

# 外部fixtureで入力ミス訂正の回収・誤訂正・遅延を集計する
evaluate-typos positive negative *args:
    cargo run --release --quiet -p slime-tools --bin slime-typo-evaluate -- --positive "{{positive}}" --negative "{{negative}}" {{args}}

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

# UD Japanese PUD (news/wiki) から独立held-outを生成する
build-pud-heldout:
    scripts/build-pud-heldout.sh

# 凍結済みモデルをUD Japanese PUD独立held-outで最終評価する
evaluate-pud-heldout *args:
    scripts/evaluate-pud-heldout.sh {{args}}

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

# macOS版をまとめて検証する
check-macos: check test-macos verify-macos

# Windows TSFアダプターをx64/x86向けに型検査する
check-windows:
    scripts/check-windows.sh

# Slime専用Landingを生成し、価格・trial・ホスト境界を検証する
check-landing:
    cd landing && pnpm build && pnpm check

# slime.unvalley.meへ静的Landingをdeployする（実Checkout URLが必須）
deploy-landing:
    cd landing && pnpm run deploy

# Git管理外の開発用追加辞書をApplication Supportへ配置する
install-local-dictionary-packs:
    scripts/install-local-dictionary-packs.sh

# Git管理外の開発用追加辞書を形式検証する
validate-local-dictionary-packs:
    cargo run -q -p slime-tools --bin slime-dictionary-pack -- validate .slime-private/dictionary-packs/*.slime-dict

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
