# ベンチマーク方針

## 目的

micro benchmarkは、変更前後で同じhot pathを比較するために使う。異なる端末の絶対値を製品性能として比較しない。必ずrelease profileで計測し、一度に一つの変数だけを変更する。

現在は外部benchmark crateを使わず、`std::hint::black_box`と`Instant`による小さなharnessを使う。辞書規模が大きくなり統計的な比較が必要になった時点でCriterionまたはDivanを再評価する。

## 計測対象

| benchmark | 内容 | 初期性能予算 |
| --- | --- | ---: |
| `romaji/nihongo` | `nihongo`全体のincremental変換とflush | 1キー平均 p95 5 ms未満の十分内側 |
| `converter/candidate_window_single_word` | 完全一致候補とN-best経路の生成・sort | 20 ms未満 |
| `converter/segmented_phrase` | `わたしはにほん`のラティス探索 | 20 ms未満 |
| `converter/n_best_phrase` | `わたしはにほん`の上位10候補を有限ビームで探索 | 20 ms未満 |
| `converter/n_best_search_20` | 同じ入力で内部探索幅だけ20へ広げた比較用ケース | 評価用。上位10件との差分を記録 |
| `converter/candidate_window_long_sentence` | 約50文字の通常候補窓でN-best bucketのtailを追跡 | 20 ms未満 |
| `converter/fixed_segment_variants_long_sentence` | 候補末尾で最良分割を固定し、各文節内8候補から最大22件を追加 | 20 ms未満 |
| `converter/short_compound_recall` | 候補末尾で起動する、完全一致語を2語組み合わせた最大16件の補助探索 | 5 ms未満 |
| `converter/three_part_compound_recall` | 各要素4候補の3語fixtureを最大16件へ絞る補助探索 | 5 ms未満 |
| `converter/one_character_segment_compound_recall` | 1かな読みの語を途中に含む3語fixtureの補助探索 | 5 ms未満 |
| `converter/kana_only_segment_compound_recall` | 辞書上かな表記だけの助詞を途中に含む混在複合の補助探索 | 5 ms未満 |
| `converter/four_part_compound_recall` | 各要素4候補の4語fixtureを最大16件へ絞る補助探索 | 5 ms未満 |
| `converter/five_part_compound_recall` | 各要素4候補の5語fixtureを最大16件へ絞る補助探索 | 5 ms未満 |
| `converter/six_part_compound_recall` | 各要素4候補の6語fixtureを最大16件へ絞る補助探索 | 5 ms未満 |
| `converter/digit_counter_phrase` | 全角数字列と年・月・日の数詞接続を含む候補窓 | 20 ms未満 |
| `engine/nihon_conversion` | engine生成から入力、変換、確定まで | 参考値。cold startを分離予定 |
| `engine/typo_correction_neighbor` | 辞書完全一致のない隣接キー誤りから、元の読みと訂正候補を生成 | 20 ms未満 |
| `engine/typo_correction_missing_vowel` | 辞書完全一致のない母音1文字欠落から、元の読みと訂正候補を生成 | 20 ms未満 |
| `engine/typo_correction_missing_geminate` | 辞書完全一致のない促音子音1文字欠落から、元の読みと訂正候補を生成 | 20 ms未満 |
| `engine/typo_correction_missing_consonant` | 6文字以上のローマ字入力における子音1文字欠落から訂正候補を生成 | 20 ms未満 |
| `engine/history_completion_{off,on}_4096_entries` | 上限4096件の履歴を保持した長寿命engineで`pafu`を入力・確定 | ON/OFFの絶対差を5 ms未満に保つ |
| `engine/adaptive_context_{empty,128}` | 分野辞書ONで、文脈なしと端末内文脈128件を比較 | 差を0.1 ms未満に保つ |
| `engine/persistent_context_500` | 分野辞書ONで、上限500件の永続文脈から完全一致を順位に反映 | 文脈なしとの差を0.1 ms未満に保つ |
| `engine/static_context_{baseline_no_pack,miss,exact,suffix}_10001_rules` | v3パック1万ルールで最大128文字の末尾検索と既存候補の順位変更 | パックなしとの差を0.1 ms未満に保つ |
| `engine/static_context_next_candidate_{baseline_no_pack,10001_rules}` | 文章の直後で`かんじ`の候補窓を開き、NextCandidateで候補注釈を再構築 | パックなしとの差を0.01 ms未満に保つ |
| `engine/static_context_pack_load_{unsigned,signed}_10001_rules` | 同じ1万ルールpackのparseと任意のEd25519署名検証 | 署名検証の追加を1 ms未満に保つ |
| `ffi/engine_{cold,warm}_create` | SwiftからFFI engineを初回・共有辞書初期化後に生成 | cold p95 100 ms未満、warm p95 1 ms未満 |

