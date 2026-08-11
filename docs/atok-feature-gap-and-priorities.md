# ATOK機能調査とSlimeの優先課題

- 調査日: 2026-08-10
- 対象: ATOK Passport Tech Ver.36、Slime `main` + 現在の作業ツリー
- 判断軸: 変換品質、ローカル完結、軽量性、入力中の安全性、テスト可能性

## 結論

ATOKの強みを機能数として追いかけるのではなく、Slimeでは次の順で取り込む。

1. 文脈を使った候補順位と、候補集合に正解を残すこと
2. 一時的な入力で普段の表記を壊さない学習
3. 読みを勝手に壊さない、説明可能で保守的な入力ミス訂正
4. ローカル履歴による省入力と、誤学習を自分で管理できるUI
5. macOS/Windowsの実アプリで候補選択まで通る配布品質

生成AI、クラウド辞書、文章校正、辞典、連絡先・カレンダー連携はATOKの主要機能だが、Slimeの「軽量・ローカル完結・入力内容を外部へ送らない」という価値を弱める。少なくとも上記5項目より先には実装しない。

## ATOKの主要機能とSlimeへの判断

| ATOKの機能群 | 公式説明の要点 | Slimeの現状 | 判断 |
| --- | --- | --- | --- |
| 文脈変換 | 単語のつながりから自然な変換結果を提示 | 辞書規則と任意のローカルニューラル再順位付けが左右文脈を使用。確定表層を変換単位を越えて一時保持 | P0。候補外の正解回収と商用可能な順位モデルが残る |
| 変換強度の学習 | 一時的な選択の強度を抑え、自然な文章を優先する場合がある | 定着/一時の強度層に加え、文脈が変えた辞書1位を単発履歴より優先 | P0の基本境界を実装。実利用false positiveを継続評価 |
| 推測変換 | 数文字から長い候補、確定直後の候補を提示 | 5回以上使ったローカル履歴とセッション文脈による補完のみ | P1。ローカルのまま段階的に拡張 |
| 入力ミス訂正 | ローマ字・かな入力特有のミスを自動修復し、ガイダンスを表示 | 元の読みを保持する保守的な候補追加と、macOS候補ガイダンスを実装 | 実装済み。false positive評価を継続 |
| 誤学習の管理 | 間違って入力した学習を取り消せる | 設定で履歴を検索、1件削除、整理、全消去が可能 | 基本境界は実装済み。即時取り消しはP2 |
| 日付・数値入力支援 | 日付候補、数値・単位の入力支援 | 日付/時刻と数詞候補は実装。単位・桁確認ナビは未実装 | P2 |
| 表現モード・連想変換 | 話し言葉、方言、文語、類義語・言い換え | 未実装 | P2以降。まず通常文の精度を上げる |
| プライバシー/プロテクト | 学習抑制、会議中の推測候補抑制 | 手動プライベートモードとmacOS secure input連動は実装 | P1。会議検出より実アプリ検証を優先 |
| 専門辞書 | 分野別辞書や追加辞書を選択 | 基本辞書、3分野、外部辞書パック境界を実装 | 実装済み。品質・ライセンスgateを維持 |
| OS横断同期 | 学習・確定履歴・登録語を複数OSで同期 | 未実装。履歴は端末内のみ | P3。暗号化・競合・削除同期の設計が先 |
| 校正・辞典・生成AI | 不適切表現の指摘、辞典検索、文章書換え | 未実装 | 非目標。別製品機能として評価する |

## 現在のSlimeの課題

### P0: 変換品質

1. **high-accuracyモデルの有償配布gateが未完了。** 現在採用している公式zenz-v3.2-small GGUFはApache-2.0を宣言し、ライセンス全文・出典・upstream/fixed checksumを同梱境界へ追加済みである。v3.1のShareAlikeを商用候補として扱う必要はなくなった。一方、公式model cardには学習元の説明がなく、第三者権利の確認と法務レビューは残る。Q5は約73.9 MBでtail latencyも大きいため、通常buildは引き続きモデルなしとする。
2. **正解が上位10件に入らない語彙・複合語が残る。** 人名、固有名詞、生産的複合語が中心。語別コスト上書きではなく、候補recallと順位を別々に改善する必要がある。
3. **学習の副作用を測る実利用データが不足している。** 単発選択による定着候補の上書きと、単発履歴による文脈候補の上書きは回帰fixtureへ追加した。今後も「学習で直った件数」だけでなく「元の正解を壊した件数」を実利用由来の固定データで測る。
4. **ライブ変換と通常変換は品質契約が異なる。** ライブ変換はconfidence gateを持つが、最終候補評価だけではキーごとの不自然な途中表記を検出できない。逐次preedit回帰と実アプリ確認を継続する。

### P0: 製品としての動作境界

1. **macOSのインストール済みIMEをTextEditで操作する回帰が未完了。** 2026-08-07に最新bundleのインストール、対象/インストール済み実行ファイルとdylibのSHA-256一致、入力ソース選択、Slimeプロセス起動までは確認した。Computer Useの合成入力はTextEditへASCIIを直接設定してInputMethodKitを通らなかったため、Space、矢印、番号、クリック、再変換の証明には使っていない。
2. **Windowsは技術スパイクの段階。** x64/x86 TSF、候補UI、設定、unsigned installerは存在するが、署名、実機install/update/uninstall、Windows Search、Narrator、各アプリ互換性が未検証。
3. **配布・更新の完成条件が未達。** macOSのDeveloper ID署名、公証、インストーラー、更新・削除、Windowsのコード署名を製品gateとして別管理する必要がある。

### P1: 入力効率と安全性

1. **入力ミス訂正の実装後データがまだ小さい。** 元の読みを保持し、訂正候補を通常候補へ追加し、macOSでは訂正理由を表示する境界は実装した。今後は実利用でfalse positiveを収集し、許可規則を安易に広げない。
2. **補完は履歴依存でcold startに弱い。** 長い定型句、確定直後の次語、一般的な省入力データはない。巨大クラウド辞書ではなく、小さな同梱データとユーザー履歴を分離して評価する。
3. **候補の説明がない。** 同音語、訂正候補、日付、数値、履歴、ユーザー辞書の由来をUIで区別できず、誤選択の防止と学習管理につながらない。
4. **誤学習の即時取り消しがない。** 設定画面から1件削除できるが、確定直後に元へ戻す操作はない。
5. **候補recallは語種ごとに穴が残る。** 候補末尾到達後に短い読みはN-best 32、長い読みはまず16、さらに拡張末尾へ進んだ場合だけ32へ深掘りする。一般複合語と固定文節、人名は専用のbounded探索を行う。加えてモデル準備済みの`high-accuracy`だけ、9文字以上の内部再採点候補を32まで広げる。初回表示を重くせず回収範囲を広げたが、組織名・地名・未知の固有名詞は引き続き評価が必要である。

### P1: 性能とサイズ

