# Phase 2 文脈モデル導入 方式選定のための文献調査レポート

対象: Rust製ローカル完結かな漢字変換エンジン(Mozc OSS辞書108万語 + 品詞クラスbigram + 語unigramコスト + Viterbi/N-best)
制約: LM追加分 5〜15MB / 変換 p95 20ms以下(Apple Mシリーズ) / コールド起動 100ms以下 / ネットワークなし / Rust実装
現状: 辞書のみの主要残存誤りは同音異義語の文脈選択。2026-08-10時点ではApache-2.0のzenz-v3.2-smallをN-best専用rankerとして使う`high-accuracy` profileがAJIMEE 200件でacc@1 80.0%、GSD test 90.1%、PUD 76.7%に達した。モデルなしの軽量経路は維持し、サイズ・遅延・学習元確認を別gateとして扱う。
調査日: 2026-07-20

---

## 1. TL;DR

### 2026-08-02 実測による更新

当初の推定後にzenz-v3.1-xsmallでN-best rescoringを実装し、held-out AJIMEEの`acc@1 0.535 → 0.655`（+12pt）を確認した一方、M3 Metalでp95 86.3 msとなり20 ms予算を超えた。ラティスcost差によるconfidence gateは精度を保って18.5%を省略したがp95 56 ms、1MB hashed studentはgold 10,000件学習でheld-out `0.525`、teacher 10,000件蒸留でも固定dev `0.359`に留まった。さらに1.74M parameterの共有bi-encoder studentは固定devを`0.3139 → 0.3331`、採点p95 6.86 msまで改善したが、凍結後のheld-out AJIMEEは現在のbaseline `0.530 → 0.520`、UD news/blog外部devは`0.6405 → 0.5921`へ悪化した。

非ニューラル側では、CC BY-SA 4.0のUD Japanese GSD train 7,050文から作った直前語context bigram（81,266遷移、weight 1500）が同一domainのheld-out testを`0.6966 → 0.7647`（+6.81pt）、候補生成込みp95 0.86 msで改善した。ただしこのhot-path値はモデル読み込みを含まない。無効な遷移表の生成を除いた後でも、同一コマンド3回の中央値は冷起動0.22秒・最大RSS 49.1 MB（モデルなし0.09秒・23.8 MB）だった。JWTD devは-0.25pt、AJIMEE top-1は差なしである。したがって、**LLM/生成は必須ではないが、狭いexact bigramも汎用解ではない**。現在は品質と起動コストの両方からどちらも製品採用No-Goとする。複数domain・約1億語・形態論付きのBCCWJは技術要件に合うが、商用商品化は有償契約、公開UD annotationはCC BY-NC-SAで本文なしである。契約するか、商用条件が明確な別corpusを構築できた場合だけclass backoffとcompact格納を再評価する。詳細な再現値は[evaluation.md](evaluation.md)を参照。

2026-08-03にJWTD trainの固定dev非重複43,605窓を追加し、word=250とGSD context=1500を独立devで凍結して合成した。JWTD devは`0.3139 → 0.3682`、GSD devは`0.6405 → 0.6828`、GSD held-out testは`0.6966 → 0.7090`となったが、AJIMEE held-outが`0.530 → 0.500`へ悪化した。また冷起動中央値1.47秒、最大RSS 204.7 MBである。**複数devの同時改善だけでは採用根拠にならず、複数held-outの主指標がすべて非悪化であることをGo条件に追加する**。

