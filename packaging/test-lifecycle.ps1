[CmdletBinding()]
param(
    [switch] $KeepArtifacts
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$packageScript = Join-Path $PSScriptRoot 'package.ps1'
$installScript = Join-Path $PSScriptRoot 'install-portable.ps1'
$upgradeScript = Join-Path $PSScriptRoot 'upgrade-portable.ps1'
$uninstallScript = Join-Path $PSScriptRoot 'uninstall-portable.ps1'
$runId = [Guid]::NewGuid().ToString('N')
$artifactDirectoryName = 'packaging-lifecycle-' + $runId
$artifactDirectory = Join-Path $repositoryRoot $artifactDirectoryName
$scenarioRoot = Join-Path ([IO.Path]::GetTempPath()) ('taskbar-groups-lifecycle-' + $runId)
$installPath = Join-Path $scenarioRoot 'install'
$purgeInstallPath = Join-Path $scenarioRoot 'purge-install'
$noiseLog = Join-Path $scenarioRoot 'lifecycle-output.log'

function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) { throw "Assertion failed: $Message" }
}

function Assert-FileText([string] $Path, [string] $Expected, [string] $Message) {
    Assert-True (Test-Path -LiteralPath $Path -PathType Leaf) $Message
    $actual = Get-Content -LiteralPath $Path -Raw
    Assert-True ($actual -ceq $Expected) $Message
}

function Invoke-PackagingScript([string] $Script, [hashtable] $Arguments) {
    $savedErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    if ($Script -eq $packageScript) {
        $output = @(& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $Script @Arguments 2>&1)
    } else {
        $output = @(& $Script @Arguments 2>&1)
    }
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $savedErrorActionPreference
    $output | ForEach-Object { $_.ToString() } | Add-Content -LiteralPath $noiseLog
    if ($exitCode -ne 0) {
        throw "Script failed with exit code ${exitCode}: $Script`n$($output -join "`n")"
    }
    return $output
}

function Invoke-LifecycleScript([string] $Script, [hashtable] $Arguments) {
    return Invoke-PackagingScript $Script $Arguments
}

try {
    New-Item -ItemType Directory -Path $scenarioRoot -Force | Out-Null
    Set-Content -LiteralPath $noiseLog -Value "Lifecycle output for $runId" -NoNewline

    Write-Host 'Building disposable release archive with packaging/package.ps1...'
    Invoke-PackagingScript $packageScript @{ OutputDirectory = $artifactDirectoryName } | ForEach-Object { Write-Host $_ }
    $cargoMetadata = & cargo metadata --manifest-path (Join-Path $repositoryRoot 'Cargo.toml') --no-deps --format-version 1 | ConvertFrom-Json
    $package = @($cargoMetadata.packages | Where-Object { $_.name -eq 'taskbar-groups' })[0]
    $archive = Join-Path $artifactDirectory ('taskbar-groups-v{0}-x86_64-pc-windows-msvc.zip' -f $package.version)
    Assert-True (Test-Path -LiteralPath $archive -PathType Leaf) 'package.ps1 did not create the expected archive.'

    Write-Host 'Running install-portable end-to-end...'
    Invoke-LifecycleScript $installScript @{ Archive = $archive; InstallPath = $installPath; Confirm = $false } | ForEach-Object { Write-Host $_ }
    foreach ($directory in @('config', 'Shortcuts', 'JITComp')) {
        Assert-True (Test-Path -LiteralPath (Join-Path $installPath $directory) -PathType Container) "Install omitted $directory."
        Set-Content -LiteralPath (Join-Path $installPath "$directory\lifecycle-sentinel.txt") -Value "preserve-$directory" -NoNewline
    }
    Write-Host 'Running upgrade-portable end-to-end and checking all protected data...'
    Invoke-LifecycleScript $upgradeScript @{ Archive = $archive; InstallPath = $installPath; Confirm = $false } | ForEach-Object { Write-Host $_ }
    foreach ($directory in @('config', 'Shortcuts', 'JITComp')) {
        Assert-FileText (Join-Path $installPath "$directory\lifecycle-sentinel.txt") "preserve-$directory" "Upgrade lost $directory data."
    }
    Set-Content -LiteralPath (Join-Path $installPath 'unknown-user-file.txt') -Value 'retain-unknown' -NoNewline

    Write-Host 'Running default uninstall and checking retention semantics...'
    Invoke-LifecycleScript $uninstallScript @{ InstallPath = $installPath; Confirm = $false } | ForEach-Object { Write-Host $_ }
    Assert-True (Test-Path -LiteralPath $installPath -PathType Container) 'Default uninstall removed the install directory despite retained data.'
    foreach ($directory in @('config', 'Shortcuts', 'JITComp')) {
        Assert-FileText (Join-Path $installPath "$directory\lifecycle-sentinel.txt") "preserve-$directory" "Default uninstall lost $directory data."
    }
    Assert-FileText (Join-Path $installPath 'unknown-user-file.txt') 'retain-unknown' 'Default uninstall removed an unknown user file.'
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $installPath 'taskbar-groups.exe'))) 'Default uninstall retained the application executable.'

    Write-Host 'Running a second install followed by purge uninstall...'
    Invoke-LifecycleScript $installScript @{ Archive = $archive; InstallPath = $purgeInstallPath; Confirm = $false } | ForEach-Object { Write-Host $_ }
    foreach ($directory in @('config', 'Shortcuts', 'JITComp')) {
        Set-Content -LiteralPath (Join-Path $purgeInstallPath "$directory\purge-sentinel.txt") -Value 'delete-me' -NoNewline
    }
    Invoke-LifecycleScript $uninstallScript @{ InstallPath = $purgeInstallPath; PurgeData = $true; Confirm = $false } | ForEach-Object { Write-Host $_ }
    Assert-True (-not (Test-Path -LiteralPath $purgeInstallPath)) 'Purge uninstall did not remove the install directory and protected data.'

    $noise = Get-Content -LiteralPath $noiseLog -Raw
    if ($noise -match 'The directory or file cannot be created\.') {
        Write-Host 'Observed noisy message: The directory or file cannot be created.'
        Write-Host 'Isolation: message came from captured packaging/lifecycle command output; lifecycle commands still exited successfully.'
    } else {
        Write-Host 'Noisy-message probe: exact message was not reproduced in this run.'
    }
    Write-Host 'Portable lifecycle validation passed.'
}
finally {
    if (-not $KeepArtifacts -and (Test-Path -LiteralPath $artifactDirectory)) {
        Remove-Item -LiteralPath $artifactDirectory -Recurse -Force -ErrorAction SilentlyContinue
    }
    if (Test-Path -LiteralPath $scenarioRoot) {
        Remove-Item -LiteralPath $scenarioRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
