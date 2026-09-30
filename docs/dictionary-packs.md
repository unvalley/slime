# 追加辞書パック

Slime本体と同梱辞書はMITで公開する。一方、外部辞書パックは独立した
著作物・製品として配布できるよう、ソース、アプリbundle、Git履歴から
分離する。

## 配置

macOS版は次のディレクトリにある拡張子`.slime-dict`の通常ファイルだけを
起動時とユーザーデータ再読み込み時に走査する。

```text
~/Library/Application Support/Slime/dictionary-packs/
```

シンボリックリンク、32 MiBを超えるファイル、UTF-8でないファイル、
壊れたパックは読み込まない。1つの不正なパックがあってもIME本体と
ほかの正常なパックは利用でき、設定画面に読み込みエラーを表示する。

## 形式

v1は既存のローカルパックとの互換用に読み込みを継続する。語彙だけの
販売・更新対象はv2、既存候補を左文脈で並べ替えるパックはv3を使う。

```text
# slime-dictionary-pack-v1
# id: sample-general
# name: 一般語彙サンプル
# version: 2026.07.1
# license: Example-Test-Only
てすとようご<TAB>試験用語
こまわり<TAB>専門小回り<TAB>6000
```

`id`は小文字ASCIIの安定した識別子とし、同じディレクトリ内で重複させない。
`name`は設定画面の表示名、`version`は販売・更新単位、`license`はパックへ
適用するライセンスの短い識別子である。entryは`読み・表記・任意の単語cost`
の3列以内とする。

v2は互換性・出典・内容整合性を必須にする。

```text
# slime-dictionary-pack-v2
# id: sample-general
# name: 一般語彙サンプル
# version: 2026.08.1
# license: Example-Test-Only
# minimum-slime-version: 0.1.0
# published-at: 2026-08-01
# provenance: fixture/generated/sample-general
# entries-sha256: <# entriesの次のbyteからEOFまでのSHA-256>
# entries
てすとようご<TAB>試験用語
こまわり<TAB>専門小回り<TAB>6000
```

`minimum-slime-version`は`MAJOR.MINOR.PATCH`で、実行中Slimeより新しいversionを要求するパックは拒否する。`published-at`は`YYYY-MM-DD`、`provenance`は提供元・生成元を追跡できる安定した識別子とする。`entries-sha256`は`# entries`直後からEOFまでのbyte列を検証し、転送不良や意図しない書換えを検出する。改行と末尾改行もdigest対象なので、生成後にentry領域を書き換えない。

v3は語彙領域の後に、任意の左文脈ルールを持てる。ルールを含むpayload全体を
検証するため、`entries-sha256`の代わりに`payload-sha256`を必須にする。

```text
# slime-dictionary-pack-v3
# id: sample-context
# name: 文脈サンプル
# version: 2026.08.1
# license: Example-Test-Only
# minimum-slime-version: 0.1.0
# published-at: 2026-08-08
# provenance: fixture/generated/sample-context
# payload-sha256: <# entriesの次のbyteからEOFまでのSHA-256>
# entries
てすとようご<TAB>試験用語
# context-rules
文章<TAB>かんじ<TAB>漢字<TAB>0
```

文脈ルールは`直前に確定した表記・現在の読み・優先する表記・priority`の4列で、
保持している直前確定表記の末尾と一致した場合に適用する。最大128文字の末尾だけを
対象とし、長い一致を短い一致より先に、同じ一致長ではpriorityが小さいものを先にする。
1回の変換で実際に前へ移動する候補は最大8件とする。ルールは通常の辞書探索ですでに
生成された候補だけを前へ移動し、辞書にない表記を作らない。ユーザー辞書と端末内履歴を
静的ルールより優先し、プライベートモードでは適用しない。直前表記はプロセス内の一時状態
だけで、履歴学習が無効でも利用できるが、文書本文やルール一致を保存・出力しない。

ローダーは次を検証する。

