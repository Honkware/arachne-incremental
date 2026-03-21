@echo off
echo ========================================
echo  Arachne Incremental Benchmark
echo ========================================
echo.
echo This will test NTFS incremental scanning
echo performance on your Windows machine.
echo.
echo REQUIREMENTS:
echo   - Windows 10/11 or Server 2016+
echo   - Administrator privileges
echo   - Internet connection (to download binary)
echo.
echo RESULTS:
echo   You'll see a comparison between:
echo   - Full directory walk (slow)
echo   - USN journal scan (fast)
echo.
pause

echo.
echo [1/4] Checking if running as Administrator...
net session >nul 2>&1
if %errorlevel% neq 0 (
    echo FAILED: Please run this script as Administrator!
    echo Right-click - Run as Administrator
    pause
    exit /b 1
)
echo PASSED: Running as Administrator

echo.
echo [2/4] Creating test directory...
set TESTDIR=C:\temp\arachne-benchmark-simple
if exist %TESTDIR% rmdir /s /q %TESTDIR%
mkdir %TESTDIR%
cd /d %TESTDIR%
echo PASSED: Directory created: %TESTDIR%

echo.
echo [3/4] Downloading arachne-incremental...
echo Please wait, downloading from GitHub...
powershell -Command "Invoke-WebRequest -Uri 'https://github.com/Honkware/arachne-incremental/releases/latest/download/arachne-incremental.exe' -OutFile 'arachne-incremental.exe' -UseBasicParsing" 2>nul
if not exist arachne-incremental.exe (
    echo FAILED: Could not download binary
    echo Please check your internet connection
    pause
    exit /b 1
)
echo PASSED: Binary downloaded

echo.
echo [4/4] Running benchmark...
echo This creates 10,000 files and measures scan time...
echo.
arachne-incremental.exe benchmark --path %TESTDIR% --count 10000

echo.
echo ========================================
echo  Benchmark Complete!
echo ========================================
echo.
echo Log saved to: %TEMP%\arachne-benchmark-*.log
echo.
echo Repository: https://github.com/Honkware/arachne-incremental
echo.
pause

REM Cleanup
cd /d C:\
if exist %TESTDIR% rmdir /s /q %TESTDIR%
