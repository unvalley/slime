# かな漢字変換の品質評価

micro benchmarkは速度を、評価suiteは変換品質を測る。AJIMEE-Benchは難しい漢字誤変換へ意図的に偏ったデータなので、単独の総合品質指標にはせず、must-pass testや将来のbalanced corpusと併用する。

## ライブ変換の逐次回帰

最終候補だけの評価では、入力途中のpreeditが前の表層に固定される問題を検出できない。そのため、`raibuhenkannno`を1キーずつ入力したときに未確定`n`を越えて`ライブ変換の`を維持する回帰をcoreとmacOS adapterの両方に置く。

局所コーパスなしで初回から`差がつく`を意味判断する商品モデルはまだない。完全一致履歴を無条件にライブ変換へ昇格させると、古い`そうしま→総島`のような履歴が現在のconfidence gateを迂回して自動表示される。通常候補では学習順位を使うが、ライブ変換は「正しいと確信できなければかなのまま」という安全境界を維持する。

## AJIMEE-Bench

実行方法:

```sh
just evaluate-ajimee
just evaluate-ajimee --context none
just evaluate-ajimee --context present --json
```

直接実行する場合:

```sh
scripts/evaluate-ajimee.sh --top-k 10 --context all
```

初回だけ評価データを`target/evaluation`へ取得する。取得元はAJIMEE-Benchのcommit `401666cd56d1a570c2021798b64b6da4396bfd45`に固定し、SHA-256を検証する。評価データを製品bundleへ含めたり、通常のbuildやtestでネットワークへ接続したりしない。

出力する指標:

- `acc@1`: 第1候補がいずれかの許容解と完全一致した割合
- `acc@k`: 上位k候補に許容解が含まれる割合
- `mrr@k`: 最初の正解候補の逆順位の平均
- `mincer@1`: 第1候補と最も近い許容解の文字誤り率
- `mincer@k`: 上位k候補と許容解の組み合わせで最小の文字誤り率
- latency `p50/p95/p99/max`: 辞書初期化を除いた候補生成時間

`--context none`は左文脈なし100件、`--context present`は左文脈あり100件、`--context all`は全200件を評価する。現在の変換器は左文脈を順位付けに使わないため、レポートの`context_used_by_engine`は`false`になる。文脈モデルを導入した場合、この区分を維持して効果を比較する。

### 2026-07-20 baseline

N-bestを10件、ユーザー履歴なし、追加辞書なしで測定した。辞書の初期化時間はlatencyから除外している。

| subset | items | acc@1 | acc@10 | MRR@10 | MinCER@1 | MinCER@10 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 左文脈なし | 100 | 0.350 | 0.580 | 0.418 | 0.100 | 0.059 |
| 左文脈あり（文脈未使用） | 100 | 0.320 | 0.580 | 0.405 | 0.135 | 0.068 |
| 全体 | 200 | 0.335 | 0.580 | 0.412 | 0.117 | 0.064 |

全体の候補生成latencyはp50 6.76 ms、p95 27.03 ms、p99 34.73 ms、最大44.63 msだった。AJIMEEは入力長が最大117文字の難例を含む。通常候補表示の20 ms予算はp95で超えているため、品質改善と並行して長文N-bestの継続的な最適化が必要である。

この初回計測で、未変換のひらがな候補へ固定コストを与えると長文ほど第1候補へ上がる問題が見つかった。未変換候補を変換済み候補より後ろへ固定した結果、左文脈なしの`acc@1`は0.140から0.350へ改善した。

### 2026-07-20 改善後

失敗84件の分析から、(1) 辞書の語彙不足（矩形・荼毘・錠前などがcost閾値5500で除外）、(2) かな数詞の未対応（ジュウジカン→従事感）、(3) カタカナ未知語の断片化（ガミラス→紙ラス）が主因と判明した。次の4施策を適用した。