- **第一候補: 文字レベル小型ニューラルLM(≈10〜25Mパラメータ、int8/int4量子化)による N-best リランキング(rescoring専用、生成はしない)**。根拠: (1) acc@10 83.5% が示す通り正解は既に候補内にあり、リランキングだけで理論上限 +30pt の伸び代がある。(2) 同一ベンチマーク(AJIMEE-Bench の元になった JWTD test 由来 200件)で、Zenzai の 26M パラメータモデル(量子化後 19.9MB)ですら生成方式で Acc@1 66.5%(Google日本語入力 54.0%)を達成しており、同音異義語の文脈選択はニューラルLMが最も直接的に解く誤りタイプであることが実証済み [Miwa+ 2025]。(3) rescoring は自己回帰生成と違い prefill(並列評価)のみで済むため、生成方式の 3.5〜6.9ms/文字という遅延制約を回避でき、p95 20ms 予算に収まる見込みが高い。
- **次点: 語彙化単語bigram(内容語中心) + Stolckeプルーニング + 量子化succinct格納による従来コストへの加点**。純Rust・学習パイプライン最小で +2〜5pt 程度(近接タスクからの推定)。第一候補の前段実装・フォールバックとしても価値がある。
- 識別モデル(構造化SVM)は BCCWJ 4万文で F値 +4.4pt の実績があるが [Tokunaga+ 2011]、素性が局所(bigram)に留まる限り「長距離の共起で決まる同音異義語」には効きにくいと原著自身が分析しており、本製品の主要残存誤りへの適合度で第一候補に劣る。

---

## 2. 方式別比較表

| 方式 | 精度向上見込み(acc@1) | モデルサイズ | 推論コスト | 実装コスト | リスク |
|---|---|---|---|---|---|
| 1. 語彙化n-gram(単語bigram/trigram + クラスバックオフ) | +2〜5pt(推定; BCCWJ変換で bigram→trigram +4.5pt [Yao+ 2018]の近接値) | プルーニング+8bit量子化で 5〜15MB に制御可能(Stolcke: サイズ26%でppl悪化<6% [Stolcke 2000]) | 数µs/クエリ(trie引き)。予算内で余裕 | 小〜中(カウント+プルーニング+trie格納、全て純Rust可) | Wikipedia偏り。低頻度同音異義語のカバレッジ不足。伸び代が小さい |
| 2. 識別モデル(SSVM/構造化パーセプトロン) | +3〜4pt(BCCWJ F値 87.9→92.3 [Tokunaga+ 2011]; かな漢字変換タスク直接の数値) | L1正則化で疎。数MB以下に制御可能 | 線形モデル、Viterbiに素性加点するだけ。<1ms | 中(素性設計、自動読み付与コーパスでの構造化学習) | 局所素性では同音異義語の長距離共起に効きにくい(原著の誤り分析)。学習データの読み推定ノイズ |
| 3a. 小型ニューラル生成(Zenzai方式: 条件付き文字LM + 投機的デコーディング) | +13〜33pt(同系ベンチで 54.0→66.5(26M)〜86.5(310M) [Miwa+ 2025]; 直接数値) | Q5_K_M で 19.9MB(26M)/72.3MB(91M)/237.2MB(310M) | 3.5〜6.9ms/文字(M2 Pro GPU)→ 20文字入力で70〜140ms。**p95 20msを超過** | 大(学習190Mペア、llama.cpp級の推論基盤、投機的デコーディング実装) | 遅延予算超過。辞書外生成(入力への非忠実)。GPU前提 |
| 3b. **小型ニューラル rescoring(推奨)** | +10〜25pt(推定; 上記3aの生成時数値と acc@10 上限83.5%からの内挿) | 10〜25Mパラメータ int8/int4 で 5〜16MB | prefillのみ・候補並列評価。26Mモデルなら数百トークンで10〜20ms(CPU)、Metal併用でさらに短縮(推定) | 中〜大(学習は必要だが、デコーダ改造不要。candle/llama.cpp bindingsで組み込み) | 学習パイプライン構築が最大コスト。量子化後の精度検証必須 |
| 3c. LSTM小型モデル(Yao+方式) | +5.6pt(BCCWJ変換 trigram 55.6→LSTM 61.2 [Yao+ 2018]; 直接数値) | 5bit k-means量子化で92%削減 ≈ 8〜10MB(逆算推定) | 3ms/キー(CPU実測) [Yao+ 2018] | 中〜大 | Transformerに比べ文脈長・精度で不利。2018年時点の設計 |
| 4a. 連語/複合語辞書の自動獲得 | +数pt相当(Mozc: BLEU 0.846→0.874、Web辞書より効果大 [Kudo+ 2011]) | 辞書エントリ追加分のみ(数MB) | ゼロ(既存ラティスに乗る) | 小 | 「次期皇帝」型の頻出コロケーションのみ。汎化しない |
| 4b. キャッシュLM/学習機能 | ユーザ適応(ベンチには効かない) | 数十KB | ゼロ同然 | 小(Mozcの疑似キャッシュモデル+交換可能制約が設計手本 [Kudo+ 2011]) | ベンチ上の同音異義語選択は改善しない |
| 4c. pointwise予測(同音異義語専用識別器) | 対象語に限れば高い(かな漢字変換での直接数値なし; KyTea系の近接タスクから) | 対象語×素性分(数MB) | <1ms | 中 | 対象語リストの保守。網羅性なし |

