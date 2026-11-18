@echo off
setlocal EnableExtensions
cd /d "%~dp0"
echo ===============================================
echo    MixSwitch v1.0.0 - Portable Release Build
echo ===============================================
where cargo >nul 2>&1
if errorlevel 1 (
  echo [ERROR] Rust/Cargo not found. Install Rust and the Visual Studio C++ Build Tools.
  pause
  exit /b 1
)
cargo build --release --locked --target x86_64-pc-windows-msvc
if errorlevel 1 (
  echo [ERROR] Build failed. Review the error output above.
  pause
  exit /b 1
)
if not exist dist mkdir dist
copy /Y "target\x86_64-pc-windows-msvc\release\mixswitch.exe" "dist\MixSwitch.exe" >nul
if errorlevel 1 (
  echo [ERROR] Could not copy executable. Close any running MixSwitch instance.
  pause
  exit /b 1
)
powershell -NoProfile -Command "$hash=(Get-FileHash 'dist\MixSwitch.exe' -Algorithm SHA256).Hash.ToLowerInvariant(); Set-Content -Path 'dist\SHA256SUMS.txt' -Value ($hash + '  MixSwitch.exe') -Encoding ascii"
if errorlevel 1 (
  echo [ERROR] Could not write SHA256SUMS.txt.
  pause
  exit /b 1
)
echo.
echo [OK] Release files created in dist\
echo      dist\MixSwitch.exe
echo      dist\SHA256SUMS.txt
echo.
echo IMPORTANT: Test the EXE on Windows before uploading it to GitHub Releases.
pause
