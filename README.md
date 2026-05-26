# Colomin

A fast, native CSV editor for **macOS, Linux, and Windows**, built with Rust and `eframe`/`egui`.

Focuses on fast file open/save, spreadsheet-style editing, and file-manager–friendly app behavior without a web stack.

## Features

- Open CSV, TSV, and delimited text files
- Auto-detect common delimiters: comma, semicolon, tab, pipe
- Column type inference on load
- Background indexing with visible loading progress
- Spreadsheet-style cell, row, and column selection
- In-place cell editing with undo/redo
- Copy/paste rectangular cell ranges
- Column sorting
- Insert/delete/reorder rows and columns, plus column rename
- Selection stats for count, sum, avg, min, max, and character length
- Multiple bundled light/dark themes
- Font, font-size, and zoom controls
- Drag-and-drop file opening
- File-manager "Open With" integration on every platform (Finder on macOS, Explorer on Windows, GNOME Files / Dolphin / Thunar on Linux via `.desktop` MimeType associations)
- Tab mode or separate-window instance mode (single-instance handoff via Unix sockets on macOS/Linux, named pipes on Windows)
- Optional session debug logging

## Run

Requires a Rust toolchain.

```bash
# Development (empty window)
cargo run

# Development with a file
cargo run -- /path/to/file.csv
```

## Build

```bash
# Release binary
cargo build --release

# All platform packages
cargo install cargo-packager cargo-generate-rpm --locked
cargo packager --release
cargo generate-rpm     # Linux only — produces a .rpm
```

Artifacts (per OS, written to `dist/` and `target/generate-rpm/`):

