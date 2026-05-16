#!/usr/bin/env bash
# ABOUTME: Builds a universal (arm64 + x86_64) binary, wraps it in a .app
# ABOUTME: bundle, and produces dist/DoomBuilder-<version>-universal.dmg
set -euo pipefail

VERSION="${VERSION:-0.0.0}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release -p doombuilder-app --target aarch64-apple-darwin
cargo build --release -p doombuilder-app --target x86_64-apple-darwin

APP="$ROOT/dist/DoomBuilder.app"
rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# Fuse the two arch slices into one universal Mach-O.
lipo -create \
  target/aarch64-apple-darwin/release/doombuilder \
  target/x86_64-apple-darwin/release/doombuilder \
  -output "$APP/Contents/MacOS/doombuilder"

# Build an .icns from the committed 256px PNG. sips upsamples the larger
# slots; acceptable until a higher-res master icon is added.
ICONSET="$ROOT/dist/AppIcon.iconset"
rm -rf "$ICONSET" && mkdir -p "$ICONSET"
SRC=crates/doombuilder-gui/assets/icon.png
for sz in 16 32 64 128 256 512; do
  sips -z "$sz" "$sz" "$SRC" --out "$ICONSET/icon_${sz}x${sz}.png" >/dev/null
  d=$((sz * 2))
  sips -z "$d" "$d" "$SRC" --out "$ICONSET/icon_${sz}x${sz}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>DoomBuilder</string>
  <key>CFBundleDisplayName</key><string>DoomBuilder</string>
  <key>CFBundleExecutable</key><string>doombuilder</string>
  <key>CFBundleIdentifier</key><string>com.github.jsh.doombuilder</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# Ad-hoc sign so Gatekeeper at least sees a valid (unnotarized) signature.
codesign --force --deep --sign - "$APP"

hdiutil create -volname "DoomBuilder" -srcfolder "$APP" -ov -format UDZO \
  "$ROOT/dist/DoomBuilder-${VERSION}-universal.dmg"

echo "Built dist/DoomBuilder-${VERSION}-universal.dmg"