現状は1,085,466語の基本辞書（抽出閾値8500、AJIMEE-Benchで精度が飽和する下限）と344語の任意分野辞書を使う。基本辞書は`build.rs`でTSVからFST+エントリ表+表層プールのバイナリ形式（計約29 MB、TSV 44 MBから圧縮）へ事前コンパイルし、`include_bytes!`でzero-copy参照する。辞書拡大直後はTSVの起動時parseで cold `Dictionary::bundled()` が約386 msだったが、コンパイル形式への移行後は約70 µs、変換ツール実行時の最大RSSは221 MB→4.2 MBになった（Apple M3、release）。

2026-08-08、Apple M3、arm64、macOS 26.6、Releaseで、macOSのキー入力hot pathをJSON応答から同期型付きaction callbackへ切り替えた後のsmokeを2,000 sample（live入力は500、warm createは1,000）で採取した。`character_no_live`はp50 708 ns / p95 792 ns、`idle_backspace`は416 ns / 459 ns、`space_conversion`は102.6 µs / 120.9 µs、`live_character_50`は2.67 µs / 4.25 µs、履歴500件の補完は10.2 µs / 11.3 µsだった。cold createは5.77 ms、warm createはp50 184 µs / p95 239 µsで、既存予算内にある。変更前を同一processで採取していないため短縮率は主張せず、この値を型付きmacOS bridgeの次回比較baselineとする。

同日、候補の確定値・従来表示・由来・補足を分離するv2 callbackへmacOSを移行した後、同じscriptで再採取した。`character_no_live`はp50 583 ns / p95 625 ns、`idle_backspace`は333 ns / 334 ns、`space_conversion`は76.6 µs / 110.7 µs、`live_character_50`は2.63 µs / 5.92 µs、履歴500件の補完は9.08 µs / 10.63 µsだった。cold createは6.33 ms、warm createはp50 156 µs / p95 195 µsで、候補metadata生成とSwift側の型付き受け渡しを含めても既存予算内にある。単発run間の短縮率は主張せず、今後はこのv2値を候補UI経路のbaselineとする。

2026-08-15、通常履歴の上限を500件から4096件へ拡張した後、最大件数fixtureで補完OFF/ONを
各1000回測定した。OFFは2429 ns/op、ONは10121 ns/op、差は約7.7 µsで5 ms予算の十分内側だった。
UD balanced dev 331件の逐次LIVE入力でも空履歴p95 0.207 ms、4096件p95 0.168 msで、
最終精度は一致した。単発runの絶対値差は改善とはみなさず、上限時の回帰がない証拠として扱う。

2026-09-30の訂正: 上記のbenchmarkは名前だけ`_4096_entries`へ変わり、通常履歴fixtureは
500件のままだった（上限500件の文脈履歴fixtureが逆に4096件になっていた）。両fixtureを
それぞれの上限へ直して再計測すると、4096件の補完ONは56,463 ns/opで、OFFとの差は約55 µsだった。
補完の線形走査が全件で先に`is_useful_history`（読み・表記の文字数計数と文字種走査）を
評価していたため、件数・読みprefixの安価な条件を先に評価するよう並べ替えた。条件は純粋な
論理積なので結果は変わらない。変更後はOFF 1,517 ns/op、ON 13,272 ns/opで、差は約12 µsになった
（各1,000反復の単発run）。

同日、履歴学習ONで4096件の履歴を持つengineの確定（`kikou`のSpace+Enter）は約18 msかかっていた。
保存のたびにファイルを3回読み、2回parseし、全件を比較関数内で`is_useful_history`を呼ぶsortで
2回並べ替えていた。書き込んだ履歴をそのままメモリへ採用して読み戻しとparseを省き、trimの
sortを事前計算したkeyによる安定sortへ変えると、約12.5 msになった。残りの大半は履歴・文脈履歴
それぞれの`sync_all`（macOSでは`F_FULLFSYNC`、1回約4.7 ms）である。確定キーの同期経路から
永続化を外すかは、クラッシュ時にどこまでの学習を保証するかの方針判断として別に扱う。

