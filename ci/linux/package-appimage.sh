#!/usr/bin/env bash
# ABOUTME: Builds the release binary and packages it as a portable AppImage
# ABOUTME: using linuxdeploy. Output: dist/DoomBuilder-<version>-x86_64.AppImage
set -euo pipefail

VERSION="${VERSION:-dev}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

cargo build --release -p doombuilder-app

# Assemble the AppDir that linuxdeploy expects.
APPDIR="$ROOT/AppDir"
rm -rf "$APPDIR"
install -Dm755 target/release/doombuilder "$APPDIR/usr/bin/doombuilder"
install -Dm644 ci/linux/doombuilder.desktop \
  "$APPDIR/usr/share/applications/doombuilder.desktop"
install -Dm644 crates/doombuilder-gui/assets/icon.png \
  "$APPDIR/usr/share/icons/hicolor/256x256/apps/doombuilder.png"

# linuxdeploy + appimagetool. Running AppImages on GitHub runners has no FUSE,
# so extract-and-run instead of mounting.
export APPIMAGE_EXTRACT_AND_RUN=1
TOOLDIR="$ROOT/.appimage-tools"
mkdir -p "$TOOLDIR"
fetch() { curl -sSL "$1" -o "$2" && chmod +x "$2"; }
fetch https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage \
  "$TOOLDIR/linuxdeploy"
fetch https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage \
  "$TOOLDIR/appimagetool"

mkdir -p "$ROOT/dist"
export OUTPUT="$ROOT/dist/DoomBuilder-${VERSION}-x86_64.AppImage"
"$TOOLDIR/linuxdeploy" \
  --appdir "$APPDIR" \
  --desktop-file "$APPDIR/usr/share/applications/doombuilder.desktop" \
  --icon-file "$APPDIR/usr/share/icons/hicolor/256x256/apps/doombuilder.png" \
  --output appimage

echo "Built $OUTPUT"
