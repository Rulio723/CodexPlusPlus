Unicode true
!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "StrFunc.nsh"

${StrTok}

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!define ROOT "..\..\.."
!include "x64.nsh"
!include "secure-recovery-acl.nsh"

Var AdminRecoveryDir
Var AdminRecoveryFile
Var PowerShell7Installed
Var PowerShell7ProgramFiles
Var PowerShell7StoreRoot
Var PowerShell7StorePattern
Var PowerShell7Path
Var PowerShell7PathIndex
Var PowerShell7Candidate
Var PowerShell7Version
Var PowerShell7Major

Name "Codex++"
OutFile "${ROOT}\dist\windows\CodexPlusPlus-${VERSION}-windows-x64-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\Codex++"
InstallDirRegKey HKCU "Software\Codex++" "InstallDir"
RequestExecutionLevel admin
SetCompressor /SOLID lzma

!define MUI_ICON "${ROOT}\apps\codex-plus-manager\src-tauri\icons\icon.ico"
!define MUI_UNICON "${ROOT}\apps\codex-plus-manager\src-tauri\icons\icon.ico"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

Function DetectPowerShell7
  StrCpy $PowerShell7Installed 0
  StrCpy $PowerShell7ProgramFiles ""
  StrCpy $PowerShell7StoreRoot ""
  StrCpy $PowerShell7StorePattern ""
  StrCpy $PowerShell7Path ""
  StrCpy $PowerShell7PathIndex 0

  StrCpy $PowerShell7Candidate "$PROGRAMFILES\PowerShell\7\pwsh.exe"
  Call CheckPowerShell7Candidate
  StrCmp $PowerShell7Installed 1 powershell7_detect_done

  StrCpy $PowerShell7Candidate "$LOCALAPPDATA\Programs\PowerShell\7\pwsh.exe"
  Call CheckPowerShell7Candidate
  StrCmp $PowerShell7Installed 1 powershell7_detect_done

  ${If} ${RunningX64}
    SetRegView 64
    Call CheckPowerShell7RegistryView
    StrCmp $PowerShell7Installed 1 powershell7_detect_done
    ReadRegStr $PowerShell7ProgramFiles HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion" "ProgramFilesDir"
    StrCpy $PowerShell7StoreRoot "$PowerShell7ProgramFiles\WindowsApps"
    StrCpy $PowerShell7StorePattern "$PowerShell7StoreRoot\Microsoft.PowerShell_*_x64__8wekyb3d8bbwe"
    Call ScanPowerShell7Store
    StrCmp $PowerShell7Installed 1 powershell7_detect_done

    SetRegView 32
    Call CheckPowerShell7RegistryView
    StrCmp $PowerShell7Installed 1 powershell7_detect_done
    ReadRegStr $PowerShell7ProgramFiles HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion" "ProgramFilesDir"
    StrCpy $PowerShell7StoreRoot "$PowerShell7ProgramFiles\WindowsApps"
    StrCpy $PowerShell7StorePattern "$PowerShell7StoreRoot\Microsoft.PowerShell_*_x86__8wekyb3d8bbwe"
    Call ScanPowerShell7Store
    StrCmp $PowerShell7Installed 1 powershell7_detect_done
  ${Else}
    SetRegView 32
    Call CheckPowerShell7RegistryView
    StrCmp $PowerShell7Installed 1 powershell7_detect_done
    ReadRegStr $PowerShell7ProgramFiles HKLM "SOFTWARE\Microsoft\Windows\CurrentVersion" "ProgramFilesDir"
    StrCpy $PowerShell7StoreRoot "$PowerShell7ProgramFiles\WindowsApps"
    StrCpy $PowerShell7StorePattern "$PowerShell7StoreRoot\Microsoft.PowerShell_*_x86__8wekyb3d8bbwe"
    Call ScanPowerShell7Store
    StrCmp $PowerShell7Installed 1 powershell7_detect_done
  ${EndIf}

powershell7_path_loop:
  ReadEnvStr $PowerShell7Path "PATH"
  ${StrTok} $PowerShell7Candidate "$PowerShell7Path" ";" "$PowerShell7PathIndex" "1"
  StrCmp $PowerShell7Candidate "" powershell7_detect_done
  StrCpy $PowerShell7Candidate "$PowerShell7Candidate\pwsh.exe"
  Call CheckPowerShell7Candidate
  StrCmp $PowerShell7Installed 1 powershell7_detect_done
  IntOp $PowerShell7PathIndex $PowerShell7PathIndex + 1
  Goto powershell7_path_loop

