# 商用配布境界

Slimeの基本入力、変換、ユーザー辞書、入力履歴、プライベートモードは、このMITライセンスのリポジトリで提供する。契約やネットワークの状態をキーイベント処理の前提にせず、商用レイヤーの障害や契約終了によって文字入力を停止させない。

## 公開リポジトリに含めるもの

- OSに依存しない変換コアとC ABI
- macOS、Windowsの入力アダプター
- ユーザー辞書、履歴、外部辞書パックの汎用形式
- 公開可能な辞書データと再現可能な評価・検証コード
- 架空データだけを使う形式・境界テスト

## 商用レイヤーへ分離するもの

- 署名・公証済みの公式配布物と更新サービス
- 決済、契約、entitlementと、将来必要になった場合の端末activationの実装・設定
- 再配布権を個別に取得した有償辞書データと生成物
- 商用インフラのURL、商品ID、資格情報、運用手順
- 購入者向けサポートと配布ポータル

商用レイヤーは基本入力を許可・拒否しない。契約中に取得したアプリは契約終了後も入力に使え、新しい公式更新、有償辞書、サポートだけが利用できなくなる形を基本契約とする。

### 販売モデル

初回版は、IME本体の利用権ではなく、公式配布物、契約期間中の更新、有償辞書の新規取得、購入者向けサポートをまとめた**年額2,400円（税込）の自動更新契約**として販売する。月額契約と無料トライアルは設けない。MITで公開したソース、利用者自身が作成したbuild、基本入力機能を契約対象にせず、契約状態によって起動や文字入力を制限しない。

初回決済と各更新決済には、決済日から10日以内の全額返金を設ける。解約は次回の自動更新を停止するもので、契約中に取得したversionと辞書を削除または無効化しない。契約終了後に停止するのは、新しい公式配布物と有償辞書の取得、更新、購入者向けサポートだけとする。

購入画面と最終確認画面では、自動更新、年額、次回請求日、解約方法、返金条件、契約終了後も使える範囲を省略せず表示する。価格を変更する場合は商用側の規約と表示を先に更新し、この公開契約も同じreleaseで更新する。

### 初回版のentitlement境界

初回商用版では端末activationを実装しない。起動時、入力開始時、キーイベント処理中の契約確認やネットワーク通信も行わない。契約状態は商用側の配布ポータルで、新しい公式PKG、有償辞書、サポートを取得できるかだけに使う。

契約中に取得したversionと辞書は契約終了後も削除・無効化せず、基本入力、変換、ユーザー辞書、履歴を引き続き利用できるようにする。利用状況から端末数制限が必要になった場合も、将来の配布・更新取得側だけで再検討し、公開コアやキーイベント経路には追加しない。

## 販売表示gate

販売を有効にする商用deploymentは、購入前に認証なしで閲覧できる
`/legal`、`/terms`、`/privacy`、`/support`を同じ製品サイト上に設ける。
販売画面と最終確認画面から各ページへ直接移動でき、リンク切れや仮文言が一つでも
残る間はcheckoutを有効にしない。

- `/legal`には価格、支払方法と時期、提供時期、更新、解約、返金、販売者情報、
  ソフトウェアの動作環境を明示する。
- `/terms`には、契約終了後も取得済みversionと辞書を利用できること、終了するのは
  将来の更新、新規取得、サポートであること、利用許諾と責任範囲を明示する。
- `/privacy`では、IME本体が入力内容を外部送信しない境界と、Web、決済、問い合わせで
  扱う情報、委託先、保存期間、削除・問い合わせ方法を分けて説明する。
- `/support`には問い合わせ先、対応範囲、回答目安、install、update、uninstall、
  返金、security issueの窓口を明示する。
- 対応OS、CPU、必要容量などの動作環境は`/legal`だけに埋めず、購入操作の近くにも
  表示する。値は配布予定artifactを測定して確定し、推測値を書かない。

実際の販売者名、住所、電話番号、問い合わせ先、商品ID、決済・配布URL、規約本文は
商用側で管理する。公開リポジトリには上記の中立な契約と、販売準備中の公開Landingに
checkoutが混入しない回帰だけを置く。

## 配布gate

公開リポジトリには、署名前artifactの再現buildと、署名・公証・install lifecycleを検証する汎用scriptを置く。署名証明書、notary資格情報、署名サービス設定、更新配信先、実際の配布artifactは商用側で管理する。