---

## 3. 各方式の詳細

### 3.1 語彙化n-gram + クラスバックオフ

**Mozc本体の設計(なぜクラスbigramで済ませているか)** — 工藤ら「統計的かな漢字変換システムMozc」(NLP2011) [1] を精読した結果:

- 言語モデルは P(y) = Π P(w_i|c_i)P(c_i|c_{i-1}) のクラスbigram、かな漢字モデルは単語-読みunigram。クラスは IPA品詞体系の最深階層 + 活用形/活用型の全展開 + 助詞・助動詞・非自立語・頻出動詞などの語彙化で、**約3000クラス**(現行OSS実装では2662クラス、接続行列2662²)。学習は130億文のWebコーパス + MeCab + MapReduce。
- **長距離文脈はtrigram化ではなく「複合語化」で擬似対応**。trigram化を避けた理由として「デコード時間、デコーダの複雑化、計算機リソース」を明示。形態素列の正規表現パターン(姓+名、名詞+接尾辞など)でWebコーパスから頻出単語列を抽出し1単語として辞書登録、左右で異なるクラスを持たせ、生起確率は構成語ごとの確率の幾何平均で近似。Anthy教師データでの評価: **複合語化なし 0.846 → あり 0.874(+Web辞書で0.885)BLEU**。複合語化の効果はWeb辞書追加より大きい。
- **識別モデルを使わない理由**(2.6節): (a) 生成モデル(頻度カウント)に比べスケールしない、(b) N-bestランキングの妥当性の根拠付けが困難(生成モデルなら頻度順に並ぶ)、(c) パラメータの解釈性、(d) 再変換(漢字→読み)が同時確率ベースなら辞書引きの入替だけで実現できる。
- 内部のLM利用: DeepWiki経由でリポジトリを確認した限り、変換・サジェスト・リランキングいずれも**クラスbigram接続行列 + 語unigramコスト以外の単語n-gram/ニューラルLMは持たない** [2]。接続行列は1バイトコスト圧縮オプション付き、辞書はLOUDS trie(キーtrie+値trie+ビットベクトル型トークン配列)。学習(ユーザ適応)は文節trigram/bigram/unigramを文脈キーとする疑似キャッシュモデルで、「交換可能制約」(内容語の品詞大分類一致 + 機能語表層一致)で副作用を抑制 [1]。サジェストは複合語辞書をそのまま流用。

**単語n-gramの効果の定量値** — Yao et al. (2018) [5] が BCCWJ のかな漢字変換タスクで直接比較:

| モデル | ppl | 変換 top-1 | 変換 top-10 |
|---|---|---|---|
| unigram | 833.55 | 26.95% | 45.85% |
| 単語bigram (Kneser-Ney) | 99.30 | 51.15% | 78.10% |
| 単語trigram (KN) | 68.11 | 55.60% | 79.65% |
| LSTM (1層256) | 41.39 | 61.20% | 88.30% |

クラスbigram相当のベースラインは含まれないが、bigram→trigram で +4.5pt、trigram→LSTM でさらに +5.6pt。クラスbigram→語彙化bigram の差分が本製品での期待値に相当し、**+2〜5pt 程度と推定**(近接タスクからの推定であり、かな漢字変換での「クラスbigram→単語bigram」直接比較の公表数値は見つからなかった)。

