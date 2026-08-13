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
- 反復して確定した局所的な語のつながりを端末内だけで再利用する文脈学習
- 明示的に有効化した場合だけ、元の入力を残して訂正理由を表示するローマ字入力ミス候補
- F6〜F10による、ひらがな・カタカナ・半角カタカナ・全角英数・半角英数変換
- 変換中の左右キーによる文節移動と、Shift+左右キーによる文節伸縮
- 選択中の文字列をCtrl+Shift+Rで変換し直す再変換
- 「きのう」「きょう」「あした」「いま」からの日付・時刻候補（日付形式は設定可能）
- 履歴の参照と学習を一時停止する、プロセス内だけのプライベートモード

設定は、メニューバーの歯車から「Slime設定…」を選びます。ユーザー辞書タブの「辞書を読み込む…」から既存IMEの書き出しファイルを移行できます。
プライベートモードも同じメニューから切り替えられ、Slimeを終了すると解除されます。macOSのセキュア入力中は自動的に同じ保護を適用します。

## License

Slime IME is licensed under the MIT License.

This project includes third-party components that are distributed
under their respective licenses. See [MOZC_DICTIONARY_LICENSE.txt](crates/slime-converter/data/MOZC_DICTIONARY_LICENSE.txt).

External dictionary packs are separate products and are not covered by the
Slime IME MIT License. See [Dictionary packs](docs/dictionary-packs.md).