1. **長文候補表示が性能予算を超える。** 2026-08-07のAJIMEE held-outではp95 23.66 msで、目標20 msを超えた。一方、JWTD devはp95 15.44 ms、短い曖昧語中心のGSD devはp95 1.12 msだった。入力長別p95をrelease buildで追う必要がある。
2. **ニューラルrerankerはhigh-accuracy限定で実装済み。** Apache-2.0のv3.2-small Q5は5 datasetの精度gateを通り、明示選択するprofileへ実装した。標準bundleへ含める判断は、約73.9 MBのサイズ、生成を含むtail latency、学習元確認が未完了のためNo-Goのままである。Q4_K_Mと蒸留studentは複数domain非悪化を満たさず不採用とした。
3. **実アプリend-to-endの性能計測が薄い。** Rust単体とInputMethodKitのmarked-text更新を分け、候補UI表示までを同一条件で測る必要がある。

### P2: 操作・アクセシビリティ

1. 候補ウインドウの文字サイズ、外観、ページング、キーマップのカスタマイズが限定的。
2. macOS候補UIのVoiceOver、クリック、複数ディスプレイ、縦書き、secure field切替を体系的に検証していない。
3. 話し言葉、文語、方言、人名優先などの表現モードがない。
4. 数値+単位、郵便番号、住所、人名などの日常入力支援が薄い。

### P3: 同期・クラウド・付加機能

1. ユーザー辞書・学習履歴の端末間同期がない。
2. 新語配信、地域語彙、クラウド辞書がない。
3. 校正、連想変換、辞典、翻訳、生成AI文章支援がない。

これらは差分ではあるが、Slimeの中心価値ではない。入力内容を外部へ送らない既定、アカウント不要、ネットワーク不要を壊す機能は、コアIMEから切り離された明示的なオプションとしてのみ再検討する。

## 今回の実装: 学習強度

従来は履歴を `last_used` 優先で並べていたため、100回使った候補でも、別候補を1回選ぶと次回から後ろへ下がった。

新しい規則:

- 5回未満は一時的な選択、5回以上は定着した選択として扱う。
- 定着候補は一時候補より先にする。
- 同じ強度の候補どうしは、従来どおり直近使用を優先する。
- `history.tsv` の形式、上限500件、プライベートモード、ライブ変換のconfidence gateは変更しない。

これはATOKの内部実装を再現するものではない。「一時的な表記が普段の表記を壊さない」という一般原則を、Slimeの既存データだけで説明可能に実装したものになる。

## 今回の実装: 2回確認による表記の切替

