; MoneyKeep's Windows setup (release-packaging.md, "Windows installer").
;
; Built by `cargo xtask installer`, which first runs `cargo xtask release` and then
; calls makensis with:
;   -DVERSION=0.1.0-alpha.1  the version (version.json's "version", semver)
;   -DVERSION_QUAD=0.1.0.1   four numbers for Windows file versions (the pre-release
;                            number, here the alpha number, is the fourth)
;   -DVERSION_ORDER=0.1.0.1.1  the version as dotted numbers that compare in semver
;                            order (major.minor.patch.stage.number; stage 1 alpha,
;                            2 beta, 3 rc, 9 a release), for the upgrade check
;   -DSOURCE_DIR=<folder>    dist\MoneyKeep-<version>: the programs, the documents,
;                            licence.txt (NOTICE with Windows line ends) and
;                            moneykeep.ico (the calculator drawing, made at build time)
;   -DOUT_FILE=<file>        dist\moneykeep-<version>-windows-x64-setup.exe (the name
;                            keep-launcher installs; release-packaging.md revision 2)
;
; Written from the NSIS documentation (https://nsis.sourceforge.io/Docs/): the script
; language, Modern UI 2, LogicLib, x64.nsh and FileFunc.nsh, all shipped with NSIS.
;
; What it does:
; - per-machine install in C:\Program Files\MoneyKeep, with administrator permission
;   (the capture server runs as a service);
; - pages: welcome (says it needs administrator permission, or which version is
;   updated), licence, options (Start menu shortcut on, Desktop shortcut off), folder
;   (skipped when updating in place), progress, finish;
; - registers in Windows "Installed apps" with name, version, publisher, icon, size
;   and the uninstaller;
; - updating: compares the installed version with this one in semver order (through
;   VERSION_ORDER, kept in the registry as "VersionOrder"), stops the capture service when it runs from this folder, replaces the
;   files (and the service's own copy in capture\), starts it again;
; - when Wi-Fi capture is set in the capture server's data folder, puts Wi-Fi
;   capture's firewall rule back if it is missing or turned off (mobile-capture.md
;   revision 18), silently;
; - asks to close MoneyKeep when it is open;
; - the uninstaller removes the program files, shortcuts and registration, stops and
;   removes the capture service and Wi-Fi capture's firewall rule, and asks whether to
;   remove the capture server's data (default No): the folder the service ran with
;   (its --data), else the current user's, and only when it holds the server's store
;   file (release-prep.md §2). Books are never touched: only the
;   files Setup installed are deleted.
; - upgrade-resilience-1 U1.1 (H1, L3): the capture service's own copy in
;   CAPTURE_COPY_DIR is stopped and refreshed whatever $INSTDIR is; the helper's JSON
;   reports from `services move-data` and `services repair-firewall` are read, and an
;   error is shown in a message box (silent installs: the log) saying phone capture
;   needs attention in MoneyKeep.
; - U1.8: removing the data also removes MoneyKeep's tunnel token file and, when
;   MoneyKeep installed it, cloudflared's service (L4); Installed apps runs the
;   uninstaller through a small cmd.exe stub that passes the original user's
;   %LOCALAPPDATA%, captured before the permission prompt (L5).
;
; Silent mode (/S, as keep-launcher runs it; release-packaging.md revision 2):
; - no pages and no questions: the default options (Start menu shortcut on, Desktop
;   shortcut off; an update leaves the shortcuts that are there alone), the folder
;   from /D= or, when updating, the installed folder;
; - MoneyKeep open: it is asked to close (as its window's close button does) and Setup
;   waits up to 30 s, then stops with exit code 2 ("Close MoneyKeep and try again");
; - the capture service is stopped and started as in a normal update;
; - exit code 0 when done, 3 when anything else stopped Setup;
; - the silent uninstall (QuietUninstallString) removes the program and never the
;   capture server's data (the question's default, No, is taken without asking).

Unicode true
ManifestDPIAware true
SetCompressor /SOLID lzma

!macro REQUIRE name
  !ifndef ${name}
    !error "${name} is not set: run cargo xtask installer (it passes VERSION, VERSION_QUAD, VERSION_ORDER, SOURCE_DIR and OUT_FILE)."
  !endif
!macroend
!insertmacro REQUIRE VERSION
!insertmacro REQUIRE VERSION_QUAD
!insertmacro REQUIRE VERSION_ORDER
!insertmacro REQUIRE SOURCE_DIR
!insertmacro REQUIRE OUT_FILE

; ----- names shared with MoneyKeep's code ---------------------------------------------

!define PRODUCT "MoneyKeep"
!define PUBLISHER "MoneyKeep Project"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\MoneyKeep"
; The capture server's Windows service (apps/moneykeep-server/src/services.rs,
; SERVER_SERVICE) and Wi-Fi capture's firewall rule (crates/mk-capture-api,
; FIREWALL_RULE), as MoneyKeep's phone capture setup installs them.
!define SERVICE "MoneyKeepCapture"
!define FIREWALL_RULE "MoneyKeep capture (Wi-Fi)"
; The capture server's data folder under %ProgramData% (security-review-1 A1;
; apps/moneykeep-server/src/services.rs data_dir: %ProgramData%\MoneyKeep\capture-server),
; MoneyKeep's machine folder there, and the folder the data was in before, under
; %LOCALAPPDATA% (services.rs user_data_dir: the "MoneyKeep" project's local data
; folder, then capture-server).
!define MACHINE_DIR "MoneyKeep"
!define CAPTURE_MACHINE_DATA "MoneyKeep\capture-server"
!define CAPTURE_DATA "MoneyKeep\data\capture-server"
; The capture server's store file, always in its data folder
; (apps/moneykeep-server/src/store.rs, DB_FILE): the folder is removed only with it.
!define CAPTURE_MARKER "inbox.sqlite"
; The folder Update server copies the service's program into
; (services.rs install_dir: %ProgramFiles%\MoneyKeep\capture).
!define CAPTURE_COPY_DIR "$PROGRAMFILES64\MoneyKeep\capture"
; cloudflared's Windows service, and the folder of MoneyKeep's tunnel token with the
; marker the helper leaves when it installed that service (services.rs token_dir,
; CLOUDFLARED_MARKER).
!define CLOUDFLARED_SERVICE "Cloudflared"
!define TOKEN_DIR "MoneyKeep\cloudflared"
!define CLOUDFLARED_MARKER "service-installed-by-moneykeep"
!define APP_EXE "moneykeep-app.exe"
!define SERVER_EXE "moneykeep-server.exe"
!define CLI_EXE "moneykeep.exe"
!define MCP_EXE "keep-mcp.exe"
!define ICON_FILE "moneykeep.ico"
; Silent mode's exit codes besides 0 (keep-launcher reads them).
!define EXIT_APP_OPEN 2
!define EXIT_FAILED 3