**サイズ制御技術**:
- Stolckeエントロピープルーニング [3]: 相対エントロピー(pplの変化)が閾値θ未満のn-gramを削除。θ=10⁻⁸ で**元の26%のサイズ、ppl悪化6%未満、ASR認識率は無劣化**。
- KenLM [4]: trie構造 + 確率の非可逆量子化(8bit等)で1n-gramあたり数バイトまで圧縮。Rustでは同等の succinct trie(LOUDS/marisa系。Mozc自身の辞書もLOUDS)を自前実装するか `marisa` 系crateを利用可能。
- 概算(推定): 8bit量子化trieで約4〜6バイト/エントリとすると、15MB予算で**約250万〜350万bigram**を格納可能。Wikipedia日本語(約10億語規模)から頻度カットオフ+Stolckeプルーニングでこの規模に絞る運用が現実的。内容語×内容語のbigram(「次期-皇帝」「複合-姓」「高官-更迭」型)に絞れば同音異義語選択への密度を上げられる。

**製品制約への適合性**: サイズ・遅延・起動・Rust実装すべて余裕で適合。学習パイプラインもカウント+プルーニングのみで最も単純。ただし伸び代の上限が低く、Wikipedia にない口語・低頻度共起は救えない。

### 3.2 識別モデル(構造化SVM / CRF系)

**Tokunaga, Okanohara & Mori (WTIM 2011)「Discriminative Method for Japanese Kana-Kanji Input Method」** [6] — かな漢字変換への識別モデル適用の初の直接比較。全文を精読:

- 手法: 構造化SVM(SSVM) + FOBOS(L1正則化、疎な解=省メモリを明示的に意図)。素性は**単語unigram、単語bigram、クラス(品詞第2階層)bigram、単語-読みペア**の4テンプレート。Viterbi互換の局所素性に制限。
- ベースライン: クラスbigram+単語bigram+単語unigramの線形和による生成モデル(加算スムージング; 補間Kneser-Neyより加算平滑の方が良かったと報告)。
- データ: BCCWJ人手アノテーション部(OC 6,476 / OW 5,934 / PN 17,007 / PB 10,347文、計約4万文)、5分割交差検証。
- 結果(F値): **ALL 87.9 → 92.3(+4.4pt)**、新聞(PN) 86.9→91.4、Yahoo!知恵袋(OC) 87.2→88.5。全体で「1〜4%の改善」。SSVMは学習文数が約1000文を超えるとベースラインを上回る。学習時間はALL(約4万文)で **Core 2 Duo 43分** — 現代のハードなら Wikipedia 規模でも構造化パーセプトロンなら回せる。
- **重要な誤り分析**: 「暴力団の抗争→構想」のような同音異義語誤りは「長距離情報を考慮できる素性があれば解決するはず」と原著が明記。つまり**局所素性のSSVMでは本製品の主要残存誤り(同音異義語の文脈選択)は原理的に取り切れない**。
- PFNのブログ(徳永) [7] も同実験を「精度向上は3%程度、学習データ約16000文と少ないことが寄与している可能性」と補足。

**関連**: Gao, Suzuki & Yu (ACL 2006) [8] は生成モデルのtop-k候補を識別LMでリランキングするドメイン適応を提案(かな漢字変換を含むIME文脈)。Mozc側の不採用理由は3.1参照。ATOKは「ATOKハイブリッドコア」等でニューラル併用を謳うが手法非公開(Zenzai論文 [9] の参考文献でも「プロプライエタリで詳細不明」扱い)。

**製品制約への適合性**: サイズ(L1で疎+ハッシュトリック)・遅延(線形加点)・Rust実装は容易。学習は「Wikipedia+JWTD trainをMeCab等で読み付与→構造化パーセプトロン」で中程度の複雑さ。ただし期待効果 +3〜4pt は語彙化bigram素性由来が大半で、方式1とかなり重複する。**方式1を「学習で重み付けする」変種と捉えるのが正確で、単独採用の妙味は薄い**。

### 3.3 軽量ニューラル

**Zenzai / zenz(azooKey)** — 三輪・高橋「ニューラルかな漢字変換システムZenzai」(NLP2025) [9] を全文精読。本製品にとって最重要の直接証拠:

