# 軽量な日本語IMEの既存調査と開発方針

- 調査日: 2026-07-19
- 初期実装: macOS
- 将来対応: Windows（方針のみ先に固定）、Linux
- 優先事項: **軽量で、インストール後すぐ使え、入力内容を外部へ送信しないこと**

## 1. 結論

このIMEは、次の方針で進めるのがよい。

1. **変換エンジンは最初からRustで実装する。**
2. **macOSはSwift + InputMethodKitで先に完成させる。WindowsはTSFの薄いネイティブアダプターという境界だけ固定し、実装はmacOS版の後に行う。**
3. **通常のローマ字入力・かな漢字変換を採用し、SKK固有の入力操作をユーザーへ要求しない。**
4. **初期版はニューラルモデル、クラウド変換、形態素解析器一式を搭載しない。**
5. **コンパクトな辞書、unigram/bigram、ラティス、Viterbi探索で文節変換する。**
6. **辞書・学習データ・設定を同梱し、アカウント作成や初回ダウンロードを不要にする。**
7. **入力内容はローカルだけで処理し、ネットワーク権限を持たない構成を既定にする。**

製品の立ち位置は「MozcやATOKの全機能を小さく再現するIME」ではない。狙うのは次の範囲である。

> OS標準IMEに近い操作で、日常文を十分に変換でき、常駐負荷と導入時の摩擦が小さいIME。

## 2. 「軽量」と「すぐ使える」の定義

軽量性をバイナリサイズだけで判断すると設計を誤る。最低でも次を別々に計測する。

| 指標 | 初期目標 | 意味 |
| --- | ---: | --- |
| ダウンロードサイズ | 各OS 10 MB以下 | 導入の心理的・通信的負担 |
| インストール後サイズ | 25 MB以下 | 本体、辞書、設定ツールの合計 |
| アイドル時RSS | 30 MB以下 | 常駐プロセスの実メモリ |
| 1キー処理時間 | p95 5 ms以下 | ローマ字→かな、状態更新、preedit反映 |
| 変換候補の初回表示 | p95 20 ms以下 | Space入力から候補表示までのエンジン時間 |
| コールド起動 | 100 ms以下 | 初回入力セッション開始まで |
| 初回利用開始 | 2分以内 | ダウンロード開始から実際に日本語を確定入力するまで |

これらは現時点で達成済みの数値ではなく、開発時の**性能予算**である。IME単体の処理時間と、OS・対象アプリを含むend-to-end時間は分けて測る。

「インストール後すぐ使える」は次を意味する。

- アカウント作成なし
- ネットワーク接続なし
- 辞書やモデルの追加ダウンロードなし
- 初回設定ウィザードなし
- 最初からローマ字入力、ひらがなモード
- 学習前でも日常文を変換可能
- アンインストールで本体を完全に除去可能
- ユーザー辞書と学習履歴は明示的に残すか削除するか選択可能

OSが求める入力ソースの有効化だけは完全には省略できない。ここは製品UIで隠すのではなく、最短の手順へ案内する。

## 3. OSの入力基盤

### 3.1 macOS: InputMethodKit