; ----- the setup program --------------------------------------------------------------

Name "${PRODUCT}"
OutFile "${OUT_FILE}"
InstallDir "$PROGRAMFILES64\${PRODUCT}"
RequestExecutionLevel admin
BrandingText "${PRODUCT} ${VERSION}"

VIProductVersion "${VERSION_QUAD}"
VIFileVersion "${VERSION_QUAD}"
VIAddVersionKey "ProductName" "${PRODUCT}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "FileDescription" "${PRODUCT} Setup"
VIAddVersionKey "LegalCopyright" "Copyright (c) 2026 MoneyKeep Project contributors (MIT OR Apache-2.0)"

!include MUI2.nsh
!include LogicLib.nsh
!include Sections.nsh
!include x64.nsh
!include FileFunc.nsh
!include WordFunc.nsh

Var OldVersion      ; the installed version, "" when none
Var OldOrder        ; its VERSION_ORDER, "" when it has none (installed before 0.1.0-alpha.1)
Var ServiceData     ; the capture service's --data folder, "" when none
Var WelcomeText     ; the first page's text
Var ServiceProgram  ; the program the capture service runs, "" when not installed
Var ServiceHere     ; 1 when that program is inside the install folder
Var ServiceWasRunning
Var ProgramDataDir  ; %ProgramData%
Var Safe            ; un.SafeFolder: 1 when $0 may be removed
Var Haystack        ; Contains: is $Needle in $Haystack? -> $Found (1 or 0)
Var Needle
Var Found
Var UserLocal       ; the uninstall's original user's %LOCALAPPDATA% (L5)

; The calculator is the setup's and the uninstaller's icon.
!define MUI_ICON "${SOURCE_DIR}\${ICON_FILE}"
!define MUI_UNICON "${SOURCE_DIR}\${ICON_FILE}"
!define MUI_ABORTWARNING

; ----- pages --------------------------------------------------------------------------

!define MUI_WELCOMEPAGE_TITLE "${PRODUCT} ${VERSION}"
!define MUI_WELCOMEPAGE_TEXT "$WelcomeText"
!insertmacro MUI_PAGE_WELCOME

