#!/bin/bash
# Build Sūcī and wrap it in a macOS application bundle (target/macos/Suci.app).
#
# Requires Homebrew's gtk4 (see BUILDING.md). The app uses the GTK libraries
# installed by Homebrew, so it runs on Macs that have them. Making a fully
# self-contained app for other people needs extra tooling (see BUILDING.md).
set -euo pipefail
cd "$(dirname "$0")/../.."
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)

cargo build --release --locked

APP=target/macos/Suci.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/suci "$APP/Contents/MacOS/suci"

# Icon: convert the SVG to an .icns if librsvg's rsvg-convert is available
if command -v rsvg-convert >/dev/null; then
    ICONSET=$(mktemp -d)/suci.iconset
    mkdir -p "$ICONSET"
    for s in 16 32 128 256 512; do
        rsvg-convert -w $s -h $s packaging/io.github.suci.Suci.svg -o "$ICONSET/icon_${s}x${s}.png"
        rsvg-convert -w $((s*2)) -h $((s*2)) packaging/io.github.suci.Suci.svg -o "$ICONSET/icon_${s}x${s}@2x.png"
    done
    iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/suci.icns"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Sūcī</string>
  <key>CFBundleDisplayName</key><string>Sūcī</string>
  <key>CFBundleIdentifier</key><string>io.github.suci.Suci</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleExecutable</key><string>suci</string>
  <key>CFBundleIconFile</key><string>suci</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>BibTeX database</string>
      <key>CFBundleTypeExtensions</key><array><string>bib</string></array>
      <key>CFBundleTypeRole</key><string>Editor</string>
    </dict>
  </array>
</dict>
</plist>
PLIST

echo "$APP"
