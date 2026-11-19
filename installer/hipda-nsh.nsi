; HiPDA·nsh 版 —— Windows 安装包（NSIS 3.11 / MUI2）
;
; 为什么用 NSIS 而不是 Inno Setup：Inno 官方安装器是 per-machine 的（要管理员），
; 而 NSIS 编译出来的安装器可以直接 RequestExecutionLevel user —— 装到
; %LOCALAPPDATA%\Programs 下、快捷方式与卸载项全在 HKCU，全程不提权。
;
; 为什么装到 LOCALAPPDATA 而不是 Program Files：提权安装对这个「个人自用、
; 单 exe」的应用没有任何好处，反而让升级/卸载多一道 UAC。
;
; 编译：scripts/package-windows.sh（本文件需要 UTF-8 BOM，否则中文会乱码）

Unicode true

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

!define APP_NAME "HiPDA·nsh 版"
!define APP_SLUG "HiPDA-nsh"
!define APP_PUBLISHER "quejuwen"
!define APP_URL "https://gitee.com/quejuwen/hipda-nsh"

; 版本与路径都允许命令行覆盖（scripts/package-windows.sh 从 nsh.json 读版本传进来）
!ifndef APP_VERSION
  !define APP_VERSION "0.1.0"
!endif
!ifndef SRC_EXE
  !define SRC_EXE "..\target\release\hipda-nsh.exe"
!endif
!ifndef OUT_FILE
  !define OUT_FILE "..\dist\${APP_SLUG}-${APP_VERSION}-windows-x64-setup.exe"
!endif
!define ICON_FILE "..\assets\icon.ico"

Name "${APP_NAME}"
OutFile "${OUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\${APP_SLUG}"
InstallDirRegKey HKCU "Software\${APP_SLUG}" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
SetCompressorDictSize 64
BrandingText "${APP_NAME} ${APP_VERSION}"

VIProductVersion "${APP_VERSION}.0"
VIAddVersionKey /LANG=2052 "ProductName" "${APP_NAME}"
VIAddVersionKey /LANG=2052 "FileDescription" "${APP_NAME} 安装程序"
VIAddVersionKey /LANG=2052 "FileVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=2052 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=2052 "CompanyName" "${APP_PUBLISHER}"
VIAddVersionKey /LANG=2052 "LegalCopyright" "非官方客户端，与 4D4Y 论坛无关联"

Icon "${ICON_FILE}"
UninstallIcon "${ICON_FILE}"

; ---- 界面 ----
!define MUI_ABORTWARNING
!define MUI_ICON "${ICON_FILE}"
!define MUI_UNICON "${ICON_FILE}"
!define MUI_WELCOMEPAGE_TITLE "${APP_NAME} ${APP_VERSION}"
!define MUI_WELCOMEPAGE_TEXT "HiPDA 论坛（4D4Y）的非官方 Windows 客户端。$\r$\n$\r$\n单文件原生程序，不需要 .NET、不需要 WebView2、不需要管理员权限。$\r$\n$\r$\n将安装到：$LOCALAPPDATA\Programs\${APP_SLUG}$\r$\n$\r$\n本项目与 4D4Y 论坛无任何官方关联，仅供个人学习使用。"

!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_SLUG}.exe"
!define MUI_FINISHPAGE_RUN_TEXT "立即启动 ${APP_NAME}"
!define MUI_FINISHPAGE_LINK "项目主页"
!define MUI_FINISHPAGE_LINK_LOCATION "${APP_URL}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"

; ---- 安装 ----
Section "主程序（必需）" SecMain
  SectionIn RO
  SetShellVarContext current

  ; 升级安装时旧进程会占着 exe，先关掉（静默失败即可：用户没开就不需要关）
  nsExec::ExecToLog 'taskkill /IM "${APP_SLUG}.exe" /F'
  Pop $0
  Sleep 300

  SetOutPath "$INSTDIR"
  ; 用 /oname 固定目标文件名：cargo 产物叫 hipda-nsh.exe（全小写），
  ; 快捷方式与控制面板里显示的是 HiPDA-nsh.exe，名字要对得上
  File /oname=${APP_SLUG}.exe "${SRC_EXE}"
  File /oname=icon.ico "${ICON_FILE}"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_SLUG}.exe" "" "$INSTDIR\${APP_SLUG}.exe" 0
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk" "$INSTDIR\uninstall.exe"

  WriteRegStr HKCU "Software\${APP_SLUG}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\${APP_SLUG}" "Version" "${APP_VERSION}"

  ; 控制面板「应用和功能」条目（HKCU，不需要管理员）
  !define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_SLUG}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "${APP_URL}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${APP_SLUG}.exe,0"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" "$0"
SectionEnd

Section /o "桌面快捷方式" SecDesktop
  SetShellVarContext current
  CreateShortCut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_SLUG}.exe" "" "$INSTDIR\${APP_SLUG}.exe" 0
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "程序本体：一个原生 exe，约 90 MB，无需运行时依赖。"
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "在桌面放一个快捷方式（不勾选则只有开始菜单入口）。"
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; ---- 卸载 ----
Section "Uninstall"
  SetShellVarContext current

  ; /REBOOTOK：exe 被占用时（比如刚被杀毒扫过）排到重启后删，不留 90 MB 残骸
  Delete /REBOOTOK "$INSTDIR\${APP_SLUG}.exe"
  Delete /REBOOTOK "$INSTDIR\icon.ico"
  Delete /REBOOTOK "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"
  Delete "$DESKTOP\${APP_NAME}.lnk"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_SLUG}"
  DeleteRegKey HKCU "Software\${APP_SLUG}"

  ; 用户数据（登录态、缓存）留在 %APPDATA% / %LOCALAPPDATA%，卸载不删 —— 
  ; 重装后不用重新登录；要彻底清干净请手动删应用数据目录。
SectionEnd