2026-08-01、Apple M3、release、1,000 iterationsで、全角数字と年・月・日を含む`converter/digit_counter_phrase`は489,084 ns/opだった。allocation benchmarkは419 allocations/op、101,387 bytes/op。数詞node追加後も20 ms予算の十分内側にある。

同日、語bigramの再ランキング対象を広げる判断のため`わたしはにほん`を探索幅10/20で比較した。幅10は1,366,503 ns/op、172 allocations/op、59,605 bytes/op、幅20は4,756,460 ns/op、300 allocations/op、152,305 bytes/opだった。状態identityを`HashMap`で索引する試作は幅20を4,592,558 ns/opへ3.4%短縮しただけで、幅10を1,794,526 ns/opへ悪化させ、割当も増えたため撤回した。広い探索を常用する前に、線形scanを置き換える低割当なbucket/heap設計が必要である。

## 初回baseline

2026-07-19、Apple M3、arm64、macOS 26.5.1で採取。表示値は1操作あたりの単純平均で、p95ではない。

| benchmark | 結果 |
| --- | ---: |
| `romaji/nihongo` | 7,847 ns/op |
| `converter/exact_candidates` | 1,573 ns/op |
| `converter/segmented_phrase` | 2,237 ns/op |
| `engine/nihon_conversion` | 18,174 ns/op |

反復回数は順に50,000、25,000、25,000、10,000。端末状態による揺れがあるため、最適化判断では同じprocess、同じ反復回数で複数回測る。

### Live Conversion smoke baseline

2026-07-20、同じApple M3環境で、入力ごとに最良変換を更新する経路を100反復で計測した。値は1キーではなく、指定文字数の入力から確定までの一連操作にかかった時間。

| benchmark | 結果 | 1キー平均 |
| --- | ---: | ---: |
| `engine/live_conversion_10` | 122,607 ns/op | 約0.012 ms |
| `engine/live_conversion_50` | 5,607,263 ns/op | 約0.112 ms |
| `engine/live_conversion_100` | 32,981,022 ns/op | 約0.330 ms |

100文字でも当初予算の1キー5 ms未満には収まる。長文ほど増加率が高いため、将来の辞書拡張時には差分ラティス化の判断材料として同じケースを再計測する。

2026-08-15、同じengine内で1回明示訂正したフレーズを次回LIVEから反映する一時履歴を追加した。
Apple M3、Release、100反復では、永続履歴count 2の`変換精度`が1.784 ms/op、同一engineの
直近選択が1.774 ms/opだった。どちらも`henkanseido`全入力とEnterを含み、約0.137 ms/キーで
同等である。同じrunの通常LIVEは10文字0.138 ms、50文字4.121 ms、100文字12.180 msで、
1キー5 ms予算内を維持した。一時履歴は直近64フレーズに固定し、user data再読込またはengine破棄で
解放する。

同日、optional neural runtimeのLIVE採点を入力threadから分離した。Apple M3、Release、
`zenz-v3.2-xsmall-Q5_K_M-fixed.gguf`、`lambda=0.2`、cost gap 1000、switch margin 0.5で、
30回のsnapshot作成はp50 0.000 ms / p95 0.001 ms、workerの候補生成+推論はp50 9.101 ms /
p95 14.015 msだった。初回のworkerは21.317 ms、同じmodelを共有したworkerと明示Spaceの並行probeでは
Space 10.308 ms、worker/applyはともに成功し、入力後の古い結果はpreeditを書き換えなかった。

同じ日に、長い読みだけ`lambda=0.3`へ上げ、LIVEで文字数の変わる自動置換を拒否するpolicyを
small Q3 requantで測定した。20回のsnapshotはp95 0.001 ms、workerはp50 18.437 ms /
p95 22.243 ms、同時Spaceは28.740 ms、最大RSSは181.97 MiBだった。`変換精度`の厳密probeは
成功したが、RSSはxsmall Q5の約94 MiBに対して倍近いため、標準モデルには採用しない。