1. 辞書抽出閾値を5500から8500へ引き上げ（170,229語→1,085,464語）。8500で精度が飽和し、全辞書（1,223,906語）でも同一スコアだった。
2. 接続行列の量子化（解像度64の1 byte）を廃止し、正確な16 bit costで格納（`UCN2`、2.7 MB→3.6 MB）。単体では`acc@1`不変、`mincer@1`のみ微改善。
3. かな数詞列を解析して半角・全角・漢数字の合成候補をラティスへ追加（単体で`acc@1` +1.5pt、`acc@10` +3pt）。
4. 辞書に無い読みへカタカナ連続の未知語ノードを追加（base 1000 + 4000/文字。大辞書下ではベンチ中立で、真の辞書外語への保険として機能）。

| subset | items | acc@1 | acc@10 | MRR@10 | MinCER@1 | MinCER@10 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 左文脈なし | 100 | 0.560 | 0.860 | 0.650 | 0.054 | 0.015 |
| 左文脈あり（文脈未使用） | 100 | 0.510 | 0.810 | 0.603 | 0.082 | 0.024 |
| 全体 | 200 | 0.535 | 0.835 | 0.627 | 0.068 | 0.019 |

全体のlatencyはp50 13.1 ms、p95 50.2 ms、p99 74.2 msで、辞書拡大によりbaseline比でおよそ2倍になった。残る失敗33件は同音異義語の文脈選択（複合姓/複合性、皇帝/工程、高官/交換）と固有名詞（棟方志功、鈴木尚典）が中心で、辞書拡充では解消しない。次の改善は`CandidateRanker`への統計言語モデル（文脈スコア）導入が本命となる。

### 2026-07-20 ニューラルN-best rescoring（Phase 2 Step 1 フィージビリティ）

[phase2-context-model-survey.md](phase2-context-model-survey.md)の第一候補「小型ニューラルLMによるN-best rescoring」を、学習なしの公開モデル**zenz-v3.1-xsmall**（GPT-2系 25.6Mパラメータ、Q5_K_M量子化 21 MB、CC BY-SA 4.0）で検証した。実行方法:

```sh
just fetch-neural-model   # GGUF取得 + pre-tokenizerメタデータ修正（要uvx）
just evaluate-dev --neural-model target/evaluation/models/zenz-v3.1-xsmall-Q5_K_M-fixed.gguf            # λスイープ
just evaluate-ajimee --neural-model target/evaluation/models/zenz-v3.1-xsmall-Q5_K_M-fixed.gguf --lambda 0.8
```

`slime-tools`の`neural` feature（`llama-cpp-2`、要cmake）でビルドされる。評価スクリプトは`--neural-model`指定時に自動でfeatureを有効にする。

手法: 既存エンジンのN-best 10候補を、zenz-v3形式プロンプト`<左文脈40字><カタカナ読み><候補>`の対数尤度`log P(候補, EOS | 文脈, 読み)`で再順位付けする。最終スコアは`(1−λ)·(−cost/500) + λ·loglik`の対数線形補間。生成はせずprefillのみで、共有プレフィックスを全シーケンスに割り当て、候補ごとに独立シーケンスを継続することで、1アイテムを原則1回のdecode呼び出しで評価する（Metalのカーネル起動オーバーヘッドが支配的なため）。

devset 400件でのλスイープ: λ=0（rescoringなし）のacc@1 0.293に対し、λ∈[0.5, 0.9]で広いプラトーがあり、最良のλ=0.8で**acc@1 0.423（+13.0pt）**。held-outのAJIMEE-Benchをλ=0.8で1回だけ評価した:

| subset | items | acc@1 | acc@10 | MRR@10 | MinCER@1 | MinCER@10 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 左文脈なし | 100 | 0.690 | 0.860 | 0.761 | 0.034 | 0.015 |
| 左文脈あり（文脈使用） | 100 | 0.620 | 0.810 | 0.703 | 0.055 | 0.024 |
| 全体 | 200 | 0.655 | 0.835 | 0.732 | 0.045 | 0.019 |

改善後（辞書施策のみ）比で全体acc@1は**0.535 → 0.655（+12.0pt）**、MRR@10は0.627 → 0.732。左文脈ありサブセットも+11ptで、エンジンとして初めて左文脈を順位付けに使った（`context_used_by_engine: true`）。acc@10はrescoringでは変わらない（候補集合は不変）。サーベイの推定（+10〜25pt）の下限側を、生成方式のZenzai xsmall（同系ベンチでAcc@1 66.5）とほぼ同水準の65.5で裏付けた。