| Platform | Outputs |
|----------|---------|
| macOS    | `Colomin.app`, `Colomin*.dmg` (universal: arm64 + x86_64) |
| Linux    | `colomin*.deb`, `Colomin*.AppImage`, `colomin*.rpm` (per arch: x86_64 + arm64) |
| Windows  | `Colomin*.msi` (x86_64 only — WiX 3.x doesn't reliably emit arm64), portable `colomin.exe` |

CI does this automatically for every `v*` tag — see `.github/workflows/release.yml`.

macOS releases fail unless these GitHub Actions secrets are set:

| Secret | Placeholder / expected value |
|--------|------------------------------|
| `MACOS_SIGNING_IDENTITY` | `Developer ID Application: YOUR_NAME_OR_ORG (TEAMID)` |
| `MACOS_CERTIFICATE` | Base64-encoded Developer ID Application `.p12` certificate |
| `MACOS_CERTIFICATE_PWD` | Password used when exporting the `.p12` |
| `AC_API_KEY_P8` | App Store Connect API key `.p8` contents |
| `AC_API_KEY_ID` | App Store Connect API key ID |
| `AC_API_KEY_ISSUER_ID` | App Store Connect issuer ID |

For quick local iteration on macOS:

```bash
cargo packager --release --formats app
ln -sf "$(pwd)/dist/Colomin.app" /Applications/Colomin.app
open /Applications/Colomin.app --args /tmp/wide_test.csv
```

## Install

Download the latest release from the [Releases page](https://github.com/saman/colomin/releases). All assets are version-pinned; `releases/latest` always serves the newest.

**macOS** (Apple Silicon + Intel, signed + notarized)
- Download `Colomin.dmg`, open it, drag `Colomin.app` to `Applications`.
- First launch opens normally — no Gatekeeper warning, since the build is notarized by Apple.

**Linux** (x86_64 + arm64)
- **`.deb`** (Debian, Ubuntu, Pop!_OS, Mint): `sudo dpkg -i Colomin*.deb` or open in your distro's App Center.
- **`.rpm`** (Fedora, RHEL, openSUSE, Rocky): `sudo dnf install ./Colomin*.rpm` (or `rpm -i`).
- **`.AppImage`** (any glibc Linux — Arch, NixOS, etc.): `chmod +x Colomin*.AppImage && ./Colomin*.AppImage`.
- All three install/register Colomin as a `.csv`/`.tsv` handler.
- Expected: GNOME Software / Discover / App Center will show "Unknown publisher" — this is the universal third-party `.deb`/`.rpm` experience and doesn't block install. Same as Slack, Discord, 1Password.

**Windows** (x86_64 native, arm64 portable)
- **x86_64** (most PCs): download `Colomin.msi`, double-click, run the installer wizard. Registers `.csv` file associations + Start Menu entry.
- **arm64** (Snapdragon X PCs, Surface Pro X): download `Colomin-portable-arm64.zip`, extract `colomin.exe`, run from anywhere. The binary is fully native arm64 — no emulation. A signed arm64 MSI is pending cargo-packager's upgrade to WiX 4.
- Also available: `Colomin-portable-x86_64.zip` for users who prefer not to install.
- Expected: SmartScreen will show "Windows protected your PC / Unknown publisher" — click *More info* → *Run anyway*. The binary is unsigned (Windows code-signing certs aren't free); same as most open-source Windows apps.

## Platform behavior

- **File-association "Open With"** routes through native APIs on each platform — Apple Events on macOS, file-association registry entries on Windows, MIME type handlers on Linux. Existing instance reuse works on all three.
- **Tab mode** opens additional files as new tabs in the running app. **Instance mode** spawns a fresh process per file.
- Single-instance handoff uses a per-user Unix socket on macOS/Linux (`/tmp/colomin-$USER.sock`) and a named pipe on Windows (`\\.\pipe\colomin-%USERNAME%`).
- Icons + document associations are generated by `cargo-packager` from `[package.metadata.packager]` in `Cargo.toml` (+ `[package.metadata.generate-rpm]` for RPM). Linux also ships an AppStream metainfo file at `/usr/share/metainfo/com.colomin.app.metainfo.xml` so the pre-install view in App Center shows the app name, icon, and description.

## Keyboard shortcuts

`Cmd` on macOS, `Ctrl` on Windows/Linux. The bindings are otherwise identical across platforms.

| Shortcut | Action |
|----------|--------|
| `Cmd/Ctrl+O` | Open file |
| `Cmd/Ctrl+S` | Save current file |
| `Cmd/Ctrl+W` | Close current tab |
| `Cmd/Ctrl+T` | New tab |
| `Cmd/Ctrl+Shift+T` | Cycle theme |
| `Cmd/Ctrl+F` | Find in table |
| `Cmd/Ctrl+=` / `Cmd/Ctrl+-` / `Cmd/Ctrl+0` | Zoom in / out / reset |
| `Cmd/Ctrl+C` | Copy selection |
| `Cmd/Ctrl+V` | Paste into cell selection |
| `Cmd/Ctrl+Z` | Undo |
| `Cmd/Ctrl+Shift+Z` or `Cmd/Ctrl+Y` | Redo |
| `Arrow keys` | Move cell selection |
| `Shift+Arrow` | Extend selection |
| `Enter` | Edit selected cell |
| `Escape` | Clear selection |
| `Delete` / `Backspace` | Clear selected cells |

## Configuration and logs

Per-platform standard locations (Colomin uses the `dirs` crate to resolve them):

| Path | macOS | Linux | Windows |
|------|-------|-------|---------|
| Settings | `~/.config/colomin/settings.json` | `~/.config/colomin/settings.json` | `%APPDATA%\colomin\settings.json` |
| Debug logs (when enabled) | `~/Library/Logs/Colomin/` | `$XDG_STATE_HOME/colomin/` or `~/.local/state/colomin/` | `%LOCALAPPDATA%\colomin\logs\` |
| Panic log | `$TMPDIR/colomin_panic.log` | `/tmp/colomin_panic.log` | `%TEMP%\colomin_panic.log` |

## Notes

- Themes are bundled from `themes/*.tokens.json`
- Icons are embedded from `assets/icons/*.svg`
- The UI stack is `eframe`/`egui` — `glow`/OpenGL backend on macOS + Linux, `wgpu`/DirectX 12 backend on Windows (avoids OpenGL driver issues on Windows servers and VMs)

## Provenance

Significant portions of this codebase were drafted with AI assistance and reviewed, tested, and integrated by a human maintainer.

## Licensing

Colomin uses a dual model:

- Open source code under AGPL v3 (or later)
- Commercial terms for official business use of branded binaries/services

See:

- [Commercial Terms](docs/COMMERCIAL.md)
- [Trademark and Branding Policy](docs/TRADEMARK_POLICY.md)

This repository documentation is a practical policy draft, not legal advice.
