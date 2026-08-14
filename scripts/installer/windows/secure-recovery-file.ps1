$ErrorActionPreference = "Stop"
$path = $env:CODEXPP_RECOVERY_FILE
if ([string]::IsNullOrWhiteSpace($path) -or -not [IO.File]::Exists($path)) {
    throw "secure recovery file is missing"
}
$path = [IO.Path]::GetFullPath($path)
$windows = [Environment]::GetFolderPath([Environment+SpecialFolder]::Windows)
if ([string]::IsNullOrWhiteSpace($windows)) {
    throw "trusted Windows directory is unavailable"
}
$base = [IO.Path]::GetFullPath([IO.Path]::Combine($windows, "Temp"))
$directory = [IO.Path]::GetDirectoryName($path)
$fileName = [IO.Path]::GetFileName($path)
$runtimeBundlePolicy = $env:CODEXPP_RECOVERY_FILE_POLICY -ceq "administrator-runtime-bundle"
$allowedFileNames = if ($runtimeBundlePolicy) {
    @(
        "codex-plus-recovery.exe",
        "codex-plus-computer-use.exe",
        "codex-code-mode-host.exe",
        "codex-command-runner.exe",
        "codex-windows-sandbox-setup.exe",
        "rg.exe"
    )
} else {
    @("codex-plus-recovery.exe")
}
if ((-not ($allowedFileNames -ccontains $fileName)) -or
    [IO.Path]::GetDirectoryName($directory) -ine $base -or
    [IO.Path]::GetFileName($directory) -cnotmatch '^CodexPlusPlus-Recovery-[0-9a-f]{32}$' -or
    ([IO.File]::GetAttributes($path) -band [IO.FileAttributes]::ReparsePoint) -or
    ([IO.File]::GetAttributes($directory) -band [IO.FileAttributes]::ReparsePoint)) {
    throw "secure recovery file path is invalid"
}
$admins = [Security.Principal.SecurityIdentifier]::new("S-1-5-32-544")
$system = [Security.Principal.SecurityIdentifier]::new("S-1-5-18")
$allow = [Security.AccessControl.AccessControlType]::Allow
$full = [Security.AccessControl.FileSystemRights]::FullControl
$directorySecurity = [IO.Directory]::GetAccessControl($directory, [Security.AccessControl.AccessControlSections]::All)
$directoryOwner = $directorySecurity.GetOwner([Security.Principal.SecurityIdentifier]).Value
$directoryRules = @($directorySecurity.GetAccessRules($true, $false, [Security.Principal.SecurityIdentifier]))
$inherit = [Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [Security.AccessControl.InheritanceFlags]::ObjectInherit
$propagation = [Security.AccessControl.PropagationFlags]::None
if (($directoryOwner -ne $admins.Value) -or -not $directorySecurity.AreAccessRulesProtected -or $directoryRules.Count -ne 2) {
    throw "secure recovery parent ACL mismatch"
}
foreach ($rule in $directoryRules) {
    $known = ($rule.IdentityReference.Value -eq $admins.Value) -or ($rule.IdentityReference.Value -eq $system.Value)
    if (-not $known -or $rule.IsInherited -or $rule.AccessControlType -ne $allow -or $rule.InheritanceFlags -ne $inherit -or $rule.PropagationFlags -ne $propagation -or ($rule.FileSystemRights -band $full) -ne $full) {
        throw "secure recovery parent ACE mismatch"
    }
}
$inherited = [IO.File]::GetAccessControl($path, [Security.AccessControl.AccessControlSections]::All)
$inheritedOwner = $inherited.GetOwner([Security.Principal.SecurityIdentifier]).Value
$inheritedRules = @($inherited.GetAccessRules($false, $true, [Security.Principal.SecurityIdentifier]))
if ($inheritedOwner -ne $admins.Value -or $inheritedRules.Count -ne 2) {
    throw "secure recovery inherited ACL mismatch"
}
foreach ($rule in $inheritedRules) {
    $known = ($rule.IdentityReference.Value -eq $admins.Value) -or ($rule.IdentityReference.Value -eq $system.Value)
    if (-not $known -or -not $rule.IsInherited -or $rule.AccessControlType -ne $allow -or ($rule.FileSystemRights -band $full) -ne $full) {
        throw "secure recovery inherited ACE mismatch"
    }
}
$security = [Security.AccessControl.FileSecurity]::new()
$security.SetOwner($admins)
$security.SetAccessRuleProtection($true, $false)
$security.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($system, $full, $allow))
$security.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($admins, $full, $allow))
[IO.File]::SetAccessControl($path, $security)
$actual = [IO.File]::GetAccessControl($path, [Security.AccessControl.AccessControlSections]::All)
$actualOwner = $actual.GetOwner([Security.Principal.SecurityIdentifier]).Value
$rules = @($actual.GetAccessRules($true, $false, [Security.Principal.SecurityIdentifier]))
if ($actualOwner -ne $admins.Value -or -not $actual.AreAccessRulesProtected -or $rules.Count -ne 2) {
    throw "secure recovery file ACL mismatch"
}
foreach ($rule in $rules) {
    $known = $rule.IdentityReference.Value -eq $admins.Value -or $rule.IdentityReference.Value -eq $system.Value
    if (-not $known -or $rule.IsInherited -or $rule.AccessControlType -ne $allow -or $rule.InheritanceFlags -ne [Security.AccessControl.InheritanceFlags]::None -or $rule.PropagationFlags -ne [Security.AccessControl.PropagationFlags]::None -or ($rule.FileSystemRights -band $full) -ne $full) {
        throw "secure recovery file ACE mismatch"
    }
}
