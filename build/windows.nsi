Unicode true
!include "MUI2.nsh"
!include "x64.nsh"

Name "Exposed Games"
!ifndef OUTPUT_FILE
  !define OUTPUT_FILE "..\dist\exposed-games-windows-x64-setup.exe"
!endif
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\Exposed Games"
InstallDirRegKey HKCU "Software\Exposed Games" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
!define MUI_ICON "icon.ico"
!define MUI_UNICON "icon.ico"
!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP "Exposed Games requires 64-bit Windows."
    Abort
  ${EndIf}
  SetRegView 64
FunctionEnd

Section "Exposed Games"
  SetOutPath "$INSTDIR"
  File /r "${PAYLOAD_DIR}\*"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateShortcut "$DESKTOP\Exposed Games.lnk" "$INSTDIR\exposed-games.exe"
  CreateShortcut "$SMPROGRAMS\Exposed Games.lnk" "$INSTDIR\exposed-games.exe"
  WriteRegStr HKCU "Software\Exposed Games" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Exposed Games" "DisplayName" "Exposed Games"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Exposed Games" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Exposed Games" "Publisher" "Chipi Chapa Corp"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Exposed Games" "DisplayIcon" "$INSTDIR\exposed-games.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Exposed Games" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Exposed Games" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Exposed Games" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Exposed Games" "NoRepair" 1
SectionEnd

Section "Uninstall"
  SetRegView 64
  !include "${UNINSTALL_FILES}"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$DESKTOP\Exposed Games.lnk"
  Delete "$SMPROGRAMS\Exposed Games.lnk"
  DeleteRegKey HKCU "Software\Exposed Games"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Exposed Games"
SectionEnd
