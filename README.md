# Colomin

Colomin is a native CSV editor for macOS built with Rust and `eframe`/`egui`.

It focuses on fast file open/save, spreadsheet-style editing, and Finder-friendly app-bundle behavior without a web stack.

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
- Finder / "Open With" integration through the macOS app bundle
- Tab mode or separate-window instance mode
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

# macOS .app bundle
./scripts/bundle.sh
```

Artifacts:

- Dev binary: `target/debug/colomin`
- Release binary: `target/release/colomin`
- Bundled app: `target/release/Colomin.app`
- Bundled executable: `target/release/Colomin.app/Contents/MacOS/Colomin`

For quick local iteration with the bundle:

```bash
ln -sf "$(pwd)/target/release/Colomin.app" /Applications/Colomin.app
open /Applications/Colomin.app --args /tmp/wide_test.csv
```

## macOS behavior

- Finder open events are handled through Apple Events, including "Open With" on a running instance.
- In tab mode, additional file opens reuse the running app as new tabs.
- In instance mode, additional opens launch a new Colomin process/window.
- The bundle icon and document associations come from `Info.plist` plus `scripts/bundle.sh`.

## Keyboard shortcuts

| Shortcut | Action |
|----------|--------|
| `Cmd+O` | Open file |
| `Cmd+S` | Save current file |
| `Cmd+W` | Close current tab |
| `Cmd+Shift+T` | New tab |
| `Cmd+T` | Cycle theme |
| `Cmd+=` / `Cmd+-` / `Cmd+0` | Zoom in / out / reset |
| `Cmd+C` | Copy selection |
| `Cmd+V` | Paste into cell selection |
| `Cmd+Z` | Undo |
| `Cmd+Shift+Z` or `Cmd+Y` | Redo |
| `Arrow keys` | Move cell selection |
| `Shift+Arrow` | Extend selection |
| `Enter` | Edit selected cell |
| `Escape` | Clear selection |
| `Delete` / `Backspace` | Clear selected cells |

## Configuration and logs

- Settings are stored at `~/.config/colomin/settings.json`
- Optional debug logs are written to `~/Library/Logs/Colomin/`

## Notes

- Themes are bundled from `themes/*.tokens.json`
- Icons are embedded from `assets/icons/*.svg`
- The UI stack is `eframe`/`egui`

## Licensing

Colomin uses a dual model:

- Open source code under AGPL v3 (or later)
- Commercial terms for official business use of branded binaries/services

See:

- [Commercial Terms](docs/COMMERCIAL.md)
- [Trademark and Branding Policy](docs/TRADEMARK_POLICY.md)

This repository documentation is a practical policy draft, not legal advice.
