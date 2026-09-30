# 変換精度改善の現在地（2026-09-12更新）

改善は作業中。現在のQAアプリは `target/macos/Slime.app` にある。ローカルのad-hoc署名・モデル同梱ビルドで、署名検証済み。利用中のIMEへのインストール、公証、公開は行っていない。「日本随一」という比較結果もまだ証明できていない。

## 現在の採用設定

- モデル: zenz v3.2 small Q4_K_M、約61 MiB。
- 明示変換のモデル重み: 1〜2文字は0.2、3〜19文字は0.3、20文字以上は0.45。
- 明示変換の候補探索: 通常10、20文字以上16。保護候補や読みそのままの候補は、この採点上限とは別に表示される場合がある。
- LIVEの重みは従来設定を維持。履歴由来・辞書パック文脈由来の候補はモデルの自動順位変更から保護する。

## 現在の評価結果

| 入力と評価方法 | 件数 | LIVE完全一致 | 明示変換完全一致 |
| --- | ---: | ---: | ---: |
| かな直接入力、全文入力後に推論 | 2,745 | 1,698 | 1,686 |
| ローマ字入力、全文入力後に推論 | 200 | 109 | 110 |
| ローマ字入力、毎文字推論 | 200 | 112 | 110 |

ローマ字200件はかな集合の一部を別の入力経路で再生している。独立した追加400例ではない。原文表記との完全一致なので、かな・漢字の許容表記差も不一致に含む。公開コーパスから独立したモデル学習holdoutを証明したものでもない。

直近では、目的語の後の短いかなを辞書の単一語へ変換する際、モデルの素の点数でも選択候補が最上位であることを確認するようにした。「記憶を無くし」「腕力を活かし」「言葉を挙げる」の3件が全文評価で期待表記に一致し、正答の悪化はなかった。誤答同士の表記変化1件は改善に含めない。逐次入力は1件改善、他の出力は一致。Core260・FFI46、QA187行（163種類）のSpace・直接Enterを確認した。詳細は `docs/live-object-model-support-2026-09-12.md`。

前回は、助詞に見える一文字を含む境界を辞書とモデルで再検討する修正を採用した。適用できない結果は元の範囲へ戻し、境界を広げた範囲に辞書だけの末尾補正を流用しない。「特典を設け」「可能な限り廃してあり」「外向的」「お砂糖とお酢」の4件が全文評価で改善し、正答の悪化はなかった。ローマ字毎文字推論は1件改善・正答の悪化0。誤答同士の出力変更は全文評価3件・毎文字推論1件あり、改善には数えない。Core259・FFI46、最終QA153行のSpace・直接Enter確定を確認した。詳細は `docs/live-boundary-fallback-2026-09-12.md`。

前回は、安定した前半の二語修正も、辞書の読み・分割一致と漢字のみの限定条件で検証するようにした。「機構…評点」から「気候…氷点」の長文が1件回復し、他の最終出力は一致した。Core252・FFI46テスト成功。最終QAアプリの108件をSpace後と直接LIVEの両Enter経路で確認した。詳細は `docs/live-two-prefix-words-2026-09-12.md`。

前回は、固定部分と末尾の両方を修復する候補を、辞書の読み・分割が一致し両側で1語ずつだけ変わる条件で適用できるようにした。「投資信託講座を解説」から「投資信託口座を開設」が回復し、広い評価で1件改善。他の最終出力は一致した。追加16入力と既存68入力の計84入力を最終QAバンドルで確認した。追加16入力での改善2件は、同じ文のかな入力とローマ字入力であり、別の2文ではない。

前回は「できない」の途中で「で」を固定する問題を修正した。追加24例は13→21（改善8・正解からの退行0）。「予約できない」「保存できない」「予約できました」などを回復した。固定部分の1語修復も辞書の読み・分割で検証し、必要な辞書探索をworker側で準備する。最終QAバンドルで追加24例＋既存44例の計68例を、Space後と直接LIVEの両Enter経路で確認した。

広い評価の全文正解数は維持したが、不正解同士の出力変更が2文ある。どちらも「できない」は直った一方、1文は「士官→弛緩」という別の誤表記への変化も伴い、期待する「仕官」は未解決。全ての局所的な表記が改善したとは扱わない。

前回は未解決かなの末尾に限定した候補の選び直しを採用し、追加44例の「母とこども」「猫とこども」を「母と子ども」「猫と子ども」に修正した（32→34、退行0）。広い評価では「1万→一万」の表記一致が1件増え、他の最終出力は一致した。数字表記による完全一致の増加を、意味理解の向上とは扱わない。最終QAバンドルでSpace後・直接LIVEの両方のEnter確定を44例確認した。

前回は「とこ」の段階で文節固定を待つようにし、「ところ→と頃」を修正した。ローマ字側は全文入力後・逐次入力とも1件改善、悪化0件。追加20例でも2例が改善し、他の出力は一致した。対象の長い1例の同期キー処理p95は0.522→0.604 ms、worker時間の中央値は39.55→58.65 ms。より広い範囲を評価する処理時間がある。詳細は個別レポートにある。

