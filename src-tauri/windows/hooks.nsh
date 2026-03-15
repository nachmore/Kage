; Kiro Assistant NSIS Installer Hooks

!macro NSIS_HOOK_POSTINSTALL
  ; The computer-control-mcp.exe should be built alongside the main binary
  ; and placed in the same output directory. The beforeBuildCommand in
  ; tauri.conf.json handles building it. We just need to verify it's there.
  ${If} ${FileExists} "$INSTDIR\computer-control-mcp.exe"
    DetailPrint "computer-control MCP server found"
  ${Else}
    DetailPrint "Warning: computer-control-mcp.exe not found in install directory"
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Remove startup registry entry on uninstall
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
  ; Remove the MCP binary
  Delete "$INSTDIR\computer-control-mcp.exe"
  ; Clean up MCP registration from mcp.json
  ; (The app handles this gracefully if the binary is missing)
!macroend
