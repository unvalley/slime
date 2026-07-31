# Slime IME

This is a lightweight Japanese IME aiming to run entirely locally. It is currently implemented for macOS.
Currently it's for me.

---

軽量・ローカル完結を目指す日本語IME。現在はmacOS向けに実装しています。

- Mozc OSS由来の基本辞書, ユーザー辞書, 無料の分野別辞書（テクノロジー・ビジネス・クリエイティブ）
- 別ライセンスの追加辞書パックをローカルにインストールできる拡張境界
- かな数詞の合成候補（半角・全角・漢数字）
- 入力全文を毎キー再評価し、曖昧な場合は読みを保つライブ変換
- 利用履歴からのローカル補完・管理の容易さ
- かなモードのまま打った英単語の逆変換候補（ぎてゅb→GitHub、experimental）
- 外部IMEからの辞書読み込み（Mac, Google日本語入力など）
- 直前の確定語に応じて候補を出し分ける、永続化しないセッション内文脈学習

設定は、メニューバーの歯車から「Slime設定…」を選びます。ユーザー辞書タブの「辞書を読み込む…」から既存IMEの書き出しファイルを移行できます。

ライブ変換の状態設計、UX上の保証、既知の限界は
[docs/live-conversion.md](docs/live-conversion.md)にまとめています。

## License

Slime IME is licensed under the MIT License.

This project includes third-party components that are distributed
under their respective licenses. See [MOZC_DICTIONARY_LICENSE.txt](crates/slime-converter/data/MOZC_DICTIONARY_LICENSE.txt).

External dictionary packs are separate products and are not covered by the
Slime IME MIT License. See [Dictionary packs](docs/dictionary-packs.md).