- macOSはcleanなGit revisionからだけrelease buildを許可し、その40文字revisionをbundleへ埋め込む。Developer ID Application署名、hardened runtime、timestamp、Developer ID Installer署名、notary acceptance、staple、Gatekeeperをすべて必須にする。PKG内から展開したappとdylibを検査し、作業ディレクトリに残った別buildを配布物の証拠にしない。
- macOSの初回配布は手動更新できるversion付きPKGとする。使い捨てVMでclean install、同一version再実行の冪等性、旧版の完全置換、旧PKGによるdowngrade防止、uninstall、ユーザー辞書・学習履歴の保持を確認する。削除後の入力メニュー反映は、ログインし直したconsumer sessionで別途確認する。
- 初回商用版の対象OSはmacOSだけとする。Windowsは公開CIの未署名ライフサイクル試験と、商用側の署名済みVM gateを通した後のプレビューとして扱い、macOS初回版の出荷を止めない。Windows向けの購入導線と一般配布は、そのgateが完了するまで有効にしない。
- Windowsのrelease installerもcleanなGit revisionからだけ構築し、revisionをinstaller、Rust DLL、COM DLL、登録helper、設定実行ファイル、ARM64X forwarderのversion metadataとuninstall registryへ埋め込む。release assemblyとverifierは全PEの40文字revisionと3要素の製品versionがclean installer sourceに一致しなければ、署名やcaller指定hashが正しくても拒否する。対象architectureの全PEを先に署名し、署名済みuninstallerを内包してから外側のinstallerも署名する。source revision、コード署名・timestamp用途のEKU、同一signer、timestamp、version、配布前payloadとinstall後payloadのhash一致、clean install、旧版からのupdate、新版install後の旧installerによるdowngrade拒否、uninstall、native/WOW6432 COM登録、ユーザーデータ保持を使い捨てVMで検証する。
- Windowsの一般配布には、公開信頼されたRSA code-signing証明書とtimestampを使う。秘密鍵はexport可能なファイルとしてCIへ保存せず、hardware-backedな署名サービス内に保持する。証明書、署名サービス、短命な認証設定、署名要求の監査logは商用側で管理し、公開側は署名する対象と検証契約だけを保持する。証明書種別だけで初回downloadのreputation警告が消えるとはみなさず、警告の有無と表示publisherもclean consumer VMで記録する。
- Windows ARM64では、x64/ARM64実装をARM64XのCOM/C ABI forwarderから読み分けるartifactと、両architectureのload probe、ARM64X/x86専用installer、未署名native lifecycleを公開CIに置く。公開側のrelease verifierとconsumer lifecycle gateは全12 payload、埋め込みuninstaller、外側installerの署名・timestamp・同一signer・hash・登録を検査する。実証明書、署名サービス設定、署名済みartifactとconsumer VM結果は商用側に置き、すべて揃うまで販売対象にしない。
- Windowsの対話確認は[Windows consumer verification](windows-consumer-verification.md)に固定する。手動観察を自動試験として扱わず、download reputationと表示publisher、desktop入力、候補操作、検索統合、accessibility、実行中serviceへの設定反映を順に記録する。gateは各記録の前後でinstaller SHA-256、source revision、signer、配布前payloadとinstall後payload、installed manifestを再検証し、途中でartifactが変わったsessionを拒否する。記録はprepare時のuser SID、session ID、boot時刻にも固定し、別sessionへ持ち越さない。x64とARM64Xのlifecycle・対話出力は最終集約gateで照合し、各architecture内のartifact binding一致と、architecture間のversion・revision・signer一致を販売条件にする。
- unsigned/ad-hoc artifactの生成とソーステストは「開発用artifact-ready」であり、配布可能の証拠にしない。

macOSのconsumer sessionで行う手動確認は [macOS consumer verification](macos-consumer-verification.md) に固定する。

### macOS release build

商用側は製品versionをCargo workspaceに設定し、前回の公式配布より大きいbuild番号を明示してbundleとPKGを作る。build番号の省略や固定値へのfallbackはrelease buildでは許可しない。release buildはrepository rootで実行し、tracked、staged、untrackedの差分が一つでもあれば失敗する。

```sh
SLIME_CODESIGN_IDENTITY='Developer ID Application: …' \
SLIME_INSTALLER_IDENTITY='Developer ID Installer: …' \
SLIME_DICTIONARY_PACK_VERIFICATION_KEYS='release-key-id<TAB>64文字の公開鍵hex' \
SLIME_DICTIONARY_PACK_VERSION_FLOORS='pack-id<TAB>MAJOR.MINOR.PATCH' \
just build-macos-release-pkg BUILD_NUMBER

SLIME_NOTARY_PROFILE='KEYCHAIN_PROFILE' \
scripts/notarize-macos-pkg.sh target/package/Slime-VERSION.BUILD_NUMBER-macos.pkg

scripts/verify-macos-release.sh target/package/Slime-VERSION.BUILD_NUMBER-macos.pkg
```