powershell7_detect_done:
  SetRegView lastused
FunctionEnd

Function CheckPowerShell7RegistryView
  ReadRegStr $PowerShell7Candidate HKLM "SOFTWARE\Microsoft\PowerShellCore" "InstallLocation"
  StrCmp $PowerShell7Candidate "" powershell7_registry_hkcu
  StrCpy $PowerShell7Candidate "$PowerShell7Candidate\pwsh.exe"
  Call CheckPowerShell7Candidate
  StrCmp $PowerShell7Installed 1 powershell7_registry_done

powershell7_registry_hkcu:
  ReadRegStr $PowerShell7Candidate HKCU "SOFTWARE\Microsoft\PowerShellCore" "InstallLocation"
  StrCmp $PowerShell7Candidate "" powershell7_registry_done
  StrCpy $PowerShell7Candidate "$PowerShell7Candidate\pwsh.exe"
  Call CheckPowerShell7Candidate

powershell7_registry_done:
FunctionEnd

Function ScanPowerShell7Store
  FindFirst $0 $1 "$PowerShell7StorePattern"
  StrCmp $1 "" powershell7_store_done

powershell7_store_loop:
  StrCpy $PowerShell7Candidate "$PowerShell7StoreRoot\$1\pwsh.exe"
  Call CheckPowerShell7Candidate
  StrCmp $PowerShell7Installed 1 powershell7_store_done
  FindNext $0 $1
  StrCmp $1 "" powershell7_store_done
  Goto powershell7_store_loop

powershell7_store_done:
  FindClose $0
FunctionEnd

Function CheckPowerShell7Candidate
  IfFileExists "$PowerShell7Candidate" 0 powershell7_candidate_done
  ClearErrors
  ${GetFileVersion} "$PowerShell7Candidate" $PowerShell7Version
  IfErrors powershell7_candidate_done
  ${StrTok} $PowerShell7Major "$PowerShell7Version" "." "0" "1"
  StrCmp $PowerShell7Major "7" powershell7_candidate_found

powershell7_candidate_done:
  Return

powershell7_candidate_found:
  StrCpy $PowerShell7Installed 1
FunctionEnd

Function CreateSecureRecoveryDirectory
  !insertmacro SetSecureRecoveryCreatePayload create_recovery_payload_set_failed
  nsExec::ExecToStack /TIMEOUT=30000 '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -EncodedCommand "${SECURE_RECOVERY_BOOTSTRAP_COMMAND}"'
  Pop $0
  Pop $1
  !insertmacro ClearSecureRecoveryCreatePayload create_recovery_payload_cleanup_failed
  StrCmp $0 0 create_recovery_directory_ok create_recovery_directory_failed

create_recovery_directory_ok:
  StrCmp $1 "" create_recovery_directory_failed
  StrCpy $AdminRecoveryDir $1
  StrCpy $AdminRecoveryFile "$AdminRecoveryDir\codex-plus-recovery.exe"
  IfFileExists "$AdminRecoveryDir\." create_recovery_directory_done create_recovery_directory_failed

create_recovery_directory_failed:
  Call CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; secure extraction directory creation failed."

create_recovery_payload_set_failed:
  !insertmacro ClearSecureRecoveryCreatePayloadUnchecked
  Call CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; secure recovery payload setup failed."

create_recovery_payload_cleanup_failed:
  !insertmacro ClearSecureRecoveryCreatePayloadUnchecked
  Call CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; secure recovery payload cleanup failed."

create_recovery_directory_done:
FunctionEnd

Function ProtectSecureRecoveryFile
  IfFileExists "$AdminRecoveryFile" protect_recovery_file_environment protect_recovery_file_failed

protect_recovery_file_environment:
  System::Call 'Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_FILE", w "$AdminRecoveryFile") i.r0'
  StrCmp $0 0 protect_recovery_file_failed
  !insertmacro SetSecureRecoveryFilePayload protect_recovery_payload_set_failed
  nsExec::ExecToStack /TIMEOUT=30000 '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -EncodedCommand "${SECURE_RECOVERY_BOOTSTRAP_COMMAND}"'
  Pop $0
  Pop $1
  !insertmacro ClearSecureRecoveryFilePayload protect_recovery_payload_cleanup_failed
  System::Call 'Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_FILE", p 0) i.r2'
  StrCmp $2 0 protect_recovery_file_failed
  StrCmp $0 0 protect_recovery_file_output protect_recovery_file_failed

protect_recovery_file_output:
  StrCmp $1 "" protect_recovery_file_done protect_recovery_file_failed

