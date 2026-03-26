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
!macroend