; The licence (NOTICE: MIT OR Apache-2.0); nothing to agree to, so the button is Next.
!define MUI_LICENSEPAGE_TEXT_TOP "MoneyKeep is free software: use it under the MIT or the Apache 2.0 licence, at your option."
!define MUI_LICENSEPAGE_TEXT_BOTTOM "The full licences are installed with MoneyKeep (LICENSE-MIT, LICENSE-APACHE)."
!define MUI_LICENSEPAGE_BUTTON "$(^NextBtn)"
!insertmacro MUI_PAGE_LICENSE "${SOURCE_DIR}\licence.txt"

; Start menu shortcut (on), Desktop shortcut (off). Leaving the page checks that
; MoneyKeep is closed.
!define MUI_COMPONENTSPAGE_NODESC
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE CheckAppClosed
!insertmacro MUI_PAGE_COMPONENTS

; The folder; skipped when updating (the update goes where MoneyKeep is).
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipFolderOnUpdate
!insertmacro MUI_PAGE_DIRECTORY

!insertmacro MUI_PAGE_INSTFILES

; No "Open MoneyKeep" box: Setup runs as administrator, and MoneyKeep should not.
!define MUI_FINISHPAGE_TEXT "${PRODUCT} ${VERSION} is installed. Open it from the Start menu."
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; ----- helpers (each for Setup and, with the un. prefix, for the uninstaller) ---------

; $Found = 1 when $Needle occurs in $Haystack (letter case ignored), else 0.
!macro CONTAINS_FN un
Function ${un}Contains
  StrCpy $Found 0
  StrLen $R1 $Needle
  StrCpy $R2 0
  ${Do}
    StrCpy $R3 $Haystack $R1 $R2
    ${If} $R3 == ""
      ${Break}
    ${EndIf}
    ${If} $R3 == $Needle
      StrCpy $Found 1
      ${Break}
    ${EndIf}
    IntOp $R2 $R2 + 1
  ${Loop}
FunctionEnd
!macroend
!insertmacro CONTAINS_FN ""
!insertmacro CONTAINS_FN "un."

; MoneyKeep open? Ask to close it: Retry checks again, Cancel stops here (on a page:
; stays on it; while installing or uninstalling: stops).
!macro APP_CLOSED_FN un
Function ${un}CheckAppClosed
  ${Do}
    nsExec::ExecToStack `"$SYSDIR\tasklist.exe" /FI "IMAGENAME eq ${APP_EXE}" /FO CSV /NH`
    Pop $0 ; exit code
    Pop $Haystack
    StrCpy $Needle "${APP_EXE}"
    Call ${un}Contains
    ${If} $Found != 1
      ${Break}
    ${EndIf}
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "MoneyKeep is open. Close it (your books are saved as you work), then press Retry." /SD IDCANCEL IDRETRY retry
    ${If} ${Silent}
      SetErrorLevel ${EXIT_APP_OPEN}
    ${EndIf}
    Abort
    retry:
  ${Loop}
FunctionEnd
!macroend
!insertmacro APP_CLOSED_FN ""
!insertmacro APP_CLOSED_FN "un."

; Silent mode: MoneyKeep open? Ask it to close (taskkill without /F sends its window
; the close request its close button sends) and wait up to 30 s; still open: stop
; with exit code 2.
!macro SILENT_CLOSE_FN un
Function ${un}SilentCloseApp
  StrCpy $R5 0
  ${Do}
    nsExec::ExecToStack `"$SYSDIR\tasklist.exe" /FI "IMAGENAME eq ${APP_EXE}" /FO CSV /NH`
    Pop $0 ; exit code
    Pop $Haystack
    StrCpy $Needle "${APP_EXE}"
    Call ${un}Contains
    ${If} $Found != 1
      ${Break}
    ${EndIf}
    ${If} $R5 == 0
      nsExec::ExecToStack `"$SYSDIR\taskkill.exe" /IM ${APP_EXE}`
      Pop $0
      Pop $0
    ${EndIf}
    ${If} $R5 >= 30
      SetErrorLevel ${EXIT_APP_OPEN}
      Quit
    ${EndIf}
    Sleep 1000
    IntOp $R5 $R5 + 1
  ${Loop}
FunctionEnd
!macroend
!insertmacro SILENT_CLOSE_FN ""
!insertmacro SILENT_CLOSE_FN "un."