その後の4集合評価では長い読みのweightを`0.5`へ上げ、候補数・推論回数を変えずにJWTDを
`0.3200→0.3525`、AJIMEEを`0.5150→0.5300`へ改善した。UD testで検出した
`通う→火曜`はweightではなく変換済み活用語尾の上書きが原因だったため、漢字語幹+かな語尾を
語尾ごと変えるLIVE変更を拒否した。UD dev/testを含む4集合で正解からの退行0を確認している。
0.6〜0.8はJWTDで最大1ポイント追加した一方、`放送局の実態→実体`の退行が出たため不採用。
代わりに短いliteral末尾のうち2字以上の漢字語だけを通常margin後に許可すると、候補数と推論量を
変えず、公開language pack込みでJWTD `0.3575→0.3625`、AJIMEE `0.5300→0.5400`となり、
UD held-out testを含む4集合でCore正解からの退行0を維持した。
さらに、かな活用語尾を保持する語幹漢字化だけを許可し、単独の格助詞・係助詞を除外すると、
候補数と推論量を変えずにJWTDは`0.3700`、AJIMEEは`0.5550`へ改善した。UD dev/testは不変で、
4集合すべて退行0を維持した。
促音直前の安定接頭辞だけを解除して全文を再評価すると、AJIMEEは同期Core `0.3050→0.3200`、
最終LIVE `0.5550→0.5700`となった。UD dev/testとJWTDは不変で、4集合すべて退行0だった。
さらに、3文字以上の未解決かなで辞書・モデル1位が一致する場合だけmargin 0.2を使うと、
AJIMEEは`0.5750`となった。2文字以下と暗黙数字を除外し、UD dev/testは不変、JWTDの追加変更は
全文かなをほぼ正しい表記へ近づける1件だけで、4集合すべて退行0だった。
さらに、辞書上位2件が単純主題`は`までに異なる場合だけ、32文字以内で安定接頭辞marginを
500から1000へ上げた。JWTDは`0.3700→0.3725`、AJIMEEは`0.5750→0.5800`、UD dev/testは
不変だった。無制限に主題を保留すると68文字の長文を1件悪化させたため不採用とし、32文字上限、
主題prefix差の確認、`には`境界の除外後は4集合で従来正解からの新規誤り0を確認した。

```sh
SLIME_LIVE_NEURAL_SAMPLES=30 \
cargo run --release -p slime-ffi --features neural \
  --example live_neural_probe -- \
  target/evaluation/models/zenz-v3.2-xsmall-Q5_K_M-fixed.gguf
```

### 接頭辞を保持するライブ変換（撤回済み）

2026-07-29、同じApple M3、arm64、Releaseで100反復を採取した。ライブ変換は
通常2経路のN-bestで異なる表記のコスト差を調べ、上位経路が同じ表記の場合だけ
4経路へ適応的に広げる。変換済み接頭辞と未確定末尾は`LivePreview`で分離して保持する。

```sh
SLIME_BENCH_ITERATIONS=100 \
SLIME_BENCH_WARMUP_ITERATIONS=10 \
SLIME_BENCH_LIVE_LENGTHS=10,50,100 \
cargo bench -q -p slime-core --bench engine
```

| benchmark | 結果 | 1キー平均 |
| --- | ---: | ---: |
| `engine/live_conversion_10` | 178,222 ns/op | 約0.018 ms |
| `engine/live_conversion_50` | 9,583,383 ns/op | 約0.192 ms |
| `engine/live_conversion_100` | 47,701,929 ns/op | 約0.477 ms |

50文字ではN-bestを常時4経路にした試作の24,261,967 ns/opから60.5%短縮した。
100文字でも1キー5 ms予算の10分の1未満である。

2026-08-01、接頭辞保持の導入前後をJWTD-train開発セット400件で比較したが、
acc@1 0.2925、acc@10 0.6725、MRR@10 0.429758、minCER@1 0.095180で完全に同一だった。
この評価は最終候補だけを対象とし、ライブ入力の途中状態を通らない。実入力では
`持っと`、`行為うの`、`1勝ちがう`のように以前の分割と新しい末尾を連結する回帰を
生んだため、接頭辞保持を撤回した。上記の速度値は当時の実装記録として残す。

### 複数辞書対応後の長文探索上限

