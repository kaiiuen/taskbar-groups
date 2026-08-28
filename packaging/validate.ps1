[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Archive
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archivePath = (Resolve-Path $Archive).Path
$zip = [System.IO.Compression.ZipFile]::OpenRead($archivePath)
try {
    $entries = @($zip.Entries | ForEach-Object { $_.FullName.Replace('\', '/') })
    $required = @(
        "taskbar-groups.exe",
        "LICENSE",
        "config/.keep",
        "Shortcuts/.keep",
        "JITComp/.keep"
    )

    foreach ($entry in $required) {
        if ($entries -notcontains $entry) { throw "Archive is missing required entry: $entry" }
    }

    $forbidden = @("reference/", "src/", "Cargo.toml", "Cargo.lock")
    foreach ($entry in $entries) {
        foreach ($prefix in $forbidden) {
            if ($entry -eq $prefix.TrimEnd("/") -or $entry.StartsWith($prefix)) {
                throw "Archive contains forbidden entry: $entry"
            }
        }
    }

    $archiveName = [System.IO.Path]::GetFileNameWithoutExtension($archivePath)
    if ($archiveName -notmatch '^taskbar-groups-v[^-]+-x86_64-pc-windows-msvc$') {
        throw "Archive name is not versioned for the MSVC target: $archiveName"
    }

    $unexpected = @($entries | Where-Object {
        $_ -notin $required -and $_ -notmatch '/$'
    })
    if ($unexpected.Count -gt 0) {
        throw "Archive contains unexpected files: $($unexpected -join ', ')"
    }

    Write-Host "Validated portable MSVC package: $archivePath"
}
finally {
    if ($zip -is [System.IDisposable]) { $zip.Dispose() }
}
