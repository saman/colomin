use gpui::*;
use std::ops::Range;

use crate::csv_engine::{self, parser};
use crate::state::{AppState, CellCoord, SelectionType, SortDirection, SortState};

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
    /// Currently editing: (row, col, text_buffer)
    editing: Option<(usize, usize, String)>,
    needs_focus: bool,
}

impl TableView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self { state, focus_handle: cx.focus_handle(), editing: None, needs_focus: true }
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
            let text = text.clone();
            self.state.update(cx, |s, _| {
                if let Some(ref mut file) = s.file {
                    file.edits.insert((row, col), text.clone());
                }
                if let Some(cached) = s.row_cache.get_mut(&row) {
                    if col < cached.len() { cached[col] = text; }
                }
                s.cache_version += 1;
                s.editing_cell = None;
            });
        }
        self.editing = None;
        cx.notify();
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        self.editing = None;
        self.state.update(cx, |s, _| { s.editing_cell = None; });
        cx.notify();
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
            let se = self.state.clone();

            self.state.update(cx, |s, _| {
                s.is_loading = true;
                s.loading_message = format!("Opening {}...",
                    std::path::Path::new(&path_str).file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path_str.clone()));
            });
            cx.notify();

            let path_clone = path_str.clone();
            cx.spawn(async move |_this, cx| {
                let result = std::thread::spawn(move || {
                    let index_result = parser::index_file(&std::path::PathBuf::from(&path_clone))?;
                    let first_chunk = parser::read_chunk_with_delim(
                        &std::path::PathBuf::from(&path_clone),
                        &index_result.row_offsets,
                        &std::collections::HashMap::new(),
                        0, 200,
                        index_result.columns.len(),
                        index_result.delimiter,
                    ).ok();
                    Ok::<_, String>((index_result, first_chunk))
                })
                .join()
                .unwrap_or_else(|_| Err("Thread panicked".into()));

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

    fn handle_key_input(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Handle cmd+key shortcuts directly
        if event.keystroke.modifiers.platform {
            match event.keystroke.key.as_str() {
                "o" => { self.on_t_open_file(&TOpenFile, window, cx); return; }
                "s" => { self.on_t_save_file(&TSaveFile, window, cx); return; }
                "t" => { self.on_t_cycle_theme(&TCycleTheme, window, cx); return; }
                "q" => { self.on_t_quit(&TQuit, window, cx); return; }
                "c" => { self.on_copy(&Copy, window, cx); return; }
                _ => {}
            }
        }

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

        let mut hdr = div().id("hdr").flex().flex_shrink_0().h(px(HEADER_HEIGHT))
            .bg(colors.surface).border_b_1().border_color(colors.border)
            .text_size(px(11.0)).text_color(colors.text_secondary);
        hdr = hdr.child(div().flex_shrink_0().w(px(ROW_NUMBER_WIDTH)).h_full().border_r_1().border_color(colors.border));

        for col in columns.iter() {
            let w = state.column_width(col.index);
            let ci = col.index;
            let is_sorted = state.sort_state.as_ref().map_or(false, |s| s.column_index == ci);
            let is_sel = state.selected_columns.contains(&ci);
            let arrow = if is_sorted { if state.sort_state.as_ref().unwrap().direction == SortDirection::Asc { " \u{2191}" } else { " \u{2193}" } } else { "" };
            let name = format!("{}{}", col.name, arrow);
            let tc = if is_sel { colors.accent } else { colors.text_secondary };
            let bg = if is_sel { colors.accent_subtle } else { colors.surface };
            let se = self.state.clone();
            hdr = hdr.child(
                div().id(ElementId::NamedInteger("h".into(), ci as u64))
                    .flex_shrink_0().w(px(w)).h_full().flex().items_center().pl(px(8.0))
                    .bg(bg).text_color(tc)
                    .cursor_pointer().truncate().child(name)
                    .on_click(move |_, _, cx| {
                        se.update(cx, |s, _| {
                            s.selected_columns.clear(); s.selected_columns.push(ci);
                            s.selection_type = Some(SelectionType::Column);
                            s.selection_anchor = None; s.selection_focus = None; s.selected_rows.clear();
                        });
                    }),
            );
        }
        hdr
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

        let hover_bg = colors.hover_row;
        let mut row = div().id(ElementId::NamedInteger("r".into(), ri as u64))
            .flex().flex_shrink_0().h(px(ROW_HEIGHT)).bg(row_bg)
            .border_b_1().border_color(colors.border)
            .text_size(px(13.0)).text_color(colors.text_primary)
            .hover(move |s| s.bg(hover_bg));

        row = row.child(
            div().flex_shrink_0().w(px(ROW_NUMBER_WIDTH)).h_full()
                .flex().items_center().justify_end().pr(px(8.0))
                .border_r_1().border_color(colors.border)
                .text_size(px(10.0)).text_color(rn_color)
                .child(format!("{}", ri + 1)),
        );

        // Get selection range for border drawing
        let sel_range = state.selection_range(); // (min_row, max_row, min_col, max_col)

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
                    // Apply dashed border on selection edges
                    // We need separate wrapper divs per edge because border_dashed applies to all borders
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
                        // Wrap cell content with relative positioning
                        cell = cell.relative().child(border_cell);
                    }
                }
            }

            row = row.child(cell);
        }
        row
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
            return div().size_full().flex().items_center().justify_center()
                .bg(colors.bg).text_size(px(14.0)).text_color(colors.text_secondary)
                .key_context("TableView")
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(Self::handle_key_input))
                .flex_col().gap(px(8.0))
                .child(state.loading_message.clone())
                .child(
                    div().text_size(px(12.0)).text_color(colors.text_tertiary)
                        .child("Indexing file\u{2026}")
                )
                .into_any_element();
        }

        if state.file.is_none() {
            return div().id("empty-state").size_full().flex().items_center().justify_center()
                .bg(colors.bg).text_size(px(16.0)).text_color(colors.text_tertiary)
                .key_context("TableView")
                .track_focus(&self.focus_handle)
                .on_key_down(cx.listener(Self::handle_key_input))
                .on_action(cx.listener(Self::on_t_open_file))
                .on_action(cx.listener(Self::on_t_quit))
                .on_action(cx.listener(Self::on_t_cycle_theme))
                .child("Open a CSV file to get started (Cmd+O)")
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
                    items.push(
                        this.render_row_el(ri, cx)
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, {
                                let se = se.clone();
                                move |ev, _, cx| {
                                    let x = ev.position.x.as_f32();
                                    if x < ROW_NUMBER_WIDTH {
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
                                    let st = se.read(cx);
                                    let mut cx2 = ROW_NUMBER_WIDTH; let mut cc = 0;
                                    for c in 0..st.col_count() { let w = st.column_width(c); if x < cx2 + w { cc = c; break; } cx2 += w; cc = c; }
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
                                move |ev, _, cx| {
                                    let x = ev.position.x.as_f32();
                                    let y = ev.position.y.as_f32();
                                    let st = se.read(cx);
                                    let mut cx2 = ROW_NUMBER_WIDTH; let mut cc = 0;
                                    for c in 0..st.col_count() { let w = st.column_width(c); if x < cx2 + w { cc = c; break; } cx2 += w; cc = c; }
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
        ).h_full();

        let se_move = self.state.clone();
        let se_up = self.state.clone();

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
            .on_mouse_move(move |ev, _, cx| {
                let st = se_move.read(cx);
                if !st.is_dragging { return; }
                let x = ev.position.x.as_f32();
                let y = ev.position.y.as_f32();
                // Calculate row from y (subtract header height)
                let row_y = y - HEADER_HEIGHT;
                if row_y < 0.0 { return; }
                let ri = (row_y / ROW_HEIGHT) as usize;
                let total = st.effective_row_count();
                let ri = ri.min(total.saturating_sub(1));
                // Calculate col from x
                let mut col_x = ROW_NUMBER_WIDTH;
                let mut cc = 0usize;
                for c in 0..st.col_count() {
                    let w = st.column_width(c);
                    if x < col_x + w { cc = c; break; }
                    col_x += w;
                    cc = c;
                }
                let cur_focus = st.selection_focus;
                drop(st);
                let new_focus = CellCoord { row: ri, col: cc };
                if cur_focus != Some(new_focus) {
                    se_move.update(cx, |s, _| {
                        s.selection_focus = Some(new_focus);
                        s.selection_type = Some(SelectionType::Cell);
                    });
                }
            })
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                se_up.update(cx, |s, _| { s.is_dragging = false; });
            })
            .child(header)
            .child(row_list)
            .into_any_element()
    }
}
