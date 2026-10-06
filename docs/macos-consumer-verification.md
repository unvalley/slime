# macOS consumer verification

Slimeの初回商用版はmacOSだけを対象にする。Developer ID署名・公証済みのversion付きPKGを正式な配布物とし、更新は新しいPKGを手動で実行する。基本入力は契約、ネットワーク、更新可否に依存させない。

この手順は、ソーステストや合成入力では証明できないInputMethodKit、配布、consumer sessionの境界を確認するために使う。使い捨てVMまたは出荷用に初期化したmacOSで、配布予定のPKGそのものを検証する。

物理入力gateは、macOSのconsoleがunlockされた対話sessionでだけ準備できる。新旧両方のconsole lock propertyを判定し、状態を特定できない場合も安全側で失敗する。画面ロック中はtokenやstateを作らず、unlock後に改めてprepareから開始する。TextEdit、再変換、private/secure-inputの各stateはprepareから2時間で失効し、期限切れ時はpending tokenも無効化する。対象processはPIDだけでなくprepare時の起動時刻と実行ファイルから作るSHA-256へbindし、同じPIDが再利用された場合や対象アプリが再起動された場合もcheckを拒否する。このfingerprintやPIDはUnified Logへ記録しない。

## 事前条件

- `scripts/verify-macos-release.sh` が配布予定PKGに対して成功している
- macOSの新規consumerアカウントで開始する
- Slimeが未インストールで、入力ソース一覧にも残っていない
- ネットワークを切っても、インストール済みSlimeで基本入力できる

## clean install

1. PKGをFinderから開き、標準Installerでインストールする。
2. `~/Library/Input Methods` ではなく `/Library/Input Methods/Slime.app` にだけ配置されたことを確認する。
3. ログアウトとログインを行い、入力ソースでSlimeを選ぶ。
4. TextEditでキーを1文字ずつ押し、`nihon` の未確定文字列が更新されることを確認する。
5. Spaceで候補を開き、矢印、番号、クリックで候補を選択し、Enterで「日本」を確定する。
6. Escape、Backspace、左右移動、文節移動、再変換がクラッシュせず、未確定文字列と候補表示が一致することを確認する。
7. private modeでは履歴を読み書きせず、通常モードへ戻すと既存履歴が利用できることを確認する。
8. secure event input中は履歴を読み書きせず、解除後は通常の学習が再開することを確認する。secure event inputは候補表示を禁止するAPIではないため、候補が表示されないこと自体は合格条件にしない。
9. TextEditを終了して再起動し、通常モードで確定したユーザー辞書・履歴・左文脈が再読込されることを確認する。

実キーがInputMethodKitの`handle`へ届いたことは、入力内容を記録しない一回限りのmarkerでも確認する。配布予定PKGを`/Library/Input Methods`へインストールし、`~/Library/Input Methods/Slime.app`が存在しないことを確認する。PKGを環境変数へ固定してSlimeを選択し、TextEditの編集領域を前面にした状態で次を実行する。

```sh
export SLIME_RELEASE_PACKAGE="$PWD/target/package/Slime-VERSION.BUILD_NUMBER-macos.pkg"
just macos-textedit-input-gate prepare
# TextEditで物理A-Zキーを1回押す（Command/Control/Optionなし）
just macos-textedit-input-gate check
```

prepareはPKGのDeveloper ID署名、公証ticket、Gatekeeperを再検証し、PKGから展開したbundleと実際にインストールされたsystem bundleの全ファイルmanifestが一致しなければ失敗する。checkもprepare時のbundle manifest、version、build番号、source revisionを再検証する。成功時はversion、build番号、source revision、PKG SHA-256、bundle manifest SHA-256を同じ順序の`Release binding JSON`として表示する。作業ディレクトリの別build、ユーザー領域のbundle、prepare後に置換されたbundleは合格証拠にしない。

prepare時に起動中のTextEdit process IDも一時的に固定し、そのprocessが前面にある間にHID system sourceから届いた修飾なしのA-Z文字イベントだけを対象にする。ランダムUUIDをUnified Logへ1回記録し、入力文字、key code、文書内容、process IDはログへ記録しない。別アプリ、再起動後の別process、入力ソース切替、Space、矢印、修飾付きcommand、private/combined event sourceからの合成入力ではtokenを消費しない。中止する場合は`just macos-textedit-input-gate cancel`でmarkerを削除する。