- 構成: GPT-2ベースの条件付き文字レベルLM(語彙6000、Byte-Fallback付き文字トークナイザ)。`<boc>文脈<boi>入力かな<boo>出力` 形式で**左文脈条件付き変換**をサポート(PinyinGPT-Concat [10] と同形式)。デコードは貪欲法 + **統計的かな漢字変換(AzooKeyKanaKanjiConverter)をドラフトモデルとする投機的デコーディング**。ドラフト側が辞書を持つため、ニューラルの「入力に忠実でない生成」を検出・拒否できる。
- 学習: llm-jp-corpus-v3 + Wikipedia日本語にMeCab+NEologdで読み推定、**約190Mペア**。llm.c、H100×1。
- 評価: **JWTD(日本語Wikipedia入力誤りデータセット)test由来の200件**(文脈なし100+左文脈付き100)— これは本製品の評価に使っている AJIMEE-Bench [11] と同一系統のデータ(AJIMEE-BenchはJWTD v2ベースでazooKeyが公開)。
- 精度(Acc@1 / CER):

| 手法 | Acc@1 | CER |
|---|---|---|
| Google日本語入力(CGI API) | 54.0 | 6.7 |
| ドラフトモデル(統計的) | 44.5 | 7.6 |
| GPT-4o | 56.0 | 7.7 |
| **Zenzai xsmall (26M)** | **66.5** | 4.6 |
| **Zenzai small (91M)** | **80.0** | 2.6 |
| **Zenzai medium (310M)** | **86.5** | 1.7 |

  本製品の 53.5% はこの表の Google日本語入力(54.0)とほぼ同水準であり、**26Mモデルで+13pt、91Mで+26ptの改善が同一系統ベンチで実証されている**。「領主の家に咆哮→奉公」のようなまさに同音異義語の文脈選択誤りが直る例が示されている。
- サイズ(llama.cpp Q5_K_M量子化後): **xsmall 19.9MB / small 72.3MB / medium 237.2MB**。xsmallをQ4系にすれば約16MB(推定)で、予算上限15MBに肉薄。
- 速度(M2 Pro Mac mini、内蔵GPU、入力1文字あたり): xsmall貪欲 3.5ms、small+投機的 6.0ms、medium+投機的 6.9ms。推論回数制約L=1でmedium 4.8ms/文字(Acc@1は70.5に低下)。**生成方式では20文字の入力で70〜140msとなり、p95 20ms予算を超える**。ドラフトモデル単体でも約1.3ms/文字。
- 実運用: azooKey-Desktop(macOS)に搭載済み。開発ブログ [12] では90MクラスをQ8_0量子化、1クエリ実測約60ms(M2 Pro)、目標20〜30msと報告。zenz-v3.1-xsmallはAndroidアプリSumireでオフライン動作実績あり。モデル(zenz-v2.5: 26M/91M/310M)と学習データはCC BY-SA 4.0でHugging Face公開 [13] — **ライセンス互換なら自前学習せず流用・蒸留のベースにできる**。

**Yao et al. (2018)「Real-time Neural-based Input Method」** [5]: 1層LSTM(隠れ256、語彙5万)でBCCWJ変換 top-1 61.2%(trigram 55.6%)。incremental selective softmax でsoftmax計算を約2桁高速化し**CPUで3ms/キー**。5bit k-means量子化で**モデルサイズ92%削減**(逆算で約8〜10MB、フル精度約100MB想定)。「小型ニューラルはCPUリアルタイム+10MB級に収まる」ことの2018年時点の実証。

**Sarhangzadeh & Watanabe (Findings of ACL 2024)** [14]: 同時機械翻訳に着想を得たアラインメントベースの逐次デコーディングで、Transformer IMEの低遅延化(先読み不要・逐次確定)を提案。逐次変換UIを持つ場合の遅延対策として参照価値。

**PERT (Xiao et al. 2022)** [15]: ピンイン→漢字変換を双方向Transformerエンコーダ(GPT型生成でなく系列ラベリング的に)で解き、n-gramとMarkov枠組みで融合して更に改善。「生成せずスコアリング/ラベリングに徹する」設計の先行例。

