[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Destination,

    [string]$Source
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Source)) {
    $pwsh = Get-Command pwsh -ErrorAction Stop
    $Source = Split-Path -Parent $pwsh.Source
}

$Source = (Resolve-Path -LiteralPath $Source).Path
$pwshPath = Join-Path $Source "pwsh.exe"
if (-not (Test-Path -LiteralPath $pwshPath -PathType Leaf)) {
    throw "PowerShell 7 runtime was not found at $pwshPath"
}

New-Item -ItemType Directory -Force -Path $Destination | Out-Null

# Keep the runtime self-contained while excluding Store package metadata that
# is not used by pwsh.exe and cannot be relocated with the portable runtime.
$excludedFiles = @(
    "AppxBlockMap.xml",
    "AppxManifest.xml",
    "AppxSignature.p7x",
    "build.manifest",
    "build.manifest.sig"
)
$excludedDirectories = @(
    "AppxMetadata",
    "microsoft.system.package.metadata",
    "preview",
    "ref"
)

Get-ChildItem -LiteralPath $Source -Force -File |
    Where-Object { $excludedFiles -notcontains $_.Name } |
    ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $Destination $_.Name) -Force
    }

# Modules contains the built-in cmdlet modules required by an interactive shell.
# Copy the remaining runtime directories too, except the excluded Store-only data.
Get-ChildItem -LiteralPath $Source -Force -Directory |
    Where-Object { $excludedDirectories -notcontains $_.Name } |
    ForEach-Object {
        $targetDirectory = Join-Path $Destination $_.Name
        New-Item -ItemType Directory -Force -Path $targetDirectory | Out-Null
        Copy-Item -Path (Join-Path $_.FullName "*") -Destination $targetDirectory -Recurse -Force
    }

$stagedPwsh = Join-Path $Destination "pwsh.exe"
if (-not (Test-Path -LiteralPath $stagedPwsh -PathType Leaf)) {
    throw "PowerShell 7 staging did not produce $stagedPwsh"
}

& $stagedPwsh -NoLogo -NoProfile -NonInteractive -Command 'if ($PSVersionTable.PSVersion.Major -lt 7) { exit 1 }'
if ($LASTEXITCODE -ne 0) {
    throw "The staged PowerShell 7 runtime failed its startup check"
}

Write-Host "Staged PowerShell runtime from $Source to $Destination"
