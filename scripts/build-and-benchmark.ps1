# Arachne Incremental Benchmark - Build from Source
# This script clones, builds, and runs the real benchmark
# Run this in PowerShell as Administrator

param(
    [int]$FileCount = 10000,
    [string]$InstallDir = "C:\tools\arachne-incremental"
)

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

function Pause-Script {
    Write-Host "`nPress ENTER to continue..." -ForegroundColor Magenta -NoNewline
    $null = Read-Host
}

# Check admin
Write-Header "Arachne Incremental - Build & Benchmark"

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole] "Administrator")
if (-not $isAdmin) {
    Write-Error "This script requires Administrator privileges"
    Write-Info "Right-click PowerShell → Run as Administrator"
    Pause-Script
    exit 1
}
Write-Success "Running as Administrator"

# Check for Rust
Write-Header "Step 1: Check Prerequisites"
Write-Info "Checking for Rust..."

try {
    $rustVersion = rustc --version 2>&1
    Write-Success "Rust found: $rustVersion"
} catch {
    Write-Error "Rust not found"
    Write-Info "Please install Rust from https://rustup.rs"
    Write-Info "Then run this script again"
    Pause-Script
    exit 1
}

Pause-Script

# Clone repo
Write-Header "Step 2: Clone Repository"
Write-Info "Installing to: $InstallDir"

if (Test-Path $InstallDir) {
    Write-Info "Directory exists, removing..."
    Remove-Item -Recurse -Force $InstallDir
}

try {
    git clone https://github.com/Honkware/arachne-incremental.git $InstallDir 2>&1 | Out-Null
    Write-Success "Repository cloned"
} catch {
    Write-Error "Failed to clone repository: $_"
    Pause-Script
    exit 1
}

Set-Location $InstallDir
Pause-Script

# Build
Write-Header "Step 3: Build Release Binary"
Write-Info "Building with Cargo (this may take 2-5 minutes)..."
Write-Info "Compiling dependencies and optimizing..."

try {
    $buildOutput = cargo build --release 2>&1
    
    if ($LASTEXITCODE -eq 0) {
        Write-Success "Build successful!"
    } else {
        Write-Error "Build failed"
        Write-Host $buildOutput
        Pause-Script
        exit 1
    }
} catch {
    Write-Error "Build error: $_"
    Pause-Script
    exit 1
}

$binaryPath = "$InstallDir\target\release\arachne-incremental.exe"
if (-not (Test-Path $binaryPath)) {
    Write-Error "Binary not found at expected path: $binaryPath"
    Pause-Script
    exit 1
}

$size = [math]::Round((Get-Item $binaryPath).Length / 1MB, 2)
Write-Success "Binary created: $size MB"

Pause-Script

# Verify binary
Write-Header "Step 4: Verify Binary"
try {
    $help = & $binaryPath --help 2>&1
    Write-Success "Binary works!"
    Write-Host ""
    $help | Select-Object -First 5 | ForEach-Object { Write-Host "  $_" -ForegroundColor Gray }
} catch {
    Write-Error "Binary verification failed: $_"
    Pause-Script
    exit 1
}

Pause-Script

# Run benchmark
Write-Header "Step 5: Run Benchmark"
Write-Info "Running benchmark with $FileCount files..."
Write-Info "This will create test files and measure performance"

$testPath = "C:\temp\arachne-benchmark-test"

Write-Host ""
Write-Host "Starting benchmark..." -ForegroundColor Cyan
Write-Host ""

try {
    & $binaryPath benchmark --path $testPath --count $FileCount 2>&1 | ForEach-Object {
        Write-Host $_
    }
} catch {
    Write-Error "Benchmark failed: $_"
}

Pause-Script

# Real test
Write-Header "Step 6: Real-World Test"
Write-Info "Testing actual change detection..."

$realTestDir = "$testPath\real-test"
New-Item -ItemType Directory -Path $realTestDir -Force | Out-Null

Write-Info "Creating bootstrap..."
& $binaryPath bootstrap --path $realTestDir --output "$realTestDir\bootstrap.json" 2>&1 | ForEach-Object { Write-Host "  $_" -ForegroundColor Gray }

Write-Info "Creating test files..."
1..5 | ForEach-Object { Set-Content "$realTestDir\file$_.txt" "content $_" }

Write-Info "Running first scan..."
$first = Measure-Command { 
    & $binaryPath scan --bootstrap "$realTestDir\bootstrap.json" --baseline "$realTestDir\baseline.json" --output "$realTestDir\events1.jsonl" 2>&1 | Out-Null 
}

Write-Info "Modifying a file..."
Set-Content "$realTestDir\file1.txt" "MODIFIED CONTENT"
Start-Sleep -Milliseconds 100

Write-Info "Running incremental scan..."
$second = Measure-Command { 
    & $binaryPath scan --bootstrap "$realTestDir\bootstrap.json" --baseline "$realTestDir\baseline.json" --output "$realTestDir\events2.jsonl" 2>&1 | Out-Null 
}

Write-Host ""
Write-Host "📊 Results:" -ForegroundColor Cyan
Write-Host "  First scan:  $($first.TotalMilliseconds.ToString('F0')) ms" -ForegroundColor Gray
Write-Host "  Incremental: $($second.TotalMilliseconds.ToString('F0')) ms" -ForegroundColor Green

if (Test-Path "$realTestDir\events2.jsonl") {
    $events = Get-Content "$realTestDir\events2.jsonl" | ConvertFrom-Json
    if ($events) {
        Write-Host ""
        Write-Host "✓ Detected changes:" -ForegroundColor Green
        $events | ForEach-Object { Write-Host "   $($_.op): $($_.path)" }
    }
}

Pause-Script

# Install to PATH option
Write-Header "Step 7: Installation"
Write-Info "Binary location: $binaryPath"
Write-Host ""
$installChoice = Read-Host "Add to PATH? (Y/N)"
if ($installChoice -eq 'Y' -or $installChoice -eq 'y') {
    $currentPath = [Environment]::GetEnvironmentVariable("Path", "Machine")
    if (-not $currentPath.Contains("$InstallDir\target\release")) {
        [Environment]::SetEnvironmentVariable("Path", "$currentPath;$InstallDir\target\release", "Machine")
        Write-Success "Added to PATH. Restart PowerShell to use 'arachne-incremental' anywhere."
    } else {
        Write-Info "Already in PATH"
    }
} else {
    Write-Info "Skipped PATH installation"
    Write-Info "To run manually: $binaryPath"
}

# Cleanup
Write-Header "Step 8: Cleanup"
Set-Location C:\
if (Test-Path $testPath) {
    Remove-Item -Recurse -Force $testPath
    Write-Success "Test files cleaned up"
}

# Final
Write-Header "Setup Complete!"
Write-Host ""
Write-Host "✅ Arachne Incremental is ready to use!" -ForegroundColor Green
Write-Host ""
Write-Host "Quick start:" -ForegroundColor Cyan
Write-Host "  arachne-incremental bootstrap --path C:\data --output bootstrap.json" -ForegroundColor Gray
Write-Host "  arachne-incremental scan --bootstrap bootstrap.json --baseline baseline.json --output changes.jsonl" -ForegroundColor Gray
Write-Host ""
Write-Host "Repository: https://github.com/Honkware/arachne-incremental" -ForegroundColor Cyan
Write-Host ""

Write-Host "Press ENTER to close..." -ForegroundColor Magenta -NoNewline
$null = Read-Host
