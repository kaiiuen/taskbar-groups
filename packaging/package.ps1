[CmdletBinding()]
param(
    [string]$OutputDirectory = "artifacts",
    [string]$Target = "x86_64-pc-windows-msvc"
)

$ErrorActionPreference = "Stop"
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot ".." )).Path
$manifestPath = Join-Path $repositoryRoot "Cargo.toml"
$metadata = & cargo metadata --manifest-path $manifestPath --no-deps --format-version 1 | ConvertFrom-Json
$package = @($metadata.packages | Where-Object { $_.name -eq "taskbar-groups" })[0]
if ($null -eq $package) {
    throw "Could not find the taskbar-groups package in Cargo metadata."
}

$version = $package.version
$archiveName = "taskbar-groups-v{0}-{1}.zip" -f $version, $Target
$outputRoot = Join-Path $repositoryRoot $OutputDirectory
$stagingRoot = Join-Path $outputRoot ("taskbar-groups-v{0}-{1}" -f $version, $Target)
$archivePath = Join-Path $outputRoot $archiveName
$binaryPath = Join-Path $repositoryRoot ("target\{0}\release\taskbar-groups.exe" -f $Target)

New-Item -ItemType Directory -Force -Path $outputRoot | Out-Null
if (Test-Path -LiteralPath $stagingRoot) { Remove-Item -LiteralPath $stagingRoot -Recurse -Force }
if (Test-Path -LiteralPath $archivePath) { Remove-Item -LiteralPath $archivePath -Force }

Write-Host "Building taskbar-groups $version for $Target"
& cargo build --locked --release --target $Target --manifest-path $manifestPath
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE." }
if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) {
    throw "Expected release binary was not produced: $binaryPath"
}

New-Item -ItemType Directory -Force -Path $stagingRoot | Out-Null
Copy-Item -LiteralPath $binaryPath -Destination (Join-Path $stagingRoot "taskbar-groups.exe")
Copy-Item -LiteralPath (Join-Path $repositoryRoot "LICENSE") -Destination (Join-Path $stagingRoot "LICENSE")

# Keep the portable runtime layout visible in the archive. The application resolves
# these directories beside its executable and creates them on first launch.
foreach ($directory in @("config", "Shortcuts", "JITComp")) {
    $path = Join-Path $stagingRoot $directory
    New-Item -ItemType Directory -Force -Path $path | Out-Null
    Set-Content -LiteralPath (Join-Path $path ".keep") -Value "" -NoNewline
}

Compress-Archive -Path (Get-ChildItem -LiteralPath $stagingRoot -Force).FullName -DestinationPath $archivePath -CompressionLevel Optimal
Write-Host "Created $archivePath"
