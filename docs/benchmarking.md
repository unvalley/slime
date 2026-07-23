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
| `engine/nihon_conversion` | engine生成から入力、変換、確定まで | 参考値。cold startを分離予定 |
| `engine/history_completion_{off,on}_500_entries` | 500件の履歴を保持した長寿命engineで`pafu`を入力・確定 | ON/OFFの絶対差を5 ms未満に保つ |
| `engine/session_context_{empty,128}` | 分野辞書ONで、文脈なしと上限128遷移の最古hitを比較 | 差を0.1 ms未満に保つ |
| `ffi/engine_{cold,warm}_create` | SwiftからFFI engineを初回・共有辞書初期化後に生成 | cold p95 100 ms未満、warm p95 1 ms未満 |

現状は1,085,466語の基本辞書（抽出閾値8500、AJIMEE-Benchで精度が飽和する下限）と344語の任意分野辞書を使う。基本辞書は`build.rs`でTSVからFST+エントリ表+表層プールのバイナリ形式（計約29 MB、TSV 44 MBから圧縮）へ事前コンパイルし、`include_bytes!`でzero-copy参照する。辞書拡大直後はTSVの起動時parseで cold `Dictionary::bundled()` が約386 msだったが、コンパイル形式への移行後は約70 µs、変換ツール実行時の最大RSSは221 MB→4.2 MBになった（Apple M3、release）。

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

### 複数辞書対応後の長文探索上限

2026-07-20、同じApple M3、arm64、Releaseで、各ケースを10回ずつ採取した。各辞書レイヤーの最長読みを超えるprefix探索を打ち切る変更だけを比較した。

| scenario | baseline p50 | baseline p95 | after p50 | after p95 |
| --- | ---: | ---: | ---: | ---: |
| Live Conversion 10文字 | 0.122 ms | 0.124 ms | 0.124 ms | 0.125 ms |
| Live Conversion 100文字 | 32.953 ms | 33.273 ms | 30.987 ms | 31.146 ms |

10文字では約0.002 msの固定費が増えたが、100文字のp50は6.0%、p95は6.4%短縮した。絶対値と長文tailの改善を優先して変更を維持する。入力列、反復回数、Release設定は変更前後で同一。

### N-best候補生成

2026-07-20、同じApple M3、arm64、Releaseで各2,000反復を3回実行した中央値。候補生成は入力位置ごとに最大80状態を保持し、ライブ変換の1-best経路とは分離している。

| scenario | 中央値 |
| --- | ---: |
| 単語候補 `にほん`（完全一致 + N-best） | 0.152 ms |
| 1-best `わたしはにほん` | 0.034 ms |
| N-best `わたしはにほん` | 1.408 ms |

N-bestはSpaceで候補UIを開くときだけ実行する。1.408 msは候補初回表示の20 ms予算内で、入力中のLive Conversionにはこの探索コストを加えない。

3つの分野別辞書を合計344語へ拡充した後、すべて有効にした通常変換をRelease、5,000反復、10 sampleで比較した。

| scenario | p50 | p95 |
| --- | ---: | ---: |
| 基本辞書のみ `nihon`変換 | 3,019 ns | 3,057 ns |
| 基本辞書 + 3分野辞書 | 3,328 ns | 3,358 ns |

レイヤー横断により約10.2%増えたが、絶対差は約0.00031 msである。辞書を無効にした場合は検索しないため、不要な分野辞書を設定から外せる構成を維持する。

### 入力履歴500件の補完コスト

2026-07-20、同じApple M3、arm64、Releaseで、履歴を上限の500件まで読み込んだ長寿命engineを使い、`pafu`の入力から確定までを各1,000反復、5 sampleで計測した。fixture生成とファイル読み込みは計測外である。

| scenario | p50 | p95 | sample |
| --- | ---: | ---: | ---: |
| 履歴候補OFF | 1,341 ns | 1,356 ns | 5 |
| 履歴候補ON（セッション文脈対応後） | 8,241 ns | 8,297 ns | 5 |

500件を線形走査し、永続履歴とセッション文脈を重複排除して結合する現在の補完は約0.0069 msの固定費を加えるが、1キー5 msの予算より十分小さい。件数上限を増やす場合は、読みprefix indexを導入する前に同じfixtureで再計測する。

### セッション文脈128件の最悪側lookup

2026-07-20、同じApple M3、arm64、Releaseで、3分野辞書を有効にし、文脈なしと上限128遷移の末尾にある一致を各1,000反復、5 sampleで比較した。入力、Space変換、composition消去までを含み、文脈の構築は計測外である。

| scenario | p50 | p95 | sample |
| --- | ---: | ---: | ---: |
| 文脈なし | 3,223 ns | 3,262 ns | 5 |
| 128遷移の最古hit | 21,147 ns | 21,374 ns | 5 |

上限まで埋まった線形lookupの差は約0.018 msで、0.1 msの個別予算と1キーp95 5 msの双方に収まる。関係はLRUで128件に固定し、永続履歴500件の上限とは独立させる。

### Swift → C ABI → Rust → Swiftのtail latency

2026-07-20、同じApple M3、arm64、Releaseで、fixture生成と入力prefixを計測外に置き、1操作ごとの分布を採取した。

| scenario | p50 | p95 | sample |
| --- | ---: | ---: | ---: |
| 通常の1キー入力 | 0.0053 ms | 0.0057 ms | 1,000 |
| `nihon`のSpace候補表示 | 0.0112 ms | 0.0113 ms | 1,000 |
| Live Conversionの50文字目 | 0.2775 ms | 0.2800 ms | 500 |
| 履歴500件で補完を更新（セッション文脈対応後） | 0.0193 ms | 0.0199 ms | 1,000 |

全ケースが1キーp95 5 ms、候補初回表示p95 20 msの予算内にある。これはIMEアダプター内の処理時間であり、対象アプリのmarked text描画を含むend-to-end値ではない。

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
SLIME_BENCH_ITERATIONS=100 SLIME_BENCH_LIVE_LENGTHS=10 cargo bench -p slime-core --bench engine
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
