; Close the running app and sidecar so NSIS can overwrite locked files.
; Do not taskkill ffmpeg.exe / ffprobe.exe globally; they may belong to other software.

!macro NSIS_HOOK_PREINSTALL
  nsExec::Exec 'taskkill /F /T /IM "红果下载.exe"'
  nsExec::Exec 'taskkill /F /T /IM "hongguo-api.exe"'
  nsExec::Exec 'taskkill /F /T /IM "hongguo-ai-worker.exe"'
  Sleep 1000
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec 'taskkill /F /T /IM "红果下载.exe"'
  nsExec::Exec 'taskkill /F /T /IM "hongguo-api.exe"'
  nsExec::Exec 'taskkill /F /T /IM "hongguo-ai-worker.exe"'
  Sleep 1000
!macroend