latency（M3、Metal）: 全体でp50 24.7 ms、p95 86.3 ms、p99 116.0 ms。rescoringの追加分はp50でおよそ+11 ms（バッチ投入約1.5 ms + GPU計算と同期・スコアリング約8.5 ms）。実装上の知見: (1) Metalのdecodeは非同期で、コストはlogits取得時の同期に現れる。(2) `llama_get_logits_ith`は呼び出しごとに同期するため、出力バッファ先頭を1回だけ取得して行を直接インデックスする。(3) softmax正規化はベクトル化可能な多項式exp近似で計算する（スカラーlibm expはdecodeと同程度の時間を消費していた）。

判断材料: 品質面はGo（+12pt、残存誤りの型に直効）。latencyはp95 20 ms予算を超過しており、製品組み込みには (a) 候補間コスト差が大きい場合のスキップ、(b) 文脈KVキャッシュの会話内再利用、(c) より小さい量子化・蒸留モデル、のいずれかが必要。ライセンスはCC BY-SA 4.0のため製品bundleへの同梱は要判断で、評価専用に留めている。継続死条件・次段はサーベイの実装ステップ2以降（自前学習）を参照。

### 2026-07-25 コスト上書きハックの除去とスコア尺度の一貫化

`いいかんじ`が`いい漢字`へ変換される不具合の調査で、must-passを通すための語別ハックが変換全体を歪めていたことが判明した。次の方針へ改めた。

1. **語別のコスト上書きを廃止する。** `build.rs`が`かんじ→漢字`の単語コストを4191から500へ書き換えていたため、格子内のあらゆる文脈で漢字が過剰に選ばれていた（`いい+漢字(500)`が辞書のフレーズエントリ`いい感じ(2264)`に勝つ）。辞書コストはMozc抽出値をそのまま使う。
2. **テスト文の丸写しエントリも廃止する。** `せいどをたかめる→精度を高める`等のgolden文をcost 500の合成エントリとして辞書へ注入していた。これらはベンチの体裁だけを整え、devset指標には寄与していなかった（除去前後でacc@1 0.2925、acc@10 0.6725、MRR 0.4298が完全一致）。
3. **全読み一致候補はBOS/EOS接続コスト込みで採点する。** 従来は候補窓で生の単語コストと接続コスト込み格子パスを混在ソートしており、`イイ感じ`が`いい感じ`より上に来る等の歪みがあった。
4. **現モデルで原理的に解けないケースは`#[ignore]`で追跡する。** 漢字/感じ、精度/制度、箸/橋は同じ名詞クラスで接続行列では区別できない。該当goldenは`context_dependent_golden_cases`（slime-core）と`semantically_ambiguous_noun_needs_context`（slime-converter）として赤のまま残し、文脈モデル導入で解消する。コスト上書きや辞書注入で緑にしない。

あわせて`evaluate-dev.sh`/`evaluate-ajimee.sh`がmacOS標準bash 3.2の`set -u`で空配列展開に失敗して実行不能だった問題を修正した。

### 2026-08-02 非ニューラル軽量rankerの比較

「文脈を使うためにLLMが必須か」を判断するため、候補集合を変えずに順位だけを変える軽量方式を比較した。すべて実験用実装であり、独立データで改善しなかった方式は製品コードへ残していない。

| 方式 | JWTD dev acc@1 | held-out AJIMEE acc@1 | 判断 |
| --- | ---: | ---: | --- |
| 現行cost | 0.2925 | 0.535 | baseline |
| 語/skip bigram（頻度加点） | 最大0.295 | 改善なし | 効果が小さすぎるため不採用 |
| 文字bigram/trigram境界素性 | 0.295 | 0.525 | held-outで-1.0ptのため不採用 |
| 疎な判別reranker（小規模学習） | 小規模注釈holdoutで+0.24pt | 0.530 | held-outで-0.5ptのため不採用 |

この結果は「LLMが必須」を意味しない。小規模な頻度表や数百例の判別学習では、残っている意味的な同音異義語を一般化できないことを示す。原著で改善したstructured SVMは約4万文、Mozcは大規模コーパスと約3,000クラスを使っており、今回の学習規模とは桁が異なる。

