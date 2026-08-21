param(
    [string]$OutputPath = (Join-Path $PSScriptRoot "secure-recovery-acl.nsh")
)

$ErrorActionPreference = "Stop"

function Encode-PowerShellPayload([string]$Path) {
    [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes([IO.File]::ReadAllText($Path)))
}

function Add-PayloadMacros(
    [System.Collections.Generic.List[string]]$Lines,
    [string]$Kind,
    [string]$Payload
) {
    $chunks = for ($offset = 0; $offset -lt $Payload.Length; $offset += 700) {
        $Payload.Substring($offset, [Math]::Min(700, $Payload.Length - $offset))
    }
    if ($chunks.Count -lt 1 -or $chunks.Count -gt 64) {
        throw "secure recovery payload chunk count is invalid"
    }

    $upper = $Kind.ToUpperInvariant()
    $Lines.Add(('!define SECURE_RECOVERY_{0}_CHUNK_COUNT "{1}"' -f $upper, $chunks.Count))
    for ($index = 0; $index -lt $chunks.Count; $index++) {
        $Lines.Add(('!define SECURE_RECOVERY_{0}_COMMAND_{1} "{2}"' -f $upper, $index, $chunks[$index]))
    }
    $Lines.Add("")
    $Lines.Add("!macro SetSecureRecovery${Kind}Payload FAILURE")
    $Lines.Add('  System::Call ''Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_SCRIPT_COUNT", w "${SECURE_RECOVERY_' + $upper + '_CHUNK_COUNT}") i.r9''')
    $Lines.Add('  StrCmp $9 0 ${FAILURE}')
    for ($index = 0; $index -lt $chunks.Count; $index++) {
        $Lines.Add('  System::Call ''Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_SCRIPT_' + $index + '", w "${SECURE_RECOVERY_' + $upper + '_COMMAND_' + $index + '}") i.r9''')
        $Lines.Add('  StrCmp $9 0 ${FAILURE}')
    }
    $Lines.Add("!macroend")
    $Lines.Add("!macro ClearSecureRecovery${Kind}Payload FAILURE")
    $Lines.Add('  System::Call ''Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_SCRIPT_COUNT", p 0) i.r9''')
    $Lines.Add('  StrCmp $9 0 ${FAILURE}')
    for ($index = 0; $index -lt $chunks.Count; $index++) {
        $Lines.Add('  System::Call ''Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_SCRIPT_' + $index + '", p 0) i.r9''')
        $Lines.Add('  StrCmp $9 0 ${FAILURE}')
    }
    $Lines.Add("!macroend")
    $Lines.Add("!macro ClearSecureRecovery${Kind}PayloadUnchecked")
    $Lines.Add('  System::Call ''Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_SCRIPT_COUNT", p 0)''')
    for ($index = 0; $index -lt $chunks.Count; $index++) {
        $Lines.Add('  System::Call ''Kernel32::SetEnvironmentVariableW(w "CODEXPP_RECOVERY_SCRIPT_' + $index + '", p 0)''')
    }
    $Lines.Add("!macroend")
    $Lines.Add("")
}

$bootstrap = Join-Path $PSScriptRoot "secure-recovery-bootstrap.ps1"
$create = Join-Path $PSScriptRoot "secure-recovery-create.ps1"
$file = Join-Path $PSScriptRoot "secure-recovery-file.ps1"
$lines = [System.Collections.Generic.List[string]]::new()
$lines.Add(('!define SECURE_RECOVERY_BOOTSTRAP_COMMAND "{0}"' -f (Encode-PowerShellPayload $bootstrap)))
Add-PayloadMacros $lines "Create" (Encode-PowerShellPayload $create)
Add-PayloadMacros $lines "File" (Encode-PowerShellPayload $file)

if ($lines[$lines.Count - 1] -eq "") {
    $lines.RemoveAt($lines.Count - 1)
}
[IO.File]::WriteAllText($OutputPath, ($lines -join [Environment]::NewLine) + [Environment]::NewLine, [Text.UTF8Encoding]::new($false))
