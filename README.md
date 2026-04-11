# Colomin

A native CSV editor built with Rust and [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui).

## Features

- Open and edit CSV, TSV, and delimited text files
- Auto-detect delimiters (comma, semicolon, tab, pipe)
- Column type inference (string, number, boolean)
- Cell selection and editing with undo/redo
- Column statistics (sum, avg, min, max) for selections
- Sort and filter rows
- Multiple themes (loaded from Zed theme format)
- Large file support with background loading
- Finder integration on macOS (.app bundle)
- Multi-window file opens from Finder (one window per file)
- Empty-state start screen with Open File button when no file is loaded
- Title bar shows filename while loading and change count when edited

## Run

Requires Rust toolchain.

```bash
# Development (empty state)
cargo run

# Development with file
cargo run -- path/to/file.csv
```

## Build

```bash
# Release binary
cargo build --release

# macOS .app bundle (required for Finder integration/icon/Info.plist)
./scripts/bundle.sh
```

The bundled app is output to `target/release/Colomin.app`.

For fast local iteration with the bundled app:

```bash
ln -sf "$(pwd)/target/release/Colomin.app" /Applications/Colomin.app
open /Applications/Colomin.app --args /tmp/wide_test.csv
```

## macOS Behavior

- Uses explicit quit mode: closing a window does not quit the app.
- Clicking the Dock icon with no open windows reopens an empty Colomin window.
- Opening files from Finder uses app-level URL handling.
- During loading, the window title shows the incoming filename.

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| Cmd+O | Open file |
| Cmd+S | Save file |
| Cmd+T | Cycle theme |
| Cmd+Q | Quit |
| Cmd+C | Copy selection |
| Arrow keys | Navigate cells |
| Shift+Arrow | Extend selection |
| Enter | Edit cell / confirm edit |
| Escape | Cancel edit / clear selection |
| Delete | Clear selected cells |

## Notes

- Bundled app executable path: `target/release/Colomin.app/Contents/MacOS/Colomin`
- Dev executable path: `target/debug/colomin`

## Licensing

Colomin uses a dual model:

- Open source code under AGPL v3 (or later)
- Commercial terms for official business use of branded binaries/services

This keeps the app free for personal, hobby, and community use while allowing paid official business usage.

See:

- [Commercial Terms](docs/COMMERCIAL.md)
- [Trademark and Branding Policy](docs/TRADEMARK_POLICY.md)

Note: this repository documentation is a practical policy draft, not legal advice.
