# Windows consumer verification

この手順は、署名検査やsilent installだけでは証明できない対話入力、候補UI、検索統合、accessibility、設定反映を確認するために使う。自動試験の代替ではなく、自動ライフサイクルgateを完走した同じreleaseをcleanな対話sessionへ再installした後に実行する。

## 前提

- 使い捨てのclean Windows 11 VMで、通常のconsumer accountへloginしている
- `scripts/test-windows-install-lifecycle.ps1`が同じ署名済みreleaseで成功している
- そのreleaseを再installし、入力sourceとして選択できる
- x64 VMにはx64/x86 payload、native ARM64 VMにはARM64X/x86 payloadを使う
- screen reader、検索欄、通常のdesktop text fieldを利用できる
- `SLIME_RELEASE_TEST_VM=1`を設定する

旧版からのupdateを検証するとき、lifecycle gateは旧installerも公開信頼chain、RSA、EKU、timestamp、承認済みsignerに固定する。通常は現行releaseと同じthumbprintを要求し、証明書更新をまたぐ場合だけ`-PreviousExpectedSignerThumbprint`で商用側が承認した旧thumbprintを明示する。単に有効な別publisherの署名を持つinstallerは旧版証拠として受け入れない。

gateは実証明書のcode-signing EKU、2048-bit以上のRSA公開鍵、timestamp EKU、signer thumbprint、製品version、source revision、installer SHA-256を検証する。`Get-AuthenticodeSignature`が公開信頼chainを`Valid`と判定しないartifactも拒否する。配布前payloadとinstall後payloadの全ファイルhashを比較し、内包uninstallerを含むinstalled manifestも固定する。途中でいずれかが変わった場合、記録済みstepを合格証拠にしない。秘密鍵がhardware-backed署名サービスからexportされていないことはartifact単体から証明できないため、鍵custodyと各署名要求の監査logを商用側のrelease evidenceとして別に固定する。

## 準備

release固有の値を一度だけsplatへまとめる。ARM64 VMでは`PayloadX64`を`PayloadARM64X`へ置き換える。

```powershell
$env:SLIME_RELEASE_TEST_VM = "1"
$release = @{
  Version = "0.1.0"
  PayloadX64 = "C:\release\x64"
  PayloadX86 = "C:\release\x86"
  Installer = "C:\release\Slime-0.1.0-windows.exe"
  ExpectedSignerThumbprint = "0123456789ABCDEF0123456789ABCDEF01234567"
  ExpectedSourceRevision = "0123456789abcdef0123456789abcdef01234567"
}

scripts/windows-consumer-input-gate.ps1 -Action Prepare @release
```

表示されたverification tokenは、このsessionだけで使う。stateは2時間で失効し、prepare時のuser SID、Windows session ID、boot時刻へ固定される。別user、再login、再起動後はartifactが同一でもrecord/checkを継続できない。

## 対話確認

次の順番を変えず、各項目を実際に観察した直後だけ`Record`する。失敗した場合は記録せず、`-Action Cancel`でsessionを破棄する。

1. `download-reputation`: ブラウザから取得してMark-of-the-Webが付いた同一installerをExplorerから開き、SmartScreenの警告・block有無、表示publisher、実行可能になるまでの操作を記録する。警告がないことを署名種別から推定しない。installer SHA-256がgateの値と一致しない場合は記録しない。
2. `desktop-input`: 通常のdesktop text fieldで逐次入力、Space変換、Enter確定、Escape、Backspaceを確認する。表示中preeditと確定文字列が二重化・欠落しないことも確認する。署名済みcontext packの固定caseを使い、既存文書の文脈末尾へcaretを移動してから入力した場合にだけ期待候補が昇格し、別位置へ移動すると以前の文脈を再利用しないことも確認する。
3. `candidate-interaction`: 候補をkeyboardの上下、番号、mouseでそれぞれ選択し、選択行と確定結果が一致することを確認する。初回候補末尾から追加された拡張候補と、訂正注記を持つ候補も各1件選び、注記は候補UIだけに残り、preeditと確定文書には選択した表記だけが入ることを確認する。
4. `search-integration`: Windowsの検索欄で読みを入力し、検索候補取得がcompositionを勝手に確定・変更せず、選択結果だけが反映されることを確認する。
5. `accessibility`: screen readerを有効にし、候補windowの表示・候補名・選択変更・閉じる操作が通知され、keyboardだけで候補を確定できることを確認する。由来付き候補では確定値と「履歴」「訂正」などの説明が読み上げられ、説明が確定文書へ入らないことも確認する。
6. `settings-reload`: serviceが動作中のまま設定を変更し、次のcompositionから反映されること、別bitnessのdesktop text fieldでも同じ設定になることを確認する。

```powershell
$token = "prepareで表示されたtoken"
scripts/windows-consumer-input-gate.ps1 -Action Record -Step download-reputation -Token $token @release
scripts/windows-consumer-input-gate.ps1 -Action Record -Step desktop-input -Token $token @release
scripts/windows-consumer-input-gate.ps1 -Action Record -Step candidate-interaction -Token $token @release
scripts/windows-consumer-input-gate.ps1 -Action Record -Step search-integration -Token $token @release
scripts/windows-consumer-input-gate.ps1 -Action Record -Step accessibility -Token $token @release
scripts/windows-consumer-input-gate.ps1 -Action Record -Step settings-reload -Token $token @release
scripts/windows-consumer-input-gate.ps1 -Action Check @release
```

`Check`は全step、現在の署名、配布前後のpayload、installed manifestを再検証し、architecture、version、source revision、signer thumbprint、installer SHA-256、installed manifest SHA-256を出力する。自動ライフサイクルgateも同じ6項目を出力し、same-version再実行とdowngrade拒否後までmanifest・ユーザーデータが不変であることを確認する。両gateが出す`Release binding JSON`は同じfield順序のcanonical JSONなので、文字列が完全一致したものだけを商用側のrelease evidenceへ保存する。stateには入力内容を保存しない。

このgateの成功は、記録したVM architectureと操作項目に対するconsumer evidenceである。別architecture、別release、別app互換性、download reputationを推定して合格扱いにしない。

## 証拠の最終照合

x64とnative ARM64の各VMで、署名済みlifecycle gateと対話gateの成功出力を次の固定名で商用側の一時ディレクトリへ保存する。

- `x64-lifecycle.log`
- `x64-interactive.log`
- `arm64x-lifecycle.log`
- `arm64x-interactive.log`

4件が揃った後、`scripts/verify-windows-consumer-evidence.ps1 -EvidenceDirectory EVIDENCE_DIRECTORY`を実行する。各ファイルにlifecycleまたはinteractive gate固有の成功markerとcanonical bindingがそれぞれちょうど1件あること、同じarchitectureのlifecycleとinteractive JSONがbyte単位で一致すること、x64とARM64Xのversion、source revision、signerが一致することを要求する。architectureごとにinstallerとinstalled manifestは異なってよい。bindingだけをコピーしたログや、別gateの成功markerを持つログは拒否する。実入力内容、候補表層、秘密鍵、署名サービス設定は証拠ディレクトリへ保存しない。この集約gateは署名済みartifactの検証やVM上の目視操作を代替しない。
