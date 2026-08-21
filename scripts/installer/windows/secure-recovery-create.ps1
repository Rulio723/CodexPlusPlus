$ErrorActionPreference = "Stop"
$aclExtensions = [Type]::GetType("System.IO.FileSystemAclExtensions, System.IO.FileSystem.AccessControl", $false)
$user = [Security.Principal.WindowsIdentity]::GetCurrent().User
if ($null -eq $user) {
    throw "current user SID is unavailable"
}
$userSid = $user.Value
$system = [Security.Principal.SecurityIdentifier]::new("S-1-5-18")
$allow = [Security.AccessControl.AccessControlType]::Allow
$inherit = [Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [Security.AccessControl.InheritanceFlags]::ObjectInherit
$propagation = [Security.AccessControl.PropagationFlags]::None
$full = [Security.AccessControl.FileSystemRights]::FullControl
$aclSections = [Security.AccessControl.AccessControlSections]::Access -bor [Security.AccessControl.AccessControlSections]::Owner -bor [Security.AccessControl.AccessControlSections]::Group
$security = [Security.AccessControl.DirectorySecurity]::new()
$security.SetOwner($user)
$security.SetAccessRuleProtection($true, $false)
$security.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($system, $full, $inherit, $propagation, $allow))
$security.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($user, $full, $inherit, $propagation, $allow))
$base = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
if ([string]::IsNullOrWhiteSpace($base)) {
    throw "trusted local application data directory is unavailable"
}
if (-not [IO.Directory]::Exists($base) -or ([IO.File]::GetAttributes($base) -band [IO.FileAttributes]::ReparsePoint)) {
    throw "trusted local application data directory is unavailable"
}
do {
    $path = [IO.Path]::Combine($base, "CodexPlusPlus-Recovery-" + [Guid]::NewGuid().ToString("N"))
} while ([IO.Directory]::Exists($path))
if ($null -ne $aclExtensions) {
    [void]$aclExtensions::CreateDirectory($security, $path)
} else {
    [void][IO.Directory]::CreateDirectory($path, $security)
}
$created = $true
if ($null -ne $aclExtensions) {
    $actual = $aclExtensions::GetAccessControl(
        [IO.DirectoryInfo]::new($path),
        $aclSections
    )
} else {
    $actual = [IO.Directory]::GetAccessControl(
        $path,
        $aclSections
    )
}
$actualOwner = $actual.GetOwner([Security.Principal.SecurityIdentifier]).Value
$rules = @($actual.GetAccessRules($true, $false, [Security.Principal.SecurityIdentifier]))
try {
    if (($actualOwner -ne $userSid) -or -not $actual.AreAccessRulesProtected -or $rules.Count -ne 2) {
        throw "secure recovery directory ACL mismatch"
    }
    foreach ($rule in $rules) {
        $known = ($rule.IdentityReference.Value -eq $userSid) -or ($rule.IdentityReference.Value -eq $system.Value)
        if (-not $known -or $rule.IsInherited -or $rule.AccessControlType -ne $allow -or $rule.InheritanceFlags -ne $inherit -or $rule.PropagationFlags -ne $propagation -or ($rule.FileSystemRights -band $full) -ne $full) {
            throw "secure recovery directory ACE mismatch"
        }
    }
} catch {
    if ($created) {
        [IO.Directory]::Delete($path, $true)
    }
    throw
}
[Console]::Out.Write($path)