2026-07-20、同じApple M3、arm64、Releaseで、各ケースを10回ずつ採取した。各辞書レイヤーの最長読みを超えるprefix探索を打ち切る変更だけを比較した。

| scenario | baseline p50 | baseline p95 | after p50 | after p95 |
| --- | ---: | ---: | ---: | ---: |
| Live Conversion 10文字 | 0.122 ms | 0.124 ms | 0.124 ms | 0.125 ms |
| Live Conversion 100文字 | 32.953 ms | 33.273 ms | 30.987 ms | 31.146 ms |

10文字では約0.002 msの固定費が増えたが、100文字のp50は6.0%、p95は6.4%短縮した。絶対値と長文tailの改善を優先して変更を維持する。入力列、反復回数、Release設定は変更前後で同一。

### N-best候補生成

2026-07-20、同じApple M3、arm64、Releaseで各2,000反復を3回実行した中央値。当時の候補生成は入力位置ごとに最大80状態を保持し、ライブ変換の1-best経路とは分離していた。

| scenario | 中央値 |
| --- | ---: |
| 単語候補 `にほん`（完全一致 + N-best） | 0.152 ms |
| 1-best `わたしはにほん` | 0.034 ms |
| N-best `わたしはにほん` | 1.408 ms |

現在もSpaceの候補UIは上位10件を探索する。ライブ変換は上位表記の信頼度判定に
2経路、表面重複時だけ4経路を使うため、この上位10件のコストは加えない。

2026-08-08の1かな要素対応前、同じ端末のRelease、各1,000反復のsmokeでは、`あさいり`のN-best 32探索は3.295 ms/opだった。2〜4語補助探索を同じprocessで測ると、2語0.0087 ms/op、各要素4候補の3語0.0206 ms/op、4語0.0513 ms/opだった。4語探索は上限3語で結果を返さない基準状態の0.0144 ms/opから約0.0377 ms増えた。補助探索は16文字、4要素、通常設定では途中状態64件（API上限512件）、出力16件に制限し、通常の候補初回表示では実行しない。

同日、Time Profilerの自己時間では`insert_n_best_node`が2,743 ms、接続cost参照が353 msで、候補生成の支配的コストは満杯bucketへの挿入だった。bucketの最大costを保持し、残らない候補を線形走査前に棄却する単一変更を比較すると、同じprofileの候補挿入は1,175 msへ57%減少した。中立な長文fixtureをRelease、変更前200反復3回、変更後500反復3回で測った中央値は13.13 ms/opから7.73 ms/opへ約41%短縮した。固定dev 400件の候補表層・costは全件一致し、held-out 200件の3回中央値はp95 23.66 msから12.10 msへ短縮した。

入力別の分析には`--export-nbest path`を使う。書き出した各項目の`input_characters`と`candidate_generation_ms`を集計し、短文平均だけで長文tailを隠さない。

1かな読みの語を含む複合を回収する変更後、各5,000反復3回の中央値は専用3語fixtureが0.0204 ms/op、既存3語fixtureが0.0208 ms/op、4語fixtureが0.0514 ms/opだった。実辞書の`あさいり`補助探索は分割候補が増えて0.0089 ms/opから0.1137 ms/opへ増加した。早期棄却後のN-best 32探索は1,000反復3回の中央値で1.1198 ms/opであり、補助探索との合計約1.23 msは明示候補操作の5 ms予算内にある。

5要素対応は同じ16文字、各要素4候補、各位置・要素数64状態、出力16件の上限を維持した。専用5語fixtureのRelease 5,000反復は、上限4要素で結果を生成しない基準0.0423 ms/opから、5要素候補を生成する0.0850 ms/opへ増えた。既存の3語、1かなを含む3語、4語fixtureはそれぞれ0.0206、0.0203、0.0510 ms/opで、明示候補操作の5 ms予算内にある。

6要素対応も読み16文字、各要素4候補、各位置・要素数64状態、出力16件の上限を維持した。同一Release build・10,000反復では、既存5要素fixtureが変更前0.084949 ms/op、変更後0.085210 ms/op、新しい6要素fixtureが0.122751 ms/opだった。追加層は通常候補ではなく、候補末尾で明示的に追加候補を探す操作だけに使う。