- 読みはひらがなと長音だけ
- 表記は制御文字を含まず128文字以内
- costは100から12,000
- 1パック250,000entry以内
- 同一パック内で読みと表記の組が重複しない
- v3は1パック100,000文脈ルール以内
- 文脈ルールの読みはひらがなと長音だけ、表記は制御文字を含まず128文字以内

Git管理外のローカル候補パックは、次のコマンドで検証・配置できる。

```console
just validate-local-dictionary-packs
just install-local-dictionary-packs
```

## 決定的な生成

非公開データを手作業でpackへ貼り付けず、公開側のbuilderでv2/v3へ正規化する。
語彙入力は`読み<TAB>表記<TAB>任意のcost`、文脈入力は
`直前表記<TAB>読み<TAB>優先表記<TAB>priority`のTSVとする。空行と`#`で始まる
コメント行は無視する。文脈入力を省略するとv2、指定するとv3を生成する。v2は
語彙入力が必須である。v3は文脈入力だけでも生成できるため、文脈データを無関係な
dummy語彙と結合せず独立した署名・version・rollback単位にできる。

```console
cargo run -q -p slime-tools --bin slime-dictionary-pack -- build \
  --id sample-context \
  --name 文脈サンプル \
  --version 2026.08.1 \
  --license Example-Test-Only \
  --minimum-slime-version 0.1.0 \
  --published-at 2026-08-08 \
  --provenance fixture/generated/sample-context \
  --entries /secure/input/entries.tsv \
  --context-rules /secure/input/context.tsv \
  --output /secure/output/sample-context.slime-dict \
  --json
```

文脈だけを持つpackでは、同じコマンドから`--entries`を省略する。

固有語・複合語のprivate entryは、信頼済みの注釈corpusを
`just generate-term-dictionary TRAIN OUTPUT`へ渡して生成できる。生成器は現在の初回候補、
短語の拡張、bounded複合、長語の固定文節候補に既にある表記を除き、追加後も通常候補へ
入らないentryを落とす。派生TSVには実語彙が含まれるため、入力corpusと同じprivate
workspaceでbuilderへ直結し、集計とSHA-256以外をrelease evidenceへ保存しない。

生成、複数splitのrecall評価、語彙を揃えたbaselineとの文脈評価、pack生成を一つの
fail-closed工程にする場合は、`just prepare-private-dictionary-candidate --help`を使う。
この工程はtrainingと除外split、改善対象のrecall/context fixture、ライセンス台帳を必須に
し、回帰0、top-1回帰0、候補latency、pack bytes、別process起動中央値・p95、最大RSSを
すべて通った場合だけ新規output directoryを作る。
既存のprivate辞書TSVを`--dictionary`で候補判定へ使う場合は、同じ語彙を収録したpackを
`--baseline-pack`で最低1件渡す。pipelineはそのpackをbaselineとcandidateの双方へ置き、
新しい文脈ルールの差分だけを評価する。
既存outputは上書きせず、成功directoryには`READY`と`UNSIGNED`を置く。生成TSVとpackには
非公開語彙が含まれるためdirectory全体をprivateに保ち、`UNSIGNED`がある状態では配布しない。
署名後の成果物は後述の`verify-signed`へ渡し、そこで初めてrelease候補とする。

公開Anthy形式corpusから、繰り返し確認できる複合語の誤順位だけを直すTSVは次で生成する。
同一行内の重複は1回と数えず、同じ読みの同率表記、漢字・カタカナ以外を含む表記、既存1位、
指定costで1位へ上がらない表記を除く。生成物を製品へ入れる前に`slime-evaluate --dictionary`で
学習外datasetとの全項目top-1差分を確認する。

```console
cargo run -q -p slime-tools --bin slime-phrase-dictionary -- \
  --input public-train-1.txt --input public-train-2.txt \
  --output compound-promotions.tsv --min-count 2 --word-cost 7000 --json
```

