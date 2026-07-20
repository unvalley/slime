# IME

軽量・ローカル完結を目指す日本語IMEです。現在はmacOS向けに実装しています。

## 現在の実装

- 1,085,464 entryのMozc OSS由来基本辞書と接続costを使う文節変換
- かな数詞の合成候補（半角・全角・漢数字）と、辞書外の読みへのカタカナ未知語候補
- macOS InputMethodKitアダプター、候補UI、Live Conversion
- 辞書登録の有無にかかわらず、変換候補へ全角カタカナ表記を重複なく追加
- ユーザー辞書と、利用履歴からのローカル補完
- 独立して有効化できる、合計344語の「テクノロジー」「ビジネス」「クリエイティブ」辞書
- Mac日本語入力の`Text Substitutions.plist`、旧Kotoeri、Google日本語入力/Mozc、Microsoft IME、ATOK辞書の読み込み
- ユーザー辞書の検索・書き出し、入力履歴の検索・学習一時停止・使われない履歴の整理・個別削除・全消去
- 直前の確定語に応じて候補を出し分ける、永続化しないセッション内文脈学習
- macOSのsecure event input中は履歴学習を自動停止

設定は、メニューバーの歯車から「Unvalley IME設定…」を選びます。ユーザー辞書タブの「辞書を読み込む…」から既存IMEの書き出しファイルを移行できます。

実装と研究上の判断は[`docs/dictionary-personalization-and-migration.md`](docs/dictionary-personalization-and-migration.md)、再現可能な速度計測は[`docs/benchmarking.md`](docs/benchmarking.md)、AJIMEE-Benchによる変換精度評価は[`docs/evaluation.md`](docs/evaluation.md)に記録しています。`just evaluate-ajimee`で現在の変換品質を再計測できます。

`./scripts/test-macos-adapter.sh`はRust engineのactionを実際のAppKit `NSTextView`へ適用し、marked textから候補確定までをユーザー文書に触れず検証します。OSによる入力ソース選択と対象アプリへの実キー配送はこのテストの範囲外です。
