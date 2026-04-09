# AGENTS.md — Colomin

## What this is
Native macOS CSV editor built with **Rust + GPUI** (Zed's UI framework). No web stack, no Zed `ui` crate — only the `gpui` crate directly.

## Build & run

```bash
# Dev (slow, opt-level 1)
cargo run -- /path/to/file.csv

# Release .app bundle (required for Finder integration, icon, Info.plist)
./scripts/bundle.sh

# Symlink for fast iteration
ln -sf $(pwd)/target/release/Colomin.app /Applications/Colomin.app

# Kill old instance before testing
pkill -f "Colomin" 2>/dev/null; sleep 0.5; open /Applications/Colomin.app --args /tmp/wide_test.csv
```

Binary name is `colomin-gpui` (not `colomin`). The `.app` bundle is at `target/release/Colomin.app`.

## Source layout

```
src/
  main.rs          — App entry, Colomin root view, Finder open via on_open_urls + Mutex queue
  state/mod.rs     — AppState, OpenFile, column_width() (default 150px), row_cache, edits
  ui/
    table.rs       — TableView (main editor, ~1300 lines, most active file)
    status_bar.rs  — StatusBar
    theme.rs       — ThemeColors, Zed theme JSON loading
  csv_engine/
    parser.rs      — index_file_with_progress, read_chunk_with_delim, read_single_row_from_reader
    types.rs       — CsvColumn, CsvMetadata
    writer.rs      — save_file
themes/            — Zed-format theme JSONs (github.json, macos-classic.json)
assets/            — spinner.svg (animated loading spinner)
```

## GPUI constraints — read before touching layout

**Only `gpui` crate** — do not import `ui`, `theme`, or other Zed workspace crates.

**Horizontal scrolling quirk:** `ListHorizontalSizingBehavior::Unconstrained` is broken — causes viewport width to collapse to 0 at max scroll. Use the manual `horizontal_offset: Rc<Cell<f32>>` approach instead:
- Rows and header use `absolute().top_0().left(px(-h_off)).w(px(total_w))` inner div inside `relative().overflow_hidden()` outer div.
- Absolute children don't affect parent layout, preventing the collapse bug.
- `ml(px(-offset))` on a non-absolute inner div **also** causes viewport collapse — don't use it.

**Scroll events:** The `ScrollWheelEvent` listener for horizontal scroll must use `DispatchPhase::Capture` (not Bubble) so it fires before the uniform_list's own Bubble-phase handler.

**Window-level mouse listeners:** Use `canvas(|_,_,_|{}, move |_,_,window,_| { window.on_mouse_event(...); })` for drag events that must fire outside element bounds. These are registered per-frame and auto-cleared.

**Mouse events on divs are hitbox-scoped** — `on_mouse_move` / `on_mouse_up` only fire within the element. Use canvas + `window.on_mouse_event` for global drag.

**`scroll_handle.0.borrow().base_handle`** — scroll handle bounds are 0×0 on first render. Use a `scrollbar_initialized` flag + 100 ms deferred `cx.notify()` so scrollbars appear after first layout.

**`ScrollHandle` API:** `offset()` returns negative values when scrolled; `max_offset()` returns positive max range; `set_offset()` does not call `cx.notify()` — call `window.refresh()` manually.

## Architecture notes

- `uniform_list` handles **vertical** scroll only. Horizontal scroll is fully manual via `horizontal_offset: Rc<Cell<f32>>` in `TableView`.
- Row cache is an unbounded `HashMap<usize, Vec<String>>` — never evicts (known issue, see `plans/plan.md` for LRU plan).
- `on_open_urls` fires on a non-GPUI thread → uses `Arc<Mutex<Vec<String>>>` queue polled every 50 ms.
- File I/O is always off-thread via `cx.spawn` + `std::thread::spawn` + `mpsc::channel`.

## GPUI source reference

```
~/.cargo/git/checkouts/zed-a70e2ad075855582/a17a1c1/crates/gpui/src/elements/
  uniform_list.rs   — UniformListScrollHandle, visible range calc
  div.rs            — ScrollHandle, paint_scroll_listener, hitbox-scoped events
  canvas.rs         — canvas() element
  list.rs           — ListHorizontalSizingBehavior
```

## Test CSV

```bash
# Wide: 30 cols × 500 rows (tests horizontal scroll)
/tmp/wide_test.csv
```
