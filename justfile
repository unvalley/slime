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

# debugビルドする
build:
    cargo build --workspace

# releaseビルドする
build-release:
    cargo build --workspace --release

# Swiftなどから接続するmacOS向けdylibを生成する
build-ffi:
    cargo build --release -p ime-ffi
    @echo "Generated target/release/libime_ffi.dylib"

# 全micro benchmarkを実行する
bench:
    cargo bench -p ime-romaji --bench romaji
    cargo bench -p ime-converter --bench converter
    cargo bench -p ime-core --bench engine

# 反復回数を減らした短時間のmicro benchmarkを実行する
bench-smoke:
    IME_BENCH_ITERATIONS=10000 cargo bench -p ime-romaji --bench romaji
    IME_BENCH_ITERATIONS=10000 cargo bench -p ime-converter --bench converter
    IME_BENCH_ITERATIONS=10000 cargo bench -p ime-core --bench engine

# benchmarkを実行せず、コンパイルだけ確認する
bench-build:
    cargo bench --workspace --no-run

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
