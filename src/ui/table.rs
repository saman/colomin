use gpui::*;
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use crate::csv_engine::{self, parser};
use crate::state::{AppState, CellCoord, EditAction, SelectionType, SortDirection, SortState};

const ROW_HEIGHT: f32 = 28.0;
const ROW_NUMBER_WIDTH: f32 = 50.0;
const HEADER_HEIGHT: f32 = 30.0;

actions!(
    table,
    [SelectAll, Copy, Paste, Undo, Redo, Delete, MoveUp, MoveDown, MoveLeft, MoveRight,
     SelectUp, SelectDown, SelectLeft, SelectRight, Escape, Enter,
     TOpenFile, TSaveFile, TCycleTheme, TQuit]
);

pub struct TableView {
    pub state: Entity<AppState>,
    focus_handle: FocusHandle,
    editing: Option<(usize, usize, String)>,
    needs_focus: bool,
    scroll_handle: UniformListScrollHandle,
    /// Shared scrollbar drag state: Some(true) = vertical, Some(false) = horizontal.
    /// Uses Rc<Cell<>> so window-level mouse listeners can access it from plain closures.
    scrollbar_drag: Rc<Cell<Option<bool>>>,
    /// Manual horizontal scroll offset (positive = scrolled right).
    /// Managed independently from the uniform_list which only handles vertical scroll.
    horizontal_offset: Rc<Cell<f32>>,
    scrollbar_initialized: bool,
    /// Column being resized (index), shared with canvas drag listeners.
    column_resize: Rc<Cell<Option<usize>>>,
    /// (start_mouse_x_in_window, start_column_width) captured on resize mouse-down.
    column_resize_start: Rc<Cell<Option<(f32, f32)>>>,
}

impl TableView {
    fn hit_test_col_from_content_x(state: &AppState, x_content: f32) -> usize {
        if state.col_count() == 0 {
            return 0;
        }
        let mut col_x = ROW_NUMBER_WIDTH;
        let mut col = 0usize;
        for c in 0..state.col_count() {
            let w = state.column_width(c);
            if x_content < col_x + w {
                col = c;
                break;
            }
            col_x += w;
            col = c;
        }
        col
    }

    fn hit_test_row_from_window_y(&self, y_window: f32, total_rows: usize) -> Option<usize> {
        if total_rows == 0 {
            return None;
        }

        let sh = self.scroll_handle.0.borrow();
        let viewport_h = sh.base_handle.bounds().size.height.as_f32();
        let scroll_y = -sh.base_handle.offset().y.as_f32();
        drop(sh);

        if viewport_h <= 0.0 {
            return None;
        }

        // Convert from window space to table-body local y, then add scroll offset
        // to get absolute virtual row position.
        let local_y = (y_window - HEADER_HEIGHT).clamp(0.0, (viewport_h - 1.0).max(0.0));
        let row = ((local_y + scroll_y) / ROW_HEIGHT).floor().max(0.0) as usize;
        Some(row.min(total_rows.saturating_sub(1)))
    }

