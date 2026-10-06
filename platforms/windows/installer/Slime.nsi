Unicode true

!include "LogicLib.nsh"
!include "MUI2.nsh"
!include "WordFunc.nsh"
!include "x64.nsh"
!insertmacro VersionCompare

!ifndef VERSION
  !error "VERSION is required"
!endif
!ifndef VERSION_QUAD
  !error "VERSION_QUAD is required"
!endif
!ifndef SOURCE_REVISION
  !error "SOURCE_REVISION is required"
!endif
!ifndef PAYLOAD_X86
  !error "PAYLOAD_X86 is required"
!endif
!ifndef OUTPUT
  !error "OUTPUT is required"
!endif

!ifdef ARM64_INSTALLER
  !ifndef PAYLOAD_ARM64X
    !error "PAYLOAD_ARM64X is required for ARM64_INSTALLER"
  !endif
  !define PAYLOAD_NATIVE "${PAYLOAD_ARM64X}"
  !define NATIVE_DIRECTORY "arm64x"
  !define NATIVE_LABEL "ARM64"
!else
  !ifndef PAYLOAD_X64
    !error "PAYLOAD_X64 is required"
  !endif
  !define PAYLOAD_NATIVE "${PAYLOAD_X64}"
  !define NATIVE_DIRECTORY "x64"
  !define NATIVE_LABEL "64-bit"
!endif

!ifdef UNINSTALLER_SIGNER
  !uninstfinalize '"${UNINSTALLER_SIGNER}" "%1"' = 0
!endif

!define PRODUCT_NAME "Slime"
!define PRODUCT_PUBLISHER "unvalley"
!define PRODUCT_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\SlimeIME"

Name "${PRODUCT_NAME} ${VERSION}"
OutFile "${OUTPUT}"
InstallDir "$PROGRAMFILES64\Slime\${VERSION}"
RequestExecutionLevel admin
SetCompressor zlib
ShowInstDetails show
ShowUninstDetails show
SilentInstall normal
SilentUnInstall normal

VIProductVersion "${VERSION_QUAD}"
VIAddVersionKey /LANG=1041 "ProductName" "${PRODUCT_NAME}"
VIAddVersionKey /LANG=1041 "CompanyName" "${PRODUCT_PUBLISHER}"
VIAddVersionKey /LANG=1041 "FileDescription" "Slime Japanese IME installer"
VIAddVersionKey /LANG=1041 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=1041 "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=1041 "Comments" "Source revision: ${SOURCE_REVISION}"
VIAddVersionKey /LANG=1041 "LegalCopyright" "Copyright (c) 2026 unvalley"

!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${NATIVE_DIRECTORY}\SlimeSettings.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Slimeの設定を開く"
!define MUI_FINISHPAGE_RUN_NOTCHECKED
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "Japanese"

Var OldInstallDir
Var OldVersion
Var ResultCode

!macro RemovePayload Root
  SetOutPath "$TEMP"
  Delete /REBOOTOK "${Root}\${NATIVE_DIRECTORY}\SlimeIME.dll"
  Delete /REBOOTOK "${Root}\${NATIVE_DIRECTORY}\slime_ffi.dll"
  Delete /REBOOTOK "${Root}\${NATIVE_DIRECTORY}\SlimeIMERegister.exe"
  Delete /REBOOTOK "${Root}\${NATIVE_DIRECTORY}\SlimeSettings.exe"
!ifdef ARM64_INSTALLER
  Delete /REBOOTOK "${Root}\${NATIVE_DIRECTORY}\SlimeIME_arm64.dll"
  Delete /REBOOTOK "${Root}\${NATIVE_DIRECTORY}\SlimeIME_x64.dll"
  Delete /REBOOTOK "${Root}\${NATIVE_DIRECTORY}\slime_ffi_arm64.dll"
  Delete /REBOOTOK "${Root}\${NATIVE_DIRECTORY}\slime_ffi_x64.dll"
!endif
  Delete /REBOOTOK "${Root}\x86\SlimeIME.dll"
  Delete /REBOOTOK "${Root}\x86\slime_ffi.dll"
  Delete /REBOOTOK "${Root}\x86\SlimeIMERegister.exe"
  Delete /REBOOTOK "${Root}\x86\SlimeSettings.exe"
  Delete /REBOOTOK "${Root}\Uninstall.exe"
  RMDir /REBOOTOK "${Root}\${NATIVE_DIRECTORY}"
!ifdef ARM64_INSTALLER
  ; Builds before native ARM64 support used x64 as their native directory.
  ; Remove that product-owned payload when an ARM64 installer supersedes it.
  Delete /REBOOTOK "${Root}\x64\SlimeIME.dll"
  Delete /REBOOTOK "${Root}\x64\slime_ffi.dll"
  Delete /REBOOTOK "${Root}\x64\SlimeIMERegister.exe"
  Delete /REBOOTOK "${Root}\x64\SlimeSettings.exe"
  RMDir /REBOOTOK "${Root}\x64"