protect_recovery_file_failed:
  !insertmacro ClearSecureRecoveryFilePayloadUnchecked
  System::Call 'Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_FILE", p 0)'
  Call CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; secure recovery file validation failed."

protect_recovery_payload_set_failed:
  !insertmacro ClearSecureRecoveryFilePayloadUnchecked
  System::Call 'Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_FILE", p 0)'
  Call CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; secure recovery payload setup failed."

protect_recovery_payload_cleanup_failed:
  !insertmacro ClearSecureRecoveryFilePayloadUnchecked
  System::Call 'Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_FILE", p 0)'
  Call CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; secure recovery payload cleanup failed."

protect_recovery_file_done:
FunctionEnd

Function CleanupSecureRecoveryDirectory
  System::Call 'Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_FILE", p 0)'
  StrCmp $AdminRecoveryDir "" cleanup_recovery_done
  SetOutPath "$WINDIR\Temp"
  StrCmp $AdminRecoveryFile "" cleanup_recovery_directory
  Delete "$AdminRecoveryFile"

cleanup_recovery_directory:
  RMDir "$AdminRecoveryDir"
  IfFileExists "$AdminRecoveryDir\." cleanup_recovery_failed cleanup_recovery_cleared

cleanup_recovery_failed:
  Abort "Administrator mode recovery failed; secure recovery cleanup failed."

cleanup_recovery_cleared:
  StrCpy $AdminRecoveryFile ""
  StrCpy $AdminRecoveryDir ""

cleanup_recovery_done:
FunctionEnd

Function RecoverAdminMode
  IfFileExists "$AdminRecoveryFile" run_recovery recovery_unavailable

run_recovery:
  nsExec::ExecToLog '"$AdminRecoveryFile" --recover-admin-mode'
  Pop $1
  StrCmp $1 0 recovery_done
  Call CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; close Codex++ and try again."

recovery_unavailable:
  Call CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; the recovery launcher is unavailable."

recovery_done:
FunctionEnd

Function TryRecoverAdminMode
  IfFileExists "$AdminRecoveryFile" try_run_recovery try_recovery_done

try_run_recovery:
  nsExec::ExecToLog /TIMEOUT=30000 '"$AdminRecoveryFile" --recover-admin-mode'
  Pop $1
  StrCmp $1 0 try_recovery_done
  DetailPrint "Administrator mode is still active; closing Codex++ before retrying recovery."

try_recovery_done:
FunctionEnd

Function StopRunningCodexPlus
  DetailPrint "Closing running Codex++, recovery helpers, and Codex..."
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-plus-manager.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-plus.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-recovery.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-admin-shim.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM ChatGPT.exe /T /F'
  Pop $0
  Sleep 1000
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-plus-manager.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-plus.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-recovery.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-admin-shim.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM ChatGPT.exe /T /F'
  Pop $0
  Sleep 500
FunctionEnd

Function un.CreateSecureRecoveryDirectory
  !insertmacro SetSecureRecoveryCreatePayload create_recovery_payload_set_failed
  nsExec::ExecToStack /TIMEOUT=30000 '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -EncodedCommand "${SECURE_RECOVERY_BOOTSTRAP_COMMAND}"'
  Pop $0
  Pop $1
  !insertmacro ClearSecureRecoveryCreatePayload create_recovery_payload_cleanup_failed
  StrCmp $0 0 create_recovery_directory_ok create_recovery_directory_failed

create_recovery_directory_ok:
  StrCmp $1 "" create_recovery_directory_failed
  StrCpy $AdminRecoveryDir $1
  StrCpy $AdminRecoveryFile "$AdminRecoveryDir\codex-plus-recovery.exe"
  IfFileExists "$AdminRecoveryDir\." create_recovery_directory_done create_recovery_directory_failed

create_recovery_directory_failed:
  Call un.CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; secure extraction directory creation failed."

create_recovery_payload_set_failed:
  !insertmacro ClearSecureRecoveryCreatePayloadUnchecked
  Call un.CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; secure recovery payload setup failed."

create_recovery_payload_cleanup_failed:
  !insertmacro ClearSecureRecoveryCreatePayloadUnchecked
  Call un.CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; secure recovery payload cleanup failed."

create_recovery_directory_done:
FunctionEnd

Function un.ProtectSecureRecoveryFile
  IfFileExists "$AdminRecoveryFile" protect_recovery_file_environment protect_recovery_file_failed

