Unicode true
!include "MUI2.nsh"
!include "x64.nsh"

Name "Raesir"
!ifndef OUTPUT_FILE
  !define OUTPUT_FILE "..\dist\raesir-windows-x64-setup.exe"
!endif
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\Raesir"
InstallDirRegKey HKCU "Software\Raesir" "InstallDir"
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
    MessageBox MB_OK|MB_ICONSTOP "Raesir requires 64-bit Windows."
    Abort
  ${EndIf}
  SetRegView 64
FunctionEnd

Section "Raesir"
  SetOutPath "$INSTDIR"
  File /r "${PAYLOAD_DIR}\*"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateShortcut "$DESKTOP\Raesir.lnk" "$INSTDIR\raesir.exe"
  CreateShortcut "$SMPROGRAMS\Raesir.lnk" "$INSTDIR\raesir.exe"
  WriteRegStr HKCU "Software\Raesir" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Raesir" "DisplayName" "Raesir"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Raesir" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Raesir" "Publisher" "Chipi Chapa Corp"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Raesir" "DisplayIcon" "$INSTDIR\raesir.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Raesir" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Raesir" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Raesir" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Raesir" "NoRepair" 1
SectionEnd

Section "Uninstall"
  SetRegView 64
  !include "${UNINSTALL_FILES}"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$DESKTOP\Raesir.lnk"
  Delete "$SMPROGRAMS\Raesir.lnk"
  DeleteRegKey HKCU "Software\Raesir"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Raesir"
SectionEnd
