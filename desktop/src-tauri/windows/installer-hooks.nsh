; Close the running app and sidecar so NSIS can overwrite locked files.
; Do not taskkill ffmpeg.exe / ffprobe.exe globally; they may belong to other software.

!macro NSIS_HOOK_PREINSTALL
  Push $0
  nsExec::Exec 'taskkill /F /T /IM "${MAINBINARYNAME}.exe"'
  Pop $0
  nsExec::Exec 'taskkill /F /T /IM "红果下载.exe"'
  Pop $0
  nsExec::Exec 'taskkill /F /T /IM "hongguo-api.exe"'
  Pop $0
  nsExec::Exec 'taskkill /F /T /IM "hongguo-ai-worker.exe"'
  Pop $0
  Sleep 1000
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Push $0
  nsExec::Exec 'taskkill /F /T /IM "${MAINBINARYNAME}.exe"'
  Pop $0
  nsExec::Exec 'taskkill /F /T /IM "红果下载.exe"'
  Pop $0
  nsExec::Exec 'taskkill /F /T /IM "hongguo-api.exe"'
  Pop $0
  nsExec::Exec 'taskkill /F /T /IM "hongguo-ai-worker.exe"'
  Pop $0
  Sleep 1000
  Pop $0
!macroend