辞書上かな表記だけの要素は、その部分読みに変換表記が一つもない場合だけ補助探索へ入れ、最終結果が全かなの経路は捨てる。助詞を含む4要素fixtureはRelease 5,000反復で0.0034 ms/opだった。実辞書の`あさいり`は0.1152 ms/op、既存3〜6要素fixtureは0.0205〜0.1228 ms/opで、候補枠と性能予算を維持した。

長文は全体のN-bestを広げず、最良分割を固定して各文節内だけ最大8候補を組み替える。中間状態256件、出力128件をAPI上限とし、製品経路は追加22件に固定した。約50文字の中立fixtureをReleaseで100 warmup、1,000反復した`converter/fixed_segment_variants_long_sentence`は7.42 ms/opだった。初回候補では実行せず、9〜128文字で候補末尾へ到達した一回だけ実行する。

同じ端末のRelease、100 warmup、10,000反復で、隣接キー訂正は0.139 ms/op、母音欠落訂正は0.200 ms/op、促音子音欠落訂正は0.474 ms/op、6文字以上に限定した子音欠落訂正は0.571 ms/opだった。正例41件と、既知語・短すぎる入力・訂正不能入力の負例41件を独立したfixtureで固定し、7種類の各訂正操作に2件以上を要求する。

3つの分野別辞書を合計344語へ拡充した後、すべて有効にした通常変換をRelease、5,000反復、10 sampleで比較した。

| scenario | p50 | p95 |
| --- | ---: | ---: |
| 基本辞書のみ `nihon`変換 | 3,019 ns | 3,057 ns |
| 基本辞書 + 3分野辞書 | 3,328 ns | 3,358 ns |

レイヤー横断により約10.2%増えたが、絶対差は約0.00031 msである。辞書を無効にした場合は検索しないため、不要な分野辞書を設定から外せる構成を維持する。

### 入力履歴500件の補完コスト

2026-08-08、同じApple M3、arm64、Releaseで、履歴を上限の500件まで読み込んだ長寿命engineを使い、`pafu`の入力から確定までを各1,000反復のsmokeで計測した。fixture生成とファイル読み込みは計測外である。

| scenario | mean | iterations |
| --- | ---: | ---: |
| 履歴候補OFF | 1,687 ns | 1,000 |
| 履歴候補ON | 8,681 ns | 1,000 |

500件を線形走査する補完は約0.0070 msの固定費を加えた。端末内文脈も別に上限500件まで走査するため、件数上限を増やす場合は同じfixtureを更新し、読みprefix indexの要否を判断する。

### 端末内文脈のlookup

2026-08-08、同じApple M3、arm64、Releaseで、3分野辞書を有効にし、文脈なし、端末内文脈128件、再読込した文脈500件を各1,000反復のsmokeで比較した。入力、Space変換、composition消去までを含み、文脈の構築とファイル読込は計測外である。

| scenario | mean | iterations |
| --- | ---: | ---: |
| 文脈なし | 1.873 ms | 1,000 |
| 端末内文脈128件 | 1.889 ms | 1,000 |
| 再読込した端末内文脈500件 | 1.875 ms | 1,000 |

単一sampleなので分布の証拠ではないが、最大の絶対差は約0.016 msで0.1 msの個別予算内だった。文脈候補は再起動前後とも同じ上限500件のストアから取得し、別のセッション順位を重ねない。

### 静的文脈パックの末尾検索

2026-08-08、同じApple M3、arm64、Releaseで、同じ読みを持つ10,001ルールの
v3パックと128文字の直前表記を使い、各5,000反復を3回実行した中央値。pack生成と
読み込みは計測外で、通常の候補生成と明示的な左文脈順位付けを含む。

| scenario | 中央値 | パックなしとの差 |
| --- | ---: | ---: |
| パックなし | 0.140850 ms | - |
| 1万ルール・一致なし | 0.170593 ms | +0.029743 ms |
| 1万ルール・完全一致 | 0.141253 ms | +0.000403 ms |
| 1万ルール・128文字末尾で一致 | 0.170773 ms | +0.029923 ms |

ルールは読みで範囲を一度だけ二分探索し、直前表記の最大128 suffixを長い順に検索する。
最悪側の追加費用も0.1 msの個別予算内にある。実パックのrelease gateでは、この
micro benchmarkに加えて非公開held-outの候補生成p95、pack読み込み、RSS、容量を測る。