**Rust推論の実現性**: llama.cpp のRustバインディング(`llama-cpp-2`等)または candle(GGUF/量子化モデル対応、Metal対応)で、GGUF化した小型モデルのローカル推論は確立済みのパス [16]。Zenzai自身がllama.cpp(C++)+Swiftで同じことをしているため、Rust側の技術リスクは低い。mmapロードなら15MBモデルのコールド起動への影響は数ms〜十数ms程度(推定)で100ms予算内。

**本製品への適合形態 = rescoring**: 生成方式(3a)は遅延予算を超えるが、本製品には既にN-best(acc@10 83.5%)がある。そこで、既存Viterbiが出したN-best候補を `左文脈+読み+候補` のスコア(対数尤度)で並べ替える **rescoring専用構成(3b)** にすれば:
- 自己回帰生成が不要で、候補文字列の**prefill(並列トークン評価)のみ**。26Mモデル・int4なら数百トークンのprefillはM系CPUで10〜20ms、Metal使用でさらに短縮(llama.cpp系の公開ベンチからの推定)。候補間で「文脈+読み」プレフィックスのKVキャッシュを共有すれば実効トークン数は候補差分のみ。
- 辞書外の文字列を出さない(入力忠実性が構造的に保証される)。Zenzaiが投機的デコーディングで苦労して得ている性質が自動的に手に入る。
- 期待効果: 上限は acc@10 の83.5%。生成方式でxsmallが66.5%を出していることから、**+10〜25pt(53.5→65〜78%)と推定**(rescoring形態での直接の公表数値はないため内挿推定。なお「mozcの変換結果をzenzで並び替える」実験記事 [17] が存在し、この構成の先行事例がある)。

### 3.4 その他の軽量手法

- **連語/複合語辞書の自動獲得**: Mozcの複合語化(3.1)が実証済み(BLEU +0.028〜0.039、Web辞書追加より効果大)[1]。Wikipediaから「名詞+名詞」「姓+名」等のパターンで高頻度列を抽出し1語化するだけで、既存エンジンに無改造で乗る。「次期皇帝」「複合姓」のような頻出コロケーション型の誤りには即効性があるが、汎化はしない。**Phase 2本命と独立に、先行して入れる価値あり(実装コスト最小)**。
- **キャッシュLM**: Kuhn & De Mori (1990) [18] が原型。Mozcの学習機能は「文節n-gramを文脈キーにした疑似キャッシュモデル + 交換可能制約」で、パラメータ本体を書き換えない設計(バージョン互換・副作用抑制のため)[1]。ユーザ体感には効くがAJIMEE-Bench的な初見文の同音異義語選択には効かない。
- **pointwise予測**: KyTea(Neubig et al. 2011)[19] 流の「その位置の判定だけを周辺文字素性で行う分類器」をかな漢字変換に転用する場合、頻出同音異義語ペア(皇帝/工程、高官/交換…)に限定した専用識別器群として実装するのが現実的。かな漢字変換タスクでの直接の公表数値はない(近接タスク: 日本語単語分割・読み推定で高精度)。対象を誤り分析上位に絞れば費用対効果は高いが、リストの保守が要る。
- **文脈付き変換の評価方法**: Zenzai/AJIMEE-Benchが確立した形式(左文脈フィールド付き評価データ、Acc@1と最小CER、複数正解許容)[9][11] をそのまま踏襲するのがよい。学習・調整にはJWTD v2 **train** から同形式のペアを生成し(AJIMEE-Benchはtest由来なので厳禁)、開発用dev分割もtrainから切る。

---

## 4. 推奨

### 第一候補: 小型文字レベルLM(10〜25Mパラメータ、int8/int4)による N-best rescoring

選定理由(再掲・要約): 残存誤りの型(同音異義語の文脈選択)に対する実証効果が桁違い(同系ベンチで26Mモデルが+13pt、91Mで+26pt [9])/ acc@10 83.5%のヘッドルームをrescoringで直接刈り取れる / prefill専用なら遅延予算内 / 辞書外生成の心配がない / zenz-v2.5(CC BY-SA 4.0)という公開モデル・公開データが存在し、ゼロから学習しなくても検証を始められる。

実装ステップ案:

