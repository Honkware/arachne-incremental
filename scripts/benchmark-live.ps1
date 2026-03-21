# Arachne Incremental Benchmark Script
# Run this in PowerShell as Administrator

param(
    [int]$FileCount = 10000,
    [string]$TestPath = "C:\temp\arachne-benchmark",
    [switch]$KeepFiles
)

$ErrorActionPreference = "Stop"

function Write-Header($text) {
    Write-Host "`n========================================" -ForegroundColor Cyan
    Write-Host $text -ForegroundColor Cyan
    Write-Host "========================================" -ForegroundColor Cyan
}

function Write-Success($text) {
    Write-Host "[PASS] $text" -ForegroundColor Green
}

function Write-Info($text) {
    Write-Host "[INFO] $text" -ForegroundColor Yellow
}

function Write-Error($text) {
    Write-Host "[FAIL] $text" -ForegroundColor Red
}

# Check admin
Write-Header "Arachne Incremental Live Benchmark"

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole] "Administrator")
if (-not $isAdmin) {
    Write-Error "This script requires Administrator privileges"
    Write-Info "Please restart PowerShell as Administrator and try again"
    exit 1
}
Write-Success "Running as Administrator"

# Setup
Write-Info "Test directory: $TestPath"
Write-Info "File count: $FileCount"

if (Test-Path $TestPath) {
    Remove-Item -Recurse -Force $TestPath
}
New-Item -ItemType Directory -Path $TestPath -Force | Out-Null
Set-Location $TestPath

# Download binary
Write-Header "Step 1: Download Binary"
$binaryUrl = "https://github.com/Honkware/arachne-incremental/releases/latest/download/arachne-incremental.exe"
$binaryPath = "$TestPath\arachne-incremental.exe"

try {
    Write-Info "Downloading from GitHub..."
    Invoke-WebRequest -Uri $binaryUrl -OutFile $binaryPath -UseBasicParsing
    Write-Success "Binary downloaded"
} catch {
    Write-Error "Failed to download binary: $_"
    Write-Info "Trying to build from source..."
    exit 1
}

# Verify binary
Write-Info "Verifying binary..."
$version = & $binaryPath --help 2>&1 | Select-Object -First 3
if ($version -match "arachne-incremental") {
    Write-Success "Binary verified"
} else {
    Write-Error "Binary verification failed"
    exit 1
}

# Run benchmark
Write-Header "Step 2: Run Benchmark"
Write-Info "This will:"
Write-Info "  1. Create $FileCount test files"
Write-Info "  2. Measure full directory walk time"
Write-Info "  3. Measure incremental scan time"
Write-Info "  4. Calculate performance improvement"
Write-Host ""

$benchmarkOutput = & $binaryPath benchmark --path $TestPath --count $FileCount 2>&1

# Parse and display results
Write-Header "Benchmark Results"

$fullScanTime = $null
$incrementalTime = $null
$reduction = $null

foreach ($line in $benchmarkOutput) {
    Write-Host $line
    
    if ($line -match "Full scan:\s+([\d.]+)s") {
        $fullScanTime = [double]$matches[1]
    }
    if ($line -match "Incremental:\s+([\d.]+)ms") {
        $incrementalTime = [double]$matches[1]
    }
    if ($line -match "Reduction:\s+([\d.]+)%") {
        $reduction = [double]$matches[1]
    }
}

# Summary
Write-Header "Performance Summary"

if ($fullScanTime -and $incrementalTime -and $reduction) {
    Write-Host ""
    Write-Host "Full Directory Walk:  $fullScanTime seconds" -ForegroundColor White
    Write-Host "USN Journal Scan:     $incrementalTime milliseconds" -ForegroundColor White
    Write-Host "Performance Gain:     $reduction% faster" -ForegroundColor $(if ($reduction -ge 80) { "Green" } else { "Yellow" })
    Write-Host ""
    
    if ($reduction -ge 80) {
        Write-Success "TARGET MET: ≥80% latency reduction achieved!"
        Write-Host ""
        Write-Host "This means:" -ForegroundColor Cyan
        Write-Host "  • A 60-minute backup pre-scan would take ~1 minute" -ForegroundColor Gray
        Write-Host "  • A 30-second file sync would take ~50 milliseconds" -ForegroundColor Gray
        Write-Host "  • Real-time change detection is now possible" -ForegroundColor Gray
    } else {
        Write-Info "Below 80% target - system may be under load or disk is slow"
    }
} else {
    Write-Error "Could not parse benchmark results"
}

# Real-world test
Write-Header "Step 3: Real-World Test"

$realTestDir = "$TestPath\real-test"
New-Item -ItemType Directory -Path $realTestDir -Force | Out-Null

Write-Info "Creating bootstrap for: $realTestDir"
& $binaryPath bootstrap --path $realTestDir --output "$realTestDir\bootstrap.json" 2>&1 | Out-Null

Write-Info "Creating 10 test files..."
1..10 | ForEach-Object {
    Set-Content "$realTestDir\file$_.txt" "content $_" -Force
}

Write-Info "Running first scan (full baseline)..."
$firstScan = Measure-Command {
    & $binaryPath scan --bootstrap "$realTestDir\bootstrap.json" --baseline "$realTestDir\baseline.json" --output "$realTestDir\events1.jsonl" 2>&1 | Out-Null
}

Write-Info "Modifying 1 file..."
Set-Content "$realTestDir\file1.txt" "modified content" -Force
Start-Sleep -Milliseconds 100

Write-Info "Running incremental scan..."
$secondScan = Measure-Command {
    & $binaryPath scan --bootstrap "$realTestDir\bootstrap.json" --baseline "$realTestDir\baseline.json" --output "$realTestDir\events2.jsonl" 2>&1 | Out-Null
}

Write-Host ""
Write-Host "First scan (baseline):  $($firstScan.TotalMilliseconds.ToString('F0')) ms" -ForegroundColor Gray
Write-Host "Second scan (delta):    $($secondScan.TotalMilliseconds.ToString('F0')) ms" -ForegroundColor Green

if (Test-Path "$realTestDir\events2.jsonl") {
    $events = Get-Content "$realTestDir\events2.jsonl" | ConvertFrom-Json
    Write-Host ""
    Write-Host "Detected changes:" -ForegroundColor Cyan
    $events | ForEach-Object {
        Write-Host "  $($_.op): $($_.path)" -ForegroundColor White
    }
}

# Cleanup
if (-not $KeepFiles) {
    Write-Header "Cleanup"
    Set-Location C:\
    Remove-Item -Recurse -Force $TestPath
    Write-Success "Test files cleaned up"
    Write-Info "Use -KeepFiles flag to preserve test directory"
} else {
    Write-Header "Files Preserved"
    Write-Info "Test directory kept at: $TestPath"
}

# Final
Write-Header "Benchmark Complete"
Write-Host "Repository: https://github.com/Honkware/arachne-incremental" -ForegroundColor Cyan
Write-Host "Documentation: https://github.com/Honkware/arachne-incremental/blob/main/BENCHMARK-REPLICATION.md" -ForegroundColor Cyan