`CFBundleShortVersionString`はworkspace version、`CFBundleVersion`は単調増加するbuild番号、`SlimeSourceRevision`はclean checkoutの`HEAD`とし、PKG receiptのversionは`VERSION.BUILD_NUMBER`にする。release gateはsource revision、appとdylibのDeveloper ID Application署名、同一Team ID、hardened runtime、危険なcode-signing entitlementの不在、Installer署名のTeam ID、PKG内の全payloadとlifecycle script、notary acceptance、staple、Gatekeeperを確認する。TextEditとprivate/secure-input gateは`SLIME_RELEASE_PACKAGE`を受け取り、PKGから展開したbundleと`/Library/Input Methods/Slime.app`の全ファイルmanifestを比較する。release verifier、VM lifecycle、物理入力gateはversion、build番号、source revision、PKG SHA-256、bundle manifest SHA-256をcanonical `Release binding JSON`で出力し、byte単位で一致することを要求する。資格情報とnotary profileは公開リポジトリへ保存しない。

商用buildは公開鍵一覧を`SlimeDictionaryPackVerificationKeys`としてInfo.plistへ入れてから
bundle全体を署名する。release buildは公開鍵が空、形式不正、重複、17鍵以上なら失敗し、
release verifierも展開したPKG内で同じ値を再検証する。公開鍵は秘密情報ではないが、実際の
key id、rotation期間、失効日は商用側のrelease metadataを正として注入する。

同時にpack IDごとの最低versionを`SlimeDictionaryPackVersionFloors`へ入れる。release buildは
下限が空、`pack-id<TAB>MAJOR.MINOR.PATCH`以外、ID重複、65件以上なら失敗し、release verifierも
PKG内の値を再検証する。公開コアは署名検証後に設定対象packのversionを比較し、下限未満の
正規署名packと一覧にないpack IDを拒否する。実pack ID、現在の下限、追加・引き上げ順序は
商用release metadataで管理し、下限を下げたapp buildをreleaseしない。新しいpack IDは、
そのIDの初期下限を含むapp policyが利用可能になってから配布する。

署名資格情報がない開発環境と公開CIでも、`just test-macos-package-policy`で署名に依存しないPKG構造を検証する。正常な未署名PKGに加えて、lifecycle script改変、想定外payload、symlink、bundleとPKGのversion不一致を拒否する回帰fixtureを毎回生成する。これはDeveloper ID署名、公証、Gatekeeper、使い捨てVMの代替ではなく、それらより前に構造上の不備を検出するgateである。

## 有償辞書

外部辞書パックは本体と別作品として扱い、pack metadataにライセンスとprovenanceを記録する。SHA-256は破損検出に使い、配布元の真正性は公開コアのEd25519署名必須loaderで検証する。loaderはパック全byteを対象にしたsidecar、複数公開鍵によるrotation、未知鍵・改ざん・無署名の拒否、pack IDごとの最低version、再読込後の同一policy維持まで実装する。実公開鍵とversion floorの商用buildへの注入、秘密鍵を使う署名処理、旧鍵の失効日、下限の単調な引き上げ、購入権は商用レイヤーで管理する。

有償語彙、原典corpus、生成途中データ、購入者情報はこのリポジトリへ追加しない。公開側にはloader、形式、制限値、検証器、架空fixtureだけを残す。

商用側では、アクセス制御された一時workspaceにprivate TSVを展開し、公開側の
`slime-dictionary-pack build`で決定的なv2/v3 packへ変換する。builderは入力順を
正規化し、既存出力を上書きせず、語彙・入力pathをlogやerrorへ出さず、共通loaderで
生成物を再検証する。次にheld-out評価を通し、採用したpackの元byteを秘密鍵管理下の
別工程で署名する。署名後の再生成・改行変換・metadata編集は禁止し、署名必須loaderで
pack、sidecar、version floor、期待pack数を`slime-dictionary-pack verify-signed`で最終検証する。
空集合、一部欠落、余分なpackも失敗させる。release evidenceへ保存するのはpack全体のSHA-256、件数、
bytes、集計評価だけとし、TSV、pack、sidecar、詳細結果は公開tree、公開CI artifact、
共有cacheへ出さない。具体的な入力形式とコマンドは[追加辞書パック](dictionary-packs.md)に固定する。

