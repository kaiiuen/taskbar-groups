[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string] $InstallPath,

    [switch] $PurgeData,
    [switch] $DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$protectedDirectories = @('config', 'Shortcuts', 'JITComp')
$applicationFiles = @('taskbar-groups.exe', 'LICENSE', 'RELEASE-METADATA.json')
$installPath = [IO.Path]::GetFullPath($InstallPath)
if (-not (Test-Path -LiteralPath $installPath -PathType Container)) { throw "Installation directory was not found: $installPath" }

$foundApplication = $false
foreach ($file in $applicationFiles) {
    if (Test-Path -LiteralPath (Join-Path $installPath $file)) { $foundApplication = $true; break }
}
if (-not $foundApplication) { throw "No recognized portable application files were found in: $installPath" }

$action = if ($PurgeData) { 'remove application files and permanently delete protected user data' } else { 'remove application files while retaining config, Shortcuts, and JITComp' }
if ($DryRun) {
    Write-Output "Dry run: would $action at $installPath. Unknown files and directories would not be removed."
    return
}
if (-not $PSCmdlet.ShouldProcess($installPath, "Uninstall portable application: $action")) { return }

try {
    foreach ($file in $applicationFiles) {
        $path = Join-Path $installPath $file
        if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force }
    }
    if ($PurgeData) {
        foreach ($directory in $protectedDirectories) {
            $path = Join-Path $installPath $directory
            if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Recurse -Force }
        }
    }

    $remaining = @(Get-ChildItem -LiteralPath $installPath -Force)
    if ($remaining.Count -eq 0) {
        Remove-Item -LiteralPath $installPath -Force
        Write-Output "Uninstalled and removed $installPath"
    } else {
        Write-Output "Removed application files from $installPath; retained $($remaining.Count) user/data item(s)."
    }
} catch {
    throw "Portable uninstall failed: $($_.Exception.Message)"
}
