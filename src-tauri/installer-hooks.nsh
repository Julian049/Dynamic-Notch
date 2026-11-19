!macro NSIS_HOOK_PREUNINSTALL
  ; Close the island so it can't rewrite its config, then undo everything it changed.
  nsExec::Exec 'taskkill /F /IM dynamic-island.exe'
  Sleep 500
  nsExec::Exec '"$INSTDIR\dynamic-island.exe" --restore'
!macroend
