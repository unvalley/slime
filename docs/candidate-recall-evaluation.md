# Candidate recall evaluation

候補品質を直す前に、期待表記がどの段階で失われているかを分けて測る。`slime-recall`は外部TSVを読み、各項目を次のいずれかへ分類する。

| stage | 意味 |
| --- | --- |
| `initial` | 通常の初回候補集合にある |
| `expanded` | 初回にはないが、8文字以下で候補末尾へ進んだときのN-best 32にある |
| `compound` | N-best 32にはないが、1かな読みや辞書上かな表記だけの語を含めた完全一致の2〜6要素を組み合わせる補助探索にある |
| `fixed_segment` | 長い読みの最良分割を維持し、各文節内の候補だけを組み替える追加22件にある |
| `recombined` | 長い読みの上位10経路で実際に観測した辞書segmentを、POS接続costを再計算して再結合した追加32件にある |
| `known_components` | 期待表記を読み128文字・64要素までの完全一致辞書表記へ分割できるが、bounded候補にはない。製品生成では抑えるかな表記も診断では既知要素として数え、語彙不足と探索不足を分離する |
| `missing` | 完全一致要素への分割もできず、少なくとも一部の語彙または新しい一般化生成規則が必要 |

集計の`missing`は後方互換のため、`known_components + unknown_components`、つまり現在の製品候補で回収できない総数を表す。`--max-missing`は両方を数え、分類追加によってrelease gateを緩めない。

## 入力

期待語TSVは1行に2列を置く。

```text
にほん	日本
あさいり	浅煎り
```

空行と`#`から始まる行は無視する。カタカナの読みはひらがなへ正規化し、同じ読み・表記の重複は入力ミスとして拒否する。

非公開辞書を一時的に重ねる場合は、1行に読み、表記、任意のコストを置く。コストを省略した場合は5000を使う。

```text
みとうろくご	未登録語	700
```

信頼できる語境界と読みを持つ注釈済み文から、複合語だけの評価TSVを作る場合は`slime-recall-corpus`を使う。入力は空白区切りの`表記/読み`列とし、2〜6要素、読み4〜16文字、各要素が2文字以上の漢字・カタカナだけで構成され、うち2要素以上に漢字がある連続だけを抽出する。重複を除き、上限を超える場合は入力全体から決定的に均等抽出する。

```sh
cargo run --release -p slime-tools --bin slime-recall-corpus -- \
  --input /absolute/path/to/annotated.txt \
  --output /absolute/path/to/compound-recall.tsv \
  --limit 2000
```

この変換は正しい読みを作るものではない。読みなし原文を辞書で自動注釈した列は、複合生成の診断には使えるが、地名・人名の読みや語境界を誤るためrelease gateへ使わない。製品gateには原典で確認済みの読み、実入力から同意を得て匿名化したfixture、または商用側で管理する正解データだけを使う。

## private辞書候補の生成

信頼できる読みと語境界を持つ非公開の注釈corpusから、現在のbounded探索で生成できない
固有語・複合語だけを辞書TSVへ抽出できる。

```console
just generate-term-dictionary /secure/train.annotated /secure/entries.tsv \
  --exclude-input /secure/held-out-source.annotated \
  --dictionary /secure/already-provided.tsv \
  --min-count 3 \
  --max-surfaces-per-reading 4 \
  --max-entries 10000 \
  --word-cost 5000 \
  --json
```

単独語は2〜32文字の読みと、漢字・カタカナを含む2〜32文字の表記を対象にする。
複合語は2〜6要素、読み4〜32文字とし、各要素は2文字以上の漢字・カタカナ列に
限定する。活用断片や助詞を機械的に連結しない。同じ行で同じ語が複数経路から得られても
文書頻度は1回として数え、既定で3行以上に現れた語だけを候補にする。
カタカナだけの表記は、中黒を除いてひらがな化した結果が読みと一致する場合に限る。
形態素辞書の発音形から`いー/イイ`のような正書法違いを作ると、長いカタカナ語の途中に
短い誤経路を挿入してLIVE表示を巻き戻すためである。漢字を含む語の発音形はこの制限を受けない。

基本辞書と`--dictionary`を重ねた状態で、初回候補、8文字以下のN-best 32、bounded
複合探索、9文字以上の固定文節探索をすべて調べ、既に選択可能な表記は出力しない。
生成entryを一時的に重ねた後、通常候補10件へ実際に入らないentryも除外する。同じ読みの
追加は既定4表記までに制限し、観測数、要素数、読み長の順に強い候補を残す。

出力は`読み<TAB>表記<TAB>cost`で決定的にsortし、既存fileを上書きしない。標準出力と
JSONは件数、棄却理由、bytes、SHA-256だけを含み、語彙、corpus行、入力pathを含めない。
生成TSVはprivate辞書の入力物であり、公開tree、公開CI artifact、共有cacheへ置かない。
自動抽出結果をそのまま採用せず、次節の`evaluate-recall`で回収、回帰、top-1変化、p95、
bytesを固定splitごとに判定する。

