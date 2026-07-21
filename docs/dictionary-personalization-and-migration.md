# 辞書・個人適応・移行の実装方針

- 調査日: 2026-07-20
- 対象: 分野別辞書、ユーザー辞書、入力履歴、既存IMEからの移行

## 結論

1. 基本辞書、分野別辞書、ユーザー辞書を、同じ検索インターフェースを持つ独立レイヤーとして扱う。
2. 分野別辞書は既定で無効にし、必要な語彙だけを設定から追加する。語彙追加と同時にmust-pass変換テストを用意する。
3. 入力履歴は確定結果を無条件に保存しない。短すぎる入力、読みと同じ表記、ほとんど入力を短縮しない補完を除外する。
4. 移行は元ファイルを変更せず、解析、正規化、重複排除が完了してからUnvalleyのユーザー辞書へ追加する。
5. 形式を推測できない行を勝手に登録せず、追加件数と除外件数を利用者へ返す。

## 複数辞書

Mozcの製品設計と辞書圧縮研究では、辞書検索を変換器から分離し、読みのprefix検索を維持した圧縮表現を採用している。Unvalleyの現在の実装は圧縮形式を固定せず、`DictionaryLayer`を変換器の境界にした。

各レイヤーは次を所有する。

- 安定したIDと表示名
- 読みでソートされたentry列
- そのレイヤーで最長の読み
- 読み、表記、左右品詞ID、単語cost

完全一致候補とViterbiラティスは、同じレイヤー集合を横断する。これにより、追加辞書の語が単語候補だけでなく文章変換にも参加する。一方、分野別辞書を無効にした場合は基本辞書1層の高速経路を使う。

分野別辞書は、テクノロジー107語、ビジネス118語、クリエイティブ119語の3種類とする。各行は`読み・表記・任意の単語cost`を持ち、固有表記は強く、一般語と競合する語は弱く順位付けする。読みの文字種、列数、cost範囲、重複、必須語彙をテストし、「けっさい→決済」「らすと→ラスト」のような一般語を全辞書ONでも壊さない回帰を持つ。

