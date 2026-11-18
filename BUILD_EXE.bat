@echo off
chcp 65001 >nul
title MixSwitch - Compact & Meter Fix v4 Builder
echo ============================================================================
echo   MixSwitch v1.0.0 - Compact UI v4 Source - Automated EXE Builder
echo   High-DPI UI ^| Green-Cyan-Blue Brand ^| Core Audio controls preserved
echo ============================================================================
echo.

cd /d "%~dp0"
echo [1/2] Compiling MixSwitch v1.0.0 (Release Mode)...
cargo build --release

if %ERRORLEVEL% NEQ 0 (
    echo.
    echo [X] Build failed! Please copy any error text above.
    pause
    exit /b 1
)

echo.
echo [2/2] Copying standalone MixSwitch.exe to folder root...
copy /Y "target\release\mixswitch.exe" "%~dp0MixSwitch.exe" >nul
if %ERRORLEVEL% NEQ 0 (
    echo.
    echo [X] Could not copy MixSwitch.exe - it is probably still running.
    echo     Exit MixSwitch from the system tray, then run this file again.
    pause
    exit /b 1
)

echo.
echo ============================================================================
echo   SUCCESS! MixSwitch branded UI build is ready:
echo   -^> %~dp0MixSwitch.exe
echo   (Make sure to exit any old running AudioHub/MixSwitch from the tray first!)
echo ============================================================================
pause
