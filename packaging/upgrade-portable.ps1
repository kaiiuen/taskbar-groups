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
$protectedDirectories = @('config', 'Shortcuts', 'JITComp')

function Get-FullPath([string] $Path) { return [IO.Path]::GetFullPath($Path) }
function Test-SafeZipEntry([string] $Name) {
    $normalized = $Name.Replace('\', '/')
    return -not ($normalized.StartsWith('/') -or $normalized -match '^[A-Za-z]:/' -or $normalized -match '(^|/)\.\.(/|$)')
}

$archivePath = Get-FullPath $Archive
$installPath = Get-FullPath $InstallPath
if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) { throw "Archive was not found: $archivePath" }
if (-not (Test-Path -LiteralPath $installPath -PathType Container)) { throw "Installation directory was not found: $installPath" }
if (-not (Test-Path -LiteralPath (Join-Path $installPath 'taskbar-groups.exe') -PathType Leaf)) { throw "Installation does not contain taskbar-groups.exe: $installPath" }

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
} finally { $zip.Dispose() }

if ($DryRun) {
    Write-Output "Dry run: would upgrade $installPath from $archivePath; config, Shortcuts, and JITComp would be preserved."
    return
}
if (-not $PSCmdlet.ShouldProcess($installPath, "Upgrade portable application from $archivePath")) { return }

$parent = Split-Path -Parent $installPath
$stage = Join-Path ([IO.Path]::GetTempPath()) ("taskbar-groups-upgrade-" + [guid]::NewGuid().ToString('N'))
$backup = Join-Path $parent ('.' + (Split-Path -Leaf $installPath) + '.backup-' + [guid]::NewGuid().ToString('N'))
$oldMoved = $false
$newMoved = $false
try {
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    Expand-Archive -LiteralPath $archivePath -DestinationPath $stage -Force
    if (-not (Test-Path -LiteralPath (Join-Path $stage 'taskbar-groups.exe') -PathType Leaf)) {
        throw 'Extracted archive is missing taskbar-groups.exe at its root.'
    }

    Move-Item -LiteralPath $installPath -Destination $backup
    $oldMoved = $true
    Move-Item -LiteralPath $stage -Destination $installPath
    $newMoved = $true
    $stage = $null

    foreach ($directory in $protectedDirectories) {
        $oldData = Join-Path $backup $directory
        $newData = Join-Path $installPath $directory
        if (Test-Path -LiteralPath $oldData -PathType Container) {
            if (Test-Path -LiteralPath $newData) { Remove-Item -LiteralPath $newData -Recurse -Force }
            Copy-Item -LiteralPath $oldData -Destination $newData -Recurse -Force
        }
    }
    Remove-Item -LiteralPath $backup -Recurse -Force
    $oldMoved = $false
    Write-Output "Upgraded portable application at $installPath; protected user data was preserved."
} catch {
    $message = $_.Exception.Message
    try {
        if ($newMoved -and (Test-Path -LiteralPath $installPath)) { Remove-Item -LiteralPath $installPath -Recurse -Force }
        if ($oldMoved -and (Test-Path -LiteralPath $backup)) { Move-Item -LiteralPath $backup -Destination $installPath }
    } catch {
        throw "Portable upgrade failed and rollback also failed. Original backup: $backup. Upgrade error: $message; rollback error: $($_.Exception.Message)"
    }
    throw "Portable upgrade failed; the original installation was restored: $message"
} finally {
    if ($null -ne $stage -and (Test-Path -LiteralPath $stage)) {
        Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
    }
    if (Test-Path -LiteralPath $backup) {
        Remove-Item -LiteralPath $backup -Recurse -Force -ErrorAction SilentlyContinue
    }
}
