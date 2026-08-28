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

    $forbidden = @("reference/", "src/", "Cargo.toml", "Cargo.lock", ".pdb")
    foreach ($entry in $entries) {
        foreach ($prefix in $forbidden) {
            if ($entry -eq $prefix.TrimEnd("/") -or $entry.StartsWith($prefix)) {
                throw "Archive contains forbidden entry: $entry"
            }
        }
    }

    $archiveName = [System.IO.Path]::GetFileNameWithoutExtension($archivePath)
    if ($archiveName -notmatch '^taskbar-groups-v[^-]+-x86_64-pc-windows-msvc$') {
        throw "Archive name is not versioned for the supported MSVC target: $archiveName"
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
    if ($metadata.target -ne "x86_64-pc-windows-msvc") { throw "Release metadata target is not the supported MSVC target." }
    if ($metadata.artifact -ne ([System.IO.Path]::GetFileName($archivePath))) { throw "Release metadata artifact name does not match the archive." }

    if ($Manifest) {
        $manifestPath = (Resolve-Path $Manifest).Path
        $manifestDocument = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
        $actualHash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
        $fileMatches = [string]$manifestDocument.file -ceq [System.IO.Path]::GetFileName($archivePath)
        $hashMatches = [string]$manifestDocument.sha256 -ceq [string]$actualHash
        if (-not ($fileMatches -and $hashMatches)) {
            throw "Release manifest does not match the archive SHA-256 or filename."
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
