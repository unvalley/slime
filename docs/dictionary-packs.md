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

v1は既存のローカルパックとの互換用に読み込みを継続する。販売・更新対象は
出典とdigestを持つv2以降を使う。

```text
# slime-dictionary-pack-v1
# id: sample-pro
# name: サンプル Pro
# version: 2026.07.1
# license: Proprietary
すらいむぷろ<TAB>Slime Pro
こまわり<TAB>専門小回り<TAB>6000
```

`id`は小文字ASCIIの安定した識別子とし、同じディレクトリ内で重複させない。
`name`は設定画面の表示名、`version`は販売・更新単位、`license`はパックへ
適用するライセンスの短い識別子である。entryは`読み・表記・任意の単語cost`
の3列以内とする。

v2は互換性・出典・内容整合性を必須にする。

```text
# slime-dictionary-pack-v2
# id: sample-pro
# name: サンプル Pro
# version: 2026.08.1
# license: Proprietary
# minimum-slime-version: 0.1.0
# published-at: 2026-08-01
# provenance: unvalley/context-packs/sample-pro
# entries-sha256: <# entriesの次のbyteからEOFまでのSHA-256>
# entries
すらいむぷろ<TAB>Slime Pro
こまわり<TAB>専門小回り<TAB>6000
```

`minimum-slime-version`は`MAJOR.MINOR.PATCH`で、実行中Slimeより新しいversionを要求するパックは拒否する。`published-at`は`YYYY-MM-DD`、`provenance`は提供元・生成元を追跡できる安定した識別子とする。`entries-sha256`は`# entries`直後からEOFまでのbyte列を検証し、転送不良や意図しない書換えを検出する。改行と末尾改行もdigest対象なので、生成後にentry領域を書き換えない。

v3は`# context-rules`を、v4は候補の利用範囲を追加する。大規模な一般語彙や
固有名詞を既存変換へ直接混ぜたくない場合は、v4の
`model-rescore-only`を使う。

```text
# slime-dictionary-pack-v4
# id: supplemental-general
# name: 補助一般語彙
# version: 2026.08.1
# license: Apache-2.0
# minimum-slime-version: 0.1.0
# published-at: 2026-08-11
# provenance: example/generated/supplemental-general
# candidate-mode: model-rescore-only
# payload-sha256: <# entriesの次のbyteからEOFまでのSHA-256>
# entries
しんたく<TAB>神託<TAB>500
```

このモードのentryは通常変換、ライブ変換、モデル未準備時の候補へ一切参加しない。
ローカル高精度モデルが利用可能になった後だけ、現在のbase winnerを先頭に保持した
再評価リクエストへ追加する。基本辞書と読み・表記が同一のentryは除外し、補助辞書
由来の候補を昇格するには通常候補より1.5高いscore marginを要求する。モデルの失敗や
不正なscoreでは画面上の候補を変更しない。
文脈ruleは通常候補の表記を昇格する機能なので、`model-rescore-only`パックとは
併用できない。

このモードはモデル未使用時の変換を隔離する機能であり、モデルが成功した場合の
精度を保証するものではない。補助語彙を増やすと候補集合とモデルの相対scoreが変わる
ため、一般辞書を一括投入せず、用途別に選定したパックごとにheld-outの改善数・回帰数、
top-1変更数、p95 latencyを比較し、回帰が改善を上回るパックは配布しない。

パック生成ツールでは`--model-rescore-only`を指定するとv4を生成する。

```console
cargo run -p slime-tools --bin slime-dictionary-pack -- build \
  --id supplemental-general --name 補助一般語彙 --version 2026.08.1 \
  --license Apache-2.0 --minimum-slime-version 0.1.0 \
  --published-at 2026-08-11 --provenance example/generated/supplemental-general \
  --entries entries.tsv --model-rescore-only --output supplemental-general.slime-dict
```

ローダーは次を検証する。

- 読みはひらがなと長音だけ
- 表記は制御文字を含まず128文字以内
- costは100から12,000
- 1パック250,000entry以内
- 同一パック内で読みと表記の組が重複しない

Git管理外のローカル候補パックは、次のコマンドで検証・配置できる。

```console
just validate-local-dictionary-packs
just install-local-dictionary-packs
```

## ライセンスと販売境界

外部パックはSlimeのMITライセンスの対象外であり、各パックのライセンスと
販売条件に従う。公開リポジトリには、パックローダー、形式仕様、テスト用の
架空データだけを置く。販売語彙、選定根拠、コスト調整データ、署名用秘密鍵は
アクセス制御された別リポジトリで管理する。

v2のSHA-256は内容整合性を検出するが、攻撃者が本文とdigestを同時に書き換えることは防げず、販売者の真正性を証明しない。改ざん防止署名と購入権利の検証はまだ含まない。本番販売時は、StoreKitまたは直接販売のentitlementを確認したインストーラーだけがパックを配置し、署名検証に成功したパックだけを読み込む段階を追加する。秘密鍵は公開repositoryやアプリbundleへ置かない。
