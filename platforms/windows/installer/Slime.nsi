Unicode true

!include "LogicLib.nsh"
!include "MUI2.nsh"
!include "x64.nsh"

!ifndef VERSION
  !error "VERSION is required"
!endif
!ifndef VERSION_QUAD
  !error "VERSION_QUAD is required"
!endif
!ifndef PAYLOAD_X64
  !error "PAYLOAD_X64 is required"
!endif
!ifndef PAYLOAD_X86
  !error "PAYLOAD_X86 is required"
!endif
!ifndef OUTPUT
  !error "OUTPUT is required"
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
VIAddVersionKey /LANG=1041 "LegalCopyright" "Copyright (c) 2026 unvalley"

!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\x64\SlimeSettings.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Slimeの設定を開く"
!define MUI_FINISHPAGE_RUN_NOTCHECKED
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "Japanese"

Var OldInstallDir
Var ResultCode

!macro RemovePayload Root
  SetOutPath "$TEMP"
  Delete /REBOOTOK "${Root}\x64\SlimeIME.dll"
  Delete /REBOOTOK "${Root}\x64\slime_ffi.dll"
  Delete /REBOOTOK "${Root}\x64\SlimeIMERegister.exe"
  Delete /REBOOTOK "${Root}\x64\SlimeSettings.exe"
  Delete /REBOOTOK "${Root}\x86\SlimeIME.dll"
  Delete /REBOOTOK "${Root}\x86\slime_ffi.dll"
  Delete /REBOOTOK "${Root}\x86\SlimeIMERegister.exe"
  Delete /REBOOTOK "${Root}\x86\SlimeSettings.exe"
  Delete /REBOOTOK "${Root}\Uninstall.exe"
  RMDir /REBOOTOK "${Root}\x64"
  RMDir /REBOOTOK "${Root}\x86"
  RMDir /REBOOTOK "${Root}"
!macroend

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP \
      "Slimeは64-bit版Windows 10以降に対応しています。" /SD IDOK
    SetErrorLevel 1633
    Quit
  ${EndIf}
  SetShellVarContext all
  SetRegView 64
  ReadRegStr $OldInstallDir HKLM "${PRODUCT_KEY}" "InstallLocation"
  ${If} $OldInstallDir == $INSTDIR
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
  IfFileExists "$OldInstallDir\x64\SlimeIMERegister.exe" 0 +2
    ExecWait '"$OldInstallDir\x64\SlimeIMERegister.exe" install "$OldInstallDir\x64\SlimeIME.dll"'
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

  SetOutPath "$INSTDIR\x64"
  File /oname=SlimeIME.dll "${PAYLOAD_X64}\SlimeIME.dll"
  File /oname=slime_ffi.dll "${PAYLOAD_X64}\slime_ffi.dll"
  File /oname=SlimeIMERegister.exe "${PAYLOAD_X64}\SlimeIMERegister.exe"
  File /oname=SlimeSettings.exe "${PAYLOAD_X64}\SlimeSettings.exe"

  SetOutPath "$INSTDIR\x86"
  File /oname=SlimeIME.dll "${PAYLOAD_X86}\SlimeIME.dll"
  File /oname=slime_ffi.dll "${PAYLOAD_X86}\slime_ffi.dll"
  File /oname=SlimeIMERegister.exe "${PAYLOAD_X86}\SlimeIMERegister.exe"
  File /oname=SlimeSettings.exe "${PAYLOAD_X86}\SlimeSettings.exe"

  ExecWait '"$INSTDIR\x64\SlimeIMERegister.exe" install "$INSTDIR\x64\SlimeIME.dll"' $ResultCode
  ${If} $ResultCode != 0
    !insertmacro FailInstall \
      "64-bit版Slimeの登録に失敗しました。インストールはロールバックされます。"
  ${EndIf}

  ExecWait '"$INSTDIR\x86\SlimeIMERegister.exe" install "$INSTDIR\x86\SlimeIME.dll"' $ResultCode
  ${If} $ResultCode != 0
    ExecWait '"$INSTDIR\x64\SlimeIMERegister.exe" uninstall "$INSTDIR\x64\SlimeIME.dll"'
    !insertmacro FailInstall \
      "32-bitアプリ用Slimeの登録に失敗しました。インストールはロールバックされます。"
  ${EndIf}

  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateDirectory "$SMPROGRAMS\Slime"
  CreateShortcut "$SMPROGRAMS\Slime\Slime 設定.lnk" \
                 "$INSTDIR\x64\SlimeSettings.exe"
  CreateShortcut "$SMPROGRAMS\Slime\Slime のアンインストール.lnk" \
                 "$INSTDIR\Uninstall.exe"

  WriteRegStr HKLM "${PRODUCT_KEY}" "DisplayName" "Slime Japanese IME"
  WriteRegStr HKLM "${PRODUCT_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${PRODUCT_KEY}" "Publisher" "${PRODUCT_PUBLISHER}"
  WriteRegStr HKLM "${PRODUCT_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${PRODUCT_KEY}" "DisplayIcon" \
              "$INSTDIR\x64\SlimeSettings.exe"
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

  ExecWait '"$INSTDIR\x64\SlimeIMERegister.exe" uninstall "$INSTDIR\x64\SlimeIME.dll"' $ResultCode
  ${If} $ResultCode != 0
    ExecWait '"$INSTDIR\x86\SlimeIMERegister.exe" install "$INSTDIR\x86\SlimeIME.dll"'
    MessageBox MB_OK|MB_ICONSTOP \
      "64-bit版Slimeの登録を解除できなかったため、アンインストールを中止しました。" /SD IDOK
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