その前の固定部分再評価範囲の変更では「短気」「企業を傘下に加え」「天覧」などを回復した。

## 採用済みの主な修正

- 固定部分と末尾で1語ずつの漢字修復を辞書の読み・分割で検証し、「投資信託口座を開設」を適用できるようにする。

- 「できない」を一つの評価範囲に保ち、「で機内」への誤分割を防ぐ。辞書の読みと分割が一致する1語の修復で、固定部分の「適し→敵視」も適用できるようにする。

- 末尾に未解決かなが残る場合、文字数に合う候補と現在表示のスコアを比較し、「母と子ども」「猫と子ども」を修復する。安定した前半の再開や、既に漢字になった対象にはこの修復を適用しない。
- 「ところ」が「と」と「頃」に固定されるのを防ぎ、並列の「と＋子…」も引き続き評価する。
- 世代番号によって、キャンセル後に同じ読みを入力し直した場合も古いLIVE結果を失効させる。
- 未確定の`n`で次の確定かな用の文節保護を消費しない。
- 文脈のある短い対象を限定的に再評価し、「家に早く帰る」「洗濯しよう」を回復する。
- 文脈付き辞書1位の確認により、毎文字推論で残っていた「銀行口座の解説」を「開設」へ修正する。
- 候補探索の重複削減、長文の候補追加と文脈候補保護、モデルQ4への変更を採用した。以前のモデル・短文重みの変更には回帰もあるため、全変更が無条件に改善だけだったとは扱わない。

## 検証と残る問題

Core250テスト、FFIのニューラル機能を含む46テストを確認した。最新Clippyは既存の `dictionary_packs.rs` に対する `chunks_exact_to_as_chunks` 警告をコマンド側で除外して成功している。C ABI smoke、Swiftアダプタ、履歴の別プロセス再読込と書き換え防止も各変更時点で確認している。実バンドルのSwiftプローブを使って表示・確定・入力途中の状態遷移を検証しているが、物理キー・InputMethodKit・実時間debounceを通す実操作は別途必要。

候補にない語や、候補があってもモデルが原文と異なる表記を選ぶ問題が残る。短い入力の「父とこ」などのかな残りや、固有名詞・活用途中の精度も未解決。入力経路や途中で推論する頻度によって結果が変わる例があるため、完全一致数だけで品質を判断しない。

短い入力のかな残りは追加診断で、途中の「父」「母」の推論適用後に発生し、最終候補不足ではないことを再現した。父は候補適用側の語尾保護にも拒否される。父の修復条件の変更は未採用で、母・猫のかな残りは限定的な候補選び直しで修復済み。 後続かなの優先を外す2案を追加検証したが、「と気」「左端」「母と板」「父とクラす」などの悪化があり見送った。完全一致数が同じでも個々の出力悪化を確認している。

## 根拠への入口

- [固定部分と末尾を同時に修復するLIVE候補](live-joint-scope-repair-2026-09-12.md)

- [「できない」の誤分割と辞書に基づく固定部分修復](live-dekinai-boundary-2026-09-12.md)

- [LIVEのかな残りを修復する長さ制約付き候補選択](live-literal-length-fallback-2026-09-12.md)

- [短文重みの採用・全件検証・学習確認](short-explicit-weight-adoption-2026-09-09.md)
- [長文の候補追加](long-explicit-width-2026-09-09.md)
- [長文の文脈候補保護](long-context-candidate-protection-2026-09-09.md)
- [候補探索の性能改善](candidate-pruning-performance-2026-09-09.md)
- [文脈ルールがない場合の処理削減](empty-context-rules-performance-2026-09-09.md)
- [Mozcとのエンジン比較](mozc-engine-comparison-2026-09-09.md)
- [候補不足とモデル採点の切り分け](short-candidate-frontier-2026-09-09.md)
- [Q5モデル再評価と見送り理由](q5-current-policy-evaluation-2026-09-09.md)
- [Q4モデルの評価・採用・バンドル確認](q4-current-policy-evaluation-2026-09-09.md)
- [古いLIVE推論の失効と再入力の修正](live-generation-invalidation-2026-09-09.md)
- [入力途中の推論を繰り返す検証](live-prefix-replay-2026-09-09.md)
- [文脈のある短いLIVE対象の採点範囲](live-contextual-cost-gate-2026-09-09.md)
- [文脈のあるLIVE対象の文字数変更](live-contextual-length-2026-09-09.md)
- [ローマ字経由のLIVE評価差](romaji-live-scope-evaluation-2026-09-09.md)
- [未確定ローマ字による境界保護の消費を修正](live-deferred-romaji-fix-2026-09-09.md)
- [文脈付き辞書1位の再確認を採用](live-contextual-base-confirmation-2026-09-09.md)
- [固定済み部分の再評価cost範囲](live-stable-repair-cost-2026-09-09.md)
- [「とこ」で文節固定を待つ修正](live-word-boundary-deferral-2026-09-09.md)

