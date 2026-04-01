# Colomin

A native CSV editor built with Rust and [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui).

## Features

- Open and edit CSV, TSV, and delimited text files
- Auto-detect delimiters (comma, semicolon, tab, pipe)
- Column type inference (string, number, boolean)
- Cell selection and editing with undo support
- Column statistics (sum, avg, min, max) for selections
- Sort and filter rows
- Multiple themes (loaded from Zed theme format)
- Large file support with background loading

## Build

Requires Rust toolchain.

```bash
# Development
cargo run
cargo run -- path/to/file.csv

# Release build
cargo build --release

# macOS .app bundle (with icon)
./scripts/bundle.sh
```

The bundled app is output to `target/release/Colomin.app`.

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

## License

MIT