## 実行

```sh
just evaluate-recall /absolute/path/to/expected.tsv
just evaluate-recall /absolute/path/to/expected.tsv --oracle-n-best
just evaluate-recall /absolute/path/to/expected.tsv \
  --dictionary /absolute/path/to/private-dictionary.tsv \
  --min-recovered 1 \
  --max-regressed 0 \
  --max-top1-regressed 0 \
  --max-top1-changed 0 \
  --max-p95-ms 20 \
  --max-dictionary-bytes 1048576 \
  --max-missing 0
```

`--dictionary`は複数指定できる。`--max-missing N`は生成欠落がN件を超えた場合に失敗し、`--json`は既定で機械可読の全結果を出力する。`--details N`を明示した場合は通常出力とJSONの項目数を制限する。非公開データをログへ出さず件数だけをCIで扱う場合は、`--details 0 --json`を指定する。この組み合わせでは`results`が空になり、読み・表記は出力されない。

`stage_rank`は期待表記が各生成段階の何番目に現れたかを1始まりで示す。
`fixed_segment_latency_ms`と`recombined_latency_ms`は、長文の追加探索を
通常の初回候補と分けて報告する。再結合は上位経路に現れた文字列を機械的に貼る処理ではなく、
各segmentを完全一致辞書entryへ戻し、読み範囲と左右POS接続が成立する完全経路だけを作る。
`--oracle-n-best`を指定した診断runでは、製品の文字数制限とは独立にN-best 32と64を
全入力へ適用し、`oracle_at_10/32/64`、項目ごとの順位、32/64候補生成のp50/p95/maxを
追加する。64探索は長文で数百msに達し得るため通常gateでは実行せず、候補幅の採否を測る
明示的なoffline診断だけに使う。

外部辞書を指定した場合は、同じ入力を基本辞書だけでも評価し、`baseline_missing`、`dictionary_recovered`、`dictionary_regressed`を追加で報告する。`dictionary_recovered`は基本辞書で`known_components`または`missing`だった項目が外部辞書込みでいずれかの生成段階へ入った件数、`dictionary_regressed`は逆に生成できなくなった件数である。`--min-recovered N`と`--max-regressed N`を使うと、語彙を出力せず増分だけをCI gateにできる。

同じ実行で、初回候補が期待表記と一致した件数、外部辞書によるtop-1改善・回帰、baselineから先頭表記自体が変化した件数、初回候補生成のp50/p95/max、外部辞書ファイルの合計bytesも報告する。`--max-top1-regressed N`、`--max-top1-changed N`、`--max-p95-ms N`、`--max-dictionary-bytes N`で上限を固定する。複数の許容表記を持つ別domainデータを2列TSVへ変換する場合は、正解数ではなく`dictionary_top1_changed = 0`を安全gateに使う。latencyは各入力を1回生成した分布で、辞書読込時間、OS候補UI、対象アプリの描画を含まない。release前には実アプリ計測を別に行う。

## 判断

- `initial`の順位だけが悪い場合はrerankingを評価する。
- `expanded`が多い場合は、常時N-bestを増やさず明示的な深い候補操作を改善する。
- `compound`が多い場合は、現在の要素数・文字数・状態数の上限内で補助探索を使う。
- `fixed_segment`が多い場合は、長文全体のN-bestを増やさず、候補末尾で最良分割の文節内候補を提示する。
- `recombined`が多い場合は、初回の上位経路は増やさず、候補末尾に達した明示操作でだけ観測済みsegmentの正規経路を追加する。
- `known_components`は要素幅や合成件数を無条件に広げず、oracle@32/@64と固定文節候補を比較し、信頼できる完全一致phrase entry、worker限定探索、または複数domainで非回帰を示した文脈モデルで解く。
- `missing`は未知要素を含むため、辞書、外部辞書レイヤー、または新しい一般化生成規則が必要で、既存候補のコスト変更では直らない。
- 低頻度の完全一致語を回収するために、全入力へ適用する要素候補数や合成候補数を無条件に増やさない。複数domainの回収数と操作時latencyが改善しない場合は、信頼できる小さな外部辞書へ完全一致語として置く。
- 外部辞書は`dictionary_recovered > 0`、`dictionary_regressed = 0`、`dictionary_top1_regressed = 0`を最低条件とする。候補生成p95とファイル総量も配布予算内に固定する。これは指定した期待TSV上の条件であり、別domainのtop-1、起動時間、実アプリ入力遅延は別の評価で非回帰を確認する。

かな表記だけの要素は、その部分読みに漢字・カタカナ等の変換表記が一つもない辞書項目に限って使う。最終表層が読みと同じ全かな経路は候補へ追加しない。

有償語彙、人物名、顧客データ、原典corpusは公開リポジトリへ置かない。公開側には評価器、形式、制限値、架空fixtureだけを残す。