; $ServiceProgram = the program the capture service runs (from its ImagePath:
; "C:\...\moneykeep-server.exe" service-run --data ...), or "" when not installed.
!macro SERVICE_PROGRAM_FN un
Function ${un}ServiceProgram
  StrCpy $ServiceProgram ""
  ReadRegStr $0 HKLM "SYSTEM\CurrentControlSet\Services\${SERVICE}" "ImagePath"
  ${If} $0 != ""
    StrCpy $1 $0 1
    StrCpy $2 " "         ; not quoted: up to the first space
    ${If} $1 == '"'
      StrCpy $0 $0 "" 1   ; quoted: up to the closing quote
      StrCpy $2 '"'
    ${EndIf}
    StrCpy $3 0
    ${Do}
      StrCpy $1 $0 1 $3
      ${If} $1 == ""
      ${OrIf} $1 == $2
        ${Break}
      ${EndIf}
      IntOp $3 $3 + 1
    ${Loop}
    StrCpy $ServiceProgram $0 $3
  ${EndIf}
FunctionEnd
!macroend
!insertmacro SERVICE_PROGRAM_FN ""
!insertmacro SERVICE_PROGRAM_FN "un."

; $ServiceData = the folder after --data in the capture service's ImagePath
; ("C:\...\moneykeep-server.exe" service-run --data "C:\Users\...\capture-server"
; --key-file; the folder is quoted when it has a space), or "" when the service is
; not installed or has no --data. Read before the service is removed.
!macro SERVICE_DATA_FN un
Function ${un}ServiceDataDir
  StrCpy $ServiceData ""
  ReadRegStr $0 HKLM "SYSTEM\CurrentControlSet\Services\${SERVICE}" "ImagePath"
  StrCpy $3 0
  StrCpy $4 ""
  ${Do}
    StrCpy $1 $0 8 $3
    ${If} $1 == ""
      ${Break}
    ${EndIf}
    ${If} $1 == " --data "
      IntOp $3 $3 + 8
      StrCpy $4 $0 "" $3
      ${Break}
    ${EndIf}
    IntOp $3 $3 + 1
  ${Loop}
  ${If} $4 != ""
    StrCpy $1 $4 1
    StrCpy $2 " "         ; not quoted: up to the next space
    ${If} $1 == '"'
      StrCpy $4 $4 "" 1   ; quoted: up to the closing quote
      StrCpy $2 '"'
    ${EndIf}
    StrCpy $3 0
    ${Do}
      StrCpy $1 $4 1 $3
      ${If} $1 == ""
      ${OrIf} $1 == $2
        ${Break}
      ${EndIf}
      IntOp $3 $3 + 1
    ${Loop}
    StrCpy $ServiceData $4 $3
  ${EndIf}
FunctionEnd
!macroend
!insertmacro SERVICE_DATA_FN ""
!insertmacro SERVICE_DATA_FN "un."

; ----- Setup --------------------------------------------------------------------------

Function .onInit
  ; Silent: anything that stops Setup before it is done exits 3 (the last section sets 0).
  ${If} ${Silent}
    SetErrorLevel ${EXIT_FAILED}
  ${EndIf}
  ${IfNot} ${RunningX64}
    MessageBox MB_OK|MB_ICONSTOP "MoneyKeep needs 64-bit Windows." /SD IDOK
    Abort
  ${EndIf}
  ; The 64-bit registry and System32 (sc.exe, net.exe, tasklist.exe, PowerShell).
  SetRegView 64
  ${DisableX64FSRedirection}
  ReadRegStr $OldVersion HKLM "${UNINSTALL_KEY}" "DisplayVersion"
  ReadRegStr $OldOrder HKLM "${UNINSTALL_KEY}" "VersionOrder"
  ReadRegStr $0 HKLM "${UNINSTALL_KEY}" "InstallLocation"
  ${If} $0 != ""
    StrCpy $INSTDIR $0
  ${EndIf}
  StrCpy $1 "Setup needs administrator permission: MoneyKeep is installed for all users, and its capture server runs as a service."
  ${If} $OldVersion == ""
    StrCpy $WelcomeText "Setup will install ${PRODUCT} ${VERSION} on this computer.$\r$\n$\r$\n$1"
  ${Else}
    ; Semver order (release-prep.md §3): 0 the same, 1 the installed one is newer,
    ; 2 this one is newer. A version from before VersionOrder is older.
    StrCpy $2 2
    ${If} $OldVersion == "${VERSION}"
      StrCpy $2 0
    ${ElseIf} $OldOrder != ""
      ${VersionCompare} "$OldOrder" "${VERSION_ORDER}" $2
    ${EndIf}
    ${If} $2 == 0
      StrCpy $WelcomeText "${PRODUCT} ${VERSION} is already installed; Setup will install it again. Your books and settings are kept.$\r$\n$\r$\n$1"
    ${ElseIf} $2 == 1
      StrCpy $WelcomeText "${PRODUCT} $OldVersion is installed, which is newer than ${VERSION}. Setup will replace it with ${VERSION}. Your books and settings are kept.$\r$\n$\r$\n$1"
    ${Else}
      StrCpy $WelcomeText "${PRODUCT} $OldVersion will be updated to ${VERSION}. Your books and settings are kept.$\r$\n$\r$\n$1"
    ${EndIf}
  ${EndIf}
  ${If} ${Silent}
    Call SilentCloseApp
  ${EndIf}