!endif
  RMDir /REBOOTOK "${Root}\x86"
  RMDir /REBOOTOK "${Root}"
!macroend

Function .onInit
!ifdef ARM64_INSTALLER
  ${IfNot} ${IsNativeARM64}
    MessageBox MB_OK|MB_ICONSTOP \
      "このinstallerはARM64版Windows 11専用です。" /SD IDOK
    SetErrorLevel 1633
    Quit
  ${EndIf}
!else
  ${IfNot} ${IsNativeAMD64}
    MessageBox MB_OK|MB_ICONSTOP \
      "このinstallerはx64版Windows 10以降専用です。" /SD IDOK
    SetErrorLevel 1633
    Quit
  ${EndIf}
!endif
  SetShellVarContext all
  SetRegView 64
  ReadRegStr $OldInstallDir HKLM "${PRODUCT_KEY}" "InstallLocation"
  ReadRegStr $OldVersion HKLM "${PRODUCT_KEY}" "DisplayVersion"
  ${If} $OldInstallDir != ""
  ${AndIf} $OldVersion == ""
    MessageBox MB_OK|MB_ICONSTOP \
      "既存のSlimeのversionを確認できないため、インストールを中止しました。" /SD IDOK
    SetErrorLevel 1603
    Quit
  ${EndIf}
  ${If} $OldVersion != ""
    ${VersionCompare} "${VERSION}" "$OldVersion" $ResultCode
  ${Else}
    StrCpy $ResultCode 1
  ${EndIf}
  ${If} $ResultCode == 2
    MessageBox MB_OK|MB_ICONSTOP \
      "新しいSlime $OldVersionがインストール済みのため、${VERSION}へのdowngradeを拒否しました。" /SD IDOK
    SetErrorLevel 1638
    Quit
  ${EndIf}
  ${If} $ResultCode == 0
    MessageBox MB_OK|MB_ICONINFORMATION \
      "Slime ${VERSION}はすでにインストールされています。" /SD IDOK
    SetErrorLevel 0
    Quit
  ${EndIf}
FunctionEnd

Function RestorePreviousRegistration
  ${If} $OldInstallDir == ""
    Return
  ${EndIf}
  IfFileExists "$OldInstallDir\${NATIVE_DIRECTORY}\SlimeIMERegister.exe" 0 native_current_missing
    ExecWait '"$OldInstallDir\${NATIVE_DIRECTORY}\SlimeIMERegister.exe" install "$OldInstallDir\${NATIVE_DIRECTORY}\SlimeIME.dll"'
    Goto native_restored
  native_current_missing:
!ifdef ARM64_INSTALLER
  IfFileExists "$OldInstallDir\x64\SlimeIMERegister.exe" 0 native_restored
    ExecWait '"$OldInstallDir\x64\SlimeIMERegister.exe" install "$OldInstallDir\x64\SlimeIME.dll"'
!endif
  native_restored:
  IfFileExists "$OldInstallDir\x86\SlimeIMERegister.exe" 0 +2
    ExecWait '"$OldInstallDir\x86\SlimeIMERegister.exe" install "$OldInstallDir\x86\SlimeIME.dll"'
FunctionEnd

!macro FailInstall Message
  Call RestorePreviousRegistration
  !insertmacro RemovePayload "$INSTDIR"
  MessageBox MB_OK|MB_ICONSTOP "${Message}" /SD IDOK
  SetErrorLevel $ResultCode
  Quit
!macroend

Section "Slime" MainSection
  SectionIn RO
  SetShellVarContext all
  SetRegView 64

  SetOutPath "$INSTDIR\${NATIVE_DIRECTORY}"
  File /oname=SlimeIME.dll "${PAYLOAD_NATIVE}\SlimeIME.dll"
  File /oname=slime_ffi.dll "${PAYLOAD_NATIVE}\slime_ffi.dll"
  File /oname=SlimeIMERegister.exe "${PAYLOAD_NATIVE}\SlimeIMERegister.exe"
  File /oname=SlimeSettings.exe "${PAYLOAD_NATIVE}\SlimeSettings.exe"
!ifdef ARM64_INSTALLER
  File /oname=SlimeIME_arm64.dll "${PAYLOAD_NATIVE}\SlimeIME_arm64.dll"
  File /oname=SlimeIME_x64.dll "${PAYLOAD_NATIVE}\SlimeIME_x64.dll"
  File /oname=slime_ffi_arm64.dll "${PAYLOAD_NATIVE}\slime_ffi_arm64.dll"
  File /oname=slime_ffi_x64.dll "${PAYLOAD_NATIVE}\slime_ffi_x64.dll"