候補表示から確定までの経路は別の一回限りmarkerで確認する。空のTextEdit文書で次を実行する。

```sh
just macos-textedit-input-gate prepare-candidates
# TextEditで物理キーから nihon、Space、Down Arrow、Enter の順に入力する
just macos-textedit-input-gate check-candidates
```

候補markerは、HID system sourceの修飾なしA-Z文字が届いた後の`candidate-shown`、物理的な矢印・番号・mouse event、`candidate-moved`、物理的な確定操作に続く`candidate-committed`だけを記録する。入力文字、候補表層、選択番号、key code、文書内容は記録しない。候補を表示しただけ、合成eventで操作した場合、選択経路へ入らずに選択が変わった場合、候補を移動せずに確定した場合は合格にしない。再変換とprivacy gateも、候補要求・確定に必要なkeyboard eventがHID system sourceでなければ完了しない。

### 候補操作

番号とクリックも、操作種別だけを追加で固定した一回限りmarkerを使う。同じインストール済みbundleと空のTextEdit文書で実行する。

```sh
just macos-textedit-input-gate prepare-number
# 物理キーから nihon、Space、2 の順に入力する
just macos-textedit-input-gate check-number

just macos-textedit-input-gate prepare-click
# 物理キーから nihon、Space の順に入力し、2行目をクリックする
just macos-textedit-input-gate check-click

just macos-textedit-input-gate prepare-reconversion
# 確定済みの日本語1語を選択し、Control+Shift+R、Enter の順に物理入力する
just macos-textedit-input-gate check-reconversion
```

番号gateは`candidate-number`、クリックgateは`candidate-click`を必要とし、どちらも実際の選択index変更と確定が続かなければ完了しない。再変換gateは物理ショートカット要求、選択範囲からの再変換開始、候補表示、確定をすべて要求する。候補番号、表層、選択文字列は記録しない。完走後、同じインストール済みbundleをTextEditで目視確認する。

1. 番号選択後に選んだ候補が確定され、未確定文字列が残らないことを確認する。
2. クリック後もTextEditが入力先であり、未確定文字列が残らないことを確認する。
3. 確定済みの「日本」だけを選択し、Control+Shift+Rで再変換する。選択範囲だけが未確定文字列になり、前後の文字を置換しないことを確認する。
4. 初回候補末尾から追加された拡張候補と、訂正注記を持つ候補も各1件選ぶ。選択行と確定結果が一致し、訂正注記は候補UIだけに残ってpreeditや文書へ入らないことを確認する。

番号、クリック、再変換は別経路なので、それぞれ1回以上の目視確認を必要とする。markerは経路の通過を証明するが、画面上の候補と確定結果の一致や、再変換で選択範囲外を変更していないことまでは証明しない。クラッシュ、候補と未確定文字列の不一致、選択範囲外の置換があれば不合格とする。

### private/secure input

履歴内容を診断ログやstateへ複製せず、`history.tsv`と`context_history.tsv`のSHA-256だけを比較する。先に設定した`SLIME_RELEASE_PACKAGE`はprivacy gateでも使い、同じPKG、system bundle manifest、version、build番号へbindする。最初に設定で履歴学習を有効にする。通常モードで曖昧な読みの先頭以外の候補を5回以上確定し、その候補が学習によって先頭になることを確認しておく。private/secure中だけその順位付けが使われず、解除後に再び使われることを目視確認する。これを履歴のread境界、ファイルハッシュの不変をwrite境界の証拠とする。

private modeはTextEditで次を行う。prepare時に起動中のTextEdit process IDをstateとInputMethodKit側のrequestへ固定し、そのprocessが前面にある間の物理入力だけを受け入れる。

```sh
just macos-input-privacy-gate prepare-private
# Slimeを選択し、メニューバーからprivate modeを有効にする
# 学習済み順位が使われないことを確認し、物理キーでnihon、Space、Enterを入力する
just macos-input-privacy-gate check-private
# check完了後にprivate modeを解除する
```

secure event inputは、それを明示的に有効化できるホストアプリの入力領域で行う。有効化したまま`prepare-secure`から`check-secure`までを完走する。gateは`IsSecureEventInputEnabled`を開始時と終了時に確認し、prepare時の前面process IDへ物理入力を固定する。prepareを実行するshell自体が前面へ移る環境では、secure-input hostのPIDを`SLIME_PRIVACY_TARGET_PROCESS_ID`へ明示してから実行する。PIDが存在しない場合やcheck前にhostが終了した場合は不合格とする。