    fn is_in_scrollbar_hit_region(
        scroll_handle: &UniformListScrollHandle,
        state: &AppState,
        mouse_pos: Point<Pixels>,
    ) -> bool {
        const SCROLLBAR_SIZE: f32 = 8.0;
        const SCROLLBAR_MARGIN: f32 = 2.0;

        let sh = scroll_handle.0.borrow();
        let bounds = sh.base_handle.bounds();
        let max_y = sh.base_handle.max_offset().y.as_f32();
        drop(sh);

        let vp_w = bounds.size.width.as_f32();
        let vp_h = bounds.size.height.as_f32();
        if vp_w <= 0.0 || vp_h <= 0.0 {
            return false;
        }

        let content_w = if let Some(file) = &state.file {
            ROW_NUMBER_WIDTH
                + file
                    .metadata
                    .columns
                    .iter()
                    .map(|c| state.column_width(c.index))
                    .sum::<f32>()
        } else {
            0.0
        };
        let max_x = (content_w - vp_w).max(0.0);

        let has_v_bar = max_y > 0.0;
        let has_h_bar = max_x > 0.0;
        if !has_v_bar && !has_h_bar {
            return false;
        }

        let corner_gap = SCROLLBAR_SIZE + SCROLLBAR_MARGIN * 2.0;
        let ox = bounds.origin.x.as_f32();
        let oy = bounds.origin.y.as_f32();
        let mx = mouse_pos.x.as_f32();
        let my = mouse_pos.y.as_f32();

        let in_v_bar = if has_v_bar {
            let track_top = oy;
            let track_bottom = oy + if has_h_bar { vp_h - corner_gap } else { vp_h };
            let bar_left = ox + vp_w - SCROLLBAR_MARGIN - SCROLLBAR_SIZE;
            let bar_right = ox + vp_w - SCROLLBAR_MARGIN;
            mx >= bar_left && mx <= bar_right && my >= track_top && my <= track_bottom
        } else {
            false
        };

        let in_h_bar = if has_h_bar {
            let track_left = ox;
            let track_right = ox + if has_v_bar { vp_w - corner_gap } else { vp_w };
            let bar_top = oy + vp_h - SCROLLBAR_MARGIN - SCROLLBAR_SIZE;
            let bar_bottom = oy + vp_h - SCROLLBAR_MARGIN;
            mx >= track_left && mx <= track_right && my >= bar_top && my <= bar_bottom
        } else {
            false
        };

        in_v_bar || in_h_bar
    }

    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self {
            state,
            focus_handle: cx.focus_handle(),
            editing: None,
            needs_focus: true,
            scroll_handle: UniformListScrollHandle::new(),
            scrollbar_drag: Rc::new(Cell::new(None)),
            horizontal_offset: Rc::new(Cell::new(0.0)),
            scrollbar_initialized: false,
            column_resize: Rc::new(Cell::new(None)),
            column_resize_start: Rc::new(Cell::new(None)),
        }
    }

    fn ensure_rows_cached(&self, start: usize, count: usize, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let file = match &state.file { Some(f) => f, None => return };
        let effective_rows = file.effective_row_count();
        let col_count = file.metadata.columns.len();
        let path = file.file_path.clone();
        let row_offsets = file.row_offsets.clone();
        let edits = file.edits.clone();
        let delimiter = file.delimiter;
        let has_sf = file.sort_permutation.is_some() || file.filter_indices.is_some();

        let mut ns = None;
        let mut ne = start;
        for i in start..(start + count).min(effective_rows) {
            if state.get_cached_row(i).is_none() {
                if ns.is_none() { ns = Some(i); }
                ne = i + 1;
            }
        }
        let ns = match ns { Some(s) => s, None => return };
        let se = self.state.clone();

        if has_sf {
            let sp = file.sort_permutation.clone();
            let fi = file.filter_indices.clone();
            cx.spawn(async move |_, cx| {
                let rows = std::thread::spawn(move || {
                    let f = std::fs::File::open(&path).map_err(|e| format!("{e}"))?;
                    let mut rdr = std::io::BufReader::new(f);
                    let mut res = Vec::new();
                    for vr in ns..ne {
                        let ar = fi.as_ref().and_then(|i| i.get(vr).copied())
                            .or_else(|| sp.as_ref().and_then(|p| p.get(vr).copied()))
                            .unwrap_or(vr);
                        let mut rd = parser::read_single_row_from_reader(&mut rdr, &row_offsets, ar, col_count, delimiter)?;
                        for c in 0..col_count { if let Some(ed) = edits.get(&(ar, c)) { rd[c] = ed.clone(); } }
                        res.push((vr, rd));
                    }
                    Ok::<_, String>(res)
                }).join().unwrap_or_else(|_| Err("panic".into()));
                if let Ok(data) = rows {
                    let _ = se.update(cx, |s, cx| {
                        for (i, r) in data { s.cache_row(i, r); }
                        s.cache_version += 1; cx.notify();
                    });
                }
            }).detach();
        } else {
            cx.spawn(async move |_, cx| {
                let chunk = std::thread::spawn(move || {
                    parser::read_chunk_with_delim(&path, &row_offsets, &edits, ns, ne - ns, col_count, delimiter)
                }).join().unwrap_or_else(|_| Err("panic".into()));
                if let Ok(chunk) = chunk {
                    let _ = se.update(cx, |s, cx| {
                        for (i, r) in chunk.rows.into_iter().enumerate() { s.cache_row(chunk.start_index + i, r); }
                        s.cache_version += 1; cx.notify();
                    });
                }
            }).detach();
        }
    }

    /// Build a key that uniquely identifies the current selection for stats caching
    fn selection_stats_key(state: &AppState) -> String {
        match &state.selection_type {
            Some(SelectionType::Column) => {
                let mut cols: Vec<usize> = state.selected_columns.clone();
                cols.sort();
                format!("col:{:?}", cols)
            }
            Some(SelectionType::Row) => {
                let mut rows: Vec<usize> = state.selected_rows.clone();
                rows.sort();
                format!("row:{:?}", rows)
            }
            Some(SelectionType::Cell) => {
                if let Some((mr, xr, mc, xc)) = state.selection_range() {
                    format!("cell:{}-{}-{}-{}", mr, xr, mc, xc)
                } else {
                    String::new()
                }
            }
            None => String::new(),
        }
    }

    /// Check if we need to compute stats and spawn a background task if so
    fn maybe_compute_stats(&self, cx: &mut Context<Self>) {
        // Do the entire check-and-set in a single update to prevent race conditions
        // that spawn thousands of background threads
        let task_data = self.state.update(cx, |state, _| {
            let key = Self::selection_stats_key(state);

            // No selection or single cell — no async stats needed
            if key.is_empty() { return None; }
            if let Some(SelectionType::Cell) = &state.selection_type {
                if let Some((mr, xr, mc, xc)) = state.selection_range() {
                    if mr == xr && mc == xc { return None; }
                    let total_cells = (xr - mr + 1) * (xc - mc + 1);
                    if total_cells < 500 {
                        let all_cached = (mr..=xr).all(|r| state.get_cached_row(r).is_some());
                        if all_cached { return None; }
                    }
                }
            }

            // If already computing/computed for this exact key, skip
            if state.stats_key == key && (state.computing_stats || state.computed_stats.is_some()) {
                return None;
            }

            let file = match &state.file { Some(f) => f, None => return None };

            let data = (
                file.file_path.clone(),
                file.row_offsets.clone(),
                file.edits.clone(),
                file.delimiter,
                file.metadata.columns.len(),
                state.selection_type.clone(),
                state.selected_columns.clone(),
                state.selected_rows.clone(),
                state.selection_range(),
            );

            // Mark as computing NOW, atomically with the check
            state.computing_stats = true;
            state.computed_stats = None;
            state.stats_key = key.clone();

            Some((key, data))
        });

        let (key, (path, row_offsets, edits, delimiter, col_count, sel_type, sel_cols, sel_rows, sel_range)) = match task_data {
            Some((k, d)) => (k, d),
            None => return,
        };

        let se = self.state.clone();
        let key_for_task = key.clone();

        cx.spawn(async move |_, cx| {
            let result = std::thread::spawn(move || {
                match sel_type {
                    Some(SelectionType::Column) => {
                        let mut count = 0usize;
                        let mut num_count = 0usize;
                        let mut sum = 0.0f64;
                        let mut min = f64::INFINITY;
                        let mut max = f64::NEG_INFINITY;

                        for &col_idx in &sel_cols {
                            if let Ok(stats) = parser::aggregate_column(
                                &path, col_idx, &row_offsets, &edits, delimiter,
                            ) {
                                count += stats.count;
                                num_count += stats.numeric_count;
                                if let Some(s) = stats.sum { sum += s; }
                                if let Some(m) = stats.min { if m < min { min = m; } }
                                if let Some(m) = stats.max { if m > max { max = m; } }
                            }
                        }

                        let avg = if num_count > 0 { sum / num_count as f64 } else { 0.0 };
                        let min = if num_count > 0 && min.is_finite() { min } else { 0.0 };
                        let max = if num_count > 0 && max.is_finite() { max } else { 0.0 };
                        Some((count, num_count, sum, avg, min, max))
                    }
                    Some(SelectionType::Row) | Some(SelectionType::Cell) => {
                        // Stream the entire file sequentially and pick out the rows we need
                        let is_row_sel = matches!(sel_type, Some(SelectionType::Row));
                        let selected_row_set: std::collections::HashSet<usize> =
                            if is_row_sel { sel_rows.iter().copied().collect() }
                            else { std::collections::HashSet::new() };
                        let (mr, xr, mc, xc) = if is_row_sel {
                            (0, row_offsets.len().saturating_sub(1), 0, col_count.saturating_sub(1))
                        } else {
                            match sel_range { Some(r) => r, None => return None }
                        };

                        let mut count = 0usize;
                        let mut num_count = 0usize;
                        let mut sum = 0.0f64;
                        let mut min = f64::INFINITY;
                        let mut max = f64::NEG_INFINITY;

                        let f = std::fs::File::open(&path).ok()?;
                        let mut buf_reader = std::io::BufReader::new(f);
                        if !row_offsets.is_empty() {
                            use std::io::Seek;
                            buf_reader.seek(std::io::SeekFrom::Start(row_offsets[mr])).ok()?;
                        }
                        let mut csv_rdr = csv::ReaderBuilder::new()
                            .has_headers(false).flexible(true).delimiter(delimiter)
                            .from_reader(buf_reader);

                        for (i, result) in csv_rdr.records().enumerate() {
                            let row_idx = mr + i;
                            if row_idx > xr { break; }
                            let record = match result { Ok(r) => r, Err(_) => continue };

                            // Check if this row is in our selection
                            let in_selection = if is_row_sel {
                                selected_row_set.contains(&row_idx)
                            } else { true };
                            if !in_selection { continue; }

                            let col_start = if is_row_sel { 0 } else { mc };
                            let col_end = if is_row_sel { col_count.saturating_sub(1) } else { xc };

                            for ci in col_start..=col_end {
                                let val = if let Some(edited) = edits.get(&(row_idx, ci)) {
                                    edited.as_str()
                                } else {
                                    record.get(ci).unwrap_or("")
                                };
                                count += 1;
                                let trimmed = val.trim();
                                if !trimmed.is_empty() {
                                    if let Ok(n) = trimmed.parse::<f64>() {
                                        if n.is_finite() {
                                            num_count += 1;
                                            sum += n;
                                            if n < min { min = n; }
                                            if n > max { max = n; }
                                        }
                                    }
                                }
                            }
                        }

                        let avg = if num_count > 0 { sum / num_count as f64 } else { 0.0 };
                        let min = if num_count > 0 && min.is_finite() { min } else { 0.0 };
                        let max = if num_count > 0 && max.is_finite() { max } else { 0.0 };
                        Some((count, num_count, sum, avg, min, max))
                    }
                    None => None,
                }
            })
            .join()
            .ok()
            .flatten();

            let _ = se.update(cx, |s, cx| {
                // Only apply if the key still matches (selection didn't change while computing)
                if s.stats_key == key_for_task {
                    s.computed_stats = result;
                    s.computing_stats = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn commit_edit(&mut self, cx: &mut Context<Self>) {
        if let Some((row, col, ref text)) = self.editing {
            let new_value = text.clone();
            self.state.update(cx, |s, _| {
                // Capture old value before overwriting (from edits map, then cache)
                let old_value = s.file.as_ref()
                    .and_then(|f| f.edits.get(&(row, col)).cloned())
                    .or_else(|| s.row_cache.get(&row).and_then(|r| r.get(col)).cloned())
                    .unwrap_or_default();
                if let Some(ref mut file) = s.file {
                    file.edits.insert((row, col), new_value.clone());
                }
                if let Some(cached) = s.row_cache.get_mut(&row) {
                    if col < cached.len() { cached[col] = new_value.clone(); }
                }
                s.cache_version += 1;
                s.editing_cell = None;
                // Push to undo stack only if value changed
                if old_value != new_value {
                    s.undo_stack.push(EditAction::CellEdit { row, col, old_value, new_value });
                    s.redo_stack.clear();
                }
            });
        }
        self.editing = None;
        cx.notify();
    }

    fn on_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() { self.cancel_edit(cx); return; }
        self.state.update(cx, |s, _| {
            let action = match s.undo_stack.pop() {
                Some(a) => a,
                None => return,
            };
            match &action {
                EditAction::CellEdit { row, col, old_value, .. } => {
                    let (r, c) = (*row, *col);
                    if let Some(ref mut file) = s.file {
                        if old_value.is_empty() {
                            file.edits.remove(&(r, c));
                        } else {
                            file.edits.insert((r, c), old_value.clone());
                        }
                    }
                    if let Some(cached) = s.row_cache.get_mut(row) {
                        if *col < cached.len() { cached[*col] = old_value.clone(); }
                    }
                    // Move selection to undone cell
                    s.selection_anchor = Some(CellCoord { row: r, col: c });
                    s.selection_focus = Some(CellCoord { row: r, col: c });
                    s.selection_type = Some(SelectionType::Cell);
                }
                EditAction::BatchCellEdit { .. } | EditAction::Structural { .. } => {}
            }
            s.redo_stack.push(action);
            s.cache_version += 1;
        });
        cx.notify();
    }

    fn on_redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() { self.cancel_edit(cx); return; }
        self.state.update(cx, |s, _| {
            let action = match s.redo_stack.pop() {
                Some(a) => a,
                None => return,
            };
            match &action {
                EditAction::CellEdit { row, col, new_value, .. } => {
                    let (r, c) = (*row, *col);
                    if let Some(ref mut file) = s.file {
                        if new_value.is_empty() {
                            file.edits.remove(&(r, c));
                        } else {
                            file.edits.insert((r, c), new_value.clone());
                        }
                    }
                    if let Some(cached) = s.row_cache.get_mut(row) {
                        if *col < cached.len() { cached[*col] = new_value.clone(); }
                    }
                    // Move selection to redone cell
                    s.selection_anchor = Some(CellCoord { row: r, col: c });
                    s.selection_focus = Some(CellCoord { row: r, col: c });
                    s.selection_type = Some(SelectionType::Cell);
                }
                EditAction::BatchCellEdit { .. } | EditAction::Structural { .. } => {}
            }
            s.undo_stack.push(action);
            s.cache_version += 1;
        });
        cx.notify();
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        self.editing = None;
        self.state.update(cx, |s, _| { s.editing_cell = None; });
        cx.notify();
    }

    /// Map a mouse position to a scroll offset and apply it (for scrollbar click/drag).
    /// This is a static helper that works with shared Rc state for use from window-level listeners.
    fn apply_scrollbar_drag(
        scrollbar_drag: &Rc<Cell<Option<bool>>>,
        scroll_handle: &UniformListScrollHandle,
        horizontal_offset: &Rc<Cell<f32>>,
        mouse_pos: Point<Pixels>,
        content_width: f32,
    ) {
        let is_vertical = match scrollbar_drag.get() {
            Some(v) => v,
            None => return,
        };
        let sh = scroll_handle.0.borrow();
        let bounds = sh.base_handle.bounds();
        drop(sh);

        if is_vertical {
            let sh = scroll_handle.0.borrow();
            let max_off = sh.base_handle.max_offset();
            let current = sh.base_handle.offset();
            drop(sh);
            let vp_h = bounds.size.height.as_f32();
            let max_y = max_off.y.as_f32();
            if vp_h <= 0.0 || max_y <= 0.0 { return; }
            let relative_y = mouse_pos.y.as_f32() - bounds.origin.y.as_f32();
            let ratio = (relative_y / vp_h).clamp(0.0, 1.0);
            let new_offset = point(current.x, px(-(ratio * max_y)));
            scroll_handle.0.borrow().base_handle.set_offset(new_offset);
        } else {
            let vp_w = bounds.size.width.as_f32();
            let max_x = (content_width - vp_w).max(0.0);
            if vp_w <= 0.0 || max_x <= 0.0 { return; }
            let relative_x = mouse_pos.x.as_f32() - bounds.origin.x.as_f32();
            let ratio = (relative_x / vp_w).clamp(0.0, 1.0);
            horizontal_offset.set(ratio * max_x);
        }
    }

    fn start_edit_from_state(&mut self, cx: &App) {
        let state = self.state.read(cx);
        if let Some((r, c, ref v)) = state.editing_cell {
            let should_start = match &self.editing {
                Some((er, ec, _)) => *er != r || *ec != c,
                None => true,
            };
            if should_start {
                self.editing = Some((r, c, v.clone()));
            }
        }
    }

    fn move_selection(&mut self, dr: i32, dc: i32, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.commit_edit(cx);
        }
        self.state.update(cx, |s, _| {
            let (rows, cols) = (s.effective_row_count(), s.col_count());
            if rows == 0 || cols == 0 { return; }
            let (cr, cc) = s.selection_focus.map(|c| (c.row as i32, c.col as i32)).unwrap_or((0, 0));
            let nr = (cr + dr).clamp(0, rows as i32 - 1) as usize;
            let nc = (cc + dc).clamp(0, cols as i32 - 1) as usize;
            s.selection_type = Some(SelectionType::Cell);
            s.selection_anchor = Some(CellCoord { row: nr, col: nc });
            s.selection_focus = Some(CellCoord { row: nr, col: nc });
            s.selected_rows.clear(); s.selected_columns.clear();
        });
        cx.notify();
    }

    fn on_move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) { self.move_selection(-1, 0, cx); }
    fn on_move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) { self.move_selection(1, 0, cx); }
    fn on_move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) { self.move_selection(0, -1, cx); }
    fn on_move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) { self.move_selection(0, 1, cx); }

    fn extend_selection(&mut self, dr: i32, dc: i32, cx: &mut Context<Self>) {
        if self.editing.is_some() { self.commit_edit(cx); }
        self.state.update(cx, |s, _| {
            let (rows, cols) = (s.effective_row_count(), s.col_count());
            if rows == 0 || cols == 0 { return; }
            // Keep anchor, move focus
            if s.selection_anchor.is_none() {
                s.selection_anchor = s.selection_focus;
            }
            let (cr, cc) = s.selection_focus.map(|c| (c.row as i32, c.col as i32)).unwrap_or((0, 0));
            let nr = (cr + dr).clamp(0, rows as i32 - 1) as usize;
            let nc = (cc + dc).clamp(0, cols as i32 - 1) as usize;
            s.selection_type = Some(SelectionType::Cell);
            s.selection_focus = Some(CellCoord { row: nr, col: nc });
            s.selected_rows.clear(); s.selected_columns.clear();
        });
        cx.notify();
    }

    fn on_select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) { self.extend_selection(-1, 0, cx); }
    fn on_select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) { self.extend_selection(1, 0, cx); }
    fn on_select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) { self.extend_selection(0, -1, cx); }
    fn on_select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) { self.extend_selection(0, 1, cx); }

    fn on_enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.commit_edit(cx);
            // Move down after commit
            self.move_selection(1, 0, cx);
        } else {
            // Start editing the selected cell
            let state = self.state.read(cx);
            if let Some(focus) = state.selection_focus {
                let val = state.get_cached_row(focus.row)
                    .and_then(|r| r.get(focus.col)).cloned().unwrap_or_default();
                drop(state);
                self.editing = Some((focus.row, focus.col, val));
                cx.notify();
            }
        }
    }

    fn on_escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.cancel_edit(cx);
            return;
        }
        let has_menu = self.state.read(cx).context_menu.is_some();
        self.state.update(cx, |s, _| {
            if has_menu { s.context_menu = None; }
            else { s.clear_selection(); }
        });
        cx.notify();
    }

    fn on_copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let preload_range = {
            let state = self.state.read(cx);
            match state.selection_type {
                Some(SelectionType::Cell) => state.selection_range().map(|(mr, xr, _, _)| (mr, xr)),
                Some(SelectionType::Row) => {
                    if state.selected_rows.is_empty() {
                        None
                    } else {
                        let min_row = *state.selected_rows.iter().min().unwrap_or(&0);
                        let max_row = *state.selected_rows.iter().max().unwrap_or(&0);
                        Some((min_row, max_row))
                    }
                }
                Some(SelectionType::Column) => {
                    let rows = state.effective_row_count();
                    if rows == 0 { None } else { Some((0, rows - 1)) }
                }
                None => None,
            }
        };

        if let Some((start, end)) = preload_range {
            let all_cached = {
                let state = self.state.read(cx);
                (start..=end).all(|r| state.get_cached_row(r).is_some())
            };
            if !all_cached {
                self.ensure_rows_cached(start, end.saturating_sub(start) + 1, cx);
                self.state.update(cx, |s, _| {
                    s.toast_message = Some("Selection is still loading. Try copy again in a moment.".to_string());
                });
                cx.notify();
                return;
            }
        }

        let state = self.state.read(cx);
        let mut lines = Vec::new();
        match state.selection_type {
            Some(SelectionType::Cell) => {
                if let Some((mr, xr, mc, xc)) = state.selection_range() {
                    for r in mr..=xr {
                        let mut cells = Vec::new();
                        for c in mc..=xc { cells.push(state.get_cached_row(r).and_then(|row| row.get(c)).cloned().unwrap_or_default()); }
                        lines.push(cells.join("\t"));
                    }
                }
            }
            Some(SelectionType::Row) => {
                for &r in &state.selected_rows { if let Some(row) = state.get_cached_row(r) { lines.push(row.join("\t")); } }
            }
            Some(SelectionType::Column) => {
                for r in 0..state.effective_row_count() {
                    let mut cells = Vec::new();
                    for &c in &state.selected_columns { cells.push(state.get_cached_row(r).and_then(|row| row.get(c)).cloned().unwrap_or_default()); }
                    lines.push(cells.join("\t"));
                }
            }
            None => {}
        }
        if !lines.is_empty() { cx.write_to_clipboard(ClipboardItem::new_string(lines.join("\n"))); }
    }

    fn on_delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            // Backspace in edit mode
            if let Some((_, _, ref mut text)) = self.editing {
                text.pop();
            }
            cx.notify();
            return;
        }
        let range = self.state.read(cx).selection_range();
        if let Some((mr, xr, mc, xc)) = range {
            self.state.update(cx, |s, _| {
                if let Some(ref mut file) = s.file {
                    for r in mr..=xr { for c in mc..=xc {
                        file.edits.insert((r, c), String::new());
                        if let Some(row) = s.row_cache.get_mut(&r) { if c < row.len() { row[c] = String::new(); } }
                    }}
                    s.cache_version += 1;
                }
            });
            cx.notify();
        }
    }

    fn on_t_open_file(&mut self, _: &TOpenFile, _window: &mut Window, cx: &mut Context<Self>) {
        let path = rfd::FileDialog::new()
            .add_filter("CSV Files", &["csv", "tsv", "txt"])
            .add_filter("All Files", &["*"])
            .pick_file();

        if let Some(path) = path {
            let path_str = path.to_string_lossy().to_string();
            let filename = path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path_str.clone());
            let se = self.state.clone();

            let progress_atomic = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let progress_for_thread = progress_atomic.clone();
            let progress_for_poll = progress_atomic.clone();

            self.state.update(cx, |s, _| {
                s.is_loading = true;
                s.loading_progress = 0.0;
                s.loading_message = filename;
            });
            cx.notify();

            // Poll progress every 50ms until is_loading is cleared by the I/O spawn
            let se_poll = se.clone();
            cx.spawn(async move |_this, cx| {
                loop {
                    cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
                    let raw = progress_for_poll.load(std::sync::atomic::Ordering::Relaxed);
                    let progress = f32::from_bits(raw);
                    let still_loading = {
                        let mut loading = true;
                        let _ = se_poll.update(cx, |s, cx| {
                            if s.is_loading {
                                s.loading_progress = progress;
                                cx.notify();
                            } else {
                                loading = false;
                            }
                        });
                        loading
                    };
                    if !still_loading { break; }
                }
            }).detach();

            let path_clone = path_str.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let result = (|| {
                    let index_result = parser::index_file_with_progress(
                        &std::path::PathBuf::from(&path_clone),
                        move |done, total| {
                            if total > 0 {
                                let p = (done as f32 / total as f32).min(1.0);
                                progress_for_thread.store(p.to_bits(), std::sync::atomic::Ordering::Relaxed);
                            }
                        },
                    )?;
                    let first_chunk = parser::read_chunk_with_delim(
                        &std::path::PathBuf::from(&path_clone),
                        &index_result.row_offsets,
                        &std::collections::HashMap::new(),
                        0, 200,
                        index_result.columns.len(),
                        index_result.delimiter,
                    ).ok();
                    Ok::<_, String>((index_result, first_chunk))
                })();
                let _ = tx.send(result);
            });

            cx.spawn(async move |_this, cx| {
                let start = std::time::Instant::now();

                // Poll until the background thread sends back the result.
                let result = loop {
                    match rx.try_recv() {
                        Ok(r) => break r,
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(50))
                                .await;
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            break Err("I/O thread panicked".into());
                        }
                    }
                };

                // Ensure loading screen is visible for at least 400ms
                let elapsed = start.elapsed();
                let min_display = std::time::Duration::from_millis(400);
                if elapsed < min_display {
                    cx.background_executor().timer(min_display - elapsed).await;
                }

                match result {
                    Ok((index_result, first_chunk)) => {
                        let _ = se.update(cx, |state, cx| {
                            let file_path = std::path::PathBuf::from(&path_str);
                            let col_count = index_result.columns.len();
                            let metadata = csv_engine::types::CsvMetadata {
                                path: path_str.clone(),
                                columns: index_result.columns.clone(),
                                total_rows: index_result.total_rows,
                                file_size_bytes: index_result.file_size_bytes,
                                delimiter: index_result.delimiter,
                                has_headers: index_result.has_headers,
                            };

                            state.file = Some(crate::state::OpenFile {
                                original_columns: metadata.columns.clone(),
                                metadata,
                                row_offsets: index_result.row_offsets,
                                edits: std::collections::HashMap::new(),
                                file_path,
                                delimiter: index_result.delimiter,
                                row_order: None,
                                inserted_rows: Vec::new(),
                                col_order: None,
                                inserted_columns: Vec::new(),
                                inserted_col_values: std::collections::HashMap::new(),
                                original_col_count: col_count,
                                sort_permutation: None,
                                filter_indices: None,
                            });
                            state.unfiltered_row_count = index_result.total_rows;
                            state.sort_state = None;
                            state.has_filter = false;
                            state.is_loading = false;
                            state.loading_progress = 1.0;
                            state.loading_message.clear();
                            state.clear_cache();
                            state.clear_selection();

                            if let Some(chunk) = first_chunk {
                                for (i, row) in chunk.rows.into_iter().enumerate() {
                                    state.cache_row(chunk.start_index + i, row);
                                }
                                state.cache_version += 1;
                            }

                            cx.notify();
                        });
                    }
                    Err(e) => {
                        eprintln!("Failed to open file: {}", e);
                        let _ = se.update(cx, |s, cx| {
                            s.is_loading = false;
                            s.loading_progress = 0.0;
                            s.loading_message.clear();
                            cx.notify();
                        });
                    }
                }
            })
            .detach();
        }
    }

    fn on_t_save_file(&mut self, _: &TSaveFile, _window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        if !state.has_unsaved_changes() { return; }
        let file = match &state.file { Some(f) => f, None => return };
        let target_path = file.file_path.clone();
        let headers: Vec<String> = file.metadata.columns.iter().map(|c| c.name.clone()).collect();
        let save_result = csv_engine::writer::save_file(file, &target_path, &headers);
        drop(state);

        match save_result {
            Ok(()) => {
                // Re-open file to refresh state
                let path_str = target_path.to_string_lossy().to_string();
                let file_path = std::path::PathBuf::from(&path_str);
                let result = parser::index_file(&file_path);
                if let Ok(index_result) = result {
                    let se = self.state.clone();
                    se.update(cx, |state, _| {
                        let col_count = index_result.columns.len();
                        let metadata = csv_engine::types::CsvMetadata {
                            path: path_str.clone(),
                            columns: index_result.columns.clone(),
                            total_rows: index_result.total_rows,
                            file_size_bytes: index_result.file_size_bytes,
                            delimiter: index_result.delimiter,
                            has_headers: index_result.has_headers,
                        };
                        let first_chunk = parser::read_chunk_with_delim(
                            &file_path, &index_result.row_offsets, &std::collections::HashMap::new(),
                            0, 200, col_count, index_result.delimiter,
                        );
                        state.file = Some(crate::state::OpenFile {
                            original_columns: metadata.columns.clone(),
                            metadata,
                            row_offsets: index_result.row_offsets,
                            edits: std::collections::HashMap::new(),
                            file_path,
                            delimiter: index_result.delimiter,
                            row_order: None,
                            inserted_rows: Vec::new(),
                            col_order: None,
                            inserted_columns: Vec::new(),
                            inserted_col_values: std::collections::HashMap::new(),
                            original_col_count: col_count,
                            sort_permutation: None,
                            filter_indices: None,
                        });
                        state.unfiltered_row_count = index_result.total_rows;
                        state.sort_state = None;
                        state.has_filter = false;
                        state.clear_cache();
                        state.clear_selection();
                        if let Ok(chunk) = first_chunk {
                            for (i, row) in chunk.rows.into_iter().enumerate() {
                                state.cache_row(chunk.start_index + i, row);
                            }
                            state.cache_version += 1;
                        }
                    });
                }
                cx.notify();
            }
            Err(e) => eprintln!("Save failed: {}", e),
        }
    }

    fn on_t_cycle_theme(&mut self, _: &TCycleTheme, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, _| s.cycle_theme());
        cx.notify();
    }

    fn on_t_quit(&mut self, _: &TQuit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    fn handle_key_input(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_none() {
            // If not editing and a printable key is pressed, start editing
            if let Some(ref ch) = event.keystroke.key_char {
                if !event.keystroke.modifiers.platform && !event.keystroke.modifiers.control {
                    let state = self.state.read(cx);
                    if let Some(focus) = state.selection_focus {
                        drop(state);
                        // Start editing with this character (replace mode)
                        self.editing = Some((focus.row, focus.col, ch.clone()));
                        cx.notify();
                    }
                }
            }
            return;
        }

        // We're in edit mode — handle input
        if let Some(ref ch) = event.keystroke.key_char {
            if !event.keystroke.modifiers.platform && !event.keystroke.modifiers.control {
                if let Some((_, _, ref mut text)) = self.editing {
                    text.push_str(ch);
                }
                cx.notify();
            }
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let colors = state.current_theme();
        let file = state.file.as_ref().unwrap();
        let columns = file.metadata.columns.clone();
        let total_w: f32 = ROW_NUMBER_WIDTH + columns.iter().map(|c| state.column_width(c.index)).sum::<f32>();

        // Horizontal offset (positive = scrolled right)
        let h_off = self.horizontal_offset.get();
        let col_resize = self.column_resize.clone();
        let col_resize_start = self.column_resize_start.clone();

        // Inner row: absolutely positioned so negative left doesn't affect parent layout
        let mut inner = div().flex().flex_shrink_0().h_full()
            .absolute().top_0().left(px(-h_off))
            .w(px(total_w));
        inner = inner.child(div().flex_shrink_0().w(px(ROW_NUMBER_WIDTH)).h_full().border_r_1().border_color(colors.border));

        for col in columns.iter() {
            let w = state.column_width(col.index);
            let ci = col.index;
            let is_sorted = state.sort_state.as_ref().map_or(false, |s| s.column_index == ci);
            let is_sel = state.selected_columns.contains(&ci);
            let arrow = if is_sorted { if state.sort_state.as_ref().unwrap().direction == SortDirection::Asc { " \u{2191}" } else { " \u{2193}" } } else { "" };
            let name = format!("{}{}", col.name, arrow);
            let tc = if is_sel { colors.accent } else { colors.text_secondary };
            let bg = if is_sel { colors.accent_subtle } else { colors.surface };
            let resize_bar_hover = colors.accent;
            let se = self.state.clone();
            let cr = col_resize.clone();
            let cr_click = col_resize.clone();
            let crs = col_resize_start.clone();
            // Resize handle: wider hit area, visible only on hover.
            // Resize handle owns the column right border; hover changes its color.
            let border_col = colors.border;
            let resize_handle = div()
                .id(ElementId::NamedInteger("rh".into(), ci as u64))
                .absolute().right(px(0.)).top_0().h_full().w(px(8.0))
                .border_r_1().border_color(border_col)
                .cursor_col_resize()
                .hover(move |s| s.border_r_3().border_color(resize_bar_hover))
                .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, _, _| {
                    cr.set(Some(ci));
                    crs.set(Some((ev.position.x.as_f32(), w)));
                });
            let hdr_cell = div().id(ElementId::NamedInteger("h".into(), ci as u64))
                    .relative()  // needed for absolute resize handle
                    .flex_shrink_0().w(px(w)).h_full().flex().items_center().pl(px(8.0))
                    .bg(bg).text_color(tc)
                    .cursor_pointer().truncate().child(name);
            inner = inner.child(
                hdr_cell
                    // on_mouse_down fires after child resize_handle.on_mouse_down (bubble order);
                    // if column_resize is already set the click started on the resize handle.
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        if cr_click.get().is_some() { return; }
                        se.update(cx, |s, _| {
                            s.selected_columns.clear(); s.selected_columns.push(ci);
                            s.selection_type = Some(SelectionType::Column);
                            s.selection_anchor = None; s.selection_focus = None; s.selected_rows.clear();
                        });
                    })
                    .child(resize_handle),
            );
        }

        // Outer container: relative for absolute child, clips overflow
        div().id("hdr").flex_shrink_0().h(px(HEADER_HEIGHT))
            .bg(colors.surface).border_b_1().border_color(colors.border)
            .text_size(px(11.0)).text_color(colors.text_secondary)
            .relative().overflow_hidden()
            .child(inner)
    }

    fn render_row_el(&self, ri: usize, cx: &App) -> Stateful<Div> {
        let state = self.state.read(cx);
        let colors = state.current_theme();
        let file = state.file.as_ref().unwrap();
        let columns = &file.metadata.columns;
        let is_row_sel = state.selected_rows.contains(&ri);
        let row_bg = if is_row_sel { colors.accent_subtle } else { colors.surface };
        let rn_color = if is_row_sel { colors.accent } else { colors.text_tertiary };
        let cached = state.get_cached_row(ri);
        let total_w: f32 = ROW_NUMBER_WIDTH + columns.iter().map(|c| state.column_width(c.index)).sum::<f32>();

        // Horizontal offset (positive = scrolled right)
        let h_off = self.horizontal_offset.get();

        let hover_bg = colors.hover_row;
        // Outer row: relative positioning context, clips overflow, fixed height
        let row_outer = div().id(ElementId::NamedInteger("r".into(), ri as u64))
            .h(px(ROW_HEIGHT)).w_full().relative().overflow_hidden()
            .bg(row_bg)
            .border_b_1().border_color(colors.border)
            .text_size(px(13.0)).text_color(colors.text_primary)
            .hover(move |s| s.bg(hover_bg));

        // Inner content: absolutely positioned so negative left doesn't affect parent layout
        let mut inner = div().flex().flex_shrink_0().h_full()
            .absolute().top_0().left(px(-h_off))
            .w(px(total_w));

        inner = inner.child(
            div().flex_shrink_0().w(px(ROW_NUMBER_WIDTH)).h_full()
                .flex().items_center().justify_end().pr(px(8.0))
                .border_r_1().border_color(colors.border)
                .text_size(px(10.0)).text_color(rn_color)
                .child(format!("{}", ri + 1)),
        );

        // If data not yet cached, render placeholder cells
        if cached.is_none() {
            for col in columns.iter() {
                let w = state.column_width(col.index);
                let is_sel = state.is_cell_selected(ri, col.index);
                let cell_bg = if is_sel { colors.accent_subtle } else { row_bg };
                inner = inner.child(
                    div().flex_shrink_0().w(px(w)).h_full()
                        .bg(cell_bg)
                        .flex().items_center().pl(px(8.0))
                        .text_color(colors.text_tertiary)
                        .child("\u{2026}")
                );
            }
            return row_outer.child(inner);
        }

        // Get selection range for border drawing
        let sel_range = state.selection_range();

        for col in columns.iter() {
            let w = state.column_width(col.index);
            let ci = col.index;

            let is_editing = self.editing.as_ref().map_or(false, |(r, c, _)| *r == ri && *c == ci);
            let val = if is_editing {
                self.editing.as_ref().map(|(_, _, t)| t.clone()).unwrap_or_default()
            } else {
                cached.and_then(|r| r.get(ci)).cloned().unwrap_or_default()
            };

            let is_edited = file.edits.contains_key(&(ri, ci));
            let is_sel = state.is_cell_selected(ri, ci);

            let cell_bg = if is_editing {
                colors.surface
            } else if is_sel {
                colors.accent_subtle
            } else if is_edited {
                colors.edited
            } else {
                row_bg
            };

            let display = if is_editing {
                format!("{}|", val)
            } else {
                val
            };

            let mut cell = div().flex_shrink_0().w(px(w)).h_full()
                .flex().items_center().pl(px(8.0))
                .bg(cell_bg)
                .truncate().child(display);

            if is_editing {
                cell = cell.border_1().border_color(colors.accent);
            } else if is_sel {
                if let Some((mr, xr, mc, xc)) = sel_range {
                    let sel_border = colors.accent;
                    let is_top = ri == mr;
                    let is_bottom = ri == xr;
                    let is_left = ci == mc;
                    let is_right = ci == xc;

                    if is_top || is_bottom || is_left || is_right {
                        let mut border_cell = div().size_full().absolute().top_0().left_0();
                        if is_top { border_cell = border_cell.border_t_1(); }
                        if is_bottom { border_cell = border_cell.border_b_1(); }
                        if is_left { border_cell = border_cell.border_l_1(); }
                        if is_right { border_cell = border_cell.border_r_1(); }
                        border_cell = border_cell.border_color(sel_border).border_dashed();
                        cell = cell.relative().child(border_cell);
                    }
                }
            }

            inner = inner.child(cell);
        }
        row_outer.child(inner)
    }
}

