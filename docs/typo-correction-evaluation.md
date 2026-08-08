# Input correction evaluation

入力訂正は正解を回収するだけでなく、正しい入力や訂正不能な入力へ不要な候補を出さないことを同時に測る。`slime-typo-evaluate`は外部fixtureを読み、語彙を出力せず集計だけを返す。

## 入力

正例は1行にローマ字入力、訂正後の読み、期待表記、編集種別を置く。

```text
nihpn	にほん	日本	neighbor
```

編集種別は`deletion`、`duplicate`、`missing_consonant`、`missing_geminate`、`missing_vowel`、`neighbor`、`transposition`のいずれかとする。

負例はローマ字入力と、fixture管理用の理由を置く。理由は評価結果へ出力しない。

```text
nihon	exact dictionary reading
```

空行と`#`から始まる行は無視する。ローマ字入力はASCII英字だけを許可し、正例・負例を通じて同じ入力が重複した場合は拒否する。構文エラーはファイルと行番号だけを報告し、入力、読み、表記、理由をエラーメッセージへ含めない。

## 実行

```sh
just evaluate-typos /absolute/path/to/positive.tsv /absolute/path/to/negative.tsv \
  --max-missing 0 \
  --max-unnecessary 0 \
  --min-per-edit 2 \
  --max-p95-ms 20 \
  --max-corrections 3 \
  --json
```

出力するのは次の集計だけである。

- 正例総数、回収数、欠落数
- 負例総数、不要な訂正を出した件数
- 編集種別ごとの総数と回収数
- 1件ずつ新しいengineで入力開始からSpace候補表示まで測ったp50/p95/max
- 1入力あたりに表示した訂正候補の最大数

`--max-missing`と`--max-unnecessary`は品質回帰、`--min-per-edit`は特定の編集種別だけへ偏ったfixture、`--max-p95-ms`は訂正探索のtail、`--max-corrections`は候補欄の占有を止める。latencyには辞書を含むengine初期化と全文字入力を含み、OS候補UIの描画は含まない。release前には実アプリの入力から候補描画までを別に測る。

## 非公開データ

実入力由来のfixture、顧客語彙、入力頻度、生成途中データは公開リポジトリへ置かない。商用側のCIから公開側の評価器へ一時入力し、集計JSONだけを保存する。入力内容を調べる必要がある失敗は、アクセス制御された商用側で行番号を使って確認する。

実入力は明示的な同意、匿名化、保存期間、削除手順を先に定める。文章全体や周辺文脈は収集せず、必要最小限の誤入力、訂正読み、期待表記、編集種別だけをfixture化する。