[ATOKの変換強度](https://atok.com/info/features/engine.html)は、直前の1回だけでなく継続的な入力傾向を使う。[長い推測候補の公式説明](https://atok.com/other/support/howtouse/mac/ip/pgs/ip_conv_auto_phrase.htm)でも、同じ入力を2回以上行った場合に学習すると説明されている。Slimeの従来の5回境界は単発誤選択に強い一方、ユーザーが普段の表記を意図的に変えても4回までは古い候補が先頭に残っていた。

新しい切替規則:

- 既に定着候補がある読みで別表記を1回選んでも、現在の候補を維持する。
- 同じ別表記を文脈なしで2回、または異なる文脈で反復した場合だけ、普段の表記を切り替える。
- 同一文脈だけで反復した表記は全体へ昇格せず、既存の文脈履歴だけで順位付けする。`日本の解答`を反復しても、文脈なしの`かいとう`は`回答`のままになる。
- 切替後に以前の候補を1回選んでも元へ戻さず、再び2回の確認を要求する。
- `history.tsv`の実使用回数と5回の補完表示条件は変更しない。2回確認だけで推測候補を解禁しない。

確認済み表記は最大500件の`history_preferences.tsv`へローカル保存する。壊れたファイルは上書きせず、対応する履歴が削除済みなら無視する。macOSの履歴管理で1件削除・整理・全消去を行った場合は、同じ表記の確認状態も同期して削除する。sidecarの同時変更を検出した場合は上書きせず、履歴本体を正とする。

コア回帰では、`漢字`100回の状態で`感じ`を1回選んでも`漢字`を維持し、2回確認後は再起動しても`感じ`を先頭にすること、`感じ`の実回数2では補完候補にならないこと、切替後の`漢字`1回では戻らないことを固定した。同一文脈の反復を全体へ漏らさない既存fixtureと、macOS設定からの個別削除も通している。

## 今回の実装: 文脈と単発履歴の変換強度

[ATOKの高精度な変換エンジン](https://atok.com/info/features/engine.html)は、同じ読みで直前に確定した語を常に優先するのではなく、日本語として自然な文章がある場合は自然さを優先すると説明している。[azooKey/Zenzai v3](https://github.com/azooKey/AzooKeyKanaKanjiConverter/blob/main/Docs/zenzai.md)も読みと候補に左右文脈を別tagで与える。Slimeではモデルを同梱せず、既に複数コーパスで非回帰を確認した辞書文脈規則とローカル履歴の競合だけを解決する。

- 周辺文脈によって辞書1位が文脈なしの1位から実際に変わった場合だけ判定する。
- 5回未満で2回確認も済んでいない単発履歴が同じ辞書候補集合にある場合、文脈1位を先にし、履歴候補は削除せず先頭より後ろに残す。
- 5回以上の定着履歴、2回確認済みの普段の表記、2回以上学習した同一文脈の履歴、ユーザー辞書、現在の辞書候補集合にない履歴は従来どおり先にする。
- 文脈なし、private mode、履歴無効時は追加判定を行わない。`history.tsv`形式も変更しない。

回帰fixtureでは、単発履歴が誤って先頭にある状態から`再帰的`、`真価が問われる`、`家宅捜索`、`石油化学`、`渡した`の5種類を文脈1位へ戻し、履歴候補が引き続き選べることを確認する。通常の履歴なし評価はGSD dev 331件、GSD test 323件、AJIMEE 200件、JWTD dev 400件の候補表層・cost・順序・labelが変更前と完全一致した。

500件の履歴を読み込んだ同一プロセスRelease測定4回では、文脈変換が履歴OFFで0.136–0.137 ms/op、単発履歴との競合判定ありで0.186–0.188 ms/opだった。追加は約0.05 msで、既存の履歴ON/OFF差5 ms未満という製品予算内に収まる。

## 今回の実装: 適応的な候補再探索

初回候補は従来どおりN-best 10件を使う。8文字以下の読みでは、ユーザーが候補の末尾を越えて進んだときだけ一度だけN-best 32件で再探索する。9文字以上は最初の末尾到達でN-best 16とbounded候補を追加し、さらにその末尾へ到達した場合だけN-best 32へ深掘りする。ライブ変換、補完、文節変換には適用しない。

`あさいり`では初回候補にない「浅煎り」を拡張後に取得できる。release benchmark 1,000反復では、同じ読みの初回候補が約0.676 ms/op、拡張候補が約3.460 ms/opだった。約2.8 msの追加コストを通常のキー入力から外し、深い候補を明示的に探す操作だけへ限定している。

長文の第二段階はJWTD 21件、AJIMEE 9件、PUD 2件を追加回収し、GSD dev/testは全件が初回候補内だった。50文字のRelease測定ではN-best 16が14.823 ms/op、32が51.764 ms/opだったため、32探索は第一拡張の末尾へさらに進んだ場合だけ実行する。

## 今回の実装: 人名候補の明示的な回収

[ATOKの人名優先変換](https://atok.com/other/support/howtouse/mac/tr/pgs/tr_conv_name.htm)は、連文節変換で人名を優先するモードを持つ。[azooKeyの辞書品詞定義](https://github.com/azooKey/AzooKeyKanaKanjiConverter/blob/93766c46e31fa6a18b7ced49dab31337780f6f45/Sources/KanaKanjiConverterModule/DictionaryManagement/Entry/CIDData.swift)も、一般固有名詞とは別に姓・名を区別する。Slimeは自動順位変更による誤変換を避け、ユーザーが候補末尾まで進んだ場合だけ、Mozc辞書の姓と名の品詞を2分割で組み合わせて最大64候補を追加する。

姓1件に対して同じ読みの名が48件先行する合成fixtureでは、N-best 32と一般複合語の各文節8件では失う表記を人名経路で回収した。初回候補とライブ変換は変更しない。Common Voice由来859件では人名経路による追加候補は0件で、通常top-1は776件のまま変化しなかった。一般的な6文字の姓+名の読みをrelease buildで1,000回測定すると0.011 ms/op、27候補だった。評価器は`initial / expanded / compound / personal_name / fixed_segment`を別々に報告する。

## 今回の実装: 再変換語彙と文脈証拠の人名境界

人名を含む逆引き語彙は明示的再変換と姓名の文脈補完に必要だが、一般語の一意性判定へ混ぜると`諏訪湖`に対する`諏訪子`のような偽の競合になる。逆引きrecordへ非人名POSの有無を追加byteなしで保持し、一般語候補の一意性を判定する場合だけ、姓・名POSしか持たない競合を除外する。人名phraseの通常順位と再変換readingは維持する。

GSD trainで`諏訪｜こ → 諏訪湖`を1件修正し、悪化0。GSD dev/test、AJIMEE、JWTD、PUDは候補配列まで完全一致した。逆引きartifactのサイズ増分はなく、GSD trainのRelease交互測定もp50 +0.0015 ms、p95 +0.0009 msで同じ測定帯だった。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 右側の付属語と一般名詞複合語

[ATOK 2026の公式一覧](https://www.atok.com/products/)は「付属語を含む語の変換を強化」と説明している。Slimeは右側のひらがなを一律に意味文脈として扱わず、候補と右prefixを連結した完全なMozc辞書語があり、その読みが現在の入力読みを延長する場合だけ証拠にする。さらに、ひらがな接続は同一POSの自立動詞へ限定し、1文字は`に`へ絞った。

逆引き索引には3〜8字の漢字からなる一般名詞→サ変名詞の低cost語2,883件だけを追加し、`下部組織`、`家宅捜索`、`化学反応`を回収した。GSD trainでは6件、devでは4件改善し悪化0、GSD testとAJIMEE held-out、JWTD devは全指標不変だった。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 「や・と」で結ぶ一般名詞

[ATOKのAI用例](https://atok.com/other/support/howtouse/mac/shrd/shrd_yougo.htm)が使う語の組み合わせと同様に、Slimeも既存Mozc辞書の完全な並列句を右文脈の証拠として使う。ただし意味推測は行わず、一般名詞POS、3〜8文字、漢字列を1個の`や`または`と`で結ぶ、辞書cost 7,500以下、右側の語境界が確認できる場合だけ既存候補を昇格する。

これにより`鼻や口`と`肩や背中`を各1件修正した。GSD train 1,940件、AJIMEE 200件、JWTD 400件、PUD phrase 446件は候補配列まで不変で、診断に使用したGSD dev/testにも悪化はなかった。逆引きartifactの増分は約10 KB、変換時の中央値差は約8.2 µs/opだった。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 一般名詞の「AのB」

並列句と同じく、既存Mozc辞書の完全な語の組み合わせを右文脈の証拠にする。一般名詞POS、3〜8文字、漢字列を1個の`の`で結ぶ、辞書cost 8,000以下、右側の語境界が確認できる、という条件をすべて満たす場合だけ既存候補を昇格する。候補生成は行わず、長い名詞の途中一致も使わない。

GSD trainで`胃の調子`、`素の状態`、`真の目的`、`未知の世界`、GSD devで`毛の根元`をtop-1へ修正し、悪化は0件だった。GSD test、AJIMEE、JWTD、PUDは候補配列まで不変だった。逆引きartifactの増分は約228 KB、50,000回×3の変換測定で右文脈参照の追加は中央値約6.1 µs/opだった。詳細は[evaluation.md](evaluation.md)に記録する。

残存誤りの再診断では、同じ完全辞書語境界をサ変名詞→サ変名詞の621句へ限定拡張し、GSD trainの`番組の司会`を1件修正した。任意POS組合せ4,430句は追加改善がないため採用しなかった。GSD dev/test、AJIMEE、JWTD、PUDは候補配列まで不変で、artifact増分は約30 KB、対象経路の中央値差は約0.8 µs/opだった。

## 今回の実装: 「最」で始まる完全辞書語

[ATOKの高精度な変換エンジン](https://atok.com/info/features/engine.html)が使う単語のつながりを、Mozcで専用品詞になっている最上級接頭辞`最`へ限定して適用する。直前文脈が`最`で終わり、`最 + 候補表層`が現在の読みと一致する完全辞書語の場合だけ既存候補を昇格し、`最下位`を回収する。一般の一文字境界は`正気`、`神化`、`海戦`、`一時`などの誤分割を起こしたため不採用とした。

GSD trainは`最下位`を1件修正してacc@1が0.7088から0.7093へ上がり、悪化0だった。GSD dev/test、AJIMEE、JWTD、PUDは候補配列まで不変で、逆引きartifactも増えない。Release測定では`最`一致経路が中央値約19.1 µs増えたが、通常の一文字末尾非一致経路は約0.35 µs増に留まった。

## 今回の実装: 右側に続く一文字接頭辞

[ATOKの品詞説明](https://atok.com/other/support/howtouse/mac/shrd/shrd_hinsi_other.htm)と[Mozcの名詞接頭詞POS](https://github.com/google/mozc/blob/master/src/data/dictionary_oss/id.def)に沿い、`未｜解決`のように一文字候補と右文脈が完全な辞書語になる場合だけ既存候補を昇格する。左側にも辞書句がある場合は、汎用接頭詞を除く個別POSの一文字候補に限定し、一般の右句を再解禁しない。

これにより`2012年8月現在｜み｜解決`を`見 → 未`へ修正した。GSD trainは1件改善・悪化0、GSD dev/testはtop-1不変、AJIMEE、JWTD、PUDは候補配列まで不変だった。逆引きartifactは増えず、Release測定の対象経路は中央値約3.3 µs増、接頭辞候補がない経路は測定誤差内だった。

## 今回の実装: 数字直後の一般名詞複合語

[ATOKのAI用例](https://atok.com/other/support/howtouse/mac/shrd/shrd_yougo.htm)が示す「登録された語の組み合わせ」を、数字直後でも構造が強い場合だけ使う。左末尾がASCIIまたは全角の10進数字で、候補と右文脈を結合した完全なMozc辞書語が一般名詞POSで始まり終わる場合に限り、既存候補を昇格する。日本語数詞、助数詞、動詞活用、固有名詞は従来の数値境界を維持する。

これにより`県警2課長`と`M9幹線道路`をtop-1へ修正した。GSD train/testで各1件改善・悪化0、GSD dev、AJIMEE、JWTD、PUDは候補配列まで不変だった。数字境界の全面解禁で強まった`架から`・`題目`は一般名詞POSと数字種別のgateで除外した。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 「する・して」が後続するサ変名詞

[ATOKのAI用例](https://atok.com/other/support/howtouse/mac/shrd/shrd_yougo.htm)が示す前後の語のつながりを、Slimeでは既存Mozc辞書の品詞へ限定して使う。右文脈が`する`または`して`で始まり、同じ読みにあるサ変名詞のうち最安表層が次点より十分に優勢な場合だけ、既存候補を昇格する。複数表層が近い場合は意味判断が必要なので順位を変えない。

GSD trainで`敵視して`を1件、診断済みのGSD testで`長居して`、`感心する`、`駆使する`、`位置する`を4件修正し、悪化は0件だった。GSD devはtop-1不変、AJIMEE、JWTD、PUDは候補表層・cost・順序まで全件不変だった。50,000回×3組の旧新交互測定では中央値差が約0.0058 ms/opだった。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 「する」の連用・過去・否定形

[ATOKの品詞対応表](https://atok.com/other/support/howtouse/mac/ap/pgs/ap_hinsi_atok8.htm)、[Mozcの名詞サ変接続とサ変活用](https://github.com/google/mozc/blob/master/src/data/dictionary_oss/id.def)、[azooKeyの左右接続ID](https://github.com/azooKey/AzooKeyKanaKanjiConverter/blob/main/Docs/dicdata_format.md)に沿い、既存の一意なサ変名詞だけを昇格する規則を`した`、`しない`、`します`、`しよう`と、句読点へ続く単独の`し`へ広げた。受動形`される`はGSD testの正解候補をtop-10から落とし、`せず`は一般名詞にも続くため採用しなかった。

これによりGSD trainの`前進し`、`即死しない`、`退位した`を修正し、悪化は0件だった。GSD dev/testはtop-1と正解順位が不変、AJIMEE、JWTD、PUDは候補配列まで不変だった。Release測定の中央値差は対象経路+0.792 µs、非該当経路+0.095 µsだった。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 「こと・もの・ため・ので・よう」への右品詞接続

[ATOKの高精度な変換エンジン](https://atok.com/info/features/engine.html)が重視する文法と単語のつながり、[azooKeyの左右接続ID](https://github.com/azooKey/AzooKeyKanaKanjiConverter/blob/main/Docs/dicdata_format.md)と[Viterbiでの接続重み](https://github.com/azooKey/AzooKeyKanaKanjiConverter/blob/main/Sources/KanaKanjiConverterModule/ConversionAlgorithms/Core/FullInputProcessing.swift)に沿い、右文脈先頭の`こと`・`もの`・`ため`・`ので`・`よう`をMozcの品詞IDで接続する。語彙costを重ねず、候補右POSから後続左POSへの相対接続costだけを最大1,100補正する。同じ活用型の意味候補は同量補正し、完全辞書句とは最大値だけを使う。

丁寧助動詞前の活用形補正も安全な範囲で強め、GSD trainの`寄ること`、`解くこと`、`誓います`、`飽きません`を4件、GSD testの`通うこと`を1件修正した。train/dev/testでtop-1悪化0、AJIMEE、JWTD、PUDは候補配列まで不変だった。汎用補正2,500は`診察です → 新札です`を起こしたため採用していない。Release測定の中央値差は対象経路+3.950 µs、非該当経路は測定誤差内だった。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: サ変名詞に続く一文字の一般名詞接尾語

[ATOKの変換エンジン](https://atok.com/info/features/engine.html)、[azooKeyの左右接続ID](https://github.com/azooKey/AzooKeyKanaKanjiConverter/blob/main/Docs/dicdata_format.md)、[Mozcの品詞定義](https://github.com/google/mozc/blob/master/src/data/dictionary_oss/id.def)に沿い、完全なMozc辞書語を文脈証拠として使う。全て漢字の三文字語、左POSがサ変名詞、右POSが一般名詞接尾語、cost 7,550以下の場合だけ文脈用逆引き索引へ加える。候補生成とword costは変えない。

これによりGSD trainの`傍聴｜けん`を`権 → 券`へ1件修正し、悪化は0件だった。GSD dev/test、AJIMEE、JWTD、PUDは候補配列まで不変、artifact増分は5,516 bytesだった。より広いcost帯はheld-outの誤候補`監査員`を強め、助詞への一般化も複数のtop-1回帰を起こしたため採用していない。Release測定の中央値差は一致経路+0.981 µs、非一致経路+1.410 µsだった。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 「第n代」と通常の「n台」の区別

[ATOKの数値入力支援](https://atok.com/other/support/howtouse/mac/ip/pgs/ip_num_assist.htm)と[azooKeyのSpecial Conversion](https://github.com/azooKey/AzooKeyKanaKanjiConverter/tree/main/Sources/KanaKanjiConverterModule/ConverterAPI/SpecialConversion)と同様、数値表記を一般の意味順位から分離する。整数直後の`だい`は従来どおり`台`とし、整数の直前が序数接頭辞`第`の場合だけ`代`を選ぶ。小数・負数には適用しない。

GSD trainの`第33代`を1件修正し、悪化は0件だった。GSD dev/test、AJIMEE、JWTD、PUDは候補配列まで不変、artifact増分もない。Mozcの専用助数詞POSを数値へ一律接続する案は`1字 → 一時`と`一突き → 一月`を強めたため採用しなかった。Release測定の中央値差は序数経路+0.116 µs、通常の台数経路+0.126 µsだった。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 割合直後の「増・減」

ASCII・全角の`%`直前が有効な整数、小数、全角数字、漢数字の場合だけ、後続の`ぞう`を`増`、`げん`を`減`へ寄せる。候補生成とword costは変えず、負数や数字のない`%`は通常変換へ残す。

GSD trainの`0.5%減`を`源 → 減`へ1件修正し、悪化は0件だった。GSD dev/test、AJIMEE、JWTD、PUDは候補配列まで不変、artifact増分もない。Release測定の中央値差は一致経路+0.802 µs、非一致経路+0.153 µsだった。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 整数直後の「匹・対」

[ATOKの数値入力支援](https://atok.com/other/support/howtouse/mac/ip/pgs/ip_num_assist.htm)と[azooKeyのSpecial Conversion](https://github.com/azooKey/AzooKeyKanaKanjiConverter/tree/main/Sources/KanaKanjiConverterModule/ConverterAPI/SpecialConversion)にならい、ASCII・全角・漢数字の整数直後だけ`ひき → 匹`、`つい → 対`を優先する。小数、負数、単独変換は従来順位を保ち、意味や表記方針が必要な`回・階`、`席・隻`、月数表記は対象外とする。

GSD trainの`42匹`と`4対`を2件修正し、悪化は0件だった。GSD dev/test、AJIMEE、JWTD、PUDは候補配列まで不変、artifact増分もない。Release測定の中央値差は一致経路-0.419 µs（測定誤差内）、非一致経路+0.094 µsだった。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 前後構造で決めるスポーツ記録・数値単位

[ATOKの数値入力支援](https://atok.com/other/support/howtouse/mac/ip/pgs/ip_num_assist.htm)と[azooKeyのSpecial Conversion](https://github.com/azooKey/AzooKeyKanaKanjiConverter/tree/main/Sources/KanaKanjiConverterModule/ConverterAPI/SpecialConversion)を参考に、数値単独では曖昧な表記を前後構造が揃う場合だけ補正する。`n勝n敗`、0〜2アウトに塁上表記が続く`n死`、整数と弦楽器名に挟まれた`弦`、整数と船舶名に挟まれた`隻`を既存候補内で優先する。

GSD trainの`1死一、二塁`、`2勝2敗`、`7弦ギター`、`3隻の客船`を4件修正し、悪化は0件だった。GSD dev/test、AJIMEE、JWTD、PUDは候補配列まで不変、artifact増分もない。Release測定の中央値差は一致経路+0.324 µs、非一致経路+0.383 µsだった。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 数字同士を結ぶ「対」

[ATOKの数値入力支援](https://atok.com/other/support/howtouse/mac/ip/pgs/ip_num_assist.htm)と[azooKeyのSpecial Conversion](https://github.com/azooKey/AzooKeyKanaKanjiConverter/tree/main/Sources/KanaKanjiConverterModule/ConverterAPI/SpecialConversion)を参考に、通常語のcost変更ではなく構造化表記として実装する。左右が整数の`1｜たい｜1`型に限って既存候補の`対`を優先し、小数や片側だけの数値には適用しない。

GSD train/testで`1対1`を各1件修正し、悪化は0件だった。GSD dev、AJIMEE、JWTD、PUDは候補配列まで不変で、Release測定の中央値差は約0.00028 ms/opだった。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 低頻度の一般名詞複合語

Mozc辞書に存在しても通常の逆引きcost上限6,500を少し超える一般名詞複合語を、候補追加ではなく直前文脈の証拠として使う。対象は左右POSが一般名詞、3〜8字の漢字列、cost 7,000以下をすべて満たす2,192 entryだけとし、人名・地域名・かな混じり表記は索引へ加えない。

GSD testの`線形作用｜そ`を`祖 → 素`へ1件修正し、GSD train/devのtop-1悪化は0件だった。AJIMEE、JWTD、PUDは候補表層・cost・順序まで全件不変だった。逆引きartifactの増分は約77 KB、Release測定の中央値差は約0.252 µs/opだった。詳細は[evaluation.md](evaluation.md)に記録する。

残存誤りをtrain/devで再評価し、cost上限7,200へ1,277 entryだけ追加した。`三陸沖`と、右文脈の`勲一等`を使う`叙正三位｜くん｜一等…`の2件を修正し、悪化は0件だった。7,500は追加改善がないため採用しなかった。境界固定後のGSD test、AJIMEE、JWTD、PUDは候補配列まで不変で、artifact増分は約48 KB、対象経路の中央値差は最大約0.302 µs/opだった。

## 今回の実装: 「たい」へ接続する動詞連用形

右文脈が`たい`で始まる場合だけ、Mozcの助動詞POSへの接続costを使い、接続可能な一語候補を最大1,500 cost昇格する。候補表層が同じでも名詞経路と動詞経路を区別し、`かい｜たい`を`回｜たい`ではなく`買い｜たい`にする。右文脈がない通常入力、ライブ変換、private modeは変更しない。

GSD devでは1件改善・悪化0、GSD train、GSD test、AJIMEE、JWTDは候補順を含めて不変だった。過去助動詞`た`への全面拡張は意味的な動詞選択を誤るため不採用とした。

## 今回の実装: 一意な後続文法接続

[ATOKのAI用例](https://atok.com/other/support/howtouse/mac/shrd/shrd_yougo.htm)は前後の語のつながりを変換に使う。[azooKeyの通常変換](https://github.com/azooKey/AzooKeyKanaKanjiConverter/blob/main/Sources/KanaKanjiConverterModule/ConversionAlgorithms/Core/FullInputProcessing.swift)も、辞書entryの左右品詞IDと接続costを含むラティスからN-bestを求める。Slimeではこれらを一般的な設計上の参考とし、ATOKの内部実装を再現せず、既存Mozc辞書の接続行列だけを使う。

文章途中の編集で右文脈が`た`・`ない`・`ます`・`て`・`で`・受身・使役の文法形から始まる場合、同じ読みに属する全exact entryについて後続POSへの接続costを比較する。最良costから1,000以内に入る漢字表層が1種類だけなら、その一語候補を最大1,500 cost昇格する。複数表層が文法的に接続できる場合は意味選択が必要なので順位を変えない。これにより`渡した`、`来られています`、`来ないね`を直し、`模し`・`燃し`などが競合する`模した`は従来順位のままにした。

GSD train・dev・testで各1件改善・悪化0、AJIMEE held-outとJWTD devは候補順を含めて不変だった。通常入力、ライブ変換、private modeには右文脈が渡らないため影響しない。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 計測範囲の「圏内・圏外」

ATOKが重視する前後の語のつながりを、意味推測ではなく構造が一意な数値表現へ限定して使う。左側が数値と距離・時間単位で終わり、読みが`けん`、右側が`内`または`外`の場合だけ`圏`を優先する。これにより`徒歩10分県内`を`徒歩10分圏内`へ修正し、地名に続く`福岡県内`は変更しない。

GSD devは1件改善・悪化0、GSD train・test、AJIMEE、JWTDは候補順を含めて不変だった。広い文脈接続上限を試すと`内服`を`ない服`へ壊したため不採用とし、数値・単位・左右表層を全て満たす狭い規則だけを採用した。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 長い読みの意味候補を深く採点

[ATOK 2026](https://www.atok.com/features/)は、`逸材入った`のような助詞省略を含む口語と、確定した`経験`に続けて`のうむ`と細切れ入力した場合の`の有無`を強化している。Slimeは後者と旧公式例`アルプス` + `のやま → の山`を既存の文書境界変換で既に再現できたため、回帰testとして固定した。一方、前者の意味選択は接続行列だけでは解けず、ローカルニューラルrankerの担当になる。

9文字以上の読みでは、モデル準備完了後に限り、ニューラル採点へ渡す候補のcost窓を1,500から2,500へ広げる。短い読み、モデルなし、モデル準備中、ロード・採点失敗、履歴・ユーザー辞書・入力ミス訂正候補は従来境界のままとする。JWTD devは6件純改善、AJIMEE held-outはtop-1不変でMRR/MinCER改善、GSD dev/testは候補数を含め不変だった。詳細は[evaluation.md](evaluation.md)に記録する。

## 組織名・地名recallの監査結果

[ATOKの固有名詞優先](https://atok.com/other/support/howtouse/mac/tr/pgs/tr_conv_name.htm)に対応する追加探索を検討したが、Mozcの組織・地域POS 14万2,957 entryから均等抽出した約4,900件はすべて初回10候補に入った。GSDの未回収地名も大半が語彙不在または読み差で、beam追加では直らない。初回候補を汚す専用順位変更は行わず、今後はライセンスを確認できる語彙だけをoptional packで評価する。

## 商用同梱の現時点判断

[zenz-v3.1-xsmallの配布ページ](https://huggingface.co/Miwa-Keita/zenz-v3.1-xsmall)は22.5M parameter、BF16、CC BY-SA 4.0と表示している。[CC BY-SA 4.0の公式deed](https://creativecommons.org/licenses/by-sa/4.0/deed.en)は商用目的の共有・翻案を許可する一方、帰属、変更表示、翻案物のShareAlike、追加制限の禁止を条件とする。したがって「商用利用不可」をNo-Go理由にはしない。

ただし、量子化済みモデルをアプリと一緒に配布する場合に、どのartifactへShareAlikeが及ぶか、ストアや署名の技術的制限と両立するかは、この技術評価だけでは確定しない。採用候補はアプリ本体から独立したoptional model artifactとして配布し、モデルの入手元、作者、ライセンス、変更内容、checksumを明示する設計を基本とする。この分離は法的結論の代用ではなく、リリース前に専門家の確認を受ける。

品質面では、BF16から直接生成したQ4_K_MはQ5_K_Mより4,430,368 bytes（21.13%）小さいが、製品条件でGSD dev 3件、GSD test 1件、PUD 5件のtop-1を落とした。AJIMEE/JWTDは同率でも複数domain非悪化を満たさないため不採用とする。現時点ではQ5を評価専用の任意モデルに留め、モデルをSlimeの標準bundleへ含めない。

その後、[zenz-v3.2-small-gguf](https://huggingface.co/Miwa-Keita/zenz-v3.2-small-gguf)の公式配布commitがApache-2.0を宣言していることを確認した。95.1M parameterのQ5_K_Mをsmall専用profileで評価し、長文候補幅を32まで広げると、現行xsmallの製品条件比でJWTD dev +28件、GSD dev +6件、AJIMEE +16件、GSD test +5件、PUD +10件となり、5 datasetすべてのtop-1とMRRが改善した。high-accuracyでは既存の9文字境界を信頼度と候補幅に使い、短文margin 0.5でGSD testを289/323から291/323へ改善しつつ、長文margin 0でJWTDを210/400まで改善する。AJIMEEも160/200から162/200へ改善し、PUDのtop-1は維持した。詳細値とchecksumは[evaluation.md](evaluation.md)に記録する。

これにより、ShareAlikeを含むv3.1を商用同梱候補にする必要はなくなった。v3.2-smallは明示的な`high-accuracy` profileとして実装し、モデルを含まない通常buildと既存xsmall向け`balanced` profileは維持する。ただし公式model cardには学習元の説明がないため、Apache-2.0 metadataだけで第三者権利まで断定せず、有償配布前の由来確認と法務レビューは残す。

候補集合外の誤りに対しては、zenz-v3.2-smallのgreedy表層を直接採用せず、完全な辞書lattice path、同じ文字数、ASCII英数字不変、2〜4個の離れた局所変更、各2文字以内、通常cost窓、追加marginをすべて満たす1候補だけを`high-accuracy`の再採点へ加えた。読みは6〜32文字に限定する。JWTDは2回prefix後の225 / 400から227 / 400、PUDは346 / 446から347 / 446へ改善し、AJIMEEとGSD dev/testを維持した。ATOKの継続的な変換強度と同様に、モデルの一度の出力だけで辞書・履歴の境界を越えない。

同じgreedy出力が通常の辞書候補に既にある場合は候補を重複追加せず、通常winnerより候補尤度が0.1〜0.2だけ高い近接一致に限って順位信号として再利用した。JWTDは227→228、PUDは347→348へ各1件改善し、AJIMEEとGSD dev/testは不変、exact悪化0だった。生成beam幅2はJWTDの辞書付きoracleを増やしたものの最終top-1を追加改善せず、対象入力のp95を約114 msから151 msへ増やしたため不採用とした。

候補内順位には、モデルが文脈なしでも好む一般頻度と、左右文脈による寄与が混在する。そこで同じ読み・候補を文脈なしでも採点し、差分の10%だけを通常尤度へ足す文脈対比を`high-accuracy`へ追加した。JWTD devで0.25以上が正解を落としたため0.1へ固定し、GSD dev 301→302、PUD 348→349、GSD test 300→301、JWTD 228とAJIMEE 168は維持、全件でexact悪化0だった。`乗せ→載せ`、`付か→突か`、`仮フォルニヤ→カリフォルニヤ`を修正した。`balanced`と文脈なし入力は二重採点せず、high-accuracyの文脈付き5候補ではmodel処理のp50が約11 ms増えた。

model指示による局所修正では、既に得ていた漢字をひらがなへ戻す変更だけを拒否する。これにより2回目レビューの`不→ふ`を止め、文脈対比後のJWTDを228→229へ改善した。漢字同士と漢字→カタカナの修正は残し、GSD dev/test、AJIMEE、PUDはすべて不変だった。通常候補に含まれるひらがな表記の選択やユーザー操作は制限しない。

さらに、同じgreedy出力が既存辞書候補にあり、第1候補から2〜4個の離れた領域だけが各2文字以内で変わる場合を複数局所一致として区別した。連続局所一致の尤度差上限0.2は変えず、複数局所だけJWTD devで改善する最小の0.25へ固定した。`統計開始依頼の局地 → 統計開始以来の極値`を回収してJWTDは229→230、GSD dev/test、AJIMEE、PUDは不変、exact悪化0だった。生成済み表層と既存候補を照合するだけなので追加推論はない。

同じ文字数という生成候補条件は、完全な辞書経路でも`いろいろ→色々`のような表層圧縮を落としていた。長さ差1〜2の短縮だけをLevenshtein整列し、2〜4領域・各側4文字以内・ASCII不変・既存cost窓・追加marginを満たす候補を許可した。長さを増やす案はPUDで余分な候補を加えたため棄却した。最終条件は`エンジェル帯に復讐渡渉していろいろな → エンジェル隊に復讐と称して色々な`を回収してJWTDを230→231へ改善し、GSD dev/test、AJIMEE、PUDの候補数とtop-1を変えなかった。

modelの強い不一致prefixを辞書へ戻す局所修正では、従来は制約付き探索の先頭1件だけを安全判定していた。先頭が離れた領域まで書き換える場合でも安全な後続表層を失わないよう、まず8候補を調べ、安全候補がない場合だけ32候補へ広げる。通常候補数、表示候補数、model推論回数は増やさず、同じ文字数・連続2文字以内・ASCII不変・漢字保持という既存条件を全候補へ適用する。JWTDは231→234、GSD dev/test、AJIMEE、PUDは不変、exact悪化0だった。

通常cost窓の外でも、9〜32文字のgreedy生成と完全一致する辞書経路があり、同じ文字数・2〜4領域・各2文字以内・ASCII不変を満たす場合だけ、複数局所生成一致の上限を2,500から3,100へ広げた。表層圧縮と通常候補には適用しない。`編目・書く → 篇目・欠く`を回収してJWTDは234→235、他4 datasetは不変、exact悪化0だった。

形状制限に収まらないgreedy全文も、EOS完了、辞書格子との完全一致、6〜32文字、cost差1,000以内を同時に満たす場合だけ通常のmodel supplemental候補へ加える。直接昇格せず追加margin 1.5を維持する。通常cost窓まで広げるとPUDでcost差1,481の誤変換が出たため採用せず、1,000で`外在42 → 外在史に`を回収した。PUDは349→350、JWTD、GSD dev/test、AJIMEEは不変、exact悪化0だった。

## 今回の実装: 初回順位を変えない固有名詞パック

[SudachiDict](https://github.com/WorksApplications/SudachiDict)のFull辞書を評価専用に
調べると、通常64候補にも正解がないJWTD 84件のうち51件は期待表記と読みを辞書語で
構成できた。一方、固有名詞entryを通常ラティスへ大量投入すると、第一候補、ライブ
変換、起動時間を同時に変える危険がある。

そこでv5辞書パックへ`explicit-search-only`を追加した。この語彙は通常変換、ライブ
変換、補完、英単語逆変換、モデル再採点から隔離し、ユーザーが候補末尾へ進んだ場合
だけ入力読み全体の完全一致を最大64件追加する。20万3,590件の一時パックでは、32件の
均等サンプルを通常末尾探索16/32から32/32へ回収した。初回回収は15/32のまま、末尾
探索時間は交互測定で通常経路と同じ測定帯だった。20万件パックのprocess起動中央値は
baseline比約107 ms、最大RSS増分は約24.2 MBである。語彙本体は同梱せず、ライセンスと頻度を
確認した配布パックを別途評価する。

## 今回の実装: 辞書で確定できる姓名の表記保護

[ATOKの固有名詞優先](https://atok.com/other/support/howtouse/mac/tr/pgs/tr_conv_name.htm)が示すように、人名は一般語の順位付けと分けて扱う必要がある。Slimeは人名候補を無条件に初回優先せず、現在の候補にMozc辞書で完全一致する姓＋名の経路がある場合だけ、`high-accuracy`のmodel指示prefix修正からその表記を保護する。フルネームentry、または隣接する姓POS＋名POSが必要で、名前POSを別解に持つ一般語だけでは保護しない。

信頼性gate後のJWTD 400件では、1回prefix修正が226→227、2回までの修正が230→231となり、`片瀬志麻→片瀬志摩`のexact悪化1件を0件にした。AJIMEEの最終精度は不変、PUD 446件、GSD dev 331件、GSD test 323件は計測時間以外の出力が保護前とバイト一致した。Release相当の2,000回単独測定は、人名保護成立が0.0346 ms/op、一般語の非成立が0.0188 ms/opだった。通常変換や表示候補には追加探索を行わない。

## 今回の実装: 安全なwhole-result一致

[ATOKの変換エンジン説明](https://atok.com/info/features/engine.html)は、直前の確定だけを常に優先せず、文脈上自然な候補を優先する学習強度を説明している。[azooKeyの現行候補評価](https://github.com/azooKey/AzooKeyKanaKanjiConverter/blob/main/Sources/KanaKanjiConverterModule/ConversionAlgorithms/Zenzai/Zenz/ZenzCandidateEvaluator.swift)は、生成が完了した全文を`wholeResult`として通常候補の再検討へ返す。Slimeはこの責務分離を参考にしつつ、model文字列を直接表示候補へ入れない境界を維持する。

`high-accuracy`のgreedy全文がEOSまで完了し、読みが6〜32文字、完全な辞書lattice path、辞書第1候補とのcost差1,000以内をすべて満たす場合だけ、局所prefix修正後の最終候補との一致を評価する。ASCII英数字列の変更、漢字からひらがなへの表記崩れ、辞書で確定できる姓名の変更は拒否する。広い3,100 cost窓はPUDで悪化したため棄却し、PUD参照後の再調整は行わず既存base confidence値1,000を使った。

旧Release FFIとの同条件比較では、信頼性gate後JWTD 232→234、PUD 346→349、GSD phrase dev 220→222へ改善し、AJIMEE 167、条件固定後に一度だけ評価したGSD phrase test 245を維持した。合計7改善・0悪化で、model推論回数と候補上限は増えない。通常build、`balanced`、履歴、ユーザー辞書、入力ミス訂正、規則候補の保護境界も変更しない。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 長文だけ遅延するwhole-result一致

ATOKが長い入力でも前後のつながりを変換へ使い、azooKeyがEOS完了した全文を通常候補の
再検討へ返す設計を参考に、`high-accuracy`の全文一致を33〜40文字へ限定的に広げる。
単純な上限拡張はGSDとPUDで既存正解を壊したため採用しない。通常N-bestを先に採点し、
model全体の首位がbase以外の既存候補、辞書cost差500〜1,000の場合だけgreedy生成を
遅延実行する。EOS完了、既存候補との完全一致、ASCII・漢字・姓名の保護をすべて満たす
場合だけ順位信号として使い、生成表層を直接追加しない。

長文専用コーパスではGSD train 2件、PUD 1件を修正し、GSD dev/testの悪化は0件だった。
製品Release FFIではJWTD 234→235、AJIMEE 167を維持し、重点21件は15→18、悪化0だった。
対象となる33〜40文字のhigh-accuracy入力では平均約40.1 ms/件を追加するが、6〜32文字、
`balanced`、モデルなしの経路は変更しない。41文字以上と、回帰が出た48文字拡張は対象外とする。
詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: modelが圧倒的な同文字数whole候補

6〜32文字でgreedy全文が完全な辞書経路に一致しても、通常のcost境界から少し外れる
候補を直接採用すると回帰が多い。そこで既存の局所・複数領域補正に入らない同文字数の
候補だけを追加採点し、辞書第1候補とのcost差1,001〜1,400、ASCII不変、漢字保持、
姓名保持に加え、生のmodel scoreが他の全候補を1.5以上上回る場合だけ昇格する。
生成文字列は直接採用せず、辞書latticeの表層とsegmentsを使う。

上限1,500はPUDで`面から測ら → 面から計ら`を起こしたため棄却した。1,400へ固定後の
Release FFI比較はGSD dev 222→224、PUD validation 349→350で、GSD test 245、
JWTD 235、AJIMEE 167を維持した。合計3改善・0悪化で、既存の6〜32文字生成を使うため
推論回数は増えない。azooKeyの生成と通常候補評価の責務分離を参考にした一般設計であり、
ATOKやazooKeyの内部実装を複製していない。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 一意な全漢字地名の表記保護

[ATOKの固有名詞優先](https://atok.com/other/support/howtouse/mac/tr/pgs/tr_conv_name.htm)は
人名と地名を独立または同時に優先できる。Slimeは通常順位を地名優先へ切り替えず、現在の
辞書第1候補に3文字以上の全漢字地名segmentがあり、同じ読みの地名表層が辞書内で一意で、
model候補がそれをひらがなへ戻す場合だけ、その変換回のmodel結果を採用しない。

これにより`五所川原警察署くるみだて駐在所`を`胡桃舘駐在所`へ1件修正した。
複数地名表記を持つ`大刀洗 / 太刀洗`は保護対象外とし、広い地名優先で発生した回帰を除いた。
JWTD 235→236、GSD dev/test、PUD、AJIMEEは全出力不変だった。詳細は
[evaluation.md](evaluation.md)に記録する。

## 今回の実装: カタカナ語の混在表記分断を防止

high-accuracy modelが辞書完全一致の`アルゴル`を`あるゴル`へ分断するケースに対し、4文字
以上の全カタカナsegmentがひらがな・カタカナ混在表記へ壊れ、変更がそのsegment内だけに
収まる場合を拒否する。全ひらがな化は許可し、一般的な表記選択や学習を固定しない。

JWTDは236→237となり、他のGSD dev/test、PUD、AJIMEEは全出力不変だった。azooKey Desktopも
[Zenzaiによるニューラルかな漢字変換](https://github.com/azooKey/azooKey-Desktop)を採用するが、
Slimeでは辞書候補と外部modelの境界で表記整合性を独立に検証する。詳細は
[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 複数segment後の右文脈複合語

[ATOKの変換エンジン説明](https://atok.com/info/features/engine.html)と
[azooKey/Zenzaiの左右文脈tag](https://github.com/azooKey/AzooKeyKanaKanjiConverter/blob/main/Docs/zenzai.md)
を参考に、Slimeの既存辞書文脈規則を複数segment候補の末尾まで適用する。文章全体を1語として
扱わず、末尾候補と右側を結ぶ完全なMozc辞書語があり、右suffixが2文字以下、または境界付きの
`AやB`・`AのB`である場合だけ既存候補を昇格する。無制限の長いsuffixは`際セット → 再セット`
の回帰を起こしたため採用しない。

GSD dev/testとPUDで合計8改善・悪化0、JWTDとAJIMEEは全出力不変だった。モデルなしの
1,089件×6回では0.8835→0.9350 ms/件（+0.0516 ms）で、計算は変換回内だけにcacheし、
文章を保存しない。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 数字列と「に＋サ変名詞」の境界

数字読みを長く結合できることは電話番号・型番に必要だが、`よんにいちし`では`421し`が
`4に位置し`より接続costで先行する。ATOKの文脈変換とazooKey/Zenzaiの左右文脈分離を参考に、
Slimeでは32候補内に「数字＋助詞に＋一意なサ変名詞」が実在し、右側が`する`活用の場合だけ
構文候補を補強する。数詞候補そのものは削除・弱化しない。

PUDで`グレード421 → グレード4に位置`を1件修正した。初期案の`みなさん → みな3`回帰は、
敬称・複数呼称にもなる単独読み`さん`を除外して解消した。GSD dev/test、JWTD、AJIMEEは
全出力不変、5 dataset合計1改善・悪化0だった。詳細は[evaluation.md](evaluation.md)に記録する。

## 今回の実装: 引用格＋受け手＋発話動詞

ATOKが前後のつながりを変換に使い、azooKey/Zenzaiが左右文脈を別tagで扱う設計を参考に、
読点・閉じ括弧直後の引用格`と`、受け手を示す`に`、発話動詞、右側の活用接続がすべて揃う
場合だけ、既存の発話候補を補強する。一般の`に行った/に言った`には適用しない。

公式v3.2-smallの製品比較でPUDの`警察に行っ → 警察に言っ`を1件修正し、悪化0だった。
GSD devは全出力不変、GSD test・JWTD・AJIMEEには発火する左境界がなかった。通常の移動表現を
保護する反例と性能測定を含む詳細は[evaluation.md](evaluation.md)に記録する。

## 次の実装順

1. v3.2-smallの学習元を作者へ確認し、Apache-2.0のNOTICE、署名、notarizationを含むhigh-accuracy artifactの配布gateを閉じる。通常buildはモデルなしを維持する。
2. 辞書制約付き生成と再採点で別々に作るllama contextを共有し、high-accuracyの追加61〜106 msを削減する。精度5 dataset非悪化を維持する。
3. 組織名・地名は、実装済みの`explicit-search-only`へライセンス・頻度付き語彙を載せ、held-out追加回収、起動時間、メモリを通るoptional packだけを配布候補にする。
4. 入力ミス訂正と学習強度の実利用false positiveを固定データへ追加する。
5. macOSを再インストールし、TextEditで逐次入力、候補操作、再変換、private/secure inputを確認する。
6. Windowsは署名以外の実機動作をVMで先に閉じ、配布可能という表現は署名・install/update/uninstall完了まで使わない。

## 公式資料

- [ATOK for Mac](https://atok.com/mac/)
- [高精度な変換エンジン](https://atok.com/info/features/engine.html)
- [ATOK for Mac 旧バージョン比較表](https://atok.com/info/comparing/mac.html)
- [カスタムATOK](https://atok.com/other/support/howtouse/mac/mn/pgs/mn_tool_custom_atok.htm)
- [ATOK 2026 新機能・追加機能](https://atok.com/features/)
- [ATOKクラウド推測変換](https://atok.com/useful/clouddic/cloud-conversion.html)
