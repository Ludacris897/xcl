# Snapshot the most recent XCL crash report even when the game/runtime still holds xcl.log open.
# No game assets, executables, save data, or shader dumps are included.
[CmdletBinding()]
param(
    [string]$LogsDir,
    [string]$OutputDir = (Join-Path $env:TEMP 'xcl-triage')
)
$ErrorActionPreference = 'Stop'
if (-not $LogsDir) {
    $root = Join-Path $env:LOCALAPPDATA 'XCL'
    $candidate = Get-ChildItem -LiteralPath $root -Recurse -File -Filter 'report.json' -ErrorAction Stop |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $candidate) { throw "No report.json found under $root" }
    $LogsDir = $candidate.DirectoryName
}
$report = Join-Path $LogsDir 'report.json'
$log = Join-Path $LogsDir 'xcl.log'
if (-not (Test-Path -LiteralPath $report)) { throw "Missing $report" }
if (-not (Test-Path -LiteralPath $log)) { throw "Missing $log" }

New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$work = Join-Path $OutputDir "xcl-$stamp"
New-Item -ItemType Directory -Path $work -Force | Out-Null

# FileShare.ReadWrite/Delete permits a snapshot of an actively written log.
foreach ($name in @('report.json', 'xcl.log')) {
    $src = Join-Path $LogsDir $name
    $dst = Join-Path $work $name
    $stream = [System.IO.File]::Open($src, [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        ([System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete))
    try {
        $output = [System.IO.File]::Create($dst)
        try { $stream.CopyTo($output) } finally { $output.Dispose() }
    } finally { $stream.Dispose() }
}
$r = Get-Content -LiteralPath (Join-Path $work 'report.json') -Raw | ConvertFrom-Json
$summary = @(
    "Crash: $($r.crash)"
    "Exit code: $($r.exit_code)"
    "Presents: $($r.d3d12.presents)"
    "DXIL failed pipelines: $($r.d3d12.failed_pipelines)"
    "BCPack requests refused: $($r.dstorage.bcpack_refused)"
)
$lines = Get-Content -LiteralPath (Join-Path $work 'xcl.log') |
    Where-Object { $_ -match 'CopyFromSubresource|unhandled exception|source disappeared|invalid/overflowing pitch|\[STUB\]|DirectStorage.*failed' } |
    Select-Object -Last 100
$summary += '--- Relevant recent log lines ---'
$summary += $lines
$summary | Set-Content -LiteralPath (Join-Path $work 'triage.txt') -Encoding UTF8
$zip = "$work.zip"
Compress-Archive -LiteralPath (Join-Path $work 'report.json'),(Join-Path $work 'xcl.log'),(Join-Path $work 'triage.txt') -DestinationPath $zip -ErrorAction Stop
Write-Host ($summary | Select-Object -First 7 | Out-String)
Write-Host "Saved: $zip"
Write-Host 'Review files for personal information before sharing.'
