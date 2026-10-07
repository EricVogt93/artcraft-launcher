Unicode True
Name "CraftLauncher"
OutFile "${OUTPUT}"
InstallDir "$LOCALAPPDATA\Programs\CraftLauncher"
RequestExecutionLevel user
SetCompressor /SOLID lzma
!include "MUI2.nsh"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
Section "CraftLauncher"
  SetOutPath "$INSTDIR"
  File "${SOURCE}\craftlauncher.exe"
  File "${SOURCE}\craftlauncher-updater.exe"
  File "${SOURCE}\craftlauncher-bootstrap.exe"
  File "${SOURCE}\craftlauncher.png"
  File "${SOURCE}\README.md"
  File "${SOURCE}\LICENSE"
  SetOutPath "$INSTDIR\licenses"
  File "${SOURCE}\licenses\*"
  SetOutPath "$INSTDIR"
  CreateShortcut "$SMPROGRAMS\CraftLauncher.lnk" "$INSTDIR\craftlauncher-bootstrap.exe"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CraftLauncher" "DisplayName" "CraftLauncher"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CraftLauncher" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CraftLauncher" "UninstallString" '$"$INSTDIR\Uninstall.exe$"'
SectionEnd
Section "Uninstall"
  Delete "$SMPROGRAMS\CraftLauncher.lnk"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CraftLauncher"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "CraftLauncher"
  Delete "$INSTDIR\craftlauncher.exe"
  Delete "$INSTDIR\craftlauncher-updater.exe"
  Delete "$INSTDIR\craftlauncher-bootstrap.exe"
  Delete "$INSTDIR\craftlauncher.png"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\licenses\archivo-OFL.txt"
  Delete "$INSTDIR\licenses\instrumentserif-OFL.txt"
  Delete "$INSTDIR\licenses\ATTRIBUTION.md"
  Delete "$INSTDIR\licenses\LICENSE"
  Delete "$INSTDIR\licenses\NOTICE"
  Delete "$INSTDIR\licenses\CHANGELOG.md"
  RMDir "$INSTDIR\licenses"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  ; The app library and documents are intentionally retained.
SectionEnd
