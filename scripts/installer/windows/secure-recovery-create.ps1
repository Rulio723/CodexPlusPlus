$ErrorActionPreference = "Stop"
$admins = [Security.Principal.SecurityIdentifier]::new("S-1-5-32-544")
$system = [Security.Principal.SecurityIdentifier]::new("S-1-5-18")
$allow = [Security.AccessControl.AccessControlType]::Allow
$inherit = [Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [Security.AccessControl.InheritanceFlags]::ObjectInherit
$propagation = [Security.AccessControl.PropagationFlags]::None
$full = [Security.AccessControl.FileSystemRights]::FullControl
$security = [Security.AccessControl.DirectorySecurity]::new()
$security.SetOwner($admins)
$security.SetAccessRuleProtection($true, $false)
$security.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($system, $full, $inherit, $propagation, $allow))
$security.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new($admins, $full, $inherit, $propagation, $allow))
$windows = [Environment]::GetFolderPath([Environment+SpecialFolder]::Windows)
if ([string]::IsNullOrWhiteSpace($windows)) {
    throw "trusted Windows directory is unavailable"
}
$base = [IO.Path]::Combine($windows, "Temp")
if (-not [IO.Directory]::Exists($base) -or ([IO.File]::GetAttributes($base) -band [IO.FileAttributes]::ReparsePoint)) {
    throw "trusted Windows temporary directory is unavailable"
}
do {
    $path = [IO.Path]::Combine($base, "CodexPlusPlus-Recovery-" + [Guid]::NewGuid().ToString("N"))
} while ([IO.Directory]::Exists($path))
[IO.Directory]::CreateDirectory($path, $security) | Out-Null
$created = $true
$actual = [IO.Directory]::GetAccessControl($path, [Security.AccessControl.AccessControlSections]::All)
$actualOwner = $actual.GetOwner([Security.Principal.SecurityIdentifier]).Value
$rules = @($actual.GetAccessRules($true, $false, [Security.Principal.SecurityIdentifier]))
try {
    if (($actualOwner -ne $admins.Value) -or -not $actual.AreAccessRulesProtected -or $rules.Count -ne 2) {
        throw "secure recovery directory ACL mismatch"
    }
    foreach ($rule in $rules) {
        $known = ($rule.IdentityReference.Value -eq $admins.Value) -or ($rule.IdentityReference.Value -eq $system.Value)
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