2026-09-30、先頭文節ルールの支持判定を追加した後に同じbenchmarkを再計測すると、
一致なし約0.59 ms、128文字末尾一致約0.57 msまで悪化し、0.1 ms予算を超えていた。
全読みルールで昇格が埋まらない場合に、部分読みのルールが存在するかを確かめずに2回目の
N-best探索を実行していたためである。全読みルールは直前で全件確認済みなので、読みの
真部分文字列に一致するルールがない場合はN-best探索を省くよう変更した。判定は読み順に
並んだルールを開始位置ごとに接頭辞範囲で絞り込み、候補がなくなった時点で打ち切る。
また、読み込み時にルールの直前表記の最大文字数を記録し、それより長いsuffixは検索しない。

候補窓でも、NextCandidateのたびに候補注釈のため辞書候補生成と文脈判定をやり直していた。
Space変換と文節候補の生成で得た文脈昇格の結果を (読み, 直前表記) をkeyにengineへ保持し、
注釈はそれを再利用する。辞書・pack・privacyが変わる`set_preferences`・`reload_user_data`で
破棄する。同じ端末、各5,000反復を3回実行した中央値:

| scenario | 変更前 | 変更後 | パックなしとの差 |
| --- | ---: | ---: | ---: |
| パックなし | 0.173 ms | 0.183 ms | - |
| 1万ルール・一致なし | 0.592 ms | 0.174 ms | 誤差内 |
| 1万ルール・完全一致 | 0.240 ms | 0.139 ms | 短い左文脈のため比較対象外 |
| 1万ルール・128文字末尾で一致 | 0.573 ms | 0.180 ms | 誤差内 |
| NextCandidate・パックなし | 0.0032 ms | 0.0032 ms | - |
| NextCandidate・1万ルール | 0.135 ms | 0.0035 ms | +0.0003 ms |

変更前の値は単発runで、短縮率の根拠ではなく予算超過の確認にだけ使う。

同じpackを読み直すengine生成は、500反復を3回実行した中央値で、無署名互換経路が
7.131146 ms、署名必須経路が7.531123 msだった。Ed25519 strict verificationの追加は
約0.400 msで、1 msのpack読込個別予算内にある。署名対応前に作成済みの同一Release
macOS dylibと対応後を比較すると、Mach-O segment合計は42,074,112 bytesから
42,336,256 bytesへ262,144 bytes（約0.62%）増えた。署名検証はpack load・reload時だけで、
候補生成やキーイベント経路では実行しない。

### 適応品質の固定gate

`just evaluate-adaptation` は、端末内文脈学習と保守的な入力ミス訂正を、実行時間ではなく改善と副作用に分けて検証する。2026-08-08の固定fixtureでは次を要求する。

| gate | 結果 |
| --- | ---: |
| 文脈学習で誤った先頭候補を直したケース | 6 / 6 |
| 文脈学習前から正しい先頭候補を維持したケース | 6 / 6 |
| 文脈学習で正解を壊したケース | 0 / 6 |
| 入力ミス訂正の正例recall | 41 / 41 |
| 入力ミス訂正を出してはいけない負例 | 0 / 41 |
| 入力ミス訂正の全入力p95 | 1.5 ms級 / 20 ms以下 |
| 1入力あたりの訂正候補 | 最大3 / 3以下 |

これは一般化を証明する母集団ではなく、既知の境界を壊さないための小さな回帰gateである。`slime-typo-evaluate`は外部の正例・負例を読み、語彙を出力せず、recall、不要訂正、編集種別、p95、訂正候補数を集計する。実入力由来のデータを追加するときも、改善件数と破壊件数を別々に報告する。

### Swift → C ABI → Rust → Swiftのtail latency

2026-07-20、同じApple M3、arm64、Releaseで、fixture生成と入力prefixを計測外に置き、1操作ごとの分布を採取した。

| scenario | p50 | p95 | sample |
| --- | ---: | ---: | ---: |
| 通常の1キー入力 | 0.0053 ms | 0.0057 ms | 1,000 |
| `nihon`のSpace候補表示 | 0.0112 ms | 0.0113 ms | 1,000 |
| Live Conversionの50文字目 | 0.2775 ms | 0.2800 ms | 500 |
| 履歴500件で補完を更新（端末内文脈対応前の参考値） | 0.0193 ms | 0.0199 ms | 1,000 |

