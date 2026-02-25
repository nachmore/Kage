; Kiro Assistant NSIS Installer Hooks

!macro NSIS_HOOK_POSTUNINSTALL
  ; Remove startup registry entry on uninstall
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}"
!macroend
