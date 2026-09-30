# LIVE逐次入力の回帰比較

`crates/slime-tools/testdata/live_romaji_prefix.json`は手作りの44例。
人物・物の並列、助詞、活用、「ところ」などを含む。
既知の未解決例もあり、全例正解を保証するデータではない。
モデル学習から独立した評価集合や、一般的な入力頻度の代表とも扱わない。

`input`は製品のローマ字表から生成したキー列、`kana_input`は元の読み、
`context_text`は左文脈、`expected_output`は許容表記。
「子供／子ども」「ちょうど／丁度」など一部の表記差は複数解にしている。
この許容解は過去の実験値を遡って変更するものではない。

## 記録と比較

モデル同梱QAアプリから、隔離コピーで記録できる。macOSとXcodeのSwiftツールチェーンが必要。
アプリ内のFFIと現在のヘッダー・Swiftアダプタは互換である必要がある。
実行先は新しいディレクトリにし、既存の記録を上書きしない。

```sh
python3 scripts/evaluate-macos-live-replay.py target/macos/Slime.app \
  crates/slime-tools/testdata/live_romaji_prefix.json target/live-before
```

修正後のQAアプリでも別の出力先へ同じコマンドを実行する。
同梱ライブラリ・モデル、入力、使用したヘッダーとSwiftソースのハッシュを
`manifest.json`に残す。入力ごとに隔離プロファイルを使い、履歴補完・学習を無効にする。
コピーだけをad-hoc署名し、利用中のIMEのインストールや切り替えは行わない。

記録用プローブは各入力文字の後に利用可能なモデルタスクを同期実行する。
入力途中の意図しない確定がなく、Space変換後のEnterが表示内容を確定することを検査する。
`--direct-live-enter`を付けるとSpaceを送らず、LIVE表示から直接Enterして確定内容が一致することを検査する。
このモードは`explicit`の代わりに`committed`と`commit_mode`を出力し、上記の比較スクリプトには渡さない。
物理キー、InputMethodKit、実時間debounceはこのプローブの検証外。
途中失敗の出力は`results.pending.json`に残し、成功時だけ`results.json`を作る。

比較には、このSwiftアダプタで記録したJSON配列を使用する。
各行は`index`、最終表示`live`、明示変換`explicit`、許容表記配列`expected`を持つ。
両方の実行で同じ入力集合・許容解・モデル・設定を使い、履歴の影響を隔離する。
この比較スクリプト単体は、実際の入力経路や途中の動作を検証しない。

```sh
python3 scripts/compare-live-replay.py target/live-before/results.json \
  target/live-after/results.json > comparison.json
```

- 終了値0：正解からの悪化、未解決表記の変化、明示変換の変化なし。
- 終了値1：正解から不正解への悪化あり。
- 終了値2：未解決表記または明示変換が変化し、要確認。入力データ不正も2で終了する。

IDで対応を取るため行順には依存しない。重複・欠落・許容解の変更はエラーにする。
終了値0でも、IME全体の品質や未知の入力での非退行を証明するものではない。

## 実際に検出した例

最初の試作では200件の完全一致数110を保ったまま、既に全文不正解だった例の
「とき→と気」「サタン→左端」が悪化した。これらは`unresolved_changes`で検出する。
次案の追加24例では「母と娘」「人と言葉」が改善し、「母と板」「父とクラす」が悪化した。
改善2件・悪化2件なので完全一致数は15のままだが、終了値1となる。

詳細と実行証跡は[短い入力の診断](live-short-literal-diagnosis-2026-09-09.md)を参照。

新しい記録スクリプトは既知の2例で実行し、従来プローブの最終表示・明示変換・worker状態数と一致した。既存出力先を拒否すること、元QAアプリの署名とライブラリが保持されることも確認した。証跡は`target/evaluation/live-replay-runner-check-20260912/`にある。44例全部をこの新スクリプトで再実行した結果ではない。

直接LIVE Enterモードも `live-fallback-direct-enter-20260912` で44例を再生し、全例で表示と確定が一致した。比較対象のSpace Enter版と最終LIVE・worker状態数も一致した。この検証は表示スコア差fallbackの試作版で行い、採用QAアプリの結果ではない。