1. **フィージビリティ検証(学習なし・1週間以内)**
   - zenz-v2.5-xsmall(26M, GGUF)を `llama-cpp-2` または candle で読み込み、既存エンジンのN-best 10候補を `左文脈+読み+候補` の対数尤度でリランキング。スコアは `既存Viterbiコスト × (1−λ) + ニューラル対数尤度 × λ` の対数線形補間。
   - JWTD v2 trainから作ったdevセットでλを調整し、遅延(p50/p95)とacc@1改善幅を実測。**この時点でp95 20msに収まるか、+10pt級が出るかを確認してからPhase 2本実装を判断**。ライセンス(CC BY-SA 4.0)が製品配布と両立するかは要確認。両立しないならこの検証結果を「自前学習のGo/No-Go判断材料」とする。
2. **データ準備**
   - Wikipedia日本語全文 + JWTD v2 train を、自前辞書/MeCabで読み推定し `(左文脈, 読み, 表記)` ペアを生成(Zenzaiと同じ手順 [9]。読み推定誤りはヒューリスティックでフィルタ)。目標数千万〜1億ペア。devはJWTD trainから分離。AJIMEE-Benchは最終評価1回のみ。
3. **学習**
   - 語彙6000程度の文字レベルトークナイザ + GPT-2型 10〜25Mパラメータ(rescoring専用なので生成品質より対数尤度の識別力を重視。候補ペアのマージン損失を混ぜる=識別的fine-tuningも検討)。llm.c/PyTorchで学習しGGUF/safetensorsへエクスポート。
   - 量子化: Q8_0→Q5_K→Q4_Kの順にdevでacc劣化を計測し、15MB以下に収まる最小劣化点を採用(Zenzaiの実測ではQ8_0で劣化なし [12]、Q5_K_Mで26M→19.9MB [9])。
4. **組み込み**
   - 変換パイプライン: Viterbi → N-best(10) → (文長・候補間コスト差が閾値以下のときだけ)ニューラルrescoring。短文・一意な変換ではスキップして平均遅延を抑える。文脈+読みプレフィックスのKVキャッシュを候補間で共有。モデルはmmapロード+遅延初期化でコールド起動100ms死守。
5. **評価**
   - devでλ・スキップ閾値・量子化を確定 → AJIMEE-Bench 200件で最終acc@1/acc@10/CER、M系実機でp50/p95遅延・メモリ・起動時間を計測。誤り分析で「rescoringで直った/直らない」を分類し、直らない分(候補外正解=16.5%)は辞書・複合語側の課題として切り分ける。

### 次点: 語彙化単語bigram(内容語中心) + Stolckeプルーニング + 量子化succinct trie

- Wikipediaから内容語bigramをカウント → 頻度カットオフ+Stolckeプルーニングで200〜300万エントリ → 8bit量子化コストをLOUDS/marisa系trieに格納(10〜15MB)。既存のパス score に `−log P(w_i|w_{i-1})` をバックオフ付きで加点するだけで、Viterbi/N-best機構は無改造。
- 期待 +2〜5pt(推定)、純Rust、学習は数時間、遅延影響ほぼゼロ。第一候補の検証が遅延・ライセンス・学習コストで頓挫した場合の保険であり、また第一候補と併用しても干渉しない(ニューラルrescoringのスキップ時カバーとして機能)。
- 併せて、実装コスト最小の **複合語自動獲得(Mozc方式)** を先行投入することを推奨(3.4参照)。

---

## 5. 参考文献