- [短い入力のかな残りと途中推論の診断](live-short-literal-diagnosis-2026-09-09.md)

各レポートの検証時点と対象を確認すること。古い設定の評価値を現在の製品結果と混同しない。変更前のQAアプリやソース、全入力・出力・ハッシュ・ログは各 `target/evaluation/` フェーズに保存している。


### 2026-09-12 初回Spaceの再結合追加を見送り

既存16経路＋最大4再結合候補を全2,745件で評価。Space 1,686→1,687（改善2・悪化1）だったが、60件交互3反復のp95中央値が97.35→128.04msとなり不採用。ソースは復元済み、QAは変更なし。採用済みLIVE 1,698 / Space 1,686を維持。詳細は [再結合実験](explicit-recombination-2026-09-12.md)。

候補数を16に維持する再結合差し替えも全件検証。Space 1,686→1,686（改善3・悪化3）で、変装→返送、大和守→大一守を含むため不採用。ソース復元済み、QA変更なし。資料は `explicit-recombination-bounded-20260912`。


### 2026-09-12 Space適用の再探索除去を採用

全かな2,745件＋ローマ字200件の出力を維持し、Spaceの60件交互3反復でp50 25.26→23.63ms、p95 118.03→93.88ms。Core 261・FFI 46テストとClippy成功。QA再構築・署名検証、Swift 187件のLIVE/Space一致、直接Enter 187件の表示と確定一致を確認。採用済みLIVE 1,698 / Space 1,686を維持。最新のソース・実行ファイル・QA検証資料は `explicit-ranking-transaction-20260912`。詳細は [Space再探索除去](explicit-ranking-transaction-2026-09-12.md)。


### 2026-09-12 ひらがなを保つ再結合候補を採用

通常候補を維持した追加候補で、固定かな2,745件のSpace 1,686→1,688（改善2・悪化0・不正解のまま2変化）。LIVE 1,698を維持。追加200件はSpace97で全出力一致、ローマ字200件も一致。Space p50 22.95→23.17ms、p95 94.47→105.40msの負担を記録して採用。Core263・FFI46成功、QA署名検証、Swift190行（166種類）でSpace2件改善と全確定を確認。最新の検証資料は `explicit-recombination-kana-20260912`。詳細は [ひらがなを保つ再結合](explicit-recombination-kana-2026-09-12.md)。


### 2026-09-12 共通コスト差上限1,500は不採用

固定かなSpace1,688→1,697（改善10・悪化1）、LIVE1,698→1,699（改善4・悪化3）。追加かなSpace97→100、ローマ字Space110→112だがLIVE109→108。共有設定では誕生とし→誕生年、競合性→強豪生などを生むため不採用。製品ソース・QAは変更なし。次はSpaceの独立設定と、新規評価対象のひらがな保持条件を検証する。資料は `cost-gap1500-20260912`。


### 2026-09-12 Space専用コスト差上限1,500を採用

共通上限1,000を維持し、Spaceだけ1,500＋新規評価対象のひらがな保持条件。固定かなSpace1,688→1,698（改善10・悪化0）、LIVE1,698を全出力・状態ごと維持。調整用追加200件97→100、新しい200件97→99、ローマ字Space110→112/LIVE109維持。Core263・FFI48（非ニューラル47）、C ABI、Clippyを確認。QA230行のLIVE一致、Space変化と全確定を確認。速度測定はp50 24.16→26.76ms、p95 113.39→105.15ms、最大155.03→170.07ms。最新資料は `explicit-gap1500-20260912`。[詳細](explicit-gap1500-2026-09-12.md)。


## 2026-09-12: two aligned word LIVE repair adopted

Worker-prepared dictionary evidence allows bounded two-word corrections across the stable-prefix boundary while retaining kana, punctuation, and dictionary segmentation. Fixed2745 LIVE1698→1699, zero losses; Space1698 and all explicit outputs unchanged. Additional validation200 unchanged. Core264/FFI48, Clippy, signed QA232 normal/direct Enter replay pass. App replay outputs unchanged; it already handled the added fixed-evaluator gain correctly. Input-side dictionary work found during timing was moved to the worker before adoption. See `docs/live-two-aligned-words-2026-09-12.md` and `target/evaluation/live-two-aligned-words-worker-20260912/adoption.json`. Goal remains incomplete; no national ranking claim.


## 2026-09-12: confidence and consistent replacement adopted

Default builds now opt into confidence-gated long Space reranking, while preserving explicit custom scoring settings. Fixed2745 Space1698→1712 (16gains,2expected-string losses,7other); LIVE1699 unchanged. New disjoint-input200 Space76→77,0losses. App272 normal/direct Enter verification passes; Space23row gains/3expected disagreements, LIVE identical. Mixed end-to-end latency is documented; isolated added ranking cost about1.65microseconds. See docs/explicit-consistent-change-2026-09-12.md and target/evaluation/explicit-consistent-change-20260912/adoption.json. Current QA app/default build enables the policy; no installation/publication. Goal remains active and national-best claim unproven.