別process起動gateはmacOSとLinuxのOS resource reportを使い、baselineとcandidateを
別々に既定5回ずつ測る。各processが同じpack件数、entry件数、文脈ルール件数、bytesを
読んだことも照合し、candidateの絶対上限に加えて中央値、p95、最大RSSのbaseline差も
固定する。これはOS cacheを破棄した測定でも
署名sidecar検証込みの測定でもない。署名済み実アプリの初回起動はrelease時に別途測る。

builderはentryを`読み・表記・cost`、文脈ルールを
`読み・直前表記・priority・優先表記`でsortし、省略costを500へ正規化する。
入力順、コメント、LF/CRLFが異なっても意味が同じなら同じbyte列とdigestになる。
生成物を公開する前に、builder自身が共通loaderで全metadata、件数上限、重複、
digestを再検証する。

入力はregular UTF-8 fileかつ各32 MiB以下、1行4,096 bytes以下に制限する。
出力は`.slime-dict`だけを許可し、既存pathを上書きせず、同じdirectory内の一時fileを
private permissionで作成・同期してからno-clobberで公開する。標準出力はformat、件数、
bytes、digestだけで、
語彙、文脈ルール、入力pathを出さない。失敗時も入力行やpathをerrorへ含めない。
ただし生成pack自体には語彙が含まれるため、入力と出力はいずれもアクセス制御された
workspaceに置き、公開CIのartifactやcacheへ保存しない。

署名後は、商用CIが同じ公開コアを使う最終gateを実行する。`DATA_DIR`直下の
`dictionary-packs/`へ検証対象のpackとsidecarだけを置き、公開鍵とversion floorは
アクセス制御された一時fileから渡す。

```console
just verify-signed-dictionary-packs \
  /secure/release-data \
  /secure/policy/verification-keys.tsv \
  /secure/policy/version-floors.tsv \
  2 \
  --json
```

公開鍵fileは`key-id<TAB>64文字のlowercase hex`、floor fileは
`pack-id<TAB>MAJOR.MINOR.PATCH`を1行1件とする。`expected`は今回配布するpackの正確な
個数で、1から64の範囲を必須とする。空directory、一部欠落、余分なpack、無署名、未知鍵、
改ざん、allowlist外ID、version下限未満、65pack以上は失敗する。

成功出力はpack数、entry総数、文脈ルール総数、署名検証した元pack全体のSHA-256だけで、
語彙、文脈ルール、pack ID、version、入力pathを含まない。失敗時も拒否件数だけを返し、
loaderの個別file名やmetadataを公開CI logへ展開しない。packのSHA-256はmetadataと改行を
含む署名対象byte全体から計算する。

最終verifier通過後は、同じ公開鍵とversion floorを使うsigned-only loaderで別process起動も
測る。baselineとcandidateでpack数が異なる場合はそれぞれ明示し、一部packの読込失敗を
速度改善として扱わない。

```console
just evaluate-pack-startup /secure/release-candidate \
  --baseline-data-dir /secure/release-baseline \
  --verification-keys /secure/policy/verification-keys.tsv \
  --version-floors /secure/policy/version-floors.tsv \
  --baseline-expected-packs 1 \
  --expected-packs 2 \
  --iterations 5 \
  --max-median-ms BUDGET \
  --max-p95-ms BUDGET \
  --max-rss-bytes BUDGET \
  --max-median-delta-ms BUDGET \
  --max-p95-delta-ms BUDGET \
  --max-rss-delta-bytes BUDGET \
  --json
```

起動評価器と`verify-signed`は公開鍵・version floorの同じparserを使う。成功JSONは件数、
bytes、起動時間、RSSだけを含み、pack ID、version、語彙、入力pathを出力しない。

## ライセンスと販売境界

外部パックはSlimeのMITライセンスの対象外であり、各パックのライセンスと
販売条件に従う。公開リポジトリには、パックローダー、形式仕様、テスト用の
架空データだけを置く。販売語彙、選定根拠、コスト調整データ、署名用秘密鍵は
アクセス制御された別リポジトリで管理する。

