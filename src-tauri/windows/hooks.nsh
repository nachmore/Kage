; Kage NSIS Installer Hooks

!macro NSIS_HOOK_POSTINSTALL
  ; Verify kage-computer-control-mcp.exe was bundled
  ${If} ${FileExists} "$INSTDIR\kage-computer-control-mcp.exe"
    DetailPrint "computer-control MCP server found"
  ${Else}
    DetailPrint "Warning: kage-computer-control-mcp.exe not found in install directory"
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Remove startup registry entry on uninstall
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
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