表記確認には、[Googleの製品名スタイルガイド](https://developers.google.com/style/product-names)、[デジタル庁デザインシステム](https://design.digital.go.jp/dads/guidance/design-system/)、[Adobeのカラーモデル解説](https://helpx.adobe.com/jp/creative-cloud/apps/colors/understand-color-modes.html)、[日本取引所グループの用語集](https://www.jpx.co.jp/glossary/all/index.html)、[国税庁のインボイス制度解説](https://www.nta.go.jp/taxes/shiraberu/taxanswer/shohi/6498.htm)を参照した。外部の用語集ファイル自体は同梱せず、語と表記をレビューした小さな補助辞書として管理する。

## 入力履歴

[Suzuki and Gao (2005)](https://aclanthology.org/H05-1034/)は、個人適応をCER改善だけでなく、元は正しかった変換を壊す副作用でも評価している。[Mozcの設計論文](https://www.anlp.jp/proceedings/annual_meeting/2011/pdf_dir/C4-3.pdf)も、選択結果を無条件に部分読みへ波及させない交換可能制約を説明している。

今回の履歴規則は次の通り。

- 読み3文字未満は保存しない。
- 表記2文字未満は保存しない。
- 読み64文字超、表記128文字超の文章サイズの確定は補完履歴として保存しない。
- 読みと表記が同一の確定は保存しない。
- 旧版で保存済みの短い・同一表記・日本語読みでない履歴も候補生成から除外し、元ファイルは明示的な整理まで保持する。
- 補完後に未入力部分が1文字だけの候補は表示しない。
- 累計5回以上使った語だけを永続履歴からの補完候補に出す。5回未満でも完全一致読みの変換順位づけとセッション内文脈候補には従来どおり使う。
- 補完として選ばれた候補は、短いprefixではなく元の完全な読みの利用時刻と回数を更新する。
- 最新利用を第一順位、利用回数を同時刻のtie-breakerにする。
- 直前の確定語と今回の候補の関係を最大128件だけセッション内で保持し、同じ文脈では全体のLRUより優先する。
- 「履歴候補を使う」と「新しい確定結果を学習する」を独立して停止できる。
- macOSのsecure event input中はユーザー設定より優先して新規学習を停止し、通常入力へ戻ると元の設定へ復帰する。
- 個別削除、現在の条件では使われない履歴の整理、全消去を提供し、ユーザー辞書には影響させない。

履歴ファイルは従来の4列形式を維持し、直前の文章を新たに保存しない。代わりに、現在のIME processで観測した`直前の読み・表記・今回の完全な読み・選択表記`の関係だけをLRU順で最大128件保持する。句読点、短い確定、学習停止への切り替えで直前文脈を切り、secure inputをまたいで関係を作らない。再起動後は従来の永続LRUだけへ戻るため、文脈による改善と生テキスト保存範囲の拡大を切り離せる。次段階では時間減衰を評価する。

secure input判定にはHIToolboxの`IsSecureEventInputEnabled()`を使う。このAPIは他プロセスを含むシステム全体の状態を返しthread-safeではないため、InputMethodKitのイベント処理ごとにメインスレッドから確認する。状態が変わったときだけRust engineへ実効設定を送り、secure input中は`history_learning=false`にする。想定外にメインスレッド外から呼ばれた場合も、APIを呼ばず学習停止側へ倒す。

[Google日本語入力のFAQ](https://support.google.com/ime/japanese/answer/166771?hl=ja)も、入力履歴に基づくサジェスト、学習候補の一時非表示、学習履歴の消去を別の操作として提供している。Unvalleyではさらに候補利用と新規学習を分け、既存履歴を利用しながら今の入力だけを記録しない状態を選べるようにした。

順位は[Mozcの`UserHistoryPredictor::GetScore`](https://github.com/google/mozc/blob/master/src/prediction/user_history_predictor.cc#L2442-L2451)と同じく最終利用時刻を主軸にする。これにより、過去に100回使った候補でも、別候補をいま1回選び直せば次回は新しい選択が先頭になる。回数は同一時刻の安定したtie-breakerに限定し、古い頻度が訂正を永久に妨げないようにした。Mozcがbigram関係をLRUよりboostする設計も参考にし、Unvalleyではまず永続化しないセッション文脈に限定して副作用を小さくした。

## Mac・Google・Microsoft IMEからの移行

Appleの[日本語ユーザ辞書ガイド](https://support.apple.com/ja-jp/guide/japanese-input-method/jpim10228/mac)は、選択した項目を`Text Substitutions.plist`として書き出し、同じ画面へドラッグして読み込む手順を案内している。Appleの書き出しには世代差があるため、`shortcut`/`phrase`と`replace`/`with`の両方を受け付ける。

Mozcの[`user_dictionary_importer.cc`](https://chromium.googlesource.com/external/mozc/+/master/src/dictionary/user_dictionary_importer.cc)は、先頭行とタブの有無からMicrosoft IME、ATOK、Kotoeri、Mozc形式を判定し、重複をfingerprintで除外している。Unvalleyの初期移行機能は次をサポートする。

| 移行元 | 入力 | 対応 |
| --- | --- | --- |
| Mac日本語入力 | XML/binary `Text Substitutions.plist` | 対応 |
| Google日本語入力 / Mozc | UTF-8/UTF-16/Shift JISのタブ区切り | 対応 |
| Microsoft IME | `!Microsoft IME`ヘッダー付きタブ区切り | 対応 |
| ATOK 11以降 | 専用ヘッダー付きタブ区切り | 対応 |
| 旧Kotoeri | 引用符付きCSV | 対応 |

読みは前後空白を除去し、カタカナをひらがなへ正規化する。元ファイルは読み取り専用で扱う。1回の読み込み上限は100,000件とし、不正行、重複、空の読み・表記を除外件数として報告する。

## 性能判断

追加辞書を導入しても、存在し得ない長さの部分文字列を全て二分探索する必要はない。各レイヤーの最長読みを保持し、それを超えたprefix探索を打ち切る。

2026-07-20、Apple M3、arm64、Release、各10 sampleで100文字Live Conversionを比較した。

| metric | baseline | after | 変化 |
| --- | ---: | ---: | ---: |
| p50 | 32.953 ms | 30.987 ms | -6.0% |
| p95 | 33.273 ms | 31.146 ms | -6.4% |

10文字ではp50が約0.122 msから約0.124 msとなり、約0.002 msの固定費が増えた。絶対値は入力予算より十分小さく、長文のtail改善が大きいため今回は維持する。辞書無効時の単一レイヤー分岐は残しており、今後の変更でも短文と長文を別々に測る。
