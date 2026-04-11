# AGENTS.md — Colomin

## What this is
Native macOS CSV editor built with **Rust + GPUI** (Zed's UI framework). No web stack, no Zed `ui` crate — only the `gpui` crate directly.

## Build & run

```bash
# Dev (opt-level 1 — faster compile, slower runtime)
cargo run -- /path/to/file.csv

# Release .app bundle (required for Finder integration, icon, Info.plist)
./scripts/bundle.sh

# Symlink for fast iteration (run once)
ln -sf $(pwd)/target/release/Colomin.app /Applications/Colomin.app

# Kill old instance before testing
pkill -f "Colomin" 2>/dev/null; sleep 0.5; open /Applications/Colomin.app --args /tmp/wide_test.csv

# Test CSV (30 cols × 500 rows — tests horizontal scroll)
/tmp/wide_test.csv
```

Binary name is `colomin` for Rust builds. Bundled app executable is `Colomin` inside `target/release/Colomin.app`.

## Source layout

```
src/
  main.rs              — App entry, Colomin root view, Finder open, context menu, key bindings
  state/mod.rs         — AppState, OpenFile, selection system, row cache, edits, sort/filter state
  ui/
    table.rs           — TableView (~1340 lines, MOST ACTIVE FILE — scrolling, rendering, editing)
    status_bar.rs      — StatusBar (3-zone: file info | selection stats | sort/filter badges)
    theme.rs           — ThemeColors, Zed theme JSON loading, bundled_themes()
  csv_engine/
    parser.rs          — index_file_with_progress, read_chunk_with_delim, search/sort/filter, aggregate_column
    types.rs           — CsvColumn, CsvMetadata, RowChunk, ColumnStats, FilterCriteria
    writer.rs          — save_file (temp file + atomic rename)
themes/                — Zed-format theme JSONs (github.json, macos-classic.json)
assets/                — spinner.svg (animated loading spinner)
plans/                 — Performance optimization plans (LRU cache, DuckDB, memmap)
```

## Architecture

### Entity hierarchy
```
Application
  └── Window (1200×800)
        └── Colomin (root view)
              ├── TableView   — flex_1, min_h_0 (fills space)
              └── StatusBar   — flex_shrink_0, h=26px (only shown when file loaded)
```
All three views share `Entity<AppState>`. `Colomin` observes `AppState` and calls `cx.notify()` on any change.

### File open flow
Open behavior is centralized in `file_open::open_file_async` (`src/file_open.rs`):
- `Colomin::open_file_async` calls into shared open logic (CLI + root handlers)
- `TableView::on_t_open_file` calls the same shared open logic (Cmd+O)

Shared flow:
1. Set `is_loading = true`, `loading_progress = 0.0`, `loading_message = filename`
2. Create `Arc<AtomicU32>` for progress (stores `f32` bits via `to_bits()`/`from_bits()`)
3. Spawn **progress polling task**: every 50ms reads atomic, updates `AppState.loading_progress`
4. Spawn **OS thread** via `std::thread::spawn`:
   - `parser::index_file_with_progress` (builds row byte offsets + column metadata)
   - Pre-reads first 200 rows via `read_chunk_with_delim`
   - Sends result via `mpsc::channel`
5. Spawn **async receiver**: polls `rx.try_recv()` every 50ms, enforces **400ms minimum** loading screen
6. On success: constructs `OpenFile`, clears cache/selection, caches first 200 rows

### Finder integration
`on_open_urls` fires on a **non-GPUI thread** — cannot call GPUI APIs directly.
Uses `Arc<Mutex<Vec<String>>>` queue, polled every 100ms from an async task (first iteration immediate).
URLs are `file://` with percent-encoding — decoded manually in `on_open_urls` callback.

### App lifecycle (macOS)
- App uses `QuitMode::Explicit` (closing windows does not implicitly quit the process).
- `on_reopen` opens an empty window when no windows are visible, with a short delay so pending Finder URL opens can win and avoid extra empty/loading windows.
- Startup empty-window creation is also delayed briefly to avoid races with Finder `open_urls` delivery.

### Row cache
- `AppState.row_cache: HashMap<usize, Vec<String>>` — keyed by **virtual** (display) row index
- Bounded with LRU-style recency tracking (`ROW_CACHE_LIMIT = 5000`, `row_cache_order`)
- `cache_version: u64` — bumped on every cache mutation, used for change detection
- `ensure_rows_cached(start, count)` in TableView:
  - **Without sort/filter**: reads contiguous chunk via `read_chunk_with_delim` (single seek + sequential read)
  - **With sort/filter**: reads rows individually via `read_single_row_from_reader`, mapping virtual → actual through `filter_indices` or `sort_permutation`

### Selection system
- `SelectionType`: `Cell`, `Row`, `Column`
- **Cell**: `selection_anchor` + `selection_focus` → `selection_range()` returns `(min_row, max_row, min_col, max_col)`
- **Row**: `selected_rows: Vec<usize>` (supports multi-select with Cmd+click)
- **Column**: `selected_columns: Vec<usize>`
- `is_cell_selected(row, col)` checks all three modes
- Cell hit-test: linear scan of column widths starting from `ROW_NUMBER_WIDTH`; uses `screen_x + horizontal_offset` to convert to content-space x before scanning

### Stats computation
- `maybe_compute_stats()` called every render in TableView
- **Atomic check-and-set**: check + mark `computing_stats = true` in single `state.update()` call to prevent spawning thousands of threads
- `stats_key` built from selection state — validated on completion to discard stale results
- Small cell ranges (<500 cells, all cached) skip async — status bar computes synchronously from cache
- Large ranges: spawns OS thread that streams CSV sequentially

### Save flow
1. `csv_engine::writer::save_file` writes to `<path>.csv.tmp`
2. Non-structural: sequential read + edit overlay. Structural: per-cell resolution through `resolve_row`/`resolve_col`
3. Atomic rename temp → target
4. Re-indexes the file to refresh state (both root and table save handlers reopen through shared async open flow)

## GPUI constraints — read before touching UI code

### Only `gpui` crate
Do **not** import `ui`, `theme`, or other Zed workspace crates. They depend on Zed's workspace system.

### Horizontal scrolling — the Unconstrained bug
`ListHorizontalSizingBehavior::Unconstrained` is **broken**. In `uniform_list.rs` prepaint, available width for items is computed as:
```
padded_bounds.width + scroll_offset.x.abs()
```
As you scroll right, items get wider layout → content gets wider → allows more scroll → feedback loop. At max scroll, viewport width collapses to **0** and rows become invisible.

**`ml(px(-offset))` also causes collapse** — negative margin on a non-absolute inner div affects parent layout measurement, triggering the same feedback loop.

**Working pattern** — absolute positioning inside relative+clipped parent:
```rust
// Outer: provides positioning context + clips overflow
let row_outer = div().relative().overflow_hidden().h(px(ROW_HEIGHT)).w_full();

// Inner: absolute so it doesn't affect parent layout
let inner = div().flex().flex_shrink_0().h_full()
    .absolute().top_0().left(px(-h_off))
    .w(px(total_w));  // explicit total content width

row_outer.child(inner)
```
Both header and rows use this identical pattern in `table.rs`.

### Scroll event dispatch
`uniform_list` registers its own `ScrollWheelEvent` listener in **Bubble phase** (`DispatchPhase::Bubble`). It processes both X and Y delta for its internal scrolling.

To intercept horizontal scroll delta before uniform_list consumes it, register your listener in **Capture phase**:
```rust
window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, _| {
    if phase != DispatchPhase::Capture { return; }
    let delta_x = match event.delta {
        ScrollDelta::Pixels(pt) => pt.x.as_f32(),
        ScrollDelta::Lines(pt) => pt.x * 20.0,  // Lines to pixels conversion
    };
    // ... update horizontal_offset, call window.refresh()
});
```

### Window-level mouse listeners via canvas
Mouse events on divs (`on_mouse_move`, `on_mouse_up`) are **hitbox-scoped** — they only fire when the cursor is within the element's bounds. For global listeners (scrollbar drag, etc.), use `canvas` + `window.on_mouse_event`:

```rust
canvas(
    |_, _, _| {},  // prepaint: no-op
    move |_, _, window, _| {  // paint: register listeners
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _| {
            if phase != DispatchPhase::Capture { return; }
            // fires regardless of cursor position
        });
    }
).w(px(0.)).h(px(0.)).absolute()  // zero-size, won't affect layout
```
These listeners are **registered per-frame** during paint and **auto-cleared** when the next frame renders. This is the intended GPUI pattern — no manual cleanup needed.

### ScrollHandle API gotchas
Access via `scroll_handle.0.borrow().base_handle`:
- `offset()` → `Point<Pixels>` — returns **negative** values when scrolled (e.g., `y = -500px` means scrolled down 500px)
- `max_offset()` → `Point<Pixels>` — returns **positive** max range
- `set_offset(Point<Pixels>)` — **does NOT call `cx.notify()`** — you must call `window.refresh()` manually
- `bounds()` → `Bounds<Pixels>` — returns **0×0 on first render** (before layout completes)

### Scrollbar initialization
Scroll handle bounds are 0×0 until after the first layout pass. Use a flag + deferred notify:
```rust
if !self.scrollbar_initialized && (vp_h == 0.0 || vp_w == 0.0) {
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(Duration::from_millis(100)).await;
        let _ = this.update(cx, |this, cx| {
            this.scrollbar_initialized = true;
            cx.notify();
        });
    }).detach();
}
```

### `window.refresh()` vs `cx.notify()`
- `cx.notify()` — marks the **entity** as changed, schedules re-render of its view. Use from within `Context<T>` or from `entity.update(cx, ...)`.
- `window.refresh()` — marks the **window** as dirty, schedules repaint. Use from window-level event handlers (canvas callbacks, `on_mouse_event`) where you don't have entity context.

## GPUI API quick reference

### Elements
```rust
div()                                        // Primary building block
canvas(prepaint_fn, paint_fn)                // Custom drawing / listener registration
uniform_list(id, item_count, render_fn)      // Virtualized list (uniform row height)
svg().path("assets/icon.svg")                // SVG rendering
img(ImageSource)                             // Image rendering
```

### Styled trait (CSS-like fluent API on all elements)
```rust
// Layout
.flex().flex_col().flex_row()                // Flex direction
.flex_1().flex_grow().flex_shrink_0()        // Flex sizing
.items_center().justify_center()             // Alignment
.gap(px(8.0))                               // Flex gap
.w(px(100.0)).h(px(28.0)).size_full()       // Sizing
.w_full().h_full().min_w(px(0.)).max_w(px(500.))
.p(px(8.0)).px(px(12.0)).py(px(4.0))       // Padding
.m(px(4.0)).ml(px(8.0)).mt(px(2.0))        // Margin

// Positioning
.relative().absolute().sticky()              // Position mode
.top(px(0.)).bottom(px(2.)).left(px(0.)).right(px(2.))
.top_0().left_0()                           // Zero shortcuts

// Overflow
.overflow_hidden().overflow_x_hidden()       // Clip content
.overflow_visible()

// Visual
.bg(color).opacity(0.5)                     // Background, opacity
.border_1().border_t_1().border_b_1()       // Borders (per-side)
.border_color(color).border_dashed()
.rounded(px(6.0)).rounded_full()            // Border radius
.shadow_sm().shadow_md()                    // Box shadow
.cursor_pointer().cursor(CursorStyle::Arrow)

// Text
.text_color(color).text_size(px(13.0))
.text_xs().text_sm().text_base().text_lg()  // Preset sizes
.truncate().text_ellipsis()                 // Text overflow
.font_weight(FontWeight::BOLD)
```

### InteractiveElement (on Div, Svg, Img, UniformList)
```rust
.on_mouse_down(MouseButton::Left, |ev: &MouseDownEvent, window, cx| { ... })
.on_mouse_up(MouseButton::Left, |ev, window, cx| { ... })
.on_mouse_move(|ev: &MouseMoveEvent, window, cx| { ... })
.on_scroll_wheel(|ev: &ScrollWheelEvent, window, cx| { ... })
.on_key_down(|ev: &KeyDownEvent, window, cx| { ... })
.on_action(cx.listener(Self::handler_method))   // Action dispatch
.key_context("MyContext")                        // For action keybinding scope
.track_focus(&focus_handle)                      // Focus tracking
.hover(|style| style.bg(hover_color))            // Hover style
```

### StatefulInteractiveElement (requires `.id()` first)
```rust
div().id("my-element")                          // Makes element stateful
    .on_click(|ev: &ClickEvent, window, cx| { ... })
    .on_hover(|&is_hovered, window, cx| { ... })
    .overflow_scroll()                          // Enable scroll on this div
    .track_scroll(&scroll_handle)               // Bind ScrollHandle
    .tooltip(|window, cx| { ... })              // Tooltip on hover
```

### Entity / Context state management
```rust
// Read state (immutable borrow)
let state = self.state.read(cx);

// Update state (mutable borrow + auto-notify)
self.state.update(cx, |state, cx| {
    state.field = value;
    cx.notify();  // trigger re-render of this entity
});

// Create listener closure with view access
cx.listener(|this: &mut Self, event, window, cx| { ... })

// Create processor (like listener but returns value — for uniform_list render)
cx.processor(|this: &mut Self, range, window, cx| -> Vec<impl IntoElement> { ... })

// Spawn async task with weak entity reference
cx.spawn(async move |this, cx| {
    // this: WeakEntity<Self>, cx: &mut AsyncWindowContext
    let _ = this.update(cx, |this, cx| { ... });
}).detach();

// Observe another entity
cx.observe(&other_entity, |this, _other, cx| cx.notify()).detach();
```

### Window API
```rust
// From window-level callbacks (canvas paint, on_mouse_event):
window.refresh();                               // Schedule repaint
window.on_mouse_event(|event, phase, window, cx| { ... });  // Per-frame listener
window.on_key_event(|event, phase, window, cx| { ... });
window.set_window_title("title");
window.mouse_position();                        // Current cursor position

// DispatchPhase:
DispatchPhase::Capture   // Fires first (back-to-front, outermost first)
DispatchPhase::Bubble    // Fires after (front-to-back, innermost first)
```

### Event types
```rust
MouseDownEvent  { button, position, modifiers, click_count }
MouseUpEvent    { button, position, modifiers, click_count }
MouseMoveEvent  { position, pressed_button, modifiers }
    .dragging() -> bool         // true if left button held

ScrollWheelEvent { position, delta, modifiers, touch_phase }
    .delta: ScrollDelta::Pixels(Point<Pixels>) | ScrollDelta::Lines(Point<f32>)

KeyDownEvent    { keystroke: Keystroke, is_held }
    keystroke.key_char: Option<String>      // printable character
    keystroke.modifiers.platform            // Cmd on macOS
    keystroke.modifiers.control
    keystroke.modifiers.shift
```

### Geometry & Color
```rust
px(f32) -> Pixels                    // Physical pixels
hsla(h, s, l, a) -> Hsla            // Color (h: 0-1, s: 0-1, l: 0-1, a: 0-1)
point(x, y) -> Point<T>
Pixels.as_f32() -> f32              // Extract raw value
Bounds { origin: Point, size: Size }
```

### Animation
```rust
svg().with_animation(
    "spin",
    Animation::new(Duration::from_millis(800)).repeat(),
    |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta)))
)
```

### Actions & Key bindings
```rust
// Define actions
actions!(namespace, [MyAction, OtherAction]);

// Bind keys (in app.run)
cx.bind_keys(vec![
    KeyBinding::new("cmd-s", SaveFile, Some("TableView")),
]);

// Handle in view
.on_action(cx.listener(Self::on_save_file))
fn on_save_file(&mut self, _: &SaveFile, window: &mut Window, cx: &mut Context<Self>) { ... }
```

## Colomin-specific patterns

### TableView constants
```rust
const ROW_HEIGHT: f32 = 28.0;
const ROW_NUMBER_WIDTH: f32 = 50.0;
const HEADER_HEIGHT: f32 = 30.0;
// Default column width: 150.0 (from AppState.column_width())
// Scrollbar: SIZE=8, MIN_THUMB=24, MARGIN=2
```

### TableView fields
| Field | Type | Purpose |
|-------|------|---------|
| `scroll_handle` | `UniformListScrollHandle` | Vertical scroll (uniform_list) |
| `horizontal_offset` | `Rc<Cell<f32>>` | Manual horizontal scroll (positive = right) |
| `scrollbar_drag` | `Rc<Cell<Option<bool>>>` | `Some(true)` = vertical drag, `Some(false)` = horizontal |
| `scrollbar_initialized` | `bool` | Guards deferred first-layout re-render |
| `editing` | `Option<(row, col, text)>` | Active cell edit state |
| `needs_focus` | `bool` | Auto-focus on first render |

### Row rendering pattern
```
row_outer  [id("r",ri), relative, overflow_hidden, h=ROW_HEIGHT, w_full, bg, border_b_1, hover]
  └── inner  [flex, flex_shrink_0, absolute, top_0, left(-h_off), w(total_w)]
        ├── row_number  [w=ROW_NUMBER_WIDTH, right-aligned, border_r_1]
        ├── cell_0      [flex_shrink_0, w=col_width, truncate]
        ├── cell_1
        └── ...
```
- Uncached rows: each cell shows `"…"` placeholder
- Editing cell: shows `"text|"` with accent border
- Selected cells: dashed border overlay via additional absolute div at selection edges
- Mouse click hit-test converts screen x → content x with `x + horizontal_offset.get()` before scanning column widths

### Undo/redo
- `Cmd+Z` undoes, `Cmd+Shift+Z` redoes single-cell edits
- `commit_edit` pushes `EditAction::CellEdit { row, col, old_value, new_value }` to `undo_stack` and clears `redo_stack`
- Old value captured inside `state.update` closure before `file.edits.insert()`
- `on_undo`/`on_redo` restore value in `file.edits` (removing key if restoring to original/empty) and update `row_cache`
- `BatchCellEdit` and `Structural` variants are no-ops in current undo/redo handlers

### Scrollbar corner gap
When both scrollbars are present, each is shortened by `SCROLLBAR_SIZE + SCROLLBAR_MARGIN * 2` (= 12px) to prevent overlap in the bottom-right corner.

### OpenFile virtual-to-actual row mapping
```
virtual_row → filter_indices[virtual] → sort_permutation[filtered] → actual file row
```
If no filter: skip first step. If no sort: skip second step. Identity when neither.

### Theme system
- Themes loaded from Zed JSON format via `include_str!` at compile time
- `bundled_themes()` returns: Colomin Light (hardcoded default) + parsed macos-classic.json + parsed github.json
- `ThemeColors` has 17 `Hsla` fields: `bg`, `surface`, `border`, `text_primary`, `text_secondary`, `text_tertiary`, `accent`, `accent_hover`, `accent_text`, `accent_subtle`, `edited`, `hover_row`, `danger`, `status_bar_bg`, `gutter_bg`, `line_number`, `selection`
- Accent color from Zed's `players[0].cursor`

## Debugging workflow

Use this order when debugging the app:

1. Reproduce in the correct runtime.
   - Use `cargo run -- /path/to/file.csv` for parser, state, selection, and normal UI issues.
   - Use `./scripts/bundle.sh` and `/Applications/Colomin.app` for Finder integration, bundle, icon, and Info.plist behavior.
   - Use `/tmp/wide_test.csv` first for horizontal scroll and layout regressions.

2. Classify the bug before editing.
   - `src/ui/table.rs` — scrolling, rendering, selection, editing, context menu, scrollbar drag
   - `src/main.rs` — app lifecycle, Finder queue, CLI open, root view wiring
   - `src/state/mod.rs` — selection model, cache, sort/filter state, undo/redo
   - `src/csv_engine/parser.rs` / `src/csv_engine/writer.rs` — indexing, chunk reads, search/sort/filter, save

3. Check known GPUI pitfalls first for UI bugs.
   - Horizontal scroll bugs usually mean `horizontal_offset`, absolute-positioned inner content, or non-Capture wheel handling.
   - Drag bugs usually mean hitbox-scoped div mouse handlers were used where `canvas` + `window.on_mouse_event` is required.
   - Missing scrollbars usually mean first-render `bounds()` is `0x0` and `scrollbar_initialized` has not re-triggered layout yet.
   - `ScrollHandle::set_offset()` does not notify; call `window.refresh()` from window-level handlers.

4. Instrument subsystem boundaries, not every render path.
   - Log file open start/finish, loading progress updates, first chunk cache fill, and cache misses in `ensure_rows_cached()`.
   - Log selection anchor/focus transitions, stats task start/finish, and stale stats result suppression.
   - Log horizontal offset changes, viewport width, content width, and scrollbar bounds when debugging layout.

5. Verify invariants after each suspect transition.
   - `horizontal_offset` stays clamped to `0..=max_x`.
   - Visible rows stay within `effective_row_count()`.
   - Row cache keys are virtual row indices, not actual source rows.
   - Cached rows have the expected column count.
   - Selection anchor/focus remain in bounds.
   - Async results still match the current file or selection before applying state updates.

6. Compare duplicated flows before fixing symptoms.
   - File open logic exists in both `Colomin::open_file_async` and `TableView::on_t_open_file`.
   - Save/reload behavior also differs between root and table paths.
   - If behavior is inconsistent across Cmd+O, CLI open, and Finder open, inspect both implementations first.

7. Fix the owning layer.
   - Parser/indexing bug: fix parser or state mapping, not table rendering.
   - Scroll/layout bug: fix GPUI layout or event handling, not cache behavior.
   - Finder bug: fix queue/decode/open flow, not generic CSV logic unless both paths are broken.

8. Prefer tests below the UI layer.
   - Add tests for delimiter detection, row offsets, chunk reads, save round-trips, sort/filter mapping, and selection helpers.
   - Use manual verification for GPUI scroll, drag, and layout behavior.

9. Verify with a short manual script.
   - Open the smallest CSV that reproduces the bug.
   - Re-run the exact interaction.
   - Verify no regression in open, edit, scroll, and save flows.

10. Remove temporary debug noise.
   - Keep only logs or assertions that help future debugging.
   - Delete one-off prints once the fix is confirmed.

## Known issues & tech debt
- **Command ownership split**: Open/save/theme/quit handlers still have root (`main.rs`) and table command entry points. Behavior is shared, but command wiring remains distributed.

## GPUI source reference

When debugging GPUI behavior, read these files directly:
```
~/.cargo/git/checkouts/zed-a70e2ad075855582/a17a1c1/crates/gpui/src/
  elements/
    uniform_list.rs   — UniformListScrollHandle, visible range calc, Unconstrained bug
    div.rs            — ScrollHandle, paint_scroll_listener, hitbox-scoped events, Styled
    canvas.rs         — canvas() function, prepaint/paint callbacks
    list.rs           — ListHorizontalSizingBehavior enum, List element
    svg.rs            — Svg element, Transformation
    img.rs            — Img element, ImageSource
  window.rs           — Window API, on_mouse_event, refresh(), DispatchPhase
  interactive.rs      — MouseEvent types, ScrollDelta, KeyDownEvent
  styled.rs           — Styled trait (all CSS-like methods)
  geometry.rs         — Pixels, Point, Bounds, Size, Hsla, px()
  element.rs          — Render, RenderOnce, IntoElement traits
  context.rs          — Context<T> methods (notify, spawn, listener, processor)
```
