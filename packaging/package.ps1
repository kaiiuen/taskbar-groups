[CmdletBinding()]
param(
    [string]$OutputDirectory = "artifacts",
    [string]$Target = "x86_64-pc-windows-msvc"
)

$ErrorActionPreference = "Stop"
$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot ".." )).Path
$manifestPath = Join-Path $repositoryRoot "Cargo.toml"
$releaseMetadataPath = Join-Path $PSScriptRoot "release-metadata.json"
$releaseMetadata = Get-Content -LiteralPath $releaseMetadataPath -Raw | ConvertFrom-Json
$metadata = & cargo metadata --manifest-path $manifestPath --no-deps --format-version 1 | ConvertFrom-Json
$package = @($metadata.packages | Where-Object { $_.name -eq "taskbar-groups" })[0]
if ($null -eq $package) {
    throw "Could not find the taskbar-groups package in Cargo metadata."
}

$version = $package.version
if ($Target -ne "x86_64-pc-windows-msvc") {
    throw "Target $Target is not supported by the release contract; only x86_64-pc-windows-msvc is implemented."
}
if ($version -ne $releaseMetadata.version) {
    throw "Cargo version $version does not match packaging/release-metadata.json version $($releaseMetadata.version)."
}
$sourceCommit = (& git -C $repositoryRoot rev-parse HEAD).Trim()
$sourceState = (& git -C $repositoryRoot status --porcelain)
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

$buildMetadata = [ordered]@{
    schemaVersion = 1
    product = $releaseMetadata.product
    version = $version
    source = [ordered]@{
        repository = $releaseMetadata.source.repository
        ref = $releaseMetadata.source.ref
        commit = $sourceCommit
        workingTree = if ($sourceState) { "modified" } else { "clean" }
    }
    target = $Target
    artifact = $archiveName
    toolchain = (& rustc -Vv | Out-String).Trim()
}
$buildMetadata | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $stagingRoot "RELEASE-METADATA.json") -Encoding utf8

# Keep the portable runtime layout visible in the archive. The application resolves
# these directories beside its executable and creates them on first launch.
foreach ($directory in @("config", "Shortcuts", "JITComp")) {
    $path = Join-Path $stagingRoot $directory
    New-Item -ItemType Directory -Force -Path $path | Out-Null
    Set-Content -LiteralPath (Join-Path $path ".keep") -Value "" -NoNewline
}

$archiveInputs = Get-ChildItem -LiteralPath $stagingRoot -Force | Sort-Object Name
Compress-Archive -Path $archiveInputs.FullName -DestinationPath $archivePath -CompressionLevel Optimal
$hash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
Set-Content -LiteralPath (Join-Path $outputRoot "manifest.json") -Encoding utf8 -Value (@{
    schemaVersion = 1
    product = $releaseMetadata.product
    version = $version
    sourceCommit = $sourceCommit
    target = $Target
    file = $archiveName
    sha256 = $hash
} | ConvertTo-Json)
Write-Host "Created $archivePath"
Write-Host "SHA256 $hash"
