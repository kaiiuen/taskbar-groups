[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Archive,
    [string]$Manifest,
    [string]$ScenarioRoot
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archivePath = (Resolve-Path $Archive).Path
$zip = [System.IO.Compression.ZipFile]::OpenRead($archivePath)
$temporaryScenario = $false
try {
    $entries = @($zip.Entries | ForEach-Object { $_.FullName.Replace('\', '/') })
    $required = @(
        "taskbar-groups.exe",
        "LICENSE",
        "RELEASE-METADATA.json",
        "config/.keep",
        "Shortcuts/.keep",
        "JITComp/.keep"
    )

    foreach ($entry in $required) {
        if ($entries -notcontains $entry) { throw "Archive is missing required entry: $entry" }
    }

    foreach ($entry in $entries) {
        $normalizedEntry = $entry.ToLowerInvariant()
        if ($normalizedEntry -match '(^|/)(reference|src)(/|$)' -or
            $normalizedEntry -match '(^|/)(cargo\.toml|cargo\.lock)$' -or
            $normalizedEntry -match '\.pdb$') {
            throw "Archive contains forbidden entry: $entry"
        }
    }

    $repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot ".." )).Path
    $cargoManifestPath = Join-Path $repositoryRoot "Cargo.toml"
    $releaseMetadataPath = Join-Path $PSScriptRoot "release-metadata.json"
    $releaseMetadata = Get-Content -LiteralPath $releaseMetadataPath -Raw | ConvertFrom-Json
    $cargoMetadata = & cargo metadata --manifest-path $cargoManifestPath --no-deps --format-version 1 | ConvertFrom-Json
    $package = @($cargoMetadata.packages | Where-Object { $_.name -eq "taskbar-groups" })[0]
    if ($null -eq $package) { throw "Could not find taskbar-groups in Cargo metadata." }

    $version = [string]$package.version
    $archiveName = [System.IO.Path]::GetFileNameWithoutExtension($archivePath)
    $expectedArchive = "taskbar-groups-v{0}-x86_64-pc-windows-msvc" -f $version
    if ($archiveName -cne $expectedArchive) {
        throw "Archive name $archiveName does not match expected package $expectedArchive."
    }
    if ([string]$releaseMetadata.version -cne $version) {
        throw "Cargo version $version does not match release metadata version $($releaseMetadata.version)."
    }

    $unexpected = @($entries | Where-Object {
        $_ -notin $required -and $_ -notmatch '/$'
    })
    if ($unexpected.Count -gt 0) {
        throw "Archive contains unexpected files: $($unexpected -join ', ')"
    }

    $metadataEntry = $zip.GetEntry("RELEASE-METADATA.json")
    $metadataReader = [System.IO.StreamReader]::new($metadataEntry.Open())
    try { $metadata = $metadataReader.ReadToEnd() | ConvertFrom-Json }
    finally { $metadataReader.Dispose() }
    if ([string]$metadata.product -cne [string]$releaseMetadata.product) { throw "Release metadata product is incorrect." }
    if ([string]$metadata.version -cne $version) { throw "Release metadata version is incorrect." }
    if ([string]$metadata.target -cne "x86_64-pc-windows-msvc") { throw "Release metadata target is not the supported MSVC target." }
    if ([string]$metadata.artifact -cne ([System.IO.Path]::GetFileName($archivePath))) { throw "Release metadata artifact name does not match the archive." }
    if ([string]$metadata.source.commit -notmatch '^[0-9a-f]{40}$') { throw "Release metadata source commit is not a full Git SHA-1." }
    if ([string]$metadata.source.repository -cne [string]$releaseMetadata.source.repository) { throw "Release metadata repository is incorrect." }
    if ([string]$metadata.source.ref -cne [string]$releaseMetadata.source.ref) { throw "Release metadata ref is incorrect." }

    if ($Manifest) {
        $manifestPath = (Resolve-Path $Manifest).Path
        $manifestDocument = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
        $actualHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
        $manifestFileName = [System.IO.Path]::GetFileName($archivePath)
        $fileMatches = [string]$manifestDocument.file -ceq $manifestFileName
        $hashMatches = [string]$manifestDocument.sha256 -ceq [string]$actualHash
        $manifestVersionMatches = [string]$manifestDocument.version -ceq $version
        $manifestTargetMatches = [string]$manifestDocument.target -ceq "x86_64-pc-windows-msvc"
        $manifestCommitMatches = [string]$manifestDocument.sourceCommit -ceq [string]$metadata.source.commit
        if (-not ($fileMatches -and $hashMatches -and $manifestVersionMatches -and $manifestTargetMatches -and $manifestCommitMatches)) {
            throw "Release manifest does not match archive filename, version, target, source commit, or SHA-256."
        }
    }

    if (-not $ScenarioRoot) {
        $ScenarioRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("taskbar-groups-validation-" + [Guid]::NewGuid().ToString("N"))
        $temporaryScenario = $true
    }
    $installRoot = Join-Path $ScenarioRoot "install"
    New-Item -ItemType Directory -Force -Path $ScenarioRoot | Out-Null

    # Clean install: extraction must produce the documented executable and directories.
    Expand-Archive -LiteralPath $archivePath -DestinationPath $installRoot -Force
    foreach ($path in @("taskbar-groups.exe", "config", "Shortcuts", "JITComp")) {
        if (-not (Test-Path (Join-Path $installRoot $path))) { throw "Clean install is missing $path." }
    }

    # Upgrade: user data beside the executable must survive replacing application files.
    $sentinel = Join-Path $installRoot "config\validation-sentinel.txt"
    Set-Content -LiteralPath $sentinel -Value "preserve" -NoNewline
    Expand-Archive -LiteralPath $archivePath -DestinationPath $installRoot -Force
    if (-not (Test-Path $sentinel)) { throw "Upgrade removed user data from config/." }

    # Uninstall: a manual portable uninstall removes the install directory, not a parent.
    Remove-Item -LiteralPath $installRoot -Recurse -Force
    if (Test-Path $installRoot) { throw "Uninstall left the install directory behind." }
    Write-Host "Validated archive contents and clean install/upgrade/uninstall scenario: $archivePath"
}
finally {
    if ($zip -is [System.IDisposable]) { $zip.Dispose() }
    if ($temporaryScenario -and (Test-Path $ScenarioRoot)) { Remove-Item -LiteralPath $ScenarioRoot -Recurse -Force }
}