次の比較対象は、(1) ライセンスを確認できる数万文以上の形態素注釈データで学習したphrase/POS classモデル、(2) 既に+12ptを確認した25.6MパラメータのN-best専用ニューラルrankerの蒸留・高速化、とする。後者は文章生成やクラウド送信を行うLLM機能ではなく、候補10件を採点するローカルの小型言語モデルである。採用条件はAJIMEEだけでなく開発セットとbalanced corpusでも改善し、p95 latency、配布サイズ、ライセンスの全条件を満たすこととする。

実装面では、`CandidateRanker::ranking_cost_with_context`、注釈済み`surface/reading`コーパスを直接評価できる`annotated`形式、旧ranker APIとの互換性だけを残した。これにより次のモデルを同じN-best・指標・latency測定で比較できる。

`さがつく→さが付く`のような部分ひらがな経路を一般則だけで抑える案も検証した。接続格子から「表記=読み」の内容語nodeを除くと対象例は`差が付く`へ改善したが、JWTD-train dev 400件のacc@1が0.2925から0.090へ、acc@10が0.6725から0.220へ大幅悪化した。活用語尾・通常のひらがな表記まで候補集合から失うため不採用とし、語別cost上書きも行わない。この失敗型は、品詞大分類を保持したphrase/POS modelまたは文脈rankerで解く。

### 2026-08-02 ニューラルrankerのconfidence gate

25.6M rankerの全件実行を避ける第一段として、既存ラティスの第1候補と次点候補のcost差が閾値を超える場合はニューラル採点を省略する方式を、JWTD-train dev 400件で比較した。`--neural-max-cost-gap N`はこの評価専用gateであり、製品コードにはまだ組み込んでいない。M3、Q5_K_M、lambda 0.8の同一環境・同一条件で連続測定した結果:

| 最大cost差 | 採点件数 | 省略率 | acc@1 | MRR@10 | p50 | p95 |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 250 | 148 | 63.0% | 0.3350 | 0.4617 | 11.3 ms | 44.8 ms |
| 500 | 219 | 45.3% | 0.3675 | 0.4828 | 17.7 ms | 50.2 ms |
| 750 | 283 | 29.3% | 0.4025 | 0.5033 | 22.0 ms | 48.8 ms |
| 1000 | 326 | 18.5% | 0.4225 | 0.5164 | 27.3 ms | 56.0 ms |
| 2000 | 387 | 3.3% | 0.4300 | 0.5232 | 33.4 ms | 62.2 ms |
| gateなし | 400 | 0% | 0.4225 | 0.5185 | 36.0 ms | 66.3 ms |

閾値1000はacc@1を維持して18.5%の推論を省き、p95を約15%削減した。一方、p95 56 msは20 ms予算を大きく超える。cost差だけでは意味的曖昧性の信頼度を十分に判定できず、gate単独は不採用とする。次段は、より小さい蒸留モデル、候補token数を減らす識別的head、またはCPUを含むruntime比較が必要である。閾値はdevでのみ調整し、held-out AJIMEEを繰り返し最適化には使わない。

### 2026-08-09 ニューラル順位変更の採用マージン

ニューラル再採点が元の第1候補とは別の候補を選んだ場合だけ、両者の補間score差に最小値を設ける案を比較した。`--neural-min-margin X`は評価専用で、同じニューラル採点結果から複数の値を一度に比較できる。zenz-v3.2-xsmall、先頭3候補、base cost差1000以下、lambda 0.7を固定し、held-outは使わずJWTD-train dev 400件とUD GSD dev 331件だけを測定した。

| 最小score差 | JWTD dev acc@1 | GSD dev acc@1 |
| ---: | ---: | ---: |
| 0 | **0.4075** | 0.8248 |
| 0.1 | 0.3950 | 0.8278 |
| 0.25 | 0.3950 | 0.8308 |
| 0.5 | 0.3950 | 0.8308 |
| 1 | 0.3850 | **0.8338** |
| 2 | 0.3550 | **0.8338** |
| 4 | 0.3200 | 0.8187 |

GSDでは曖昧な順位変更を抑える効果があった一方、JWTDでは最小の0.1から悪化した。単一のscore差はドメインをまたぐ信頼度になっていないため、製品の採用条件には加えず、held-out評価も行わない。今後はscore差だけでなく、入力長、文脈有無、base cost差を含む校正済みconfidenceを独立した学習・評価境界として扱う。