impl Focusable for TableView {
    fn focus_handle(&self, _: &App) -> FocusHandle { self.focus_handle.clone() }
}

impl Render for TableView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Auto-focus once on first render
        if self.needs_focus {
            self.needs_focus = false;
            cx.focus_self(window);
        }

        let state = self.state.read(cx);
        let colors = state.current_theme();

        if state.is_loading {
            let filename = state.loading_message.clone();
            let bg = colors.bg;
            let text_color = colors.text_secondary;
            let accent = colors.accent;

            return div()
                .id("loading-screen")
                .size_full().flex().items_center().justify_center()
                .bg(bg)
                .key_context("TableView")
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(Self::handle_key_input))
                .flex_col()
                .gap(px(16.0))
                .child(
                    svg()
                        .path("assets/spinner.svg")
                        .w(px(40.0)).h(px(40.0))
                        .text_color(accent)
                        .with_animation(
                            "spinner-rotate",
                            Animation::new(Duration::from_millis(800)).repeat(),
                            |svg, delta| {
                                svg.with_transformation(Transformation::rotate(percentage(delta)))
                            },
                        )
                )
                .child(
                    div()
                        .id("loading-filename")
                        .text_size(px(14.0))
                        .text_color(text_color)
                        .child(if filename.is_empty() { "Loading…".to_string() } else { filename })
                )
                .into_any_element();
        }

        if state.file.is_none() {
            let colors = self.state.read(cx).current_theme();
            return div().id("empty-state").size_full().flex().items_center().justify_center()
                .bg(colors.bg).text_size(px(16.0))
                .key_context("TableView")
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(Self::handle_key_input))
                .on_action(cx.listener(Self::on_t_open_file))
                .on_action(cx.listener(Self::on_t_quit))
                .on_action(cx.listener(Self::on_t_cycle_theme))
                .flex_col()
                .gap(px(16.0))
                .child(
                    div()
                        .id("open-file-btn")
                        .bg(colors.accent)
                        .text_color(colors.accent_text)
                        .rounded(px(6.0))
                        .px(px(20.0))
                        .py(px(10.0))
                        .cursor_pointer()
                        .child("Open File")
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                            this.on_t_open_file(&TOpenFile, window, cx);
                        }))
                )
                .child(div().text_color(colors.text_tertiary).text_size(px(14.0))
                    .child("or press Cmd+O"))
                .into_any_element();
        }

        let total_rows = state.effective_row_count();
        drop(state);

        // Pick up pending edit from double-click
        self.start_edit_from_state(cx);

        // Trigger async stats computation if needed
        self.maybe_compute_stats(cx);

        let header = self.render_header(cx);
        let colors = self.state.read(cx).current_theme();

        let row_list = uniform_list(
            "rows", total_rows,
            cx.processor(|this: &mut Self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>| {
                this.ensure_rows_cached(range.start, range.end - range.start, cx);
                let mut items = Vec::new();
                for ri in range {
                    let se = this.state.clone();
                    let h_off_rc = this.horizontal_offset.clone();
                    let sh_for_rows = this.scroll_handle.clone();
                    let sb_drag_for_rows = this.scrollbar_drag.clone();
                    items.push(
                        this.render_row_el(ri, cx)
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, {
                                let se = se.clone();
                                let h_off_rc = h_off_rc.clone();
                                let sh_for_rows = sh_for_rows.clone();
                                let sb_drag_for_rows = sb_drag_for_rows.clone();
                                move |ev, _, cx| {
                                    if sb_drag_for_rows.get().is_some() { return; }
                                    let st = se.read(cx);
                                    if Self::is_in_scrollbar_hit_region(&sh_for_rows, &st, ev.position) {
                                        return;
                                    }
                                    // Adjust screen x by horizontal scroll offset to get content-space x
                                    let x = ev.position.x.as_f32() + h_off_rc.get();
                                    if x < ROW_NUMBER_WIDTH {
                                        drop(st);
                                        se.update(cx, |s, _| {
                                            if ev.modifiers.platform {
                                                if s.selected_rows.contains(&ri) { s.selected_rows.retain(|&r| r != ri); }
                                                else { s.selected_rows.push(ri); }
                                            } else { s.selected_rows.clear(); s.selected_rows.push(ri); }
                                            s.selection_type = Some(SelectionType::Row);
                                            s.selection_anchor = None; s.selection_focus = None; s.selected_columns.clear();
                                            s.context_menu = None;
                                        });
                                        return;
                                    }
                                    let cc = Self::hit_test_col_from_content_x(&st, x);
                                    drop(st);
                                    se.update(cx, |s, _| {
                                        s.selection_type = Some(SelectionType::Cell);
                                        if ev.modifiers.shift {
                                            s.selection_focus = Some(CellCoord { row: ri, col: cc });
                                        } else {
                                            s.selection_anchor = Some(CellCoord { row: ri, col: cc });
                                            s.selection_focus = Some(CellCoord { row: ri, col: cc });
                                            s.is_dragging = true;
                                        }
                                        s.selected_rows.clear(); s.selected_columns.clear();
                                        s.context_menu = None; s.editing_cell = None;
                                    });
                                }
                            })
                            .on_mouse_down(MouseButton::Right, {
                                let se = se.clone();
                                let h_off_rc = h_off_rc.clone();
                                let sh_for_rows = sh_for_rows.clone();
                                let sb_drag_for_rows = sb_drag_for_rows.clone();
                                move |ev, _, cx| {
                                    if sb_drag_for_rows.get().is_some() { return; }
                                    let st = se.read(cx);
                                    if Self::is_in_scrollbar_hit_region(&sh_for_rows, &st, ev.position) {
                                        return;
                                    }
                                    // Adjust screen x by horizontal scroll offset to get content-space x
                                    let x = ev.position.x.as_f32() + h_off_rc.get();
                                    let y = ev.position.y.as_f32();
                                    let cc = Self::hit_test_col_from_content_x(&st, x);
                                    let already = st.is_cell_selected(ri, cc);
                                    drop(st);
                                    se.update(cx, |s, _| {
                                        if !already {
                                            s.selection_type = Some(SelectionType::Cell);
                                            s.selection_anchor = Some(CellCoord { row: ri, col: cc });
                                            s.selection_focus = Some(CellCoord { row: ri, col: cc });
                                            s.selected_rows.clear(); s.selected_columns.clear();
                                        }
                                        s.context_menu = Some((x, y, ri, cc));
                                    });
                                }
                            })
                            .on_click({
                                let se = se.clone();
                                move |event, _, cx| {
                                    if event.click_count() >= 2 {
                                        let st = se.read(cx);
                                        let (r, c) = st.selection_focus.map(|f| (f.row, f.col)).unwrap_or((ri, 0));
                                        let val = st.get_cached_row(r).and_then(|row| row.get(c)).cloned().unwrap_or_default();
                                        drop(st);
                                        se.update(cx, |s, _| { s.editing_cell = Some((r, c, val)); });
                                    }
                                }
                            })
                    );
                }
                items
            }),
        )
        .size_full()
        .flex_grow()
        .track_scroll(&self.scroll_handle);

        // --- Scrollbar computation ---
        const SCROLLBAR_SIZE: f32 = 8.0;
        const SCROLLBAR_MIN_THUMB: f32 = 24.0;
        const SCROLLBAR_MARGIN: f32 = 2.0;

        let sh = self.scroll_handle.0.borrow();
        let s_off = sh.base_handle.offset();
        let s_max = sh.base_handle.max_offset();
        let s_bounds = sh.base_handle.bounds();
        drop(sh);

        let vp_h = s_bounds.size.height.as_f32();
        let vp_w = s_bounds.size.width.as_f32();
        let off_y = s_off.y.as_f32();   // negative when scrolled down
        let max_y = s_max.y.as_f32();   // positive max scrollable distance

        // Horizontal scroll: computed from our manual state and column widths
        let state_ref = self.state.read(cx);
        let content_w: f32 = if let Some(file) = &state_ref.file {
            ROW_NUMBER_WIDTH + file.metadata.columns.iter().map(|c| state_ref.column_width(c.index)).sum::<f32>()
        } else { 0.0 };
        drop(state_ref);
        let max_x = (content_w - vp_w).max(0.0);
        let h_off = self.horizontal_offset.get().clamp(0.0, max_x);
        // Sync clamped value back
        self.horizontal_offset.set(h_off);

        let thumb_color = hsla(0., 0., 0.5, 0.45);

        // After the first layout, the scroll handle has real bounds — trigger re-render
        // so scrollbars appear without requiring user interaction first.
        if !self.scrollbar_initialized && (vp_h == 0.0 || vp_w == 0.0) {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(100)).await;
                let _ = this.update(cx, |this, cx| {
                    this.scrollbar_initialized = true;
                    cx.notify();
                });
            }).detach();
        }

        // Determine which scrollbars are present for corner gap calculation
        let has_v_bar = max_y > 0.0 && vp_h > 0.0;
        let has_h_bar = max_x > 0.0 && vp_w > 0.0;
        let corner_gap = SCROLLBAR_SIZE + SCROLLBAR_MARGIN * 2.0;

        // Vertical scrollbar
        let v_bar: Option<Stateful<Div>> = if has_v_bar {
            let v_bar_h = if has_h_bar { vp_h - corner_gap } else { vp_h };
            let content_h = vp_h + max_y;
            let thumb_h = (vp_h / content_h * v_bar_h).max(SCROLLBAR_MIN_THUMB);
            let track_h = v_bar_h - thumb_h;
            let scroll_pos = -off_y;
            let thumb_top = if max_y > 0.0 { scroll_pos / max_y * track_h } else { 0.0 };
            let drag = self.scrollbar_drag.clone();
            let sh = self.scroll_handle.clone();
            let h_off_rc = self.horizontal_offset.clone();
            Some(
                div().id("v-scrollbar").absolute().top(px(0.)).right(px(SCROLLBAR_MARGIN))
                    .w(px(SCROLLBAR_SIZE)).h(px(v_bar_h))
                    .cursor(CursorStyle::Arrow)
                    .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, _, _| {
                        drag.set(Some(true));
                        Self::apply_scrollbar_drag(&drag, &sh, &h_off_rc, ev.position, content_w);
                    })
                    .child(
                        div().absolute().top(px(thumb_top))
                            .w(px(SCROLLBAR_SIZE)).h(px(thumb_h))
                            .rounded(px(SCROLLBAR_SIZE / 2.0))
                            .bg(thumb_color)
                    )
            )
        } else { None };

        // Horizontal scrollbar
        let h_bar: Option<Stateful<Div>> = if has_h_bar {
            let h_bar_w = if has_v_bar { vp_w - corner_gap } else { vp_w };
            let thumb_w = (vp_w / content_w * h_bar_w).max(SCROLLBAR_MIN_THUMB);
            let track_w = h_bar_w - thumb_w;
            let thumb_left = if max_x > 0.0 { h_off / max_x * track_w } else { 0.0 };
            let drag = self.scrollbar_drag.clone();
            let sh = self.scroll_handle.clone();
            let h_off_rc = self.horizontal_offset.clone();
            Some(
                div().id("h-scrollbar").absolute().bottom(px(SCROLLBAR_MARGIN)).left(px(0.))
                    .h(px(SCROLLBAR_SIZE)).w(px(h_bar_w))
                    .cursor(CursorStyle::Arrow)
                    .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, _, _| {
                        drag.set(Some(false));
                        Self::apply_scrollbar_drag(&drag, &sh, &h_off_rc, ev.position, content_w);
                    })
                    .child(
                        div().absolute().left(px(thumb_left))
                            .h(px(SCROLLBAR_SIZE)).w(px(thumb_w))
                            .rounded(px(SCROLLBAR_SIZE / 2.0))
                            .bg(thumb_color)
                    )
            )
        } else { None };

        // Global window-level mouse listeners for scrollbar drag (via canvas paint callback).
        // These fire regardless of cursor position — essential for native-feel scrollbar dragging.
        let drag_for_canvas = self.scrollbar_drag.clone();
        let sh_for_canvas = self.scroll_handle.clone();
        let h_off_for_canvas = self.horizontal_offset.clone();
        let cw_for_canvas = content_w;
        let col_resize_for_canvas = self.column_resize.clone();
        let col_resize_start_for_canvas = self.column_resize_start.clone();
        let state_for_resize = self.state.clone();
        let scrollbar_canvas = canvas(|_, _, _| {}, move |_, _, window, _| {
            // Capture scrollbar mousedown before row handlers in bubble phase.
            let drag_down = drag_for_canvas.clone();
            let sh_down = sh_for_canvas.clone();
            let h_off_down = h_off_for_canvas.clone();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, _| {
                if phase != DispatchPhase::Capture { return; }
                if event.button != MouseButton::Left { return; }

                let sh = sh_down.0.borrow();
                let bounds = sh.base_handle.bounds();
                let max_off = sh.base_handle.max_offset();
                drop(sh);

                let vp_w = bounds.size.width.as_f32();
                let vp_h = bounds.size.height.as_f32();
                if vp_w <= 0.0 || vp_h <= 0.0 { return; }

                let max_x = (cw_for_canvas - vp_w).max(0.0);
                let max_y = max_off.y.as_f32();

                let has_v_bar = max_y > 0.0;
                let has_h_bar = max_x > 0.0;
                if !has_v_bar && !has_h_bar { return; }

                const SCROLLBAR_SIZE: f32 = 8.0;
                const SCROLLBAR_MARGIN: f32 = 2.0;
                let corner_gap = SCROLLBAR_SIZE + SCROLLBAR_MARGIN * 2.0;

                let ox = bounds.origin.x.as_f32();
                let oy = bounds.origin.y.as_f32();
                let mx = event.position.x.as_f32();
                let my = event.position.y.as_f32();

                let in_v_bar = if has_v_bar {
                    let track_top = oy;
                    let track_bottom = oy + if has_h_bar { vp_h - corner_gap } else { vp_h };
                    let bar_left = ox + vp_w - SCROLLBAR_MARGIN - SCROLLBAR_SIZE;
                    let bar_right = ox + vp_w - SCROLLBAR_MARGIN;
                    mx >= bar_left && mx <= bar_right && my >= track_top && my <= track_bottom
                } else { false };

                let in_h_bar = if has_h_bar {
                    let track_left = ox;
                    let track_right = ox + if has_v_bar { vp_w - corner_gap } else { vp_w };
                    let bar_top = oy + vp_h - SCROLLBAR_MARGIN - SCROLLBAR_SIZE;
                    let bar_bottom = oy + vp_h - SCROLLBAR_MARGIN;
                    mx >= track_left && mx <= track_right && my >= bar_top && my <= bar_bottom
                } else { false };

                if in_v_bar {
                    drag_down.set(Some(true));
                    Self::apply_scrollbar_drag(&drag_down, &sh_down, &h_off_down, event.position, cw_for_canvas);
                    window.refresh();
                } else if in_h_bar {
                    drag_down.set(Some(false));
                    Self::apply_scrollbar_drag(&drag_down, &sh_down, &h_off_down, event.position, cw_for_canvas);
                    window.refresh();
                }
            });

            // --- Column resize drag ---
            let cr_move = col_resize_for_canvas.clone();
            let crs_move = col_resize_start_for_canvas.clone();
            let state_move = state_for_resize.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture { return; }
                let col_idx = match cr_move.get() { Some(c) => c, None => return };
                if !event.dragging() { return; }
                let (start_x, start_w) = match crs_move.get() { Some(s) => s, None => return };
                let delta = event.position.x.as_f32() - start_x;
                // Snap to whole pixels to avoid subpixel seams in column selection fills.
                let new_w = (start_w + delta).max(30.0).round();
                state_move.update(cx, |s, _| {
                    s.column_widths.insert(col_idx, new_w);
                });
                window.refresh();
            });

            let cr_up = col_resize_for_canvas.clone();
            let crs_up = col_resize_start_for_canvas.clone();
            window.on_mouse_event(move |_event: &MouseUpEvent, phase, window, _| {
                if phase != DispatchPhase::Capture { return; }
                if cr_up.get().is_none() { return; }
                cr_up.set(None);
                crs_up.set(None);
                window.refresh();
            });

            // --- Scrollbar drag ---
            // MouseMoveEvent: track drag position globally
            let drag_move = drag_for_canvas.clone();
            let sh_move = sh_for_canvas.clone();
            let h_off_move = h_off_for_canvas.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _| {
                if phase != DispatchPhase::Capture { return; }
                if drag_move.get().is_none() { return; }
                if !event.dragging() { return; }
                Self::apply_scrollbar_drag(&drag_move, &sh_move, &h_off_move, event.position, cw_for_canvas);
                window.refresh();
            });

            // MouseUpEvent: release drag globally
            let drag_up = drag_for_canvas.clone();
            window.on_mouse_event(move |_event: &MouseUpEvent, phase, window, _| {
                if phase != DispatchPhase::Capture { return; }
                if drag_up.get().is_none() { return; }
                drag_up.set(None);
                window.refresh();
            });

            // Ensure selection drag always ends even if mouse-up occurs outside table hitbox.
            let state_sel_up = state_for_resize.clone();
            window.on_mouse_event(move |_event: &MouseUpEvent, phase, _window, cx| {
                if phase != DispatchPhase::Capture { return; }
                let _ = state_sel_up.update(cx, |s, cx| {
                    if s.is_dragging {
                        s.is_dragging = false;
                        cx.notify();
                    }
                });
            });

            // ScrollWheelEvent: capture horizontal scroll wheel and update horizontal_offset.
            // Use Capture phase so we get the event before uniform_list's scroll handler
            // consumes it in Bubble phase.
            let h_off_scroll = h_off_for_canvas.clone();
            let max_x_for_scroll = max_x;
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, _| {
                if phase != DispatchPhase::Capture { return; }
                let delta_x = match event.delta {
                    ScrollDelta::Pixels(pt) => pt.x.as_f32(),
                    ScrollDelta::Lines(pt) => pt.x * 20.0,
                };
                if delta_x == 0.0 { return; }
                let cur = h_off_scroll.get();
                let new_val = (cur - delta_x).clamp(0.0, max_x_for_scroll);
                if new_val != cur {
                    h_off_scroll.set(new_val);
                    window.refresh();
                }
            });
        }).w(px(0.)).h(px(0.)).absolute();

        div()
            .size_full().flex().flex_col().bg(colors.bg)
            .key_context("TableView")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_move_up))
            .on_action(cx.listener(Self::on_move_down))
            .on_action(cx.listener(Self::on_move_left))
            .on_action(cx.listener(Self::on_move_right))
            .on_action(cx.listener(Self::on_escape))
            .on_action(cx.listener(Self::on_copy))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_enter))
            .on_action(cx.listener(Self::on_select_up))
            .on_action(cx.listener(Self::on_select_down))
            .on_action(cx.listener(Self::on_select_left))
            .on_action(cx.listener(Self::on_select_right))
            .on_action(cx.listener(Self::on_t_open_file))
            .on_action(cx.listener(Self::on_t_save_file))
            .on_action(cx.listener(Self::on_t_cycle_theme))
            .on_action(cx.listener(Self::on_t_quit))
            .on_key_down(cx.listener(Self::handle_key_input))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                // Cell selection drag (scrollbar drag is handled by global canvas listeners)
                if this.scrollbar_drag.get().is_some() { return; }
                let st = this.state.read(cx);
                if !st.is_dragging { return; }
                // Only drag-select while mouse button is actively held.
                // This prevents scroll gestures from moving selection under the cursor.
                if !ev.dragging() {
                    drop(st);
                    this.state.update(cx, |s, _| {
                        s.is_dragging = false;
                    });
                    return;
                }
                // Adjust screen x by horizontal scroll offset to get content-space x
                let x = ev.position.x.as_f32() + this.horizontal_offset.get();
                let total = st.effective_row_count();
                let ri = match this.hit_test_row_from_window_y(ev.position.y.as_f32(), total) {
                    Some(r) => r,
                    None => return,
                };
                let cc = Self::hit_test_col_from_content_x(&st, x);
                let cur_focus = st.selection_focus;
                drop(st);
                let new_focus = CellCoord { row: ri, col: cc };
                if cur_focus != Some(new_focus) {
                    this.state.update(cx, |s, _| {
                        s.selection_focus = Some(new_focus);
                        s.selection_type = Some(SelectionType::Cell);
                    });
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                // Cell selection drag release (scrollbar mouse-up is handled globally)
                this.state.update(cx, |s, _| { s.is_dragging = false; });
            }))
            // Header row (static, clipped, synced with horizontal scroll)
            .child(header)
            // Body: uniform_list + scrollbar overlays + global drag canvas
            .child(
                div()
                    .flex_grow()
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    .child(row_list)
                    .child(scrollbar_canvas)
                    .children(v_bar)
                    .children(h_bar)
            )
            .into_any_element()
    }
}