FunctionEnd

Function SkipFolderOnUpdate
  ${If} $OldVersion != ""
    Abort ; skips the page
  ${EndIf}
FunctionEnd

; Before files are replaced: stop the capture service when its program is in the
; install folder, or (upgrade-resilience-1 H1) in the service's own copy folder
; CAPTURE_COPY_DIR, whatever $INSTDIR is (Setup's /D=C:\MK).
Function StopCaptureService
  StrCpy $ServiceHere 0
  StrCpy $ServiceWasRunning 0
  Call ServiceProgram
  ${If} $ServiceProgram != ""
    StrLen $0 "$INSTDIR\"
    StrCpy $1 $ServiceProgram $0
    ${If} $1 == "$INSTDIR\"
      StrCpy $ServiceHere 1
    ${EndIf}
    StrLen $0 "${CAPTURE_COPY_DIR}\"
    StrCpy $1 $ServiceProgram $0
    ${If} $1 == "${CAPTURE_COPY_DIR}\"
      StrCpy $ServiceHere 1
    ${EndIf}
  ${EndIf}
  ${If} $ServiceHere == 1
    nsExec::ExecToStack `"$SYSDIR\sc.exe" query ${SERVICE}`
    Pop $0 ; exit code
    Pop $Haystack
    StrCpy $Needle "RUNNING"
    Call Contains
    ${If} $Found == 1
      DetailPrint "Stopping the capture server ($ServiceProgram)"
      nsExec::ExecToLog `"$SYSDIR\net.exe" stop ${SERVICE}`
      Pop $0
      StrCpy $ServiceWasRunning 1
      Sleep 1000 ; the process lets go of its program file
    ${EndIf}
  ${EndIf}
FunctionEnd

; After the files: the service's own copy gets the new server too (so the running
; server has this build's capture version and MoneyKeep asks for no update), then the
; service starts again if it was running.
Function StartCaptureService
  ${If} $ServiceHere == 1
  ${AndIf} $ServiceProgram != "$INSTDIR\${SERVER_EXE}"
    ${GetFileName} $ServiceProgram $0
    ${If} $0 == "${SERVER_EXE}"
      DetailPrint "Updating the capture server's copy: $ServiceProgram"
      File "/oname=$ServiceProgram" "${SOURCE_DIR}\${SERVER_EXE}"
    ${EndIf}
  ${EndIf}
  ${If} $ServiceWasRunning == 1
    DetailPrint "Starting the capture server"
    nsExec::ExecToLog `"$SYSDIR\net.exe" start ${SERVICE}`
    Pop $0
  ${EndIf}
FunctionEnd

; mobile-capture.md revision 18: when Wi-Fi capture is set in the capture server's
; data folder (the service's --data, else the current user's), put Wi-Fi capture's
; firewall rule back if it is missing or turned off — silently, as administrator
; (Setup runs elevated). The uninstaller removes the rule; MoneyKeep's own setup
; never ran again after a reinstall, so phones on Wi-Fi could not reach the computer.
; `moneykeep-server services repair-firewall` reads server.toml and does nothing
; when Wi-Fi capture is off.
Function RepairWifiFirewall
  Call ServiceDataDir
  StrCpy $0 $ServiceData
  ${If} $0 == ""
    ReadEnvStr $ProgramDataDir PROGRAMDATA
    StrCpy $0 "$ProgramDataDir\${CAPTURE_MACHINE_DATA}"
  ${EndIf}
  ${If} ${FileExists} "$0\server.toml"
    DetailPrint "Checking Wi-Fi capture's firewall rule ${FIREWALL_RULE}"
    nsExec::ExecToStack `"$INSTDIR\${SERVER_EXE}" services repair-firewall --data "$0"`
    Pop $1 ; exit code
    Pop $Haystack ; {"ok": true, "actions": […]} or {"ok": false, "error": "…"}
    StrCpy $R6 "Wi-Fi capture's firewall rule could not be put back."
    Call CheckReport
  ${EndIf}
FunctionEnd

; upgrade-resilience-1 U1.1 (L3): $1 = the helper's exit code, $Haystack = its JSON
; report, $R6 = what to say when the report has no "error". When it failed: the error
; in a message box (silent: the log), saying phone capture needs attention in
; MoneyKeep.
Function CheckReport
  StrCpy $Needle '"ok":true'
  Call Contains
  ${If} $1 == 0
  ${AndIf} $Found == 1
    Return
  ${EndIf}
  ; The "error" text: from after "error":" to the next quote.
  StrCpy $Needle '"error":"'
  StrLen $R1 $Needle
  StrCpy $R2 0
  ${Do}
    StrCpy $R3 $Haystack $R1 $R2
    ${If} $R3 == ""
      ${Break}
    ${EndIf}
    ${If} $R3 == $Needle
      IntOp $R2 $R2 + $R1
      StrCpy $R4 $R2
      ${Do}
        StrCpy $R3 $Haystack 1 $R4
        ${If} $R3 == '"'
        ${OrIf} $R3 == ""
          ${Break}
        ${EndIf}
        IntOp $R4 $R4 + 1
      ${Loop}
      IntOp $R4 $R4 - $R2
      StrCpy $R6 $Haystack $R4 $R2
      ${Break}
    ${EndIf}
    IntOp $R2 $R2 + 1
  ${Loop}
  DetailPrint "Phone capture needs attention in MoneyKeep: $R6"
  MessageBox MB_OK|MB_ICONEXCLAMATION "Phone capture needs attention in MoneyKeep:$\r$\n$\r$\n$R6$\r$\n$\r$\nOpen MoneyKeep: Settings > Phone capture." /SD IDOK
FunctionEnd

; security-review-1 A1: when the capture service still uses the data folder in a
; user's profile, the server moves it to its protected folder under %ProgramData%
; and runs under its own account from then on (the old copy is kept; MoneyKeep
; offers to remove it). Silent, as administrator.
Function MoveCaptureData
  Call ServiceProgram
  ${If} $ServiceProgram != ""
    DetailPrint "Moving the capture server's data to its protected folder, if needed"
    nsExec::ExecToStack `"$INSTDIR\${SERVER_EXE}" services move-data`
    Pop $1 ; exit code
    Pop $Haystack ; the helper's report (JSON)
    StrCpy $R6 "The capture server's data could not be moved to its protected folder."
    Call CheckReport
  ${EndIf}
FunctionEnd

Section "MoneyKeep (required)" SecMain
  SectionIn RO
  Call CheckAppClosed
  Call StopCaptureService

  SetOutPath "$INSTDIR"
  File "${SOURCE_DIR}\${APP_EXE}"
  File "${SOURCE_DIR}\${SERVER_EXE}"
  File "${SOURCE_DIR}\${CLI_EXE}"
  File "${SOURCE_DIR}\${MCP_EXE}"
  File "${SOURCE_DIR}\LICENSE-APACHE"
  File "${SOURCE_DIR}\LICENSE-MIT"
  File "${SOURCE_DIR}\NOTICE"
  File "${SOURCE_DIR}\ATTRIBUTION.md"
  ; Every crate compiled into the programs with its licence (security-review-1 B4);
  ; About MoneyKeep → Third-party licences opens it.
  File "${SOURCE_DIR}\THIRD-PARTY-LICENSES.txt"
  File "${SOURCE_DIR}\version.json"
  File "${SOURCE_DIR}\${ICON_FILE}"

  Call StartCaptureService
  Call MoveCaptureData
  Call RepairWifiFirewall

  ; Windows "Installed apps".
  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "VersionOrder" "${VERSION_ORDER}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${ICON_FILE}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  ; L5: Installed apps runs this as the user, before the permission prompt, so
  ; cmd.exe expands that user's %LOCALAPPDATA% and hands it to the uninstaller.
  WriteRegStr HKLM "${UNINSTALL_KEY}" "UninstallString" '"$SYSDIR\cmd.exe" /d /c start "" "$INSTDIR\uninstall.exe" /LOCALAPPDATA="%LOCALAPPDATA%"'
  WriteRegStr HKLM "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "EstimatedSize" "$0"
SectionEnd

Section "Start menu shortcut" SecStartMenu
  SetShellVarContext all
  CreateShortcut "$SMPROGRAMS\${PRODUCT}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${ICON_FILE}" 0
SectionEnd

Section /o "Desktop shortcut" SecDesktop
  SetShellVarContext all
  CreateShortcut "$DESKTOP\${PRODUCT}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${ICON_FILE}" 0
SectionEnd

; An update with a shortcut turned off removes the one an earlier version made. A
; silent update chose nothing, so it leaves the shortcuts that are there alone.
Section "-Shortcuts turned off"
  ${IfNot} ${Silent}
    SetShellVarContext all
    ${IfNot} ${SectionIsSelected} ${SecStartMenu}
      Delete "$SMPROGRAMS\${PRODUCT}.lnk"
    ${EndIf}
    ${IfNot} ${SectionIsSelected} ${SecDesktop}
      Delete "$DESKTOP\${PRODUCT}.lnk"
    ${EndIf}
  ${EndIf}
SectionEnd

; The last section: Setup got this far, so it succeeded (silent mode's exit code 0).
Section "-Done"
  SetErrorLevel 0
SectionEnd

; ----- the uninstaller ----------------------------------------------------------------

Function un.onInit
  ${If} ${Silent}
    SetErrorLevel ${EXIT_FAILED}
  ${EndIf}
  SetRegView 64
  ${DisableX64FSRedirection}
  ${If} ${Silent}
    Call un.SilentCloseApp
  ${EndIf}
  ; L5: the original user's %LOCALAPPDATA% (from the stub), when it is a real
  ; folder; else this account's.
  SetShellVarContext current
  StrCpy $UserLocal $LOCALAPPDATA
  SetShellVarContext all
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/LOCALAPPDATA=" $R1
  ${IfNot} ${Errors}
    StrCpy $R2 $R1 1
    ${If} $R2 == '"'
      StrCpy $R1 $R1 "" 1
      StrCpy $R1 $R1 -1
    ${EndIf}
    ${GetFileAttributes} "$R1" "REPARSE_POINT" $R2
    ${If} $R1 != ""
    ${AndIf} ${FileExists} "$R1\*.*"
    ${AndIf} $R2 != 1
      StrCpy $UserLocal $R1
    ${EndIf}
  ${EndIf}
FunctionEnd

; The capture server's program copy in `dir` (Update server's), and the folder if empty.
!macro REMOVE_CAPTURE_COPY dir
  Delete "${dir}\${SERVER_EXE}"
  Delete "${dir}\${SERVER_EXE}.old"
  Delete "${dir}\${SERVER_EXE}.prev"
  RMDir "${dir}"
!macroend

; L8 (security-review-1 A1): $Safe = 1 when the folder $0 may be removed: neither it
; nor its parent is a link or junction (a reparse point), and it is one of the places
; MoneyKeep keeps the capture server's data (%ProgramData%\MoneyKeep\capture-server, or
; the current user's %LOCALAPPDATA%\MoneyKeep\data\capture-server).
Function un.SafeFolder
  StrCpy $Safe 0
  ${GetFileAttributes} "$0" "REPARSE_POINT" $1
  ${If} $1 == 1
    Return
  ${EndIf}
  ${GetParent} "$0" $2
  ${GetFileAttributes} "$2" "REPARSE_POINT" $1
  ${If} $1 == 1
    Return
  ${EndIf}
  ${If} $0 == "$ProgramDataDir\${CAPTURE_MACHINE_DATA}"
  ${OrIf} $0 == "$UserLocal\${CAPTURE_DATA}"
    StrCpy $Safe 1
  ${EndIf}
FunctionEnd

; Remove the capture server's data folder $0 (asked once, before: $3 = 1 for Yes) when
; it is safe (un.SafeFolder) and holds the server's store file; else say why it stays.
Function un.RemoveCaptureData
  ${If} ${FileExists} "$0\*.*"
    Call un.SafeFolder
    ${If} $Safe != 1
      DetailPrint "Kept $0: it is a link or junction, or not where MoneyKeep keeps the capture server's data."
    ${ElseIf} ${FileExists} "$0\${CAPTURE_MARKER}"
      ${If} $3 == 1
        DetailPrint "Removing the capture server's data in $0"
        RMDir /r "$0"
      ${EndIf}
    ${Else}
      DetailPrint "Kept $0: it has no ${CAPTURE_MARKER}, so it is not the capture server's data folder."
    ${EndIf}
  ${EndIf}
FunctionEnd

Section "Uninstall"
  Call un.CheckAppClosed

  ; The capture service, when MoneyKeep's phone capture setup installed it (its data
  ; folder is read first: removing the service removes its command line).
  Call un.ServiceDataDir
  Call un.ServiceProgram
  ${If} $ServiceProgram != ""
    DetailPrint "Removing the capture server service"
    nsExec::ExecToLog `"$SYSDIR\net.exe" stop ${SERVICE}`
    Pop $0
    nsExec::ExecToLog `"$SYSDIR\sc.exe" delete ${SERVICE}`
    Pop $0
    Sleep 1000
  ${EndIf}
  ; Wi-Fi capture's firewall rule (nothing happens when there is none).
  DetailPrint "Removing the firewall rule ${FIREWALL_RULE}"
  nsExec::ExecToLog `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -Command "Remove-NetFirewallRule -DisplayName '${FIREWALL_RULE}' -ErrorAction SilentlyContinue"`
  Pop $0

  ; Only the files Setup installed (books are never in this list), and the capture
  ; server's copies.
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\${SERVER_EXE}"
  Delete "$INSTDIR\${CLI_EXE}"
  Delete "$INSTDIR\${MCP_EXE}"
  Delete "$INSTDIR\LICENSE-APACHE"
  Delete "$INSTDIR\LICENSE-MIT"
  Delete "$INSTDIR\NOTICE"
  Delete "$INSTDIR\ATTRIBUTION.md"
  Delete "$INSTDIR\THIRD-PARTY-LICENSES.txt"
  Delete "$INSTDIR\version.json"
  Delete "$INSTDIR\${ICON_FILE}"
  !insertmacro REMOVE_CAPTURE_COPY "$INSTDIR\capture"
  !insertmacro REMOVE_CAPTURE_COPY "${CAPTURE_COPY_DIR}"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  SetShellVarContext all
  Delete "$SMPROGRAMS\${PRODUCT}.lnk"
  Delete "$DESKTOP\${PRODUCT}.lnk"
  DeleteRegKey HKLM "${UNINSTALL_KEY}"

  ; The capture server's data (certificates, paired phones, the inbox): its folder
  ; under %ProgramData% (or the folder the service ran with), and the old copy in the
  ; current user's folder (security-review-1 A1). Kept unless the user says Yes; each
  ; is removed only when it is a real folder in its expected place (L8) and holds the
  ; server's store file.
  ReadEnvStr $ProgramDataDir PROGRAMDATA
  StrCpy $0 $ServiceData
  ${If} $0 == ""
    StrCpy $0 "$ProgramDataDir\${CAPTURE_MACHINE_DATA}"
  ${EndIf}
  StrCpy $4 "$UserLocal\${CAPTURE_DATA}"
  ${IfNot} ${FileExists} "$0\${CAPTURE_MARKER}"
    StrCpy $0 $4
  ${EndIf}
  StrCpy $3 0
  ${If} ${FileExists} "$0\${CAPTURE_MARKER}"
  ${OrIf} ${FileExists} "$4\${CAPTURE_MARKER}"
    MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "Also remove the capture server's data (certificates and paired phones) in $0?" /SD IDNO IDNO keep_data
    StrCpy $3 1
    keep_data:
  ${EndIf}
  Call un.RemoveCaptureData
  ${If} $0 != $4
    StrCpy $0 $4
    Call un.RemoveCaptureData
  ${EndIf}
  ${If} $3 == 1
    ; L4: MoneyKeep's tunnel token file and, when MoneyKeep installed it (its
    ; marker, or the service runs a token file of MoneyKeep's), cloudflared's
    ; service.
    StrCpy $0 "$ProgramDataDir\${TOKEN_DIR}"
    ${GetFileAttributes} "$0" "REPARSE_POINT" $1
    ${If} $1 != 1
    ${AndIf} ${FileExists} "$0\*.*"
      ReadRegStr $Haystack HKLM "SYSTEM\CurrentControlSet\Services\${CLOUDFLARED_SERVICE}" "ImagePath"
      StrCpy $Needle "$0\"
      Call un.Contains
      ${If} ${FileExists} "$0\${CLOUDFLARED_MARKER}"
      ${OrIf} $Found == 1
        DetailPrint "Removing the cloudflared service MoneyKeep installed"
        nsExec::ExecToLog `"$SYSDIR\sc.exe" stop ${CLOUDFLARED_SERVICE}`
        Pop $1
        nsExec::ExecToLog `"$SYSDIR\sc.exe" delete ${CLOUDFLARED_SERVICE}`
        Pop $1
      ${EndIf}
      DetailPrint "Removing MoneyKeep's tunnel token file"
      Delete "$0\*.token"
      Delete "$0\${CLOUDFLARED_MARKER}"
      RMDir "$0"
    ${EndIf}
    ; MoneyKeep's machine folder: its download folder, then the folder when empty.
    StrCpy $0 "$ProgramDataDir\${MACHINE_DIR}"
    ${GetFileAttributes} "$0" "REPARSE_POINT" $1
    ${If} $1 != 1
      ${GetFileAttributes} "$0\tmp" "REPARSE_POINT" $1
      ${If} $1 != 1
        RMDir /r "$0\tmp"
      ${EndIf}
      RMDir "$0"
    ${EndIf}
  ${EndIf}
  SetErrorLevel 0
SectionEnd
