$ErrorActionPreference = "Stop"
$aclExtensions = [Type]::GetType("System.IO.FileSystemAclExtensions, System.IO.FileSystem.AccessControl", $false)
$path = $env:CODEXPP_RECOVERY_FILE
if ([string]::IsNullOrWhiteSpace($path) -or -not [IO.File]::Exists($path)) {
    throw "secure recovery file is missing"
}
$path = [IO.Path]::GetFullPath($path)
$base = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
if ([string]::IsNullOrWhiteSpace($base)) {
    throw "trusted local application data directory is unavailable"
}
$base = [IO.Path]::GetFullPath($base)
if (-not [IO.Directory]::Exists($base) -or ([IO.File]::GetAttributes($base) -band [IO.FileAttributes]::ReparsePoint)) {
    throw "trusted local application data directory is unavailable"
}
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
$user = [Security.Principal.WindowsIdentity]::GetCurrent().User
if ($null -eq $user) {
    throw "current user SID is unavailable"
}
$userSid = $user.Value
$system = [Security.Principal.SecurityIdentifier]::new("S-1-5-18")
$allow = [Security.AccessControl.AccessControlType]::Allow
$full = [Security.AccessControl.FileSystemRights]::FullControl
$aclSections = [Security.AccessControl.AccessControlSections]::Access -bor [Security.AccessControl.AccessControlSections]::Owner -bor [Security.AccessControl.AccessControlSections]::Group
if ($null -ne $aclExtensions) {
    $directorySecurity = $aclExtensions::GetAccessControl(
        [IO.DirectoryInfo]::new($directory),
        $aclSections
    )
} else {
    $directorySecurity = [IO.Directory]::GetAccessControl(
        $directory,
        $aclSections
    )
}
$directoryOwner = $directorySecurity.GetOwner([Security.Principal.SecurityIdentifier]).Value
$directoryRules = @($directorySecurity.GetAccessRules($true, $false, [Security.Principal.SecurityIdentifier]))
$inherit = [Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [Security.AccessControl.InheritanceFlags]::ObjectInherit
$propagation = [Security.AccessControl.PropagationFlags]::None
if (($directoryOwner -ne $userSid) -or -not $directorySecurity.AreAccessRulesProtected -or $directoryRules.Count -ne 2) {
    throw "secure recovery parent ACL mismatch"
}
foreach ($rule in $directoryRules) {
    $known = ($rule.IdentityReference.Value -eq $userSid) -or ($rule.IdentityReference.Value -eq $system.Value)
    if (-not $known -or $rule.IsInherited -or $rule.AccessControlType -ne $allow -or $rule.InheritanceFlags -ne $inherit -or $rule.PropagationFlags -ne $propagation -or ($rule.FileSystemRights -band $full) -ne $full) {
        throw "secure recovery parent ACE mismatch"
    }
}
$security = [Security.AccessControl.FileSecurity]::new()
$security.SetOwner($user)
$security.SetAccessRuleProtection($true, $false)
$security.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($system, $full, $allow))
$security.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($user, $full, $allow))
if ($null -ne $aclExtensions) {
    $aclExtensions::SetAccessControl([IO.FileInfo]::new($path), $security)
    $actual = $aclExtensions::GetAccessControl(
        [IO.FileInfo]::new($path),
        $aclSections
    )
} else {
    [IO.File]::SetAccessControl($path, $security)
    $actual = [IO.File]::GetAccessControl(
        $path,
        $aclSections
    )
}
$actualOwner = $actual.GetOwner([Security.Principal.SecurityIdentifier]).Value
$rules = @($actual.GetAccessRules($true, $false, [Security.Principal.SecurityIdentifier]))
if ($actualOwner -ne $userSid -or -not $actual.AreAccessRulesProtected -or $rules.Count -ne 2) {
    throw "secure recovery file ACL mismatch"
}
foreach ($rule in $rules) {
    $known = $rule.IdentityReference.Value -eq $userSid -or $rule.IdentityReference.Value -eq $system.Value
    if (-not $known -or $rule.IsInherited -or $rule.AccessControlType -ne $allow -or $rule.InheritanceFlags -ne [Security.AccessControl.InheritanceFlags]::None -or $rule.PropagationFlags -ne [Security.AccessControl.PropagationFlags]::None -or ($rule.FileSystemRights -band $full) -ne $full) {
        throw "secure recovery file ACE mismatch"
    }
}