!endif

  SetOutPath "$INSTDIR\x86"
  File /oname=SlimeIME.dll "${PAYLOAD_X86}\SlimeIME.dll"
  File /oname=slime_ffi.dll "${PAYLOAD_X86}\slime_ffi.dll"
  File /oname=SlimeIMERegister.exe "${PAYLOAD_X86}\SlimeIMERegister.exe"
  File /oname=SlimeSettings.exe "${PAYLOAD_X86}\SlimeSettings.exe"

  ExecWait '"$INSTDIR\${NATIVE_DIRECTORY}\SlimeIMERegister.exe" install "$INSTDIR\${NATIVE_DIRECTORY}\SlimeIME.dll"' $ResultCode
  ${If} $ResultCode != 0
    !insertmacro FailInstall \
      "${NATIVE_LABEL}版Slimeの登録に失敗しました。インストールはロールバックされます。"
  ${EndIf}

  ExecWait '"$INSTDIR\x86\SlimeIMERegister.exe" install "$INSTDIR\x86\SlimeIME.dll"' $ResultCode
  ${If} $ResultCode != 0
    ExecWait '"$INSTDIR\${NATIVE_DIRECTORY}\SlimeIMERegister.exe" uninstall "$INSTDIR\${NATIVE_DIRECTORY}\SlimeIME.dll"'
    !insertmacro FailInstall \
      "32-bitアプリ用Slimeの登録に失敗しました。インストールはロールバックされます。"
  ${EndIf}

  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\Slime"
  CreateShortcut "$SMPROGRAMS\Slime\Slime 設定.lnk" \
                 "$INSTDIR\${NATIVE_DIRECTORY}\SlimeSettings.exe"
  CreateShortcut "$SMPROGRAMS\Slime\Slime のアンインストール.lnk" \
                 "$INSTDIR\Uninstall.exe"

  WriteRegStr HKLM "${PRODUCT_KEY}" "DisplayName" "Slime Japanese IME"
  WriteRegStr HKLM "${PRODUCT_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${PRODUCT_KEY}" "SourceRevision" "${SOURCE_REVISION}"
  WriteRegStr HKLM "${PRODUCT_KEY}" "Publisher" "${PRODUCT_PUBLISHER}"
  WriteRegStr HKLM "${PRODUCT_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${PRODUCT_KEY}" "DisplayIcon" \
              "$INSTDIR\${NATIVE_DIRECTORY}\SlimeSettings.exe"
  WriteRegStr HKLM "${PRODUCT_KEY}" "UninstallString" \
              "$\"$INSTDIR\Uninstall.exe$\""
  WriteRegStr HKLM "${PRODUCT_KEY}" "QuietUninstallString" \
              "$\"$INSTDIR\Uninstall.exe$\" /S"
  WriteRegDWORD HKLM "${PRODUCT_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${PRODUCT_KEY}" "NoRepair" 1

  ${If} $OldInstallDir != ""
  ${AndIf} $OldInstallDir != $INSTDIR
    !insertmacro RemovePayload "$OldInstallDir"
  ${EndIf}
SectionEnd

Function un.onInit
  SetShellVarContext all
  SetRegView 64
FunctionEnd

Section "Uninstall"
  SetShellVarContext all
  SetRegView 64
  ReadRegStr $0 HKLM "${PRODUCT_KEY}" "InstallLocation"
  ${If} $0 != $INSTDIR
    !insertmacro RemovePayload "$INSTDIR"
    SetErrorLevel 0
    Quit
  ${EndIf}

  ExecWait '"$INSTDIR\x86\SlimeIMERegister.exe" uninstall "$INSTDIR\x86\SlimeIME.dll"' $ResultCode
  ${If} $ResultCode != 0
    MessageBox MB_OK|MB_ICONSTOP \
      "32-bitアプリ用Slimeの登録を解除できませんでした。" /SD IDOK
    SetErrorLevel $ResultCode
    Quit
  ${EndIf}

  ExecWait '"$INSTDIR\${NATIVE_DIRECTORY}\SlimeIMERegister.exe" uninstall "$INSTDIR\${NATIVE_DIRECTORY}\SlimeIME.dll"' $ResultCode
  ${If} $ResultCode != 0
    ExecWait '"$INSTDIR\x86\SlimeIMERegister.exe" install "$INSTDIR\x86\SlimeIME.dll"'
    MessageBox MB_OK|MB_ICONSTOP \
      "${NATIVE_LABEL}版Slimeの登録を解除できなかったため、アンインストールを中止しました。" /SD IDOK
    SetErrorLevel $ResultCode
    Quit
  ${EndIf}

  Delete "$SMPROGRAMS\Slime\Slime 設定.lnk"
  Delete "$SMPROGRAMS\Slime\Slime のアンインストール.lnk"
  RMDir "$SMPROGRAMS\Slime"
  DeleteRegKey HKLM "${PRODUCT_KEY}"
  !insertmacro RemovePayload "$INSTDIR"
  RMDir "$PROGRAMFILES64\Slime"
SectionEnd