protect_recovery_file_environment:
  System::Call 'Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_FILE", w "$AdminRecoveryFile") i.r0'
  StrCmp $0 0 protect_recovery_file_failed
  !insertmacro SetSecureRecoveryFilePayload protect_recovery_payload_set_failed
  nsExec::ExecToStack /TIMEOUT=30000 '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -EncodedCommand "${SECURE_RECOVERY_BOOTSTRAP_COMMAND}"'
  Pop $0
  Pop $1
  !insertmacro ClearSecureRecoveryFilePayload protect_recovery_payload_cleanup_failed
  System::Call 'Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_FILE", p 0) i.r2'
  StrCmp $2 0 protect_recovery_file_failed
  StrCmp $0 0 protect_recovery_file_output protect_recovery_file_failed

protect_recovery_file_output:
  StrCmp $1 "" protect_recovery_file_done protect_recovery_file_failed

protect_recovery_file_failed:
  !insertmacro ClearSecureRecoveryFilePayloadUnchecked
  System::Call 'Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_FILE", p 0)'
  Call un.CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; secure recovery file validation failed."

protect_recovery_payload_set_failed:
  !insertmacro ClearSecureRecoveryFilePayloadUnchecked
  System::Call 'Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_FILE", p 0)'
  Call un.CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; secure recovery payload setup failed."

protect_recovery_payload_cleanup_failed:
  !insertmacro ClearSecureRecoveryFilePayloadUnchecked
  System::Call 'Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_FILE", p 0)'
  Call un.CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; secure recovery payload cleanup failed."

protect_recovery_file_done:
FunctionEnd

Function un.CleanupSecureRecoveryDirectory
  System::Call 'Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_FILE", p 0)'
  StrCmp $AdminRecoveryDir "" cleanup_recovery_done
  SetOutPath "$WINDIR\Temp"
  StrCmp $AdminRecoveryFile "" cleanup_recovery_directory
  Delete "$AdminRecoveryFile"

cleanup_recovery_directory:
  RMDir "$AdminRecoveryDir"
  IfFileExists "$AdminRecoveryDir\." cleanup_recovery_failed cleanup_recovery_cleared

cleanup_recovery_failed:
  Abort "Administrator mode recovery failed; secure recovery cleanup failed."

cleanup_recovery_cleared:
  StrCpy $AdminRecoveryFile ""
  StrCpy $AdminRecoveryDir ""

cleanup_recovery_done:
FunctionEnd

Function un.RecoverAdminMode
  IfFileExists "$AdminRecoveryFile" run_recovery recovery_unavailable

run_recovery:
  nsExec::ExecToLog '"$AdminRecoveryFile" --recover-admin-mode'
  Pop $1
  StrCmp $1 0 recovery_done
  Call un.CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; close Codex++ and try again."

recovery_unavailable:
  Call un.CleanupSecureRecoveryDirectory
  Abort "Administrator mode recovery failed; the recovery launcher is unavailable."

recovery_done:
FunctionEnd

Function un.TryRecoverAdminMode
  IfFileExists "$AdminRecoveryFile" try_run_recovery try_recovery_done

try_run_recovery:
  nsExec::ExecToLog /TIMEOUT=30000 '"$AdminRecoveryFile" --recover-admin-mode'
  Pop $1
  StrCmp $1 0 try_recovery_done
  DetailPrint "Administrator mode is still active; closing Codex++ before retrying recovery."

try_recovery_done:
FunctionEnd

Function un.StopRunningCodexPlus
  DetailPrint "Closing running Codex++, recovery helpers, and Codex..."
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-plus-manager.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-plus.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-recovery.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-admin-shim.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM ChatGPT.exe /T /F'
  Pop $0
  Sleep 1000
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-plus-manager.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-plus.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-recovery.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM codex-plus-admin-shim.exe /T /F'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM ChatGPT.exe /T /F'
  Pop $0
  Sleep 500
FunctionEnd

Section "Install"
  Call DetectPowerShell7
  StrCmp $PowerShell7Installed 1 install_powershell7_skip install_powershell7_required

install_powershell7_required:
  DetailPrint "未检测到本机 PowerShell 7，强制安装内置 PowerShell 7。"
  Goto install_powershell7_done

install_powershell7_skip:
  DetailPrint "检测到本机已安装 PowerShell 7，跳过安装内置 PowerShell 7。"

