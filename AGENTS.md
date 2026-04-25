# AGENTS.md — Colomin

## What this is
Native macOS CSV editor built with **Rust + eframe/egui**.

- Active UI stack: `eframe`, `egui`, `egui_extras`
- Data/file work: `csv`, background threads, temp-file save + atomic rename
- macOS integration: `.app` bundle, Apple Events, Unix socket forwarding in tab mode

## Build & run

For any real build artifact, always build via:

```bash
./scripts/bundle.sh
```

That is the canonical build path for this repo because it also refreshes the macOS app bundle, bundle metadata, and Finder-facing behavior. Do not treat `cargo build --release` as the primary build command in agent workflows.

```bash
# Development (empty window)
cargo run

# Development with a file
cargo run -- /path/to/file.csv

# Canonical build command
./scripts/bundle.sh

# Fast local iteration with the bundled app
ln -sf "$(pwd)/target/release/Colomin.app" /Applications/Colomin.app

# Kill old instance and open a test file through the bundle
pkill -f "Colomin" 2>/dev/null; sleep 0.5; open /Applications/Colomin.app --args /tmp/wide_test.csv
```

Useful paths:

- Dev binary: `target/debug/colomin`
- Release binary produced by `./scripts/bundle.sh`: `target/release/colomin`
- Bundled executable: `target/release/Colomin.app/Contents/MacOS/Colomin`
- Persisted settings: `~/.config/colomin/settings.json`
- Debug logs when enabled: `~/Library/Logs/Colomin/`
- Finder bundle metadata: `Info.plist`

## Source layout

```text
src/
  main.rs                — startup, viewport/icon setup, socket forwarding, Apple Event bootstrap
  app.rs                 — ColominApp, tab management, IPC polling, status bar, settings, save/open glue
  file_open.rs           — background indexing + first-chunk preload + progress handle
  apple_events.rs        — macOS Finder/Open With integration via winit delegate method injection
  debug_log.rs           — optional zero-overhead-when-off debug logging
  state/
    app_state.rs         — AppState, selection model, row cache, layout caches, theme memory
    types.rs             — OpenFile, EditAction, RowSource/ColSource, selection/sort enums
  ui/
    mod.rs               — active UI module exports
    table.rs             — ACTIVE egui table view: scrolling, selection, editing, menus, undo/redo
    theme.rs             — DTCG token parsing + egui visuals
    icons.rs             — embedded/tinted SVG icon helpers
    stats.rs             — sync/async selection stats helpers
  csv_engine/
    parser.rs            — delimiter detection, indexing, row reads, sort, aggregate stats
    query.rs             — search/filter helpers
    writer.rs            — temp save + atomic rename
    types.rs             — CSV metadata/types
themes/                  — bundled `*.tokens.json` theme files
assets/                  — icons, app icon, spinner, misc assets
plans/                   — planning notes and older references
docs/                    — licensing / site docs
```

## Runtime architecture

### App shape

```text
main.rs
  -> ColominApp
       -> Vec<TabState>
            -> AppState
            -> TableView
            -> optional background receivers:
               file load / sort / stats
```

Key points:

- `ColominApp` owns tabs, active tab index, tab mode, IPC receiver, font catalog, debug-log toggle, and UI scale.
- `TabState` owns the per-tab `AppState`, the live `TableView`, and optional background-task handles.
- The status bar and settings popover are rendered from `app.rs::show_status_bar_for`; there is no active standalone status-bar module.

### Tab mode vs instance mode

- `tab_mode` is persisted in `~/.config/colomin/settings.json` and defaults to `true`.
- In tab mode, a second launch with a file forwards the path to the running app over `/tmp/colomin-$USER.sock`.
- In instance mode, additional opens spawn a new process instead of reusing the existing window.

### macOS file-open behavior

`main.rs` always installs Apple Event support on macOS so Finder "Open With" works for both fresh launches and already-running instances.

Important coupling:

- `main.rs` sets up the shared `mpsc::channel<String>`
- `apple_events.rs` injects `application:openURLs:`, `application:openFiles:`, and `application:openFile:` into winit's app delegate at launch time
- `ColominApp` polls the receiver every frame and routes paths through `open_file_in_tab`

There is an explicit `cli_arg_dedup` guard in `app.rs` to avoid an infinite spawn/open loop when macOS re-delivers the startup file as an Apple Event.

## File open flow

Shared path:

1. `ColominApp::open_file_in_tab` decides whether to reuse the active empty tab, switch to an already-open tab, create a new tab, or spawn a new process.
2. `TabState::start_loading` sets `is_loading`, zeroes progress, stores the filename, and creates a `LoadingHandle`.
3. `file_open::open_file_async` spawns a background thread.
4. The thread runs `csv_engine::parser::index_file_with_progress`, then preloads the first 200 rows with `read_chunk_with_delim`.
5. Progress is exposed via `Arc<AtomicU32>` holding `f32::to_bits()`.
6. `app.rs` polls the handle each frame, updates the progress bar/title, and calls `file_open::apply_loaded_file` on success.

`apply_loaded_file` resets:

- file state
- selection
- cache
- column widths / row heights
- sort/filter flags
- loading state

It also seeds the row cache with the first preloaded chunk.

## AppState and data model

`AppState` is the main mutable view model for a tab.

### Selection

- `SelectionType`: `Cell`, `Row`, `Column`
- Cell ranges are derived from `selection_anchor` + `selection_focus`
- Row and column selection are stored as index vectors
- `selection_stats_key()` is used to invalidate async stats work when selection changes

### Row cache

- `row_cache: HashMap<usize, Vec<String>>`
- LRU-ish eviction via `row_cache_order`
- `ROW_CACHE_LIMIT = 5000`
- Keys are **display-space data row indices** after header adjustment and after sort/filter mapping

### Layout caches

- `RowLayout` caches row tops / total content height
- `ColumnLayout` caches total content width
- Call `invalidate_row_layout()` / `invalidate_col_layout()` after row-height, column-width, or structural changes

### OpenFile

`OpenFile` carries:

- CSV metadata and row byte offsets
- per-cell edits
- inserted rows / inserted columns
- row and column order indirection
- `sort_permutation`
- `filter_indices`
- renamed-column state

### Header-row mode

`header_row_enabled` changes behavior in a few places:

- `true`: table header shows actual column names
- `false`: header shows Excel-style `A`, `B`, `C` labels and display row `0` becomes a synthetic row containing the column names

That means editing display row `0` when the header row is off renames columns instead of editing CSV data.

## Active UI behavior

The live table implementation is `src/ui/table.rs`.

### Scrolling

- Horizontal scrolling is handled by an outer `egui::ScrollArea::horizontal()`
- Vertical scrolling is handled inside `egui_extras::TableBuilder`
- Native scrollbars are hidden
- Colomin paints custom overlay scrollbars pinned to the right and bottom edges
- `TableView` stores explicit drag/alpha/content/viewport fields for both axes

If scrolling or thumb behavior breaks, start in `src/ui/table.rs`, not the legacy `src/ui/table/` directory.

### Table features

- sticky header row
- custom row-number gutter
- column resize handles in the header
- row resize handles in the gutter
- single-cell solid selection border
- multi-cell animated dashed border
- row highlight on hover (toggleable from settings)
- column rename on header double-click
- context menus on header, rows, and cells

### Context menu actions

Header menu:

- insert column left/right
- sort ascending/descending
- move column left/right
- rename
- delete column

Row menu:

- insert row above/below
- move row up/down
- delete row

Cell menu:

- copy cell value
- sort by that column

### Keyboard behavior

Inside the active table:

- Arrow keys move the cell selection
- Shift+Arrow extends the selection
- Enter starts editing the anchor cell
- Escape clears the selection
- Delete / Backspace clears selected cells
- Cmd+C copies the current cell range
- Cmd+V pastes tab/newline-shaped clipboard data into the current anchor
- Cmd+Z undoes
- Cmd+Shift+Z or Cmd+Y redoes

App-level shortcuts in `app.rs`:

- Cmd+O open file
- Cmd+S save
- Cmd+W close tab
- Cmd+Shift+T new tab
- Cmd+T cycle theme
- Cmd+= / Cmd+- / Cmd+0 zoom

## Async work

### Sorting

- UI writes `state.pending_sort = Some((column, ascending))`
- `app.rs` notices it, spawns a background thread, and calls `csv_engine::parser::sort_rows`
- On completion it writes `file.sort_permutation`, sets `sort_state`, clears the row cache, and invalidates row layout

### Selection stats

- Small selections are computed synchronously from cached/display data
- Large selections are computed on a background thread from a `ui::stats::StatsSnapshot`
- Async completions are accepted only when the returned key matches `selection_stats_key()`

### Debug logging

`src/debug_log.rs` is the active logging path.

- Logging is off by default
- When disabled, the `dlog!` macro is designed to have effectively zero runtime overhead
- When enabled from the settings popover, logs are written to `~/Library/Logs/Colomin/`

## Save flow

`Cmd+S` is handled in `app.rs::handle_save_tab`.

1. `csv_engine::writer::save_file` writes to `<target>.csv.tmp`
2. Non-structural saves stream the original file and overlay per-cell edits
3. Structural saves resolve rows/columns through `row_order` / `col_order`
4. Temp file is atomically renamed onto the target
5. The tab reloads through the normal async open path

Practical consequence: save/reload resets view-only state like current sort/filter UI state unless that state has been materialized into structural data.

## Themes, fonts, and settings

### Themes

Themes are **DTCG design token** JSON files, not Zed themes.

Bundled at compile time:

- `Colomin Light`
- `Colomin Dark`
- `GitHub Light`
- `GitHub Dark`

`ui/theme.rs` parses `themes/*.tokens.json` with `include_str!` and applies colors to egui visuals at runtime.

### Theme selection rules

- First launch uses the first dark theme if macOS is currently in dark mode
- Theme selection is remembered only in process statics right now; it is not written to `settings.json`

### Fonts and zoom

- System fonts are enumerated through `fontdb`
- Selecting a font injects it into egui's proportional family
- `font_size` is separate from `ui_scale`
- Both live in the settings popover rendered from `app.rs`

Persisted config fields:

- `selected_font`
- `tab_mode`
- `font_size`
- `ui_scale`

## Current quirks / tech debt

- `EditAction::Structural` is recorded but currently a no-op in undo/redo handlers.
- Search/filter helpers exist in `csv_engine::query.rs`, but the current egui UI does not expose a full search/filter surface.
- If you change bundle metadata, keep `Info.plist`, `Cargo.toml` bundle metadata, and `scripts/bundle.sh` aligned.

## Debugging workflow

1. Reproduce with `cargo run -- file.csv` for general data/UI issues.
2. When you need to build, use `./scripts/bundle.sh` rather than `cargo build --release`.
3. Use `./scripts/bundle.sh` and `/Applications/Colomin.app` for Finder/Open With, icon, and bundle behavior.
4. Turn on Debug logging from the settings popover before instrumenting ad hoc prints.
5. For load/save issues inspect `src/file_open.rs`, `src/csv_engine/parser.rs`, and `src/csv_engine/writer.rs`.
6. For selection, editing, scrolling, resizing, or context-menu bugs inspect `src/ui/table.rs` first.
7. After changing widths, heights, or structure, verify cache/layout invalidation:
   `clear_cache()`, `invalidate_row_layout()`, `invalidate_col_layout()`.
8. For macOS open-routing issues inspect `src/main.rs` and `src/apple_events.rs` together; they are tightly coupled.
