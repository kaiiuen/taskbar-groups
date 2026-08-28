[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string] $Archive,

    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string] $InstallPath,

    [switch] $DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-FullPath([string] $Path) {
    return [IO.Path]::GetFullPath($Path)
}

function Test-SafeZipEntry([string] $Name) {
    $normalized = $Name.Replace('\', '/')
    return -not ($normalized.StartsWith('/') -or $normalized -match '^[A-Za-z]:/' -or $normalized -match '(^|/)\.\.(/|$)')
}

$archivePath = Get-FullPath $Archive
$installPath = Get-FullPath $InstallPath
if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) { throw "Archive was not found: $archivePath" }
if ([IO.Path]::GetExtension($archivePath) -ne '.zip') { throw "Archive must be a .zip file: $archivePath" }
if (Test-Path -LiteralPath $installPath) { throw "Install path already exists. Use upgrade-portable.ps1 for an existing installation: $installPath" }

Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::OpenRead($archivePath)
try {
    $entries = @($zip.Entries)
    foreach ($entry in $entries) {
        if (-not (Test-SafeZipEntry $entry.FullName)) { throw "Archive contains an unsafe path: $($entry.FullName)" }
    }
    if ($null -eq ($entries | Where-Object { $_.FullName -eq 'taskbar-groups.exe' })) {
        throw "Archive does not contain taskbar-groups.exe at its root."
    }
    $metadataEntry = $entries | Where-Object { $_.FullName -eq 'RELEASE-METADATA.json' } | Select-Object -First 1
    if ($null -eq $metadataEntry) { throw "Archive does not contain RELEASE-METADATA.json at its root." }
    $reader = [IO.StreamReader]::new($metadataEntry.Open())
    try { $metadata = $reader.ReadToEnd() | ConvertFrom-Json } finally { $reader.Dispose() }
    if ($metadata.target -ne 'x86_64-pc-windows-msvc') {
        throw "Archive target is not the supported x64 MSVC target."
    }
} finally {
    $zip.Dispose()
}

if ($DryRun) {
    Write-Output "Dry run: would install $archivePath to $installPath and preserve executable-relative user data (new target is required)."
    return
}

if (-not $PSCmdlet.ShouldProcess($installPath, "Install portable application from $archivePath")) { return }

$parent = Split-Path -Parent $installPath
if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
}
$stage = Join-Path ([IO.Path]::GetTempPath()) ("taskbar-groups-install-" + [guid]::NewGuid().ToString('N'))
try {
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    Expand-Archive -LiteralPath $archivePath -DestinationPath $stage -Force
    if (-not (Test-Path -LiteralPath (Join-Path $stage 'taskbar-groups.exe') -PathType Leaf)) {
        throw 'Extracted archive is missing taskbar-groups.exe at its root.'
    }
    Move-Item -LiteralPath $stage -Destination $installPath
    $stage = $null
    Write-Output "Installed portable application to $installPath"
} catch {
    throw "Portable install failed: $($_.Exception.Message)"
} finally {
    if ($null -ne $stage -and (Test-Path -LiteralPath $stage)) {
        Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
    }
}