install_powershell7_done:
  Call CreateSecureRecoveryDirectory
  SetOutPath "$AdminRecoveryDir"
  File /oname=codex-plus-recovery.exe "${ROOT}\dist\windows\app\codex-plus-plus.exe"
  Call ProtectSecureRecoveryFile

  Call TryRecoverAdminMode
  Call StopRunningCodexPlus
  Call RecoverAdminMode
  Call CleanupSecureRecoveryDirectory

  SetOutPath "$INSTDIR"
  File "${ROOT}\dist\windows\app\codex-plus-plus.exe"
  File "${ROOT}\dist\windows\app\codex-plus-plus-manager.exe"
  File "${ROOT}\dist\windows\app\codex-plus-admin-shim.exe"
  SetOutPath "$INSTDIR\admin-terminal"
  File /oname=pwsh.exe "${ROOT}\dist\windows\app\admin-terminal\pwsh.exe"
  StrCmp $PowerShell7Installed 1 install_runtime_skip install_runtime_required

install_runtime_required:
  DetailPrint "正在安装内置 PowerShell 7 runtime。"
  SetOutPath "$INSTDIR\runtime\powershell7"
  File /r "${ROOT}\dist\windows\app\runtime\powershell7\*"
  Goto install_runtime_done

install_runtime_skip:
  DetailPrint "已跳过内置 PowerShell 7 runtime。"

install_runtime_done:
  SetOutPath "$INSTDIR"

  Delete "$DESKTOP\Codex++ 绠＄悊宸ュ叿.lnk"
  Delete "$SMPROGRAMS\Codex++\Codex++ 绠＄悊宸ュ叿.lnk"

  CreateShortcut "$DESKTOP\Codex++.lnk" "$INSTDIR\codex-plus-plus.exe" "" "$INSTDIR\codex-plus-plus.exe"
  CreateShortcut "$DESKTOP\Codex++ 管理工具.lnk" "$INSTDIR\codex-plus-plus-manager.exe" "" "$INSTDIR\codex-plus-plus-manager.exe"
  CreateDirectory "$SMPROGRAMS\Codex++"
  CreateShortcut "$SMPROGRAMS\Codex++\Codex++.lnk" "$INSTDIR\codex-plus-plus.exe" "" "$INSTDIR\codex-plus-plus.exe"
  CreateShortcut "$SMPROGRAMS\Codex++\Codex++ 管理工具.lnk" "$INSTDIR\codex-plus-plus-manager.exe" "" "$INSTDIR\codex-plus-plus-manager.exe"
  CreateShortcut "$SMPROGRAMS\Codex++\卸载 Codex++.lnk" "$INSTDIR\uninstall.exe" "" "$INSTDIR\codex-plus-plus-manager.exe"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "Software\Codex++" "InstallDir" "$INSTDIR"
  WriteRegDWORD HKCU "Software\Codex++" "AdminRecoveryCli" 1
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Codex++" "DisplayName" "Codex++"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Codex++" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Codex++" "Publisher" "BigPizzaV3"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Codex++" "DisplayIcon" "$INSTDIR\codex-plus-plus-manager.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Codex++" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Codex++" "UninstallString" "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
  Call un.CreateSecureRecoveryDirectory
  SetOutPath "$AdminRecoveryDir"
  File /oname=codex-plus-recovery.exe "${ROOT}\dist\windows\app\codex-plus-plus.exe"
  Call un.ProtectSecureRecoveryFile

  Call un.TryRecoverAdminMode
  Call un.StopRunningCodexPlus
  Call un.RecoverAdminMode
  Call un.CleanupSecureRecoveryDirectory

  Delete "$DESKTOP\Codex++.lnk"
  Delete "$DESKTOP\Codex++ 管理工具.lnk"
  Delete "$DESKTOP\Codex++ 绠＄悊宸ュ叿.lnk"
  Delete "$SMPROGRAMS\Codex++\Codex++.lnk"
  Delete "$SMPROGRAMS\Codex++\Codex++ 管理工具.lnk"
  Delete "$SMPROGRAMS\Codex++\Codex++ 绠＄悊宸ュ叿.lnk"
  Delete "$SMPROGRAMS\Codex++\卸载 Codex++.lnk"
  RMDir "$SMPROGRAMS\Codex++"

  Delete "$INSTDIR\codex-plus-plus.exe"
  Delete "$INSTDIR\codex-plus-plus-manager.exe"
  Delete "$INSTDIR\codex-plus-admin-shim.exe"
  Delete "$INSTDIR\admin-terminal\pwsh.exe"
  RMDir "$INSTDIR\admin-terminal"
  RMDir /r "$INSTDIR\runtime\powershell7"
  RMDir "$INSTDIR\runtime"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Codex++"
  DeleteRegKey HKCU "Software\Codex++"
SectionEnd
