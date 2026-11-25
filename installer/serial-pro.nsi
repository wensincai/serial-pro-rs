/*
 * serial-pro installer (NSIS).
 *
 * Build:
 *   cd serial-pro-rs
 *   cargo build --release
 *   makensis installer\serial-pro.nsi
 *   Output: installer\serial-pro-setup.exe
 *
 * Layout expectations (relative to serial-pro-rs root):
 *   target\release\serial-pro.exe
 *   README.md
 */

  Unicode true
  !include "MUI2.nsh"

;--------------------------------

  !define PRODUCT_NAME "serial-pro"
  !define PRODUCT_VERSION "0.4.0"
  !define PRODUCT_PUBLISHER "serial-pro project"
  !define PRODUCT_WEB_SITE "https://github.com/wensincai/serial-pro-rs"

  Name "${PRODUCT_NAME} ${PRODUCT_VERSION}"
  OutFile "serial-pro-setup.exe"
  InstallDir $PROGRAMFILES64\serial-pro
  InstallDirRegKey HKLM "Software\serial-pro" "Install_Dir"

  ; serial-pro 不装驱动不注册服务；但默认安装到 ProgramFiles 需要管理员
  RequestExecutionLevel admin

  ShowInstDetails show
  ShowUninstDetails show

;--------------------------------
; Pages

  !define MUI_WELCOMEPAGE_TITLE_3LINES
  !define MUI_FINISHPAGE_TITLE_3LINES

  !define MUI_FINISHPAGE_RUN
  !define MUI_FINISHPAGE_RUN_TEXT "Launch serial-pro"
  !define MUI_FINISHPAGE_RUN_FUNCTION LaunchApp
  !define MUI_FINISHPAGE_RUN_NOTCHECKED

  !insertmacro MUI_PAGE_WELCOME
  !insertmacro MUI_PAGE_LICENSE "..\README.md"
  !insertmacro MUI_PAGE_COMPONENTS
  !insertmacro MUI_PAGE_DIRECTORY
  !insertmacro MUI_PAGE_INSTFILES
  !insertmacro MUI_PAGE_FINISH

  !insertmacro MUI_UNPAGE_CONFIRM
  !insertmacro MUI_UNPAGE_INSTFILES

  !insertmacro MUI_LANGUAGE "English"
  !insertmacro MUI_LANGUAGE "SimpChinese"

;--------------------------------
; Sections

Section "serial-pro (required)" sec_main
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "..\target\release\serial-pro.exe"
  File /oname=ReadMe.txt "..\README.md"

  ; 便携模式注册表：记录安装目录（卸载用）
  WriteRegStr HKLM "Software\serial-pro" "Install_Dir" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\serial-pro" \
                   "DisplayName" "serial-pro - Serial Debugging Workstation"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\serial-pro" \
                   "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\serial-pro" \
                   "DisplayVersion" "${PRODUCT_VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\serial-pro" \
                   "Publisher" "${PRODUCT_PUBLISHER}"
  WriteUninstaller "$INSTDIR\uninstall.exe"
SectionEnd

Section "Start Menu Shortcut" sec_sm
  SetOutPath "$INSTDIR"
  CreateDirectory "$SMPROGRAMS\serial-pro"
  CreateShortcut "$SMPROGRAMS\serial-pro\serial-pro.lnk" "$INSTDIR\serial-pro.exe"
  CreateShortcut "$SMPROGRAMS\serial-pro\Uninstall serial-pro.lnk" "$INSTDIR\uninstall.exe"
SectionEnd

Section "Desktop Shortcut" sec_desk
  CreateShortcut "$DESKTOP\serial-pro.lnk" "$INSTDIR\serial-pro.exe"
SectionEnd

; 默认勾选：主程序（RO）+ 开始菜单；桌面快捷方式默认关闭
Function .onInit
  SectionGetFlags ${sec_desk} $0
  IntOp $1 ${SF_SELECTED} ~
  IntOp $0 $0 & $1
  SectionSetFlags ${sec_desk} $0
FunctionEnd

Function LaunchApp
  Exec '"$INSTDIR\serial-pro.exe"'
FunctionEnd

;--------------------------------
; Uninstaller

Section "Uninstall"
  ; 关闭正在运行的实例
  nsExec::ExecToLog 'taskkill /IM serial-pro.exe /F'

  Delete "$INSTDIR\serial-pro.exe"
  Delete "$INSTDIR\ReadMe.txt"
  Delete "$INSTDIR\uninstall.exe"
  ; 便携设置（若用户把 settings.json 留在了安装目录）
  Delete "$INSTDIR\settings.json"
  Delete "$INSTDIR\settings.json.bakup"

  RMDir /r "$SMPROGRAMS\serial-pro"
  Delete "$DESKTOP\serial-pro.lnk"

  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\serial-pro"
  DeleteRegKey HKLM "Software\serial-pro"
  RMDir "$INSTDIR"
SectionEnd
