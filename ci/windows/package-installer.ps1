# ABOUTME: Builds the release .exe, derives a .ico from the app icon, and
# ABOUTME: compiles ci/windows/installer.iss into dist/DoomBuilder-*-setup.exe
$ErrorActionPreference = "Stop"

$version = if ($env:VERSION) { $env:VERSION } else { "0.0.0" }
$root = (Resolve-Path "$PSScriptRoot\..\..").Path
Set-Location $root

cargo build --release -p doombuilder-app

New-Item -ItemType Directory -Force -Path "$root\dist" | Out-Null

# GitHub's windows-latest image ships ImageMagick; use it to build a
# multi-resolution .ico from the committed 256px PNG.
magick crates\doombuilder-gui\assets\icon.png `
  -define icon:auto-resize=256,128,64,48,32,16 `
  "$PSScriptRoot\doombuilder.ico"

# Inno Setup is installed via choco in the workflow; iscc lives here.
$iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
& "$iscc" `
  "/DMyAppVersion=$version" `
  "/DSourceDir=$root\target\release" `
  "/DOutputDir=$root\dist" `
  "$PSScriptRoot\installer.iss"

Write-Host "Built dist\DoomBuilder-$version-setup.exe"
