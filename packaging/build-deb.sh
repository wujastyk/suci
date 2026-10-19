#!/bin/sh
# Build a .deb for Ubuntu 24.04 / Linux Mint 22 (GTK 4.14).
set -eu
cd "$(dirname "$0")/.."
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
ARCH=$(dpkg --print-architecture)
cargo build --release --locked
PKG=target/deb/suci_${VERSION}_${ARCH}
rm -rf "$PKG"
install -Dm755 target/release/suci "$PKG/usr/bin/suci"
strip "$PKG/usr/bin/suci" || true
install -Dm644 packaging/io.github.suci.Suci.desktop "$PKG/usr/share/applications/io.github.suci.Suci.desktop"
install -Dm644 packaging/io.github.suci.Suci.svg "$PKG/usr/share/icons/hicolor/scalable/apps/io.github.suci.Suci.svg"
install -Dm644 README.md "$PKG/usr/share/doc/suci/README.md"
install -Dm644 LICENSE "$PKG/usr/share/doc/suci/copyright"
SIZE=$(du -sk "$PKG/usr" | cut -f1)
mkdir -p "$PKG/DEBIAN"
cat > "$PKG/DEBIAN/control" <<CTRL
Package: suci
Version: ${VERSION}
Section: text
Priority: optional
Architecture: ${ARCH}
Depends: libgtk-4-1 (>= 4.14), libc6 (>= 2.39)
Installed-Size: ${SIZE}
Maintainer: Dominik Wujastyk
Description: Fast BibTeX / biblatex bibliography manager
 Sūcī opens large .bib libraries instantly, keeps JabRef's file format,
 fetches records by DOI or ISBN, formats citations (Chicago, New Hart's
 Rules, AMA and any CSL style), links PDFs, checks integrity, and handles
 IAST diacritics and Sanskrit alphabetical order.
CTRL
cat > "$PKG/DEBIAN/postinst" <<'POST'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database -q /usr/share/applications || true; fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then gtk-update-icon-cache -q -t /usr/share/icons/hicolor || true; fi
POST
chmod 755 "$PKG/DEBIAN/postinst"
dpkg-deb --root-owner-group --build "$PKG" >/dev/null
echo "$PKG.deb"