Appleの[InputMethodKit](https://developer.apple.com/documentation/inputmethodkit)は、クライアントアプリとの通信、候補ウィンドウ、入力モードを扱う公式フレームワークである。[`IMKServer`](https://developer.apple.com/documentation/inputmethodkit/imkserver?language=objc)が入力メソッドのサーバーとなり、入力セッションごとに[`IMKInputController`](https://developer.apple.com/documentation/inputmethodkit/imkinputcontroller?language=objc)が作られる。

実装上の要点:

- macOS側はSwiftで`IMKInputController`を実装する。
- `NSEvent`、`NSRange`、`IMKTextInput`をRustコアへ渡さない。
- OSイベントを共通の`InputEvent`へ変換し、Rustが返す`SlimeAction`をmarked text、確定文字列、候補UIへ反映する。
- 候補UIは初期段階では`IMKCandidates`を優先し、独自ウィンドウは互換性を確認してから検討する。
- SwiftUIをIMEの必須依存にしない。入力経路はInputMethodKit/AppKitだけで成立させる。

配布上の要点:

- 入力メソッドbundleを`/Library/Input Methods`へ配置する`.pkg`を基本とする。
- Developer IDで署名し、Appleの[公証](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)を通す。
- インストール後に「システム設定 → キーボード → テキスト入力 → 編集」へ誘導し、入力ソースを追加してもらう。Appleの[入力ソース設定ガイド](https://support.apple.com/ja-jp/guide/mac-help/mchl84525d76/mac)でも入力ソースの追加はユーザー操作になっている。
- 実在するIMEでもログアウト・再ログインが必要になる例がある。azooKeyの[インストール手順](https://github.com/azooKey/azooKey-Desktop)も再ログインを案内しているため、「再ログイン不要」を保証せず、まず再ログインなしで検出を試し、見えない場合だけ案内する。

macOSで完全なゼロクリック有効化を目標にするのは現実的ではない。代わりに、インストール完了画面で次の1操作を明確にする。

> 入力ソースに「IME名」を追加してください。

### 3.2 Windows: Text Services Framework（将来方針）

この節は将来の移植を妨げないための設計方針であり、現在の実装対象ではない。macOS MVPが安定し、RustコアのAPIと辞書形式が固まるまでは、Windows用DLL、インストーラー、互換性試験へ着手しない。

新規のWindows IMEは[Text Services Framework (TSF)](https://learn.microsoft.com/en-us/windows/win32/tsf/text-services-framework)で実装する。Microsoftの[カスタムIME要件](https://learn.microsoft.com/en-us/windows/apps/develop/input/input-method-editor-requirements)は、IMM32方式ではなくTSFを使用するよう求めている。

実装上の要点:

- TSFからロードされるin-process COM DLLとして実装する。
- `ITfTextInputProcessor(Ex)`、キーストロークsink、composition、edit session、display attributes、候補UIを段階的に実装する。
- UILessモード、Windows Search、UWP/packaged app、通常のWin32アプリを別々に検証する。
- OSやホストアプリと同じプロセスへロードされるため、panic、例外、重いI/O、ネットワーク処理を入力コールバック内で実行しない。
- 将来のWindows初期版では、x64 Windows上の32-bitアプリも対象にするため、x64 DLLとx86 DLLを用意する。ARM64はその後の追加候補とする。

配布上の要点:

- TSFの[Text Service Registration](https://learn.microsoft.com/en-us/windows/win32/tsf/text-service-registration)に従い、COM、言語プロファイル、カテゴリを登録する。
- DLLとインストーラーをデジタル署名する。Microsoftも[サードパーティIMEにデジタル署名を要求](https://learn.microsoft.com/en-us/windows/apps/develop/input/input-method-editors)している。
- レジストリを直接編集して既定IMEにしない。
- 公式要件にある`InstallLayoutOrTip`をインストーラーから呼び、ユーザーの有効な入力メソッドへ追加する。これにより、再起動や設定画面での手動追加を避けられる。
- インストール後はWin+Spaceで選択できる状態を完成条件とする。既定IMEを勝手に変更しない。

Microsoftの[SampleIME](https://github.com/microsoft/Windows-classic-samples/tree/main/Samples/IME/cpp/SampleIME)はC++中心で、将来TSFの実装面を確認する基準になる。着手時は次の順序で判断する。

1. 最初の実動スパイクは公式サンプルに近い薄いC++ shellで行う。
2. RustコアをC ABIで呼び出し、実アプリ互換性を先に確立する。
3. `windows` crateで同等のCOM DLLを安全に構築できると確認できた場合だけ、shellをRustへ移す。

Microsoft公式の[`windows-rs`](https://github.com/microsoft/windows-rs)はCOMを含むWindows APIをRustから呼べる。TSFの[`ITfTextInputProcessor`](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/UI/TextServices/struct.ITfTextInputProcessor.html)も生成済みだが、「APIを呼べる」ことと「プロダクション品質のIME DLL、登録、アンロード、複数bitnessを完成できる」ことは別問題である。

### 3.3 Linuxは初期リリースから外す

LinuxはIBus、Fcitx 5、Wayland、X11、デスクトップ環境ごとの差が大きい。現在はmacOSだけを実装し、将来Windows、その後Linuxへ広げられるようRustコアのAPIをOS非依存に保つ。

Linux対応時は、独自にWayland/XIMを直接抱えるより、最初にIBusエンジンを実装し、その後Fcitx 5を評価する。現在の[IBus](https://github.com/ibus/ibus)はLinux向け入力フレームワークとして稼働している。

## 4. 既存IME・変換エンジンの調査

### 4.1 比較表

| 実装 | 対象 | 方式・構成 | 軽量性への示唆 | 今回の判断 |
| --- | --- | --- | --- | --- |
| Apple日本語入力 / Microsoft IME | 各OS標準 | OS統合された一般的なかな漢字変換 | 導入摩擦の基準 | キー操作と初期状態の基準にする |
| [Mozc](https://github.com/google/mozc) | macOS / Windows / Linuxほか | C++、辞書、ラティス、予測、学習、複数プロセス/ツール | 高品質・広機能だが構成とビルドが大きい | 設計資料として参照。直接forkしない |
| Google日本語入力 | macOS / Windows | Mozcを起源とする商用品質IME | 一般ユーザーが期待する操作・品質 | 互換操作と評価コーパスの比較対象 |
| ATOK | macOS / Windowsほか | 商用の高機能IME | 品質重視だが今回の軽量・OSS方針とは異なる | 機能競争をしない |
| [AquaSKK](https://github.com/codefirst/aquaskk) | macOS | 文法解析をしないSKK | 小さい実装が成立する | 状態機械・辞書の参考。操作方式は採用しない |
| [macSKK](https://github.com/mtgto/macSKK) | macOS | Swift、SKK、Sandbox、外部依存なし | 配布物が小さく、セキュリティ方針が明確 | macOS実装・配布・テストの重要な参考 |
| [CorvusSKK](https://github.com/nathancorvussolis/corvusskk) | Windows | C++、TSF、SKK、インストーラー | 小さいWindows IMEが実現可能 | TSF登録、bitness、配布の重要な参考 |
| [Rime/librime](https://github.com/rime/librime) | macOS / Windows / Linux | C++共通コア + Squirrel/Weasel等のOS frontend | コアとfrontend分離の成功例。依存は多い | アーキテクチャを参考。日本語エンジンとしては採用しない |
| [libkkc](https://github.com/ueno/libkkc) | 主にLinux | C/Vala、MARISA trie、N-gram | 小さな統計変換器の実例だが長期停滞・GPL | アルゴリズムを参考。依存にはしない |
| [Akaza](https://github.com/akaza-im/akaza) | Linux/IBus | Rust、MARISA/Cedar trie、unigram/bigram、Viterbi、学習 | 今回に最も近いRust統計IME。ただし既定モデルは大きい | `libakaza`を詳細評価し、再利用可能部分を検討 |
| [azooKey Desktop](https://github.com/azooKey/azooKey-Desktop) | macOS | Swift + 統計変換 + Zenzaiニューラル変換 | 高精度だがモデルと配布物が大きい | ニューラル追加パックの将来参考 |
| [imekit](https://github.com/SergioRibera/imekit) | macOS / Windows / Linuxを標榜 | RustでIMK/TSF/Wayland等を抽象化 | 方向性は近いが若く、OS登録・実運用の検証が不足 | 初期依存にはしない。継続監視 |

### 4.2 配布物サイズの実測スナップショット

2026-07-19にGitHub Releases APIで取得した最新の圧縮配布物サイズ。これはインストール後サイズやRSSではない。

| 製品 | 配布物 | サイズ |
| --- | --- | ---: |
| macSKK 2.18.1 | [DMG](https://github.com/mtgto/macSKK/releases/download/2.18.1/macSKK-2.18.1.dmg) | 2.22 MB |
| CorvusSKK 3.3.2 | [EXE](https://github.com/nathancorvussolis/corvusskk/releases/download/3.3.2/corvusskk-3.3.2.exe) | 4.15 MB |
| Rime Squirrel 1.1.2 | [PKG](https://github.com/rime/squirrel/releases/download/1.1.2/Squirrel-1.1.2.pkg) | 25.50 MB |
| Rime Weasel 0.17.4 | [EXE](https://github.com/rime/weasel/releases/download/0.17.4/weasel-0.17.4.0-installer.exe) | 12.43 MB |
| azooKey Desktop 0.1.4 | [PKG](https://github.com/azooKey/azooKey-Desktop/releases/download/v0.1.4/azooKey-release-signed.pkg) | 123.69 MB |
| Akaza default model 2026.602.0 | [モデルのみ](https://github.com/akaza-im/akaza/releases/download/v2026.602.0/akaza-default-model.tar.gz) | 165.23 MB |

この比較から分かること:

- SKK系はmacOS/Windowsとも数MBの配布が成立している。
- クロスプラットフォームコアや設定自由度を持つRimeは中規模になる。
- ニューラルモデルまたは大規模統計モデルを同梱すると、100 MB級になりやすい。
- 「Rustだから軽い」「Swiftだから重い」とは言えない。辞書・モデル・依存関係が支配的である。

### 4.3 Mozcから学ぶこと

[Mozc](https://github.com/google/mozc)はAndroid、macOS、Linux、Windowsなどを対象にした大規模な日本語IMEであり、OS frontendと共通エンジンを分ける先例である。

採用したい考え方:

- OSアダプターと変換エンジンの分離
- 入力状態を明示的なコマンド/状態機械として扱う
- 辞書探索、変換、予測、学習を分離する
- 辞書をビルド時にランタイム向け形式へ変換する
- golden testとストレステストを充実させる
- 読みと表記のtrieに[LOUDS](https://github.com/google/mozc/tree/master/src/storage/louds)を使うような、静的辞書の圧縮

初期版で採用しないもの:

- Qt設定ツール
- 多数の補助プロセス
- 大規模な予測機能
- Web由来の巨大語彙
- Mozc全体のfork

Mozc OSS辞書の[`README.txt`](https://github.com/google/mozc/blob/master/src/data/dictionary_oss/README.txt)によると、公開辞書はIPAdicを基本に、カタカナ語や複合語などを追加したものだが、商用Google日本語入力のWeb由来大規模語彙は含まれない。コードがBSDでも辞書は複数由来なので、コードのライセンスだけを見て辞書を流用してはいけない。

### 4.4 SKK系から学ぶこと

SKKは文法的解析を行わず、ユーザーが変換開始位置や送り仮名を明示する。そのためエンジンを小さくしやすい。[SKKの設計者による解説](https://www.fos.kuis.kyoto-u.ac.jp/~masahiko/papers/jssst32.pdf)でも、SKKは「Simple Kana to Kanji Converter」であり、日本語の文法的解析をしないことが特徴とされる。

長所:

- 変換エンジンと辞書が小さい
- 候補決定が予測しやすい
- ユーザー学習が単純
- オフラインで完結しやすい

短所:

- 通常IMEと異なる大文字・送り仮名操作を学ぶ必要がある
- インストール直後に誰でも使えるという目標と衝突する
- 文全体の自然な一括変換はユーザー操作へ依存する

したがって、SKKの**実装上の単純さ、辞書の小ささ、明示的な状態機械**は取り入れるが、SKK操作自体は採用しない。

SKK辞書の公式配布ページにはS/M/ML/Lの段階がある。2026-07-19時点の未圧縮ファイルサイズはGitHub API上で、Sが約0.06 MB、Mが約0.14 MB、Lが約4.49 MBだった。[公式辞書一覧](https://skk-dev.github.io/dict/)も、辞書によって語彙と使用感が大きく変わると説明している。ただし主要辞書はGPL v2 or laterであるため、製品同梱前に配布形態とライセンス方針を確定させる必要がある。

### 4.5 Rimeから学ぶこと

[librime](https://github.com/rime/librime)はC++の共通コアに対し、macOSのSquirrel、WindowsのWeasel、Linuxのibus-rimeなどを分けている。複数OSで入力フレームワークが異なっても、変換ロジックを共有できる実証例である。

一方、Boost、LevelDB、MARISA、OpenCC、yaml-cppなど複数の依存を持つ。今回の日本語IMEでは、設定DSLやプラグイン基盤を初期要件にせず、共通コアの依存数を抑える。

### 4.6 Akazaから学ぶこと

[Akaza](https://github.com/akaza-im/akaza)はRust製の統計的かな漢字変換IMEで、今回のエンジン方針に最も近い。

Akazaの構成:

- 読みから候補をtrieで検索
- ラティスを構築
- unigram/bigramコストを使用
- Viterbiで最小コスト経路を選択
- ユーザー確定結果からunigram/bigram頻度を学習
- 数字や日付を動的候補として生成

この基本構造は採用候補である。ただし最新のdefault model圧縮配布物は約165 MBあり、今回のサイズ予算には合わない。`libakaza`のコード再利用、辞書形式、MARISA依存、モデル生成パイプライン、ライセンスを個別に評価し、モデルは独自に小さく作る必要がある。

### 4.7 imekitをすぐ採用しない理由

[imekit](https://github.com/SergioRibera/imekit)はRustからmacOS IMK、Windows TSF、Linux入力プロトコルを扱うことを目指しており、方向性は一致している。しかし2026-07-19時点では0.2系、少数コミットの若い実装である。

特に確認が必要な点:

- Windowsで完全なTSF text service DLLとインストール登録まで提供するか
- `SendInput`やIMM32 fallbackに依存せず、TSF compositionとして全アプリで動作するか
- macOSでInputMethod bundle、`IMKServer`、`IMKInputController`のライフサイクルをどこまで担うか
- 署名、公証、x86/x64、アンインストール、OS更新への追従

共通抽象は参考にできるが、初期製品の基盤として依存するには実アプリでの検証が不足している。macOSではInputMethodKitを直接使い、Windows部分の採用判断はWindows対応へ着手するときにsmoke testを作ってから行う。

## 5. 論文・技術資料から得られる設計判断

かな漢字変換研究の系譜、21件の主要研究・資料の比較、数値、評価指標、実験計画は[軽量日本語IMEのためのかな漢字変換・入力研究レビュー](./kana-kanji-conversion-literature-review.md)に分離して整理した。この節は、そこから製品方針へ直接反映する判断の要約である。

### 5.1 統計的かな漢字変換はグラフ探索として扱える

[Discriminative Method for Japanese Kana-Kanji Input Method](https://aclanthology.org/W11-3502/)は、未分割のかな文を単語へ分割し、かな漢字変換をグラフ探索として構成する考え方を整理している。言語モデル方式と識別モデル方式を比較しているが、初期実装では説明可能で実装量の小さいコスト付きラティス + Viterbiが適する。

基本形:

1. 入力されたかな列の各位置から辞書の共通接頭辞を検索する。
2. 読み、表記、品詞/接続情報、単語コストを持つノードを作る。
3. 単語間コストを辺としてラティスを作る。
4. Viterbiで最小コスト経路を求める。
5. 上位k経路または選択文節の候補を生成する。

### 5.2 bigramは軽量IMEの妥当な出発点

[Statistical Input Method based on a Phrase Class n-gram Model](https://aclanthology.org/W12-4801/)は、かな漢字変換で速度とモデルサイズの都合からbigramが使われること、phrase/class化によってtrigramより小さく同等以上の精度を得られる可能性を示している。

同論文の比較では、phrase class bigramはF値90.41%、語彙5,550、非ゼロ頻度206,978で、word-pronunciation trigramのF値90.21%、語彙22,801、非ゼロ頻度645,996より小さかった。データセットと現代の製品要件は異なるため数値をそのまま製品性能とは見なせないが、「高次n-gramを増やす前にphrase/class化を検討する」根拠になる。

初期方針:

- unigram + pruned bigram
- 助詞をまたぐ曖昧性には、限定的なskip-bigramまたはルールを検討
- trigram全面導入はモデルサイズ計測後
- phrase化は頻出定型句だけに限定

### 5.3 ユーザー学習は効果と副作用を同時に測る

[A Comparative Study on Language Model Adaptation Using New Evaluation Metrics](https://www.microsoft.com/en-us/research/publication/a-comparative-study-on-language-model-adaptation-using-new-evaluation-metrics/)は、かな漢字変換の適応でCER改善だけでなく、適応による副作用も評価している。

学習機能は「直前に選んだ候補を常に1位にする」だけでは危険である。初期実装では次を守る。

- 読み、表記、直前語または文頭/文末だけを小さく学習する
- 時間減衰または上限を設ける
- 学習データをユーザー辞書と分離する
- 学習のリセット、無効化、エクスポートを可能にする
- 学習あり/なしで回帰コーパスを比較する

### 5.4 ニューラル変換は高精度だが初期の軽量目標と合わない

[ニューラルかな漢字変換システム Zenzai](https://www.anlp.jp/proceedings/annual_meeting/2025/pdf_dir/P1-19.pdf)は、文脈を使うニューラル変換で高い精度を示す。Q5_K_M量子化後でもモデルはxsmall 19.9 MB、small 72.3 MB、medium 237.2 MBで、M2 Pro上の推論時間は条件により入力1文字あたり数msから数十msだった。

これは将来機能として有望だが、初期版では次の理由から採用しない。

- モデル単体で初期インストール予算を消費する
- キー入力ごとの推論がCPU/GPU、電力、端末差の影響を受ける
- WindowsとmacOSで推論バックエンドの差が増える
- 辞書変換だけでもフォールバックとして必要

将来導入する場合は、既定エンジンを置き換えず、統計エンジンが生成したN-bestの**任意ダウンロード式リスコアラー**にする。

[Alignment-Based Decoding Policy for Low-Latency and Anticipation-Free Neural Japanese Input Method Editors](https://aclanthology.org/2024.findings-acl.479/)も、Transformer IMEでは計算量と途中出力による遅延が主要課題であると説明している。ライブ変換を将来入れる場合も、毎キー全文再計算ではなく、確定したprefixを再利用する必要がある。

### 5.5 入力遅延は変換精度と別の品質軸

[Effects of Text Input Latency on Performance and Task Load](https://doi.org/10.1145/3626705.3627784)では、20 msと200 msの条件を比較し、200 msの遅延が訂正作業と主観的負荷へ悪影響を与えた。IMEでは変換候補だけでなく、preeditの追従とBackspaceの反応が重要である。

そのため、候補品質だけでなく次を継続計測する。

- key downからpreedit更新まで
- Spaceから候補表示まで
- 候補移動から表示更新まで
- Enterから確定文字列反映まで
- Backspace連打、長文、候補100件時のworst case

### 5.6 辞書は語彙を削る前に表現を圧縮する

[Efficient dictionary and language model compression for input method editors](https://aclanthology.org/W11-3503/)は、1,345,900語の辞書を59.1 MBのplain textから13.3 MBへ圧縮しながら、共通接頭辞検索、予測検索、逆引きを維持した。約3,000クラスの遷移表も、17.4 MBの2次元配列から2.9 MBのsuccinct treeへ縮小している。

この結果から、Core辞書の語彙数を先に極端に削るのではなく、読み・表記trie、token配列、カタカナ生成、疎な遷移表を別々に圧縮する。LOUDS、minimal FST、flat sorted arrayは同じデータでサイズ、RSS、cold/warm lookupを比較して決める。

### 5.7 平均精度だけで出荷しない

[統計的かな漢字変換システム Mozc](https://www.anlp.jp/proceedings/annual_meeting/2011/pdf_dir/C4-3.pdf)は、平均評価が改善または維持されても「昨日」「午後」のような基本語が変換できないと製品品質を大きく損なうため、絶対に誤ってはいけない変換例の回帰試験を出荷条件へ加えたと説明している。

今回も評価を次の四層へ分ける。

1. 基本語・助詞・活用・数字のmust-pass
2. balanced corpus上のCER、LCS-F、Acc@k
3. 同音異義や誤変換報告を集めた難例
4. 学習による改善と副作用

平均値が上がってもmust-passが落ちたbuildは出荷しない。

### 5.8 ユーザー学習は副作用を独立評価する

Mozcの初期実装では、選択結果を広く学習したことで使うほど意図しない候補が出る問題があり、内容語の品詞と機能語表層が交換可能な場合へ学習を制限して改善した。[言語モデル適応の比較研究](https://aclanthology.org/H05-1034/)も、同じCERのmodelで既存正解を壊す副作用が異なり得ることを示している。

したがって、読みと表記だけの無条件昇格は行わない。文脈、品詞、機能語境界、頻度上限、時間減衰を使い、学習deltaはsystem dictionaryと分離してrollback可能にする。

### 5.9 誤入力は自動確定しない

[かな漢字変換における誤入力の訂正](https://cir.nii.ac.jp/crid/1573950401966931328)では、自動訂正の再現率44%に対して誤り率3%だった。実験条件は古いが、IMEが珍しい固有名詞を一般語へ勝手に変える危険は現在も同じである。

初期版では、辞書完全一致がない場合に限り編集距離やkeyboard隣接を使った候補を別枠表示し、自動確定しない。通常の語彙学習とtypo訂正履歴も分離する。

### 5.10 評価benchmarkを一つに依存しない

[AJIMEE-Bench](https://github.com/azooKey/AJIMEE-Bench)は実際の漢字変換誤りを基にした200件の難例で、複数の許容解も扱える。一方、難例へ意図的に偏っており、日常文の分布、候補操作、速度、学習副作用は測れない。

AJIMEE-Benchは難例回帰として使い、BCCWJ等のbalanced corpus、独自must-pass、操作コスト、性能benchmarkと組み合わせる。corpusを学習・評価に使えることと、生成辞書やmodelを製品配布できることは分けてライセンス確認する。

## 6. 辞書の調査と方針

### 6.1 辞書は最大のサイズ・ライセンスリスク

候補となる資料:

| 辞書 | 特徴 | サイズ/ライセンス上の注意 |
| --- | --- | --- |
| Mozc OSS辞書 | IPAdicベース + カタカナ語・複合語等 | 生データと接続表が大きく、複数由来ライセンス |
| SKK-JISYO | 読み→候補が単純、S/M/Lを選べる | 小さいが主要辞書はGPL系 |
| [UniDic](https://clrd.ninjal.ac.jp/unidic/en/) | 高品質な形態論情報 | 配布元の通常版でも数百MB。triple licenseから選択可能 |
| [SudachiDict](https://github.com/WorksApplications/SudachiDict) | Apache-2.0、現代語・正規化が豊富 | Coreでも約200MB級で、そのまま同梱できない |
| IPAdic | 成熟した品詞・接続情報 | 古い語彙。帰属・免責表示が必要 |
| 独自コーパス生成 | サイズと語彙を制御可能 | コーパスの利用条件、読み推定、品質管理が必要 |

初期方針:

1. 辞書の採用をコード実装より先に法務・配布条件まで確定する。
2. 原辞書をそのまま同梱せず、必要フィールドだけをビルド時に抽出する。
3. 読みtrie、表記テーブル、単語コスト、接続情報を別セクションにする。
4. ランタイム形式は固定・read-only・memory-map可能にする。
5. ユーザー辞書は小さな追記ログまたは別ファイルにし、システム辞書を再構築しない。

### 6.2 初期辞書のサイズ戦略

目標は次の三層である。

- **Core語彙**: 日常語、基本的な活用、一般的な固有名詞
- **生成候補**: 数字、日付、時刻、記号、全角/半角、ひらがな/カタカナ
- **User語彙**: ユーザー辞書と確定履歴

住所、郵便番号、顔文字、大量の人名、専門用語は初期Coreへ無制限に入れない。必要なら後からオプション辞書にする。

ランタイム形式の候補:

- 読みのprefix検索: LOUDS trie、FST、または同等の静的succinct trie
- 表記: UTF-8 string pool + offset
- コスト: 量子化した整数
- bigram: 頻出遷移だけをpruneし、sorted pairまたは圧縮trieで保存
- 読み込み: mmap + lazy page-in

SQLiteは設定やユーザー辞書編集には便利だが、キー入力ごとのシステム辞書探索には初期採用しない。

## 7. 採用する製品仕様

### 7.1 初期入力機能

- ローマ字→ひらがな
- かな入力は初期版後でもよいが、コアAPIで追加可能にする
- Spaceで通常のかな漢字変換
- Space/上下キーで候補移動
- Enterで確定
- Escapeで変換取消
- Backspace/Delete
- 左右キーで文節移動
- Shift+左右またはOS標準に近い操作で文節伸縮
- F6〜F10相当のひらがな・カタカナ・全角英数・半角英数変換
- 数字、日付、時刻の動的変換
- ユーザー辞書
- 最小限の変換学習

### 7.2 初期版から外す機能

- クラウド変換
- LLM変換
- ライブ変換
- 次文予測
- 絵文字・顔文字の巨大辞書
- 同期アカウント
- テーマや候補ウィンドウの高度なカスタマイズ
- プラグインシステム
- Linux frontend
- 自動収集テレメトリ

外す理由は、サイズだけでなく、初回設定、プライバシー説明、障害点、互換性試験を増やすためである。

### 7.3 プライバシーとセキュリティ

IMEはパスワードや個人情報を扱い得る。macSKKも[README](https://github.com/mtgto/macSKK#%E7%89%B9%E5%BE%B4)でSandboxと外部ライブラリ削減を明示している。

初期ポリシー:

- 変換は完全オフライン
- 本体プロセスはネットワーク接続しない
- 入力文字列をログへ出さない
- crash reportは既定OFF、送る場合もユーザーが内容を確認する
- 学習データはローカル保存
- secure input/password fieldでは候補・学習・ログを無効化
- Rust FFI境界でpanicを越境させない
- Windows DLLの`DllMain`で重い初期化をしない
- 辞書ファイルは署名/ハッシュを検証し、壊れていればかな入力へ安全にfallbackする

## 8. 推奨アーキテクチャ

```text
IME workspace
├── crates/
│   ├── slime-core/          # 状態機械、InputEvent -> SlimeAction
│   ├── slime-romaji/        # ローマ字→かな
│   ├── slime-converter/     # ラティス、Viterbi、N-best、文節
│   ├── ime-dictionary/    # 静的辞書、user dictionary
│   ├── ime-learning/      # 小さなunigram/bigram適応
│   ├── slime-ffi/           # C ABI、panic隔離、UTF-8境界
│   └── slime-tools/         # 辞書compiler、評価CLI
├── platforms/
│   ├── macos/             # Swift + InputMethodKit
│   └── windows/           # 将来のTSF COM DLL + installer（当面は設計資料のみ）
├── data/
│   ├── source/            # ライセンス追跡可能な辞書ソース
│   ├── generated/         # ビルド生成物。原則git管理しない
│   └── evaluation/        # golden/evaluation corpus
└── docs/
```

### 8.1 共通イベント境界

```rust
pub enum InputEvent {
    Character(char),
    Space,
    Enter,
    Escape,
    Backspace,
    Delete,
    MoveLeft,
    MoveRight,
    NextCandidate,
    PreviousCandidate,
    ExpandSegment,
    ShrinkSegment,
    Activate,
    Deactivate,
}

pub enum SlimeAction {
    UpdatePreedit(Preedit),
    ShowCandidates(CandidateList),
    HideCandidates,
    Commit(String),
    Clear,
    ForwardKey,
    PersistLearning(LearningDelta),
}
```

OS側の責務:

- OSイベントを`InputEvent`へ変換
- `SlimeAction`をOS APIへ反映
- カーソル位置、DPI、候補ウィンドウ、入力セッションの管理
- インストール、登録、署名、アップデート

Rustコアの責務:

- 入力モードとcomposition状態
- ローマ字→かな
- 辞書検索と候補生成
- 文節と候補選択
- 学習差分
- OS非依存の性能計測と回帰テスト

### 8.2 FFI方針

- ABIはC互換に固定する。
- 文字列はUTF-8 + pointer/lengthで渡す。
- Rust所有メモリをSwift/C++側で直接解放させず、専用free関数を用意する。
- 1イベント入力→1レスポンス取得の同期APIから始める。
- OSのオブジェクトポインターやCOM interfaceをRustコアへ渡さない。
- FFI入口を`catch_unwind`で保護し、エラー時は`ForwardKey`または安全なcomposition cancelへfallbackする。

## 9. インストール体験

### 9.1 macOS

理想フロー:

1. 署名・公証済み`.pkg`を開く。
2. インストールする。
3. 完了画面から入力ソース設定を開く。
4. 「IME名」を追加する。
5. メニューバーまたはControl+Spaceで選択して入力する。

設計上の約束:

- Homebrewは補助経路であり、一般ユーザーの主経路にしない。
- 辞書を初回起動時にダウンロードしない。
- ログアウトが必要な場合だけ、その理由と保存前の注意を表示する。
- アップデートで入力ソース登録やユーザー辞書を壊さない。

### 9.2 Windows（将来）

理想フロー:

1. 署名済みインストーラーを開く。
2. インストールする。
3. インストーラーがTSF profileを登録し、`InstallLayoutOrTip`で有効化する。
4. Win+Spaceで「IME名」を選び、入力する。

設計上の約束:

- 再起動を要求しないことを目標とする。
- 手作業の`regsvr32`を要求しない。
- 管理者権限の要否をインストール方式のスパイクで確定する。
- x64/x86 DLLを同じインストーラーで正しく配置・登録する。
- uninstall時にlanguage profile、category、COM登録を確実に削除する。

## 10. テスト・評価計画

### 10.1 Rustコア

- ローマ字変換のtable test
- composition状態機械の全遷移
- 入力列→preedit/actionのgolden test
- 読み→期待候補の回帰コーパス
- 文節伸縮、取消、再変換のテスト
- 辞書破損、不正UTF-8、巨大入力へのfuzz test
- 学習前後の候補順位と副作用
- dictionary lookup、変換、N-bestのbenchmark

### 10.2 OS統合

macOS:

- TextEdit、Safari、Chrome、VS Code、Terminal、JetBrains IDE、Microsoft Office
- marked text、候補位置、確定、取消、アプリ切替
- Secure Inputとパスワード欄
- Apple Silicon / Intel

Windows（将来）:

- 以下は将来の移植時に実施し、macOS MVPの完成条件には含めない
- Notepad、Word、Edge、Chrome、VS Code、Windows Terminal、JetBrains IDE
- Win32、UWP/packaged app、Electron、Java
- x64アプリと32-bitアプリ
- UILess mode、DPI、複数モニター
- install/update/uninstall後のTSF登録

### 10.3 導入テスト

現在はクリーンなmacOS VMで、次をリリースごとに録画・計測する。Windows対応へ着手した段階で同じ試験をWindows VMにも適用する。

- ダウンロードから最初の「日本語」確定までの時間
- クリック数
- 再ログイン/再起動の有無
- 警告ダイアログの内容
- uninstall後に残るファイルと登録

## 11. 開発ロードマップ

### Phase 0: macOSの技術スパイク

- Rustコアの`Character -> UpdatePreedit -> Commit`最小実装
- macOS InputMethodKitからRust FFIを呼び、TextEditへ入力
- 署名前のローカルinstaller prototype
- この段階ではかな漢字変換を作り込まない

完了条件: macOSのTextEditでRustコアから「にほん」をpreedit表示し、「日本」を確定できる。

### Phase 1: 軽量変換コア

- ローマ字→かな
- 静的辞書compiler
- prefix lookup
- unigram + pruned bigram
- Viterbi、文節、N-best
- CLI評価器とbenchmark

完了条件: 代表的な日常文コーパスを変換でき、辞書・モデルがサイズ予算内に入る。

### Phase 2: macOS MVP

- 候補UI
- 入力モード
- ユーザー辞書
- `.pkg`、署名、公証
- クリーン環境の導入テスト

### Phase 3: macOS版の品質改善

- 小さなユーザー学習
- 語彙とコストの改善
- アプリ互換性修正
- cold start、RSS、latencyの最適化
- Linux IBus frontendの再評価

### Future W: Windows MVP（macOS版安定後まで着手しない）

- Rustコアと辞書形式を変更せずにTSFへ接続する技術スパイク
- x64/x86 TSF DLL
- 候補UI、UILess mode
- signed installer
- `InstallLayoutOrTip`
- クリーン環境の導入テスト

## 12. 直近で確定すべき未解決事項

1. **辞書ライセンス**: IPAdic/UniDic/SKK/Mozc由来データのどれを採用し、生成物をどのライセンスで配布できるか。
2. **最低macOS**: macOS 13以降を仮置きし、利用者範囲とテストコストで確定する。
3. **Windows shellの言語（将来）**: 着手時にC++で公式サンプルへ寄せる案と`windows-rs`案を実動比較する。現在は決定しない。
4. **辞書サイズ予算**: 圧縮後8 MBを仮上限にし、変換品質とのPareto曲線を測る。
5. **ライセンス方針**: コアをMIT/Apache-2.0にするか、辞書由来条件を含めGPL系にするか。
6. **製品名とbundle/profile ID**: インストール・更新互換性に直結するため、実装初期に固定する。

## 13. 採用判断のまとめ

### 採用

- Rust共通コア
- Swift + InputMethodKit adapter
- 将来のTSF adapterを妨げないC ABI
- 通常のローマ字入力
- trie + unigram/bigram + Viterbi
- bundled offline dictionary
- ローカル学習
- signed/notarized native installer

### 保留

- Windows adapterの全面Rust化
- Windows実装の着手時期
- Akaza/libakazaのコード再利用
- skip-bigram、phrase class model
- Linux IBus
- optional neural N-best rescoring

### 不採用（初期版）

- Swift共通コア
- Mozc全体のfork
- SKK操作を既定にすること
- neural modelの標準同梱
- cloud conversion
- 初回辞書ダウンロード
- SQLiteによるhot-path辞書検索
- 未検証のcross-platform IME abstractionへの全面依存

## 14. 主な参照資料

### OS公式

- Apple: [InputMethodKit](https://developer.apple.com/documentation/inputmethodkit)
- Apple: [IMKServer](https://developer.apple.com/documentation/inputmethodkit/imkserver?language=objc)
- Apple: [IMKInputController](https://developer.apple.com/documentation/inputmethodkit/imkinputcontroller?language=objc)
- Apple: [Macの入力ソース設定](https://support.apple.com/ja-jp/guide/mac-help/mchl84525d76/mac)
- Apple: [Notarizing macOS software before distribution](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)
- Microsoft: [Custom IME requirements](https://learn.microsoft.com/en-us/windows/apps/develop/input/input-method-editor-requirements)
- Microsoft: [Text Services Framework](https://learn.microsoft.com/en-us/windows/win32/tsf/text-services-framework)
- Microsoft: [Text Service Registration](https://learn.microsoft.com/en-us/windows/win32/tsf/text-service-registration)
- Microsoft: [Input Method Editors](https://learn.microsoft.com/en-us/windows/apps/develop/input/input-method-editors)
- Microsoft: [windows-rs](https://github.com/microsoft/windows-rs)

### 実装

- [Mozc](https://github.com/google/mozc)
- [Rime/librime](https://github.com/rime/librime)
- [macSKK](https://github.com/mtgto/macSKK)
- [AquaSKK](https://github.com/codefirst/aquaskk)
- [CorvusSKK](https://github.com/nathancorvussolis/corvusskk)
- [Akaza](https://github.com/akaza-im/akaza)
- [libkkc](https://github.com/ueno/libkkc)
- [azooKey Desktop](https://github.com/azooKey/azooKey-Desktop)
- [imekit](https://github.com/SergioRibera/imekit)
- [SKK辞書](https://skk-dev.github.io/dict/)

### 論文・研究資料

- Tokunaga, Okanohara, Mori (2011): [Discriminative Method for Japanese Kana-Kanji Input Method](https://aclanthology.org/W11-3502/)
- Maeta, Mori (2012): [Statistical Input Method based on a Phrase Class n-gram Model](https://aclanthology.org/W12-4801/)
- Microsoft Research: [A Comparative Study on Language Model Adaptation Using New Evaluation Metrics](https://www.microsoft.com/en-us/research/publication/a-comparative-study-on-language-model-adaptation-using-new-evaluation-metrics/)
- Sarhangzadeh, Watanabe (2024): [Alignment-Based Decoding Policy for Low-Latency and Anticipation-Free Neural Japanese Input Method Editors](https://aclanthology.org/2024.findings-acl.479/)
- ensan (2025): [ニューラルかな漢字変換システム Zenzai](https://www.anlp.jp/proceedings/annual_meeting/2025/pdf_dir/P1-19.pdf)
- Schmid et al. (2023): [Effects of Text Input Latency on Performance and Task Load](https://doi.org/10.1145/3626705.3627784)
- 佐藤雅彦: [SKKの思想を含む解説](https://www.fos.kuis.kyoto-u.ac.jp/~masahiko/papers/jssst32.pdf)

## 15. 調査上の制約

- 配布物サイズは2026-07-19時点の公開release assetであり、将来変わる。
- 配布物サイズからRSS、起動時間、変換速度は推定していない。
- 論文間でデータセット、正解定義、入力単位が異なるため、精度数値を横並びにはしていない。
- macOSの実アプリ互換性、署名済みinstaller、更新・削除は未実装であり、Phase 0以降で実機確認が必要である。Windowsは将来着手時に別途検証する。
- 辞書ライセンスの記述は技術調査であり、最終的な法的判断ではない。