```sh
export SLIME_PRIVACY_TARGET_PROCESS_ID="SECURE_INPUT_HOST_PID" # 必要な場合だけ
just macos-input-privacy-gate prepare-secure
# Slimeを選択し、学習済み順位が使われないことを確認する
# secure event inputを保ったまま、物理キーでnihon、Space、Enterを入力する
just macos-input-privacy-gate check-secure
```

最後にsecure event inputとprivate modeの両方を解除し、通常学習の再開を別gateで確認する。

```sh
just macos-input-privacy-gate prepare-resume
# Slimeを選択し、先に学習した順位が再び使われることを確認して通常変換を1回確定する
# 物理キーでnihon、Space、Enterを入力する
just macos-input-privacy-gate check-resume
```

各gateは、期待するprivate/secure状態が続いている間の物理A-Z入力、候補表示、確定を一回限りmarkerで要求する。markerはmode、段階名、ランダムUUIDだけを記録し、入力文字、候補、文書内容を記録しない。private/secure中は両ファイルがbyte単位で不変、解除後は`history.tsv`が変化すれば合格とする。別の入力ソースで何も操作しなかった場合や、候補を確定しなかった場合は合格にしない。既存履歴を消去したり、入力内容をgateへ渡したりしない。中止時は`just macos-input-privacy-gate cancel`を使う。

### 証拠の最終照合

各checkの標準出力を商用側の一時ディレクトリへ保存し、次の固定名を使う。

- `release-verifier.log`
- `package-lifecycle.log`
- `textedit-character.log`
- `textedit-candidates.log`
- `textedit-number.log`
- `textedit-click.log`
- `textedit-reconversion.log`
- `privacy-private.log`
- `privacy-secure.log`
- `privacy-resume.log`

すべて揃った後に`./scripts/verify-macos-consumer-evidence.sh EVIDENCE_DIRECTORY`を実行する。各ファイルに、そのgate固有の成功markerとcanonical `Release binding JSON`がそれぞれちょうど1件あり、10件のbindingがbyte単位で一致する場合だけ成功する。bindingだけを別ログへコピーしたファイルや、別gateの成功markerを持つファイルは拒否する。入力内容、候補表層、署名資格情報をこのディレクトリへ保存する必要はない。最終照合に成功しても目視確認、署名・公証、実機操作そのものの代替にはしない。

## manual update

1. 新版の`CFBundleVersion`が旧版より大きく、製品versionが同じか新しいことを確認する。
2. 旧版でユーザー辞書、通常履歴、左文脈履歴を作り、それぞれの確認用データを記録する。
3. 新版PKGをFinderから実行する。
4. `/Library/Input Methods/Slime.app` が新版へ完全に置換され、旧版のファイルやsymlinkが残っていないことを確認する。
5. ログアウトとログインの後、TextEditで新版が選択・入力できることを確認する。
6. 旧版で作ったユーザー辞書と履歴が保持され、破損や重複がないことを確認する。
7. 新版のinstall後に旧版PKGを実行し、installerが拒否または何も変更せず、bundle、PKG receipt、ユーザーデータが新版のままであることを確認する。
8. 同じ新版PKGをもう一度実行し、bundle manifest、PKG receipt、ユーザーデータが変わらないことを確認する。

## uninstall

1. `scripts/uninstall-macos-system.sh` と同じ削除対象を使う商用側のアンインストール手順を実行する。
2. ログアウトとログインの後、Slimeが入力ソース一覧から消え、`/Library/Input Methods/Slime.app` が存在しないことを確認する。
3. 他の入力ソースや無関係なInput Methodが削除されていないことを確認する。
4. ユーザー辞書と履歴が保持されていることを確認する。ユーザーデータ削除は別の明示操作として扱う。

## 合格条件

- 署名・公証済みの配布予定PKGそのものがclean install、manual update、downgrade防止、uninstallを完走する
- release verifier、VM lifecycle、TextEdit、private/secure-inputの成功出力がbyte単位で同じ`Release binding JSON`を示す
- 物理キー入力がInputMethodKitを通り、TextEditで候補選択まで成功する
- ログインし直したconsumer sessionでも入力ソースの追加・更新・削除が正しく反映される
- ユーザーデータ保持とprivate/secure inputの境界に逸脱がない
- unsigned、ad-hoc署名、ソース直起動、アクセシビリティ経由の合成入力は、この合格証拠として扱わない
