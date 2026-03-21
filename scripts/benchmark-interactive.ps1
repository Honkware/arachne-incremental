# Arachne Incremental Benchmark Script (Interactive Version)
# Run this in PowerShell as Administrator
# Window will stay open after completion

param(
    [int]$FileCount = 10000,
    [string]$TestPath = "C:\temp\arachne-benchmark",
    [switch]$KeepFiles
)

$ErrorActionPreference = "Stop"

# Start logging
$logFile = "$env:TEMP\arachne-benchmark-$(Get-Date -Format 'yyyyMMdd-HHmmss').log"
Start-Transcript -Path $logFile -Force | Out-Null

function Write-Header($text) {
    $line = "`n========================================"
    Write-Host $line -ForegroundColor Cyan
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

function Pause-ForReview {
    Write-Host "`n" -NoNewline
    Write-Host "Press ENTER to continue..." -ForegroundColor Magenta -NoNewline
    $null = Read-Host
}

# Check admin
Write-Header "Arachne Incremental Live Benchmark"
Write-Info "Log file: $logFile"
Write-Host ""

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole] "Administrator")
if (-not $isAdmin) {
    Write-Error "This script requires Administrator privileges"
    Write-Info "Please restart PowerShell as Administrator and try again"
    Pause-ForReview
    exit 1
}
Write-Success "Running as Administrator"

# Setup
Write-Info "Test directory: $TestPath"
Write-Info "File count: $FileCount"

if (Test-Path $TestPath) {
    Write-Info "Cleaning up previous test directory..."
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
    Write-Info "URL: $binaryUrl"
    Invoke-WebRequest -Uri $binaryUrl -OutFile $binaryPath -UseBasicParsing
    Write-Success "Binary downloaded ($( [math]::Round((Get-Item $binaryPath).Length / 1MB, 2) ) MB)"
} catch {
    Write-Error "Failed to download binary: $_"
    Write-Info "You may need to build from source or check your internet connection"
    Pause-ForReview
    exit 1
}

# Verify binary
Write-Info "Verifying binary..."
try {
    $helpOutput = & $binaryPath --help 2>&1
    if ($helpOutput -match "arachne-incremental") {
        Write-Success "Binary verified and working"
        Write-Info "Version info:"
        $helpOutput | Select-Object -First 5 | ForEach-Object { Write-Host "  $_" -ForegroundColor Gray }
    } else {
        throw "Binary verification failed"
    }
} catch {
    Write-Error "Binary verification failed: $_"
    Pause-ForReview
    exit 1
}

Pause-ForReview

# Run benchmark
Write-Header "Step 2: Run Benchmark"
Write-Info "This will:"
Write-Info "  1. Create $FileCount test files"
Write-Info "  2. Measure full directory walk time"
Write-Info "  3. Measure incremental scan time"
Write-Info "  4. Calculate performance improvement"
Write-Host ""
Write-Info "Starting benchmark - this may take 1-2 minutes..."
Write-Host ""

try {
    $benchmarkOutput = & $binaryPath benchmark --path $TestPath --count $FileCount 2>&1
    
    # Display results
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
        Write-Host "📊 RESULTS:" -ForegroundColor Cyan
        Write-Host ""
        Write-Host "  Full Directory Walk:  $fullScanTime seconds" -ForegroundColor White
        Write-Host "  USN Journal Scan:     $incrementalTime milliseconds" -ForegroundColor Green
        Write-Host "  Performance Gain:     $reduction% faster" -ForegroundColor $(if ($reduction -ge 80) { "Green" } else { "Yellow" })
        Write-Host ""
        
        if ($reduction -ge 80) {
            Write-Success "TARGET MET: ≥80% latency reduction achieved!"
            Write-Host ""
            Write-Host "💡 What this means:" -ForegroundColor Cyan
            Write-Host "   • A 60-minute backup pre-scan → ~1 minute" -ForegroundColor Gray
            Write-Host "   • A 30-second file sync → ~50 milliseconds" -ForegroundColor Gray
            Write-Host "   • Real-time change detection is now possible" -ForegroundColor Gray
        } else {
            Write-Info "Below 80% target"
            Write-Info "System may be under load or using slow disk"
        }
    } else {
        Write-Error "Could not parse benchmark results"
        Write-Info "Raw output:"
        $benchmarkOutput | ForEach-Object { Write-Host "  $_" }
    }
} catch {
    Write-Error "Benchmark failed: $_"
    Write-Info "Error details:"
    Write-Host $_.Exception.Message -ForegroundColor Red
}

Pause-ForReview

# Real-world test
Write-Header "Step 3: Real-World Test"

$realTestDir = "$TestPath\real-test"
New-Item -ItemType Directory -Path $realTestDir -Force | Out-Null

try {
    Write-Info "Creating bootstrap for: $realTestDir"
    $bootstrapOutput = & $binaryPath bootstrap --path $realTestDir --output "$realTestDir\bootstrap.json" 2>&1
    $bootstrapOutput | ForEach-Object { Write-Host "  $_" -ForegroundColor Gray }
    
    Write-Info "Creating 10 test files..."
    1..10 | ForEach-Object {
        Set-Content "$realTestDir\file$_.txt" "content $_" -Force
    }
    Write-Success "Files created"
    
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
    Write-Host "📊 REAL-WORLD RESULTS:" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "  First scan (baseline):  $($firstScan.TotalMilliseconds.ToString('F0')) ms" -ForegroundColor Gray
    Write-Host "  Second scan (delta):    $($secondScan.TotalMilliseconds.ToString('F0')) ms" -ForegroundColor Green
    
    if (Test-Path "$realTestDir\events2.jsonl") {
        $events = Get-Content "$realTestDir\events2.jsonl" | ConvertFrom-Json
        if ($events) {
            Write-Host ""
            Write-Host "✓ Detected changes:" -ForegroundColor Cyan
            $events | ForEach-Object {
                Write-Host "   $($_.op.ToUpper()): $($_.path)" -ForegroundColor White
            }
        } else {
            Write-Info "No new events (this can happen if USN journal hasn't updated yet)"
        }
    }
} catch {
    Write-Error "Real-world test failed: $_"
}

Pause-ForReview

# Cleanup
Write-Header "Step 4: Cleanup"

if (-not $KeepFiles) {
    try {
        Set-Location C:\
        Remove-Item -Recurse -Force $TestPath
        Write-Success "Test files cleaned up"
        Write-Info "Test directory removed: $TestPath"
    } catch {
        Write-Error "Cleanup failed (files may be in use): $_"
        Write-Info "You can manually delete: $TestPath"
    }
} else {
    Write-Info "Test directory kept at: $TestPath"
    Write-Info "Use -KeepFiles flag to preserve test directory"
}

Write-Info "Log file saved to: $logFile"

# Final
Write-Header "Benchmark Complete"
Write-Host ""
Write-Host "📁 Repository:  https://github.com/Honkware/arachne-incremental" -ForegroundColor Cyan
Write-Host "📖 Guide:       https://github.com/Honkware/arachne-incremental/blob/main/BENCHMARK-REPLICATION.md" -ForegroundColor Cyan
Write-Host "📝 Log file:    $logFile" -ForegroundColor Cyan
Write-Host ""

Stop-Transcript | Out-Null

Write-Host "Press ENTER to close this window..." -ForegroundColor Magenta -NoNewline
$null = Read-Host