1. 工藤拓, 小松弘幸, 花岡俊行, 向井淳, 田畑悠介. 統計的かな漢字変換システムMozc. 言語処理学会第17回年次大会 (NLP2011). https://www.anlp.jp/proceedings/annual_meeting/2011/pdf_dir/C4-3.pdf
2. google/mozc リポジトリ(LOUDS辞書・接続行列実装; DeepWiki照会). https://github.com/google/mozc / https://deepwiki.com/google/mozc
3. A. Stolcke. Entropy-based Pruning of Backoff Language Models. 1998/2000. https://arxiv.org/abs/cs/0006025
4. K. Heafield. KenLM: Faster and Smaller Language Model Queries. WMT 2011. https://aclanthology.org/W11-2123/
5. J. Yao, R. Shu, X. Li, K. Ohtsuki, H. Nakayama. Real-time Neural-based Input Method. arXiv:1810.09309, 2018. https://arxiv.org/abs/1810.09309
6. H. Tokunaga, D. Okanohara, S. Mori. Discriminative Method for Japanese Kana-Kanji Input Method. WTIM 2011 (IJCNLP Workshop). https://aclanthology.org/W11-3502/
7. Preferred Networks技術ブログ. 日本語かな漢字変換における識別モデルの適用とその考察について (NLP2011). https://tech.preferred.jp/ja/blog/nlp2011-inputmethod-structuredsvm/
8. J. Gao, H. Suzuki, B. Yu. Approximation Lasso Methods for Language Modeling. ACL 2006(識別LMによるIME候補リランキング/ドメイン適応). https://aclanthology.org/P06-1029/
9. 三輪敬太, 高橋直希. ニューラルかな漢字変換システムZenzai. 言語処理学会第31回年次大会 (NLP2025), P1-19. https://www.anlp.jp/proceedings/annual_meeting/2025/pdf_dir/P1-19.pdf
10. M. Tan et al. Exploring and Adapting Chinese GPT to Pinyin Input Method. ACL 2022. https://aclanthology.org/2022.acl-long.133/
11. azooKey/AJIMEE-Bench (JWTD v2ベースのIME評価ベンチマーク). https://github.com/azooKey/AJIMEE-Bench
12. Miwa. ニューラルかな漢字変換エンジン「Zenzai」をazooKey on macOSに搭載します. Zenn. https://zenn.dev/azookey/articles/ea15bacf81521e (関連: さくらのナレッジ https://knowledge.sakura.ad.jp/42901/ , AzooKeyKanaKanjiConverter Zenzaiドキュメント https://github.com/azooKey/AzooKeyKanaKanjiConverter/blob/main/Docs/zenzai.md )
13. zenz-v2.5 モデル/データセット (CC BY-SA 4.0). https://huggingface.co/Miwa-Keita/zenz-v2.5-medium / https://huggingface.co/datasets/Miwa-Keita/zenz-v2.5-dataset
14. A. Sarhangzadeh, T. Watanabe. Alignment-Based Decoding Policy for Low-Latency and Anticipation-Free Neural Japanese Input Method Editors. Findings of ACL 2024. https://aclanthology.org/2024.findings-acl.479/
15. J. Xiao et al. PERT: A New Solution to Pinyin to Character Conversion Task. arXiv:2205.11737, 2022. https://arxiv.org/abs/2205.11737
16. llama.cpp Rustバインディング/candle. https://github.com/ggml-org/llama.cpp / https://github.com/utilityai/llama-cpp-rs / https://github.com/huggingface/candle
17. mozcの変換結果をzenzで並び替えてみる. Zenn. https://zenn.dev/4g/articles/4ac53b947d2fb3
18. R. Kuhn, R. De Mori. A Cache-Based Natural Language Model for Speech Recognition. IEEE TPAMI, 1990. https://ieeexplore.ieee.org/document/56193
19. G. Neubig, Y. Nakata, S. Mori. Pointwise Prediction for Robust, Adaptable Japanese Morphological Analysis (KyTea). ACL 2011. https://aclanthology.org/P11-2093/
20. 森信介, 土屋雅稔, 山地治, 長尾真. 確率的モデルによる仮名漢字変換. 情報処理学会論文誌 40(7), 1999(統計的かな漢字変換の原典). https://ipsj.ixsq.nii.ac.jp/records/13136
21. Y. Tanaka, Y. Murawaki, D. Kawahara, S. Kurohashi. Building a Japanese Typo Dataset from Wikipedia's Revision History (JWTD). ACL 2020 SRW. https://aclanthology.org/2020.acl-srw.31/

※ 数値のうち「推定」と明記したもの(語彙化bigramの+2〜5pt、rescoringの+10〜25pt、prefill遅延、量子化後サイズの一部)は、かな漢字変換タスクでの直接の公表値がないため近接タスク・同系実験からの内挿である。それ以外の数値はすべて上記出典の本文から採録した。