### 2026-08-09 EOSを除外した候補score

[azooKeyKanaKanjiConverterの現行候補評価](https://github.com/azooKey/AzooKeyKanaKanjiConverter/blob/8e3a6eb89e088efd868aa28dadb74c697df4e6fb/Sources/KanaKanjiConverterModule/ConversionAlgorithms/Zenzai/Zenz/ZenzCandidateEvaluator.swift)は、候補tokenごとにモデルの最尤tokenとの一致を確認し、候補末尾のEOSを順位scoreに使わない。Slimeは候補とEOSの合計対数尤度をN-bestへ補間していたため、`--neural-score-mode`を追加し、同じdecode結果からEOSの有無とtoken平均を比較した。公式実装は制約付き逐次探索であり、Slimeの上位3候補再ランキングと同一方式ではないが、EOSを候補表記の評価から分離する根拠として用いた。

JWTD devとGSD devだけで、EOS除外・lambda 0.85を凍結した。token平均は両devで悪化したため不採用とした。凍結後にAJIMEEとGSD testを各1回だけ現行方式と比較した。

| dataset | EOS込み / lambda 0.7 acc@1 | EOS除外 / lambda 0.85 acc@1 | MRR変化 | MinCER@1変化 |
| --- | ---: | ---: | ---: | ---: |
| JWTD-train dev 400件 | 0.4075 | **0.4200** | 0.4943 → 0.5010 | 0.0750 → 0.0742 |
| UD GSD dev 331件 | 0.8248 | **0.8580** | 0.8885 → 0.9062 | 0.1611 → 0.1354 |
| held-out AJIMEE 200件 | **0.6350** | **0.6350** | 0.6869 → 0.6869 | 0.0508 → 0.0506 |
| UD GSD held-out test 323件 | 0.8266 | **0.8638** | 0.8863 → 0.9074 | 0.1605 → 0.1202 |

製品FFIもEOS除外scoreとlambda 0.85へ切り替えた。xsmall実モデルによる50回の明示変換はp50 7.90 ms、p95 12.45 msで、候補数3・base cost差1000以下という既存の実行境界は維持する。AJIMEE非悪化と別domain held-out改善を同時に満たしたため採用する。

### 2026-08-02 大規模な非生成判別reranker

数百例では学習量が不足していた可能性を切り分けるため、JWTD v2 train全体から同じ読み信頼性フィルタを通過した48,393件を生成した。元JSONL行番号の`mod 10 == 0`を固定dev（4,788件）、残り43,605件を学習候補とし、`source_split + index`の複合identityで重複を除外する。AJIMEEはJWTD test由来なので引き続き最終held-outとした。

候補表層の文字unigram/bigram/trigramと、直前3文字の左文脈との交差素性を262,144次元へhashし、averaged perceptronでN-bestを再順位付けした。10,000学習例、3 epoch、1,048,576 byteの`f32`重みで、分離devのweight sweepは0.5が最良だった。

| dataset | baseline acc@1 | reranker acc@1 | MRR@10 | MinCER@1 | 判断 |
| --- | ---: | ---: | ---: | ---: | --- |
| JWTD-train固定dev 4,788件 | 0.3139 | 0.3926 | 0.5041 | 0.0769 | +7.87pt、weight選択に使用 |
| held-out AJIMEE 200件 | 0.5350 | 0.5250 | 0.6349 | 0.0641 | -1.0ptのため不採用 |

AJIMEEではMRRとMinCERはbaseline（0.627、0.068）より改善したが、主指標のtop-1完全一致が悪化した。分離devだけの大幅改善を根拠に製品へ入れると、Wikipedia修正窓の表記傾向へ過適合する。数万件級へ増やすだけでは、局所文字素性が意味的同音異義語を一般化できないという問題は解けない。

### 2026-08-02 25.6M teacherから1MB studentへの順位蒸留

正解表記ではなく、品質効果が確認済みのzenz-v3.1-xsmallがlambda 0.8で選んだN-best先頭候補をteacher labelとして、同じ1MB hashed perceptronへ蒸留した。teacherは学習時だけ使い、student推論は文字素性の加算だけである。

| teacher例 | 固定dev acc@1 | baseline比 | student採点p95 | model |
| ---: | ---: | ---: | ---: | ---: |
| 2,000 | 0.3480 | +3.41pt | 0.035 ms | 1,048,576 bytes |
| 10,000 | 0.3594 | +4.55pt | 0.020 ms | 1,048,576 bytes |

例数を5倍にしても改善は+1.15ptに留まり、teacher本体のdev `acc@1 0.423`級を再現できなかった。速度とサイズは十分だが、studentの局所hash表現に意味判断を保持する容量がない。dev段階でGo条件を満たさないためAJIMEEを追加評価せず、この蒸留方式は不採用とする。次にニューラルを比較するなら、文字n-gramへの蒸留ではなく、共有token embeddingと低rank/小hidden層を持つN-best専用studentを学習し、5–10M parameter以下で品質・p95・配布サイズを同時に測る。

### 2026-08-03 共有bi-encoder student

局所hash表現の容量不足を切り分けるため、MLXを使う評価専用の文字Transformer studentを実装した。左文脈末尾40文字と読みを1回だけencodeし、最大10件の候補表層は同じencoderで並列encodeする。両者のvector、要素積、絶対差を小さなMLPへ渡し、既存Viterbi costへ加算する。文章生成、辞書外候補、ネットワーク送信は行わない。64件をtrain/dev兼用にしたsanity checkではteacher順位を10 epochで100%再現し、loss/optimizer/model wiringが正常であることを先に確認した。

zenz-v3.1-xsmall（lambda 0.8）の先頭候補をteacher labelとした10,000件で、hidden 192、3層、6 head、1,737,985 parameterを3 epoch学習した。weightは固定devだけで選び、AJIMEEへ当てる前に0.5へ凍結した。

| dataset / epoch | baseline acc@1 | student acc@1 | MRR@10 | 備考 |
| --- | ---: | ---: | ---: | --- |
| JWTD-train固定dev 4,788件 / epoch 1 | 0.3139 | 0.3193 | 0.4499 | weight 2.0 |
| JWTD-train固定dev 4,788件 / epoch 2 | 0.3139 | 0.3244 | 0.4538 | weight 1.0 |
| JWTD-train固定dev 4,788件 / epoch 3 | 0.3139 | **0.3331** | **0.4585** | weight 0.5を凍結 |
| held-out AJIMEE 200件 | 0.5300 | **0.5200** | 0.6216 | baseline MRR 0.6236、-1.0pt |
| UD Japanese GSD外部dev 331件 | 0.6405 | **0.5921** | 0.7270 | 凍結weight 0.5、-4.84pt |

10候補を採点する単体latencyはApple GPUでp50 3.62 ms、p95 6.86 ms、重みは6,956,261 bytes（FP32。parameter数からのFP16概算3.48 MB）だった。速度とサイズは25.6M teacherより大幅に改善したが、held-outの主指標を悪化させたため製品へは組み込まない。候補生成自体も同じAJIMEEでp95約26 msを要するため、単純な常時追加は全体20 ms予算にも収まらない。

固定devで+1.92pt出てもheld-outで反転し、後述のnews/blog外部devではさらに大きく悪化した。同じJWTD修正窓10,000件へのteacher蒸留をモデル拡大だけで続ける根拠はない。AJIMEEはこの設定の最終報告に一度使用済みなので、以後の調整には戻さない。

## UD Japanese GSD 外部ドメイン評価

[UD Japanese GSD](https://universaldependencies.org/treebanks/ja_gsd/index.html) r2.18（commit `33e7310b58308e85fd2b33a2fc3ef3e434f821c7`）を、CC BY-SA 4.0の評価専用キャッシュとして追加した。Wikipedia修正履歴ではなくnews/blogを原文とし、手動由来の短単位語境界、UniDic品詞、表層発音を持つ。製品bundleには含めない。

```sh
just build-balanced-devset
just evaluate-balanced-dev --json
```

生成器は各文から、同梱辞書で同じ読みを持つ漢字表層が2件以上ある内容語を最大1件選ぶ。入力はUniDic表層発音、期待値は原文表層、文脈は現在語より前の最大40文字である。句読点と空白も左文脈に保持する。train 7,050文から学習用注釈列と1,940件、dev 507文から331件、test 543文から323件を生成した。単一の表記だけを正解とするため絶対精度より差分を重視し、testは方式とweightをdevで凍結した後にだけ使う。

初回baselineはdev `acc@1 0.6405`、`acc@10 0.9758`、test `0.6966` / `0.9814`だった。正解のほぼ全てがN-best内にあり、文脈順位モデルを測る目的に適している。

### 直前語context bigram

UD trainの表層・読み列から、直前語表層→現在語（表層・読み）の81,266遷移を数えた。候補先頭segmentと左文脈末尾が一致する場合だけ、`weight × (floor(log2(count)) + 1)`を既存costから引く評価専用rankerである。devだけでweightを探索し、同率の大きな値より副作用の小さい1500に凍結した後、testと他データセットを各1回評価した。

| dataset | baseline acc@1 | context bigram acc@1 | MRR@10 | 判断 |
| --- | ---: | ---: | ---: | --- |
| UD GSD dev 331件 | 0.6405 | **0.7190** | 0.8139 | +7.85pt、weight選択に使用 |
| UD GSD held-out test 323件 | 0.6966 | **0.7647** | 0.8461 | +6.81pt、同一news/blog domainで再現 |
| JWTD-train dev 400件 | 0.2925 | **0.2900** | 0.4278 | -0.25pt |
| AJIMEE 200件 | 0.5300 | **0.5300** | 0.6199 | top-1差なし、MRR悪化 |

UD testの候補生成込みlatencyはbaseline p95 0.85 ms、rankerあり0.86 msだった。ただしこれは起動時のモデル読み込みを含まない。2.24 MBの注釈テキストでも、無効なword/skip表まで構築した初期実装は冷起動0.69秒・最大RSS 105.1 MBを要した。有効なcontext表だけを構築すると、同一コマンド3回の中央値は0.22秒・最大RSS 49.1 MBまで改善したが、モデルなしの0.09秒・23.8 MBと比べると約0.13秒・25.3 MBの追加である。採点hot pathは軽くても、現形式の起動コストは製品基準を満たさない。

81,266遷移のmatch率はdev 5.97%、test 6.55%だった。少数の高価値な共起だけで同一domainを+6〜8pt改善できるため、LLMや文章生成が必須という仮説は否定できる。一方、Wikipedia系では改善せず、7,050文のexact bigramを汎用モデルとして同梱するのはNo-Goである。次は再配布・商用条件が明確で、複数domainを含む十分な規模の形態素注釈データを確保できた場合に限り、class backoff、pruning、compact格納を評価する。

一般会話を増やす候補としてTatoeba/Tanakaも確認したが、[公式説明](https://www.edrdg.org/wiki/Tanaka_Corpus.html)自身が文章は自然・代表的でなく、統計分析には使うべきでないと警告しているため学習には使用しない。

より適合するBCCWJは書籍、雑誌、新聞、白書、ブログ、ネット掲示板、法律など約1億語に形態論情報を持つが、本文は有償契約対象で再配布不可である。[商業利用料金](https://clrd.ninjal.ac.jp/bccwj/fee.html)は2021年10月以降、2年・社内利用者10人以内で40万円（税別）からであり、成果物の商品化には契約が必要となる。公開UD-BCCWJもannotationがCC BY-NC-SA 4.0で、ライセンス問題により本文は含まれない。契約なしに製品モデル学習へ使わない。

### JWTD + GSD複数ドメイン固定合成

GSD単独のexact contextが他ドメインへ汎化しなかったため、JWTD v2 trainの訂正文から語彙bigram用の`surface/reading`列を追加生成した。元JSONL行番号の`mod 10 == 0`を固定devとして除外し、残る43,605窓を学習側に限定する。生成コマンドは次のとおり。

```sh
just build-jwtd-context-corpus
```

注釈列は7,386,118 bytes、SHA-256 `256b9b4b56f5e7c665934b38243cf6bbd5e431baff93b605adb76d39dc9f0256`である。GSD trainと合わせた固定モデルはword 315,156遷移、context 282,446遷移。word重みはJWTD分離devだけで250、context重みは先のGSD devだけで1500に凍結し、組合せ後に再調整していない。

| dataset | baseline acc@1 | word=250 | word=250 + context=1500 | 判断 |
| --- | ---: | ---: | ---: | --- |
| JWTD固定dev 4,788件 | 0.3139 | 0.3680 | **0.3682** | +5.43pt |
| GSD dev 331件 | 0.6405 | 0.5831 | **0.6828** | word単独は反転、合成は+4.23pt |
| GSD held-out test 323件 | 0.6966 | — | **0.7090** | +1.24pt |
| AJIMEE held-out 200件 | 0.5300 | — | **0.5000** | -3.0pt、No-Go |

AJIMEEの`acc@10`は0.835のままで、MRRは0.6236級から0.6169へ悪化した。GSD testでわずかに再現しても、別held-outの主指標を悪化させるため製品へは入れない。また、`差→が`はJWTD学習側に7文あったが、全体最良のword重み250でも`差がつく`は第4候補のままだった。該当語のために重みを750へ上げても第4候補で、JWTD devの改善幅は+3.86ptまで下がる。単例に合わせた再調整は行わない。

固定合成の候補生成込みp95はJWTD dev 18.4 ms、GSD test 1.41 ms、AJIMEE 48.2 ms。さらに3回の冷起動中央値は1.47秒、最大RSS 204.7 MBで、現在のテキスト読み込み形式は配布要件を大きく超える。これは品質gateを通った後にprune/compact化すべき実装課題であり、品質No-Goのモデルを圧縮する理由にはしない。

## JWTD-train開発セット

AJIMEE-BenchはJWTD v2のtest分割由来のため、コストや言語モデルの調整に使うと過適合する。調整はJWTD v2の**train分割**から自動生成した開発セットで行い、AJIMEE-Benchはheld-outの報告専用とする。

```sh
just build-devset      # 初回のみJWTD v2(約99 MB)を取得し、400件を生成
just evaluate-dev      # 開発セットで評価
```

生成方法（`slime-tools/src/devset.rs`）: 単一diffが`kanji-conversion_a`の文対から誤り周辺の変換窓を切り出し、同梱辞書の表層→読み逆引き（読みが一意な表層のみ）で読みを推定する。誤変換前後の表層の読みが両方導出でき、かつ一致しない場合は読み推定を信頼せず破棄する。ページ偏りを避けるため等間隔サンプリングする。

制約: 許容解は1変種のみ（AJIMEEのような人手の許容解列挙が無い）ため、絶対値はAJIMEEより体系的に低く出る（例: 「ヒトの進化」に対して「人の進化」は不正解扱い）。**相対比較（調整前後の差分）にのみ使う。** 初回計測（2026-07-20、400件）: acc@1 0.293、acc@10 0.673、MRR@10 0.430。

この開発セットの初回運用で、かな数詞パーサが「せんぜん（戦前）」を1000+1000=2000と解釈する誤りを検出し、位取り単位の降順制約（千→百→十）を追加した。

## 元の日本語Wikipedia入力誤りデータセット

AJIMEE-Benchは、日本語Wikipedia入力誤りデータセットv2のtestデータから漢字誤変換200件を抽出し、かな入力、変換範囲、複数の許容解を人手確認した評価用データである。

元データは約70万文対を含み、誤字、脱字、衍字、転字、漢字誤変換など複数の問題が混在する。そのままかな漢字変換評価へ使うのではなく、将来の統計モデルでは次のように扱う。

1. 学習には元データの`train`分割だけを使用する。
2. `kanji-conversion`を抽出し、読みと変換対象範囲を別途生成・検証する。
3. AJIMEE-Benchは元データのtest由来なので、学習や調整には使用しない。
4. must-pass、AJIMEE、balanced corpusの結果を別々に報告する。

## ライセンス

AJIMEE-Benchの評価データと元の日本語Wikipedia入力誤りデータセットはCC BY-SA 3.0。AJIMEE-Benchの`utils.py`と`test_utils.py`はCC0 1.0。UD Japanese GSDはCC BY-SA 4.0。評価データはダウンロードキャッシュとして分離し、利用時は各配布元のライセンスと帰属表示に従う。
