# Slime

This is a lightweight Japanese IME aiming to run entirely locally. It is currently implemented for macOS.

---

軽量・ローカル完結を目指す日本語IME。現在はmacOS向けに実装しています。

- Mozc OSS由来の基本辞書, ユーザー辞書, 分野別辞書（テクノロジー・ビジネス・クリエイティブ）
- かな数詞の合成候補（半角・全角・漢数字）
- 利用履歴からのローカル補完・管理の容易さ
- かなモードのまま打った英単語の逆変換候補（ぎてゅb→GitHub、experimental）
- 外部IMEからの辞書読み込み（Mac, Google日本語入力など）
- 直前の確定語に応じて候補を出し分ける、永続化しないセッション内文脈学習

設定は、メニューバーの歯車から「Slime設定…」を選びます。ユーザー辞書タブの「辞書を読み込む…」から既存IMEの書き出しファイルを移行できます。

## License

MIT([LICENSE](LICENSE)）。同梱するMozc由来の辞書・接続データは別ライセンスで、[crates/slime-converter/data/MOZC_DICTIONARY_LICENSE.txt](crates/slime-converter/data/MOZC_DICTIONARY_LICENSE.txt)のnoticeが適用されます。ビルドした.appにはこのnoticeを同梱します。