v2/v3のSHA-256は内容整合性を検出するが、攻撃者が本文とdigestを同時に書き換えることは
防げず、販売者の真正性を証明しない。商用buildは公開コアの署名必須loaderへ信頼済み
Ed25519公開鍵を注入し、パックと同じ場所の`.slime-dict.sig` sidecarを検証する。

```text
# slime-dictionary-signature-v1
# key-id: release-2026-a
# signature-ed25519: <128文字のlowercase hex>
```

署名対象は`.slime-dict`の元byte全体で、改行、metadata、payload digestも含む。sidecarは
1,024 bytes以下のregular fileに限り、key idは64 bytes以下のlowercase英数字と`-`だけを
許可する。複数の公開鍵を渡せるため、旧鍵と新鍵を同時に信頼するrotation期間を作れ、
次回load時に旧鍵を外すことで失効できる。検証はEd25519 strict verificationを使う。

通常の`bundled_with_user_data`と`slime_create_with_data_dir`は、利用者が作る無署名パック
との互換性を維持する。公式商用buildは必ず
`bundled_with_user_data_and_pack_trust`またはC ABIの
`slime_create_with_signed_data_dir`を使い、無署名・未知鍵・改ざん済みパックを読み込まない。
C ABIへ渡す公開鍵は`key-id<TAB>64文字のlowercase hex`を1行1鍵とする。

秘密鍵、実際のkey id、署名処理、購入権利は公開repositoryやアプリbundleへ置かない。
直接販売のentitlementを確認した商用側の配布処理だけが、署名済みpackとsidecarを配置する。
署名だけでは古い正規packの再配置を防げないため、公開コアはpack IDごとの最低versionも
署名policyへ追加できる。rollback対象の公式packはversionを`MAJOR.MINOR.PATCH`に固定し、
署名検証後にmetadataのversionが下限未満なら読み込まない。C ABIは
`slime_create_with_signed_data_dir_and_version_floors`へ
`pack-id<TAB>MAJOR.MINOR.PATCH`を1行1packで渡す。このconstructorでは一覧にないpack IDも
拒否する。公式packを追加するときは、IDと初期下限を
含むapp policyを先に配布してからpackを公開する。一般利用者向けの既存constructorとpack
metadataの互換性は変わらない。

公式macOS release buildは`SLIME_DICTIONARY_PACK_VERSION_FLOORS`を必須にし、検証済みの値を
`SlimeDictionaryPackVersionFloors`として署名前のInfo.plistへ入れる。実際のpack ID、現在の
下限、引き上げ時期は商用release metadataを正とする。下限を下げたbuildを配布しないこと、
新しい下限を持つappが利用者へ到達してから古いpackを配布対象外にすることは商用CIの責任である。

非公開の語彙・文脈ルールは別リポジトリで生成し、公開treeへコピーしない。商用CIは
複数domainの固定splitで、回収数、top-1回帰、主要held-out、p95、起動時間、RSS、
pack bytesを集計値だけで判定する。あるdomainで改善しても、別の主要held-outで
top-1またはMRRが悪化したパックは採用しない。入力形式と機械判定条件は
[文脈パック評価](context-pack-evaluation.md)に固定する。

商用pipelineは、private TSVを一時workspaceへ展開し、上記builderでpackを生成し、
held-out gateを通したbyte列を秘密鍵管理下の別工程でそのまま署名する。署名後にpackを
再生成・改行変換・metadata編集してはいけない。署名済みpackとsidecarを署名必須loaderで
`verify-signed`からversion floorと期待pack数を含めて最終検証し、pack全体のSHA-256と
集計評価だけをrelease evidenceへ残す。private TSV、
生成pack、sidecar、詳細評価は公開repository、公開CI log、共有cacheへ残さない。
