#!/bin/bash
# Build Sūcī on Windows and collect it, with the GTK libraries it needs, into
# a folder that runs on any Windows 10/11 PC: target/windows/Suci (and a .zip).
#
# Run in the MSYS2 "UCRT64" terminal (see BUILDING.md).
set -euo pipefail
cd "$(dirname "$0")/../.."
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
PREFIX=/ucrt64

cargo build --release --locked

DIST=target/windows/Suci
rm -rf "$DIST"
mkdir -p "$DIST/bin" "$DIST/lib" "$DIST/share/glib-2.0/schemas" "$DIST/share/icons"
cp target/release/suci.exe "$DIST/bin/"

copy_deps() {
    # every DLL from the MSYS2 prefix that a binary needs
    ldd "$1" | awk -v p="$PREFIX" 'index($3, p) == 1 {print $3}' | sort -u | while read -r dll; do
        [ -e "$DIST/bin/$(basename "$dll")" ] || cp "$dll" "$DIST/bin/"
    done
}
copy_deps "$DIST/bin/suci.exe"

# image loaders (for SVG icons) and their own dependencies
if [ -d "$PREFIX/lib/gdk-pixbuf-2.0" ]; then
    cp -r "$PREFIX/lib/gdk-pixbuf-2.0" "$DIST/lib/"
    for f in "$DIST"/lib/gdk-pixbuf-2.0/2.10.0/loaders/*.dll; do copy_deps "$f"; done
fi

# settings schemas and icon themes
cp "$PREFIX/share/glib-2.0/schemas/gschemas.compiled" "$DIST/share/glib-2.0/schemas/"
cp -r "$PREFIX/share/icons/Adwaita" "$PREFIX/share/icons/hicolor" "$DIST/share/icons/"
mkdir -p "$DIST/share/icons/hicolor/scalable/apps"
cp packaging/io.github.suci.Suci.svg "$DIST/share/icons/hicolor/scalable/apps/"
cp README.md LICENSE "$DIST/"

# a launcher at the top of the folder
cat > "$DIST/Suci.bat" <<'BAT'
@echo off
start "" "%~dp0bin\suci.exe" %*
BAT

(cd target/windows && rm -f "suci-${VERSION}-windows-x64.zip" && zip -qr "suci-${VERSION}-windows-x64.zip" Suci)
echo "target/windows/suci-${VERSION}-windows-x64.zip"
