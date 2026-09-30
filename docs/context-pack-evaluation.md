# 文脈パック評価

非公開の語彙・左文脈ルールを公開リポジトリへコピーせず、v3辞書パックを
基本辞書と比較する。評価器は入力例、候補、誤答を出力せず、集計値だけを返す。

## private corpusからの生成

公開側の`slime-context-rules`は、空白区切りの`表記/読み`列を持つ注釈済みcorpusから
`直前表記<TAB>読み<TAB>優先表記<TAB>0`を決定的に生成する。実corpus、生成TSV、
追加辞書は有償側のアクセス制御された一時workspaceだけに置く。

```console
just generate-context-rules /secure/train.annotated /secure/context-rules.tsv \
  --exclude-input /secure/held-out-source.annotated \
  --dictionary /secure/entries.tsv \
  --min-count 3 \
  --min-margin 2 \
  --min-share-bps 7500 \
  --max-rules 10000 \
  --json
```

同じ注釈行は入力順やひらがな・カタカナの読み表記にかかわらずhashで重複除外する。
`--exclude-input`に指定した注釈行も学習前に除外する。split自体はcorpusの取得・注釈を
行う商用側で先に固定し、調整後にheld-outをtrainingへ戻さない。

候補昇格ルールになるのは、同じ`直前表記・読み`で次のすべてを満たす表記だけである。

- 既定で3回以上観測され、次点との差が2回以上、全観測の75%以上を占める。
- 基本辞書と`--dictionary`で重ねた語彙の通常候補10件に存在する。
- 優勢表記をひらがなへ戻すだけのruleではない。読みと同じかな表記の優先は、
  静的corpus ruleではなくユーザー自身の履歴へ委ねる。
- 現在の先頭候補ではない。先頭を変えないruleや生成不能な表記は出力しない。
- 1つの`直前表記・読み`につき優勢な表記を1件だけ選ぶ。同数首位は除外する。

上限を超えた場合は観測数、次点との差、占有率の順に強いruleを残し、出力自体は
`読み・直前表記・表記`でsortする。標準出力とJSONは行数、除外数、各棄却理由、
rule数、bytes、出力SHA-256だけを含み、語彙、入力内容、pathを含めない。生成TSVは
既存fileを上書きせず、Unixではprivate permissionで作る。

## 入力

商用側で次のTSVを固定splitごとに作る。

```text
直前に確定した表記<TAB>現在の読み<TAB>期待表記
```

- 読みはひらがなと長音だけにする。
- 表記は空でなく、制御文字を含まず128文字以内とする。
- 直前表記は実際の確定単位でよい。文脈ルールはその最大128文字の末尾に対して、
  長い一致から順に適用される。
- 同じ`直前表記・読み`は1つのsplit内で重複させない。
- 調整用dev、同一domain test、別domain held-outを別ファイルにする。
- 入力TSV、pack、詳細な候補列は公開treeやCI artifactへ保存しない。

`--data-dir`には、`dictionary-packs/`以下に評価対象の`.slime-dict`だけを置いた
隔離ディレクトリを渡す。ユーザー辞書や履歴を混ぜない。

公開評価データを使う場合は、同じ評価器へAJIMEE形式のJSON配列を直接渡せる。`context_text`、
カタカナ`input`、複数の`expected_output`を読み、入力だけをひらがなへ正規化する。詳細な
読みや候補を出力しない性質はTSV入力と同じである。

```console
just evaluate-context-pack /evaluation/candidate /evaluation/items.json \
  --baseline-data-dir /evaluation/baseline \
  --format ajimee --json
```

```console
just evaluate-context-pack /private/run/data /private/run/held-out.tsv \
  --baseline-data-dir /private/run/baseline-data \
  --min-context-rules 1 \
  --min-added-context-rules 1 \
  --min-top1-improved 1 \
  --max-top1-regressed 0 \
  --max-topk-regressed 0 \
  --min-accuracy-delta 0 \
  --min-mrr-delta 0 \
  --max-p95-ms 20 \
  --max-pack-load-ms BUDGET \
  --max-pack-bytes BUDGET \
  --json
```

語彙パックと文脈パックを同じreleaseで評価する場合、`--baseline-data-dir`には語彙パック
だけ、`--data-dir`には同じ語彙パックと文脈パックを置く。これにより、語彙追加で初めて
候補へ入った効果を文脈順位の改善として数えない。JSONはbaseline側とcandidate側の
pack数、entry数、文脈ルール数、bytesを別々に返す。指定を省略した場合だけ、従来どおり
同梱辞書をbaselineにする。

`--min-added-context-rules`はcandidateの文脈ルール数からbaselineの文脈ルール数を引いた
差に適用する。既存packに文脈ルールがあるだけでは、新しい文脈packのgateを通過できない。

## 出力と判定

出力するのは件数、pack数、entry数、文脈ルール数、pack bytes、baselineとpackの
accuracy@1、accuracy@K、MRR@K、top-1改善・回帰・変化件数、top-K回収・回帰、
候補生成latency、engine構築時間だけである。読み、表記、ファイルパス、候補、失敗例は
JSONにも含めない。入力形式エラーも行番号と種別だけを返し、行内容を引用しない。

採用条件は次のすべてを満たすこととする。

- 調整に使っていない複数domainでaccuracy@1とMRR@Kが非悪化。
- 主要held-outでtop-1回帰とtop-K回帰が0。
- 改善対象splitではtop-1改善が1件以上。
- 候補生成p95が既存の20 ms予算内。
- pack bytesとengine構築時間がreleaseごとに凍結した予算内。
- corpus、派生ルール、packの商用利用・再配布条件が台帳化済み。

engine構築時間は同一process内の参考値である。private候補pipelineは追加で
`slime-pack-startup-evaluate`を使い、語彙だけのbaselineと語彙・文脈を持つcandidateを
それぞれ独立processで既定5回起動する。実行ファイル起動からpack読込までの中央値、p95、
最大RSSの絶対値とbaseline差を固定する。ただしOS filesystem cacheを強制破棄せず、
unsigned候補を通常loaderで読む値なので、署名検証込みの実アプリ初回起動を証明しない。
商用releaseでは同じ評価器へ公開鍵、version floor、期待pack数を渡してsigned-only loaderも
測り、その後に署名済み実アプリも別に測る。
一つのdomainで大きく改善しても、別の主要held-outを悪化させたpackは採用しない。

## 実行境界

評価は明示された直前表記を一時的に候補問い合わせへ渡すだけで、セッション履歴、
ユーザー履歴、入力TSVを書き換えない。文脈ルールは辞書探索ですでに存在する候補だけを
最大8件前へ移動するため、評価時だけ架空の表記を生成することもない。複数のルールが
一致した場合は、長い末尾一致を短い一致より優先し、同じ末尾ではpriority順に評価する。
長い読みではN-best候補内の隣接文節も評価する。ただし基準候補と同じ読み境界を持ち、
その境界の表記が異なる候補だけを昇格対象にする。異なる分割を規則の根拠として扱わず、
内部文節規則はSpace後の明示変換にだけ適用して、逐次LIVEの書き換え増加を避ける。
