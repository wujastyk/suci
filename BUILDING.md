# Building Sūcī

Sūcī is written in Rust and uses GTK 4. Building it needs three things on
any system: the **Rust toolchain**, **GTK 4 (version 4.14 or later)**, and
**pkg-config**, which tells Rust where GTK is. The first build downloads and
compiles about 200 small Rust libraries and takes 3–10 minutes; later builds
are quick.

- [Linux (Debian, Ubuntu, Mint)](#linux)
- [macOS](#macos)
- [Windows](#windows)
- [After building](#after-building)

---

## Linux

```sh
sudo apt install build-essential libgtk-4-dev git curl
curl https://sh.rustup.rs -sSf | sh        # installs Rust; accept the defaults
source ~/.cargo/env

git clone https://github.com/wujastyk/suci.git
cd suci
cargo build --release
./target/release/suci                      # run it
packaging/build-deb.sh                     # optional: make an installable .deb
```

On Fedora use `sudo dnf install gtk4-devel gcc`; on Arch, `sudo pacman -S gtk4 base-devel`.

---

## macOS

Works on macOS 12 or later, on both Apple Silicon and Intel Macs.

1. **Install Apple's command-line tools** (compiler, git). In Terminal:

   ```sh
   xcode-select --install
   ```

2. **Install Homebrew** by following the one-line instructions at <https://brew.sh>.

3. **Install GTK 4 and helpers:**

   ```sh
   brew install gtk4 adwaita-icon-theme librsvg pkgconf
   ```

   `adwaita-icon-theme` supplies the toolbar icons; `librsvg` draws them
   and is also used to make the app icon.

4. **Install Rust:**

   ```sh
   curl https://sh.rustup.rs -sSf | sh
   source ~/.cargo/env
   ```

5. **Build and run:**

   ```sh
   git clone https://github.com/wujastyk/suci.git
   cd suci
   cargo build --release
   ./target/release/suci
   ```

6. **Optional – make a double-clickable app:**

   ```sh
   packaging/macos/make-app.sh
   cp -r target/macos/Suci.app /Applications/
   ```

   The first time you open it, macOS may say the app is from an unidentified
   developer: right-click (or Control-click) **Suci.app** in Applications and choose
   **Open**, then **Open** again. This is needed only once.

**Notes for macOS**

- Keyboard shortcuts use **⌘** where the Linux version uses Ctrl
  (⌘S save, ⌘F search, ⌘L add from DOI/ISBN, ⌘U BibTeX source …).
- The app built this way uses the GTK libraries installed by Homebrew, so it
  runs on your Mac (or any Mac with the same Homebrew packages). Giving it to
  someone without Homebrew needs the libraries copied into the bundle, e.g.
  with `dylibbundler` or GTK's `gtk-mac-bundler`; this is not automated yet.
- If `cargo build` reports that `gtk4` or `glib-2.0` was not found, run
  `brew link gtk4` and open a new Terminal window.

---

## Windows

The simplest route uses **MSYS2**, which provides GTK, Rust and the C compiler
together. Everything is done inside one MSYS2 terminal; no Windows settings
need changing.

1. **Install MSYS2** from <https://www.msys2.org> (accept the default folder
   `C:\msys64`).

2. Open **"MSYS2 UCRT64"** from the Start menu (the icon with a yellow-brown
   background; not "MSYS2 MSYS"). Bring it up to date:

   ```sh
   pacman -Syu
   ```

   If it asks you to close the window, do so, reopen **MSYS2 UCRT64**, and run
   `pacman -Syu` once more.

3. **Install GTK 4, Rust and the tools:**

   ```sh
   pacman -S --needed git zip \
     mingw-w64-ucrt-x86_64-gtk4 \
     mingw-w64-ucrt-x86_64-adwaita-icon-theme \
     mingw-w64-ucrt-x86_64-librsvg \
     mingw-w64-ucrt-x86_64-rust \
     mingw-w64-ucrt-x86_64-pkgconf \
     mingw-w64-ucrt-x86_64-gcc
   ```

4. **Build and run** (still in the UCRT64 terminal):

   ```sh
   git clone https://github.com/wujastyk/suci.git
   cd suci
   cargo build --release
   ./target/release/suci.exe
   ```

5. **Make a folder you can use outside MSYS2 or give to others:**

   ```sh
   packaging/windows/bundle.sh
   ```

   This creates `target/windows/Suci/` (and a `.zip` of it) containing
   `suci.exe` with all the GTK files it needs. Copy the folder anywhere, for
   example to `C:\Program Files\Suci` or your Documents, and start
   **Suci.bat** (or `bin\suci.exe`). You can pin it to the Start menu or make a
   desktop shortcut to `bin\suci.exe`.
   Windows SmartScreen may warn about an unrecognised app the first time:
   click **More info → Run anyway**.

**Notes for Windows**

- To build from an ordinary PowerShell window with Microsoft's compiler
  instead, follow the "gvsbuild" instructions in the
  [gtk-rs book](https://gtk-rs.org/gtk4-rs/stable/latest/book/installation_windows.html).
- File links written by JabRef on Windows (`C:\…`) are understood, as are
  Linux paths when a library is shared between systems, as long as the PDF
  folder is set in Preferences.
- `suci.exe --check library.bib` prints an integrity report, but only when run
  from a terminal with a debug build (`cargo build`); the release build has no
  console window.

---

## Status on macOS and Windows

Sūcī was developed and tested on Linux. The code is portable and the
automatic GitHub builds compile and test it on macOS and Windows, but the
interface has not yet been tried by a person on those systems. One thing to
check there: **Copy reference** puts rich text (with italics) on the
clipboard; if italics are lost when pasting into Mail or Word, please report it.

---

## After building

- **JabRef's PDF folder** is read automatically on all three systems (from
  `~/.java` on Linux, `~/Library/Preferences/com.apple.java.util.prefs.plist`
  on macOS, and the registry on Windows). If PDFs show as "not found", set the
  folder in **Preferences**.
- **Settings** are stored in:
  - Linux: `~/.config/suci/settings.json`
  - macOS: `~/Library/Application Support/suci/settings.json`
  - Windows: `%APPDATA%\suci\settings.json`
- **Extra citation styles** (`.csl` files) go in a `styles` folder beside the
  settings file; **Preferences → Open styles folder** opens it.
- **Tests:** `cargo test --release` runs the test suite on any system.

## Automatic builds on GitHub

Every push to GitHub builds and tests Sūcī on Linux, macOS and Windows
(`.github/workflows/release.yml`). Pushing a version tag, e.g.

```sh
git tag v0.3.1 && git push origin v0.3.1
```

publishes a GitHub Release with the Linux `.deb` and the Windows `.zip`
attached. macOS users build with the steps above.