非公開語彙の候補recallは、商用側で`読み<TAB>期待表記`のTSVを生成し、公開側の`just evaluate-recall PATH`へ一時入力して測る。非公開辞書の効果も測る場合は`--dictionary PATH`を繰り返し指定し、`読み<TAB>表記<TAB>任意コスト`のレイヤーを評価プロセス中だけ重ねる。評価器は初回候補、短い読みの拡張候補、2〜6要素の補助探索、長文の固定文節候補、生成欠落を分け、基本辞書から外部辞書で回収した件数と逆に失った件数、top-1改善・回帰・先頭表記の変化、初回候補生成p95、辞書総bytesも報告する。入力TSV、辞書、結果を公開treeへコピーしない。商用CIでは`--details 0 --json --min-recovered N --max-regressed 0 --max-top1-regressed 0 --max-p95-ms N --max-dictionary-bytes N`を指定し、読み・表記を含まない集計だけを保存する。別domainでは`--max-top1-changed 0`も要求し、起動時間、実アプリ入力遅延を別に非回帰確認する。詳細は[Candidate recall evaluation](candidate-recall-evaluation.md)に固定する。

信頼済みの注釈corpusから追加候補を作る場合は、公開側の
`just generate-term-dictionary TRAIN OUTPUT`を使う。生成器は単独の固有語候補と2〜6要素の
複合語候補を数え、重複・除外行をhashで処理し、既存のbounded生成経路で届く表記と、
追加後も通常候補へ入らないentryを除く。corpus、除外split、派生TSVは商用側だけに置き、
集計とSHA-256だけを保存する。

学習前の左文脈順位は、公開側のv3辞書パック形式と既存候補だけを並べ替える
runtimeを使い、実ルールと学習corpusは商用側だけに置く。ユーザー辞書と端末内履歴を
静的ルールより優先し、プライベートモードでは無効にする。採用には、調整用devとは
別の複数domainでtop-1とMRRが非悪化、候補生成p95、冷起動、RSS、pack bytesが予算内、
全入力データの再配布・商用利用条件が台帳化済み、という条件をすべて要求する。
語彙や文脈を含む詳細結果は保存せず、集計値だけをrelease evidenceにする。
商用CIは公開側の`just evaluate-context-pack DATA_DIR INPUT`を使い、入力・候補・誤答を
出力せずにbaselineとの差だけを判定する。詳細は
[文脈パック評価](context-pack-evaluation.md)に固定する。

語彙と文脈を同じreleaseで用意する場合は、商用CIから
`just prepare-private-dictionary-candidate`を使う。語彙生成、文脈生成、改善対象と別domainの
recall/context gate、pack生成を同じprivate一時workspaceで完結させる。文脈評価のbaselineには
生成語彙packだけ、candidateには同じ語彙packと文脈packを置き、語彙の回収を文脈改善へ
誤算入しない。ライセンス台帳は内容をartifactへ複製せずSHA-256だけを残す。成功物も
`UNSIGNED`であり、秘密鍵を使う署名と`verify-signed`の最終gateまでは配布しない。
同じ工程で語彙だけのbaselineと語彙・文脈を持つcandidateを独立processで反復起動し、
process起動からpack読込までの中央値・p95と最大RSSの絶対値・差分を予算内に固定する。
この値はunsigned通常loaderの事前gateであり、署名済み実アプリの
初回起動とRSSを置き換えない。
署名後は`verify-signed`と同じ公開鍵・version floor parserを使うprocess起動gateを再実行し、
署名sidecar検証を含む中央値・p95・最大RSSとbaseline差も固定する。baselineとcandidateの
期待pack数を別々に必須指定し、欠落packによる見かけ上の高速化を許可しない。

派生ルールは公開側の`just generate-context-rules TRAIN OUTPUT`で生成する。trainingと
held-outのsplit、注釈済みcorpus、任意の追加辞書、生成TSVは商用側だけに置き、
公開artifactへ出さない。生成器は重複・除外行をhashで処理し、通常候補にない表記、
同数首位、弱い多数決、既に先頭の表記を除き、集計と出力SHA-256だけを返す。

実入力由来の訂正fixtureも商用側だけで管理する。公開側の`just evaluate-typos POSITIVE NEGATIVE`へ一時入力し、欠落、不要訂正、編集種別ごとの回収、p95、訂正候補数の集計だけを保存する。文章全体や周辺文脈は収集せず、明示的な同意、匿名化、保存期間、削除手順を先に固定する。詳細は[Input correction evaluation](typo-correction-evaluation.md)に固定する。
