; Kage NSIS Installer Hooks

; Kill any running MCP sidecar so its exe isn't locked when we (over)write
; or delete it. The Tauri template already handles kage.exe itself, but the
; sidecar is spawned by the agent backend (kiro-cli), not by kage, so it
; can outlive the app — e.g. when the update path releases the Job Object's
; kill-on-close before handing off to this installer. A locked exe surfaces
; as "Error opening file for writing" with Abort/Retry/Ignore.
; Defence in depth: the app-side fix (tree-kill on ACP disconnect) is the
; primary line; this catches anything that still slipped through.
;
; TWO hard rules learned the hard way (a silent /UPDATE install hung
; forever with no visible UI when this blocked):
;   1. NO /T. nsExec::Exec is SYNCHRONOUS — the installer waits for
;      taskkill to return. `taskkill /T` walks the process tree, which
;      can wedge indefinitely on a degraded process table; the sidecar
;      is a leaf process with nothing worth tree-killing anyway.
;   2. ALWAYS /TIMEOUT. Even without /T, never let a stuck taskkill block
;      the installer — bound the wait so a hang becomes a skipped kill
;      (the app-side reap already covers the normal case), not a dead
;      install. nsExec force-terminates the child when the timeout fires.
!macro KAGE_KILL_SIDECARS
  DetailPrint "Stopping Kage helper processes..."
  nsExec::Exec /TIMEOUT=5000 'taskkill /F /IM kage-computer-control-mcp.exe'
  Pop $0 ; discard result — "not found" (128) and "timeout" are both fine
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro KAGE_KILL_SIDECARS
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro KAGE_KILL_SIDECARS
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Verify kage-computer-control-mcp.exe was bundled
  ${If} ${FileExists} "$INSTDIR\kage-computer-control-mcp.exe"
    DetailPrint "computer-control MCP server found"
  ${Else}
    DetailPrint "Warning: kage-computer-control-mcp.exe not found in install directory"
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Remove both autostart mechanisms on real uninstalls only. The
  ; $UpdateMode guard matters: the uninstall section also runs during
  ; auto-updates, and an unguarded delete here would wipe the user's
  ; autostart preference on every silent update (the template's own
  ; Run-key delete carries the same guard for the same reason).
  ${If} $UpdateMode <> 1
    ; Run-key entry (fallback mechanism — see src/os/windows/startup.rs)
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
    ; Logon Scheduled Task (preferred mechanism). Task name is
    ; "Kage Autorun for <user>" — MUST stay in sync with
    ; TASK_NAME_PREFIX in startup.rs. Best-effort: /TIMEOUT so a wedged
    ; schtasks can't hang the uninstall (same rule as KAGE_KILL_SIDECARS
    ; above); result discarded because "task not found" is the common,
    ; fine case.
    ReadEnvStr $0 "USERNAME"
    nsExec::Exec /TIMEOUT=10000 'schtasks /Delete /TN "Kage Autorun for $0" /F'
    Pop $0
  ${EndIf}
  ; Remove the MCP binary
  Delete "$INSTDIR\kage-computer-control-mcp.exe"

  ; Honor the "Delete the application data" checkbox for Kage's REAL data
  ; dirs. Tauri's template only removes $APPDATA\${BUNDLEID} /
  ; $LOCALAPPDATA\${BUNDLEID} (com.kage.launcher), but the app stores
  ; everything under the bespoke `kage` dir (dirs::config_dir().join("kage")
  ; in src/config.rs) — so the checkbox silently deleted nothing and a
  ; reinstall skipped the first-run wizard (first_run_completed survived).
  ; The $UpdateMode guard mirrors the template's own delete-app-data
  ; block, so auto-updates can never wipe user data through this path.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    ; Config, extensions, extension-data, steering docs (Roaming).
    RMDir /r "$APPDATA\kage"
    ; Logs, EBWebView profile, pocket-tts leftovers (Local). This is also
    ; $INSTDIR for the currentUser install mode; the binaries above are
    ; already gone, this sweeps the data remnants the uninstall leaves.
    RMDir /r "$LOCALAPPDATA\kage"
  ${EndIf}
!macroend