全ケースが1キーp95 5 ms、候補初回表示p95 20 msの予算内にある。これはIMEアダプター内の処理時間であり、対象アプリのmarked text描画を含むend-to-end値ではない。

### debounce後のLIVE worker候補幅

2026-08-15、small Q3モデルと公開language packを使い、同期入力から分離された実
`snapshot → worker → apply`経路を単独processで測定した。読み9文字以上だけ16-best、
短い読みと同期Spaceは10-bestである。

| dataset | worker p50 | worker p95 | max | 品質差 |
| --- | ---: | ---: | ---: | --- |
| JWTD dev 400件 | 34.55 ms | 90.42 ms | 125.09 ms | 正解 +2、退行0 |
| AJIMEE 200件 | 27.04 ms | 94.58 ms | 176.77 ms | 正解不変、退行0 |

32-bestはJWTDで正解がさらに1件増えたが、p50 `84.99 ms`、p95 `325.25 ms`、最大
`537.66 ms`だったため不採用とした。worker処理は入力スレッドを塞がないが、180 msを越える
結果は次の入力で世代不一致になりやすい。候補recallだけでなく、この完了時間を採用条件にする。

同じ16-bestで、通常cost-gap 1000を10文字以上のtargetだけ1500へ緩和したA/Bでは、
JWTDの正解が`154→156`、退行0となった。単独processの通常gateはp95 `93.43 ms`、
限定緩和は反復2回で`100.90/111.22 ms`だった。無制限採点は短いtargetの退行を生じるため
採用せず、追加処理は180 ms debounce後のworkerだけに閉じる。

限定緩和を維持したまま長文候補上限を12/14/15へ絞ると、12/14件はJWTDの正解を1件失い、
15件は16件と同じ`156/400`、改善73、退行0を維持した。さらにAJIMEEは
`116→117/200`へ改善し、UD dev/testは不変だった。同一条件の単独processで15件のp95は
JWTD `80.23/81.40 ms`、AJIMEE `112.95 ms`、16件は`87.72/132.80 ms`だったため、
長文workerの上限を15件へ縮小した。

### engine cold / warm生成

2026-07-20、同じApple M3、arm64、Releaseで、benchmark processを5回起動して初回engine生成を各1回、辞書初期化後のengine生成を各1,000回計測した。

| scenario | p50 | p95 | sample |
| --- | ---: | ---: | ---: |
| 170,229語の初回解析を含むcold生成 | 66.282 ms | 66.822 ms | 5 process |
| `Arc`共有後のwarm生成 | 約0.030 ms | 約0.030 ms | 5 × 1,000 |

cold生成は100 ms予算内で、同じinput method process内の後続セッションは1 ms予算を大幅に下回る。現時点ではcompiled辞書ファイルを追加せず、単一の埋め込みTSVを初回だけ解析する構成を維持する。辞書を25万語以上へ増やす際はcold p95とbundle sizeを再計測し、100 msを超えた時点で生成済みbinary形式を検討する。

## 実行方法

```sh
cargo bench -p slime-romaji --bench romaji
cargo bench -p slime-converter --bench converter
cargo bench -p slime-core --bench engine
```

短いsmoke run:

```sh
SLIME_BENCH_ITERATIONS=10000 cargo bench -p slime-core --bench engine
```

Live Conversionの長さを一つに絞る場合:

```sh
SLIME_BENCH_ITERATIONS=100 SLIME_BENCH_WARMUP_ITERATIONS=10 SLIME_BENCH_LIVE_LENGTHS=10 cargo bench -p slime-core --bench engine
```

Live Conversionを省き、分野別辞書の差だけを測る場合:

```sh
SLIME_BENCH_ITERATIONS=10000 SLIME_BENCH_LIVE_LENGTHS= cargo bench -p slime-core --bench engine
```

## 今後追加する計測

- allocation countと割り当てbyte数
- compiled dictionaryのファイルサイズ
- mmap直後とwarm後のRSS
- cold/warm prefix lookup
- 候補数10/100/1000件
- 10/50/100文字の入力
- user dictionaryあり/なし
- TextEditでのkey down → marked text反映時間

性能を理由に`unsafe`、特殊なhasher、arena、small-string最適化を導入する場合は、先にこのbenchmarkでbottleneckを示す。
