use std::io::{BufReader, Seek, SeekFrom};

use eframe::egui;
use egui_extras::{Column, TableBuilder};

use crate::state::{AppState, BatchEditEntry, CellCoord, EditAction, SelectionType};

pub struct TableView {
    pub editing: Option<(usize, usize, String)>,
    /// Index of row whose bottom edge is being dragged for resize, or usize::MAX for global resize.
    row_resize: Option<usize>,
    row_resize_start_y: f32,
    row_resize_start_h: f32,
}

impl TableView {
    pub fn new() -> Self {
        Self {
            editing: None,
            row_resize: None,
            row_resize_start_y: 0.0,
            row_resize_start_h: 0.0,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut AppState, ctx: &egui::Context) {
        let Some(_) = state.file else { return };

        self.handle_keyboard(state, ctx);

        let colors = state.current_theme();
        let row_count = state.display_row_count();
        let col_count = state.col_count();
        let gutter_width = state.row_number_width();

        let header_bg  = colors.gutter_bg;
        let gutter_bg  = colors.gutter_bg;
        let line_num   = colors.line_number;
        let text_pri   = colors.text_primary;
        let text_sec   = colors.text_secondary;
        let sel_color  = colors.selection;
        let edit_color = colors.edited;
        let surf       = colors.surface;
        let accent     = colors.accent;
        let border     = colors.border;

        let col_names: Vec<String> = state.file.as_ref()
            .map(|f| f.metadata.columns.iter().map(|c| c.name.clone()).collect())
            .unwrap_or_default();

        // Sort state for header arrows
        let sort_col = state.sort_state.as_ref().map(|s| s.column_index);
        let sort_asc = state.sort_state.as_ref().map(|s| {
            matches!(s.direction, crate::state::SortDirection::Asc)
        });

        let mut table = TableBuilder::new(ui)
            .striped(false)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(gutter_width))
            .min_scrolled_height(0.0);

        for col in 0..col_count {
            let w = state.column_width(col);
            table = table.column(Column::initial(w).at_least(40.0).resizable(true));
        }

        // ── Header ──
        table
            .header(28.0, |mut header| {
                header.col(|ui| {
                    ui.painter().rect_filled(ui.max_rect(), 0.0, header_bg);
                });
                for (col_idx, name) in col_names.iter().enumerate() {
                    header.col(|ui| {
                        let rect = ui.max_rect();
                        ui.painter().rect_filled(rect, 0.0, header_bg);
                        let is_sorted = sort_col == Some(col_idx);
                        let label = if is_sorted {
                            let arrow = if sort_asc == Some(true) { " ↑" } else { " ↓" };
                            format!("{}{}", name, arrow)
                        } else {
                            name.clone()
                        };
                        let color = if is_sorted { accent } else { text_sec };
                        let resp = ui.allocate_rect(rect, egui::Sense::click());
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            &label,
                            egui::FontId::proportional(12.0),
                            color,
                        );
                        if resp.clicked() {
                            // Toggle sort: same col → flip direction; new col → ascending
                            let ascending = if sort_col == Some(col_idx) {
                                sort_asc != Some(true)
                            } else {
                                true
                            };
                            state.pending_sort = Some((col_idx, ascending));
                            self.editing = None;
                        }
                        if resp.clicked() && ctx.input(|i| i.modifiers.shift) {
                            // Shift-click → column select
                            state.pending_sort = None;
                            state.selection_type = Some(SelectionType::Column);
                            self.editing = None;
                            if !state.selected_columns.contains(&col_idx) {
                                state.selected_columns.push(col_idx);
                            }
                        }
                    });
                }
            })
            // ── Body ──
            .body(|body| {
                // Use heterogeneous_rows so per-row heights work
                let heights: Vec<f32> = (0..row_count)
                    .map(|r| state.row_height_for(r))
                    .collect();
                body.heterogeneous_rows(heights.into_iter(), |mut row| {
                    let display_row = row.index();
                    let row_h = state.row_height_for(display_row);

                    let row_sel = state.selection_type == Some(SelectionType::Row)
                        && state.selected_rows.contains(&display_row);

                    // ── Gutter cell ──
                    row.col(|ui| {
                        let rect = ui.max_rect();
                        let bg = if row_sel { sel_color } else { gutter_bg };
                        ui.painter().rect_filled(rect, 0.0, bg);

                        // Row number
                        let num_rect = egui::Rect::from_min_max(
                            rect.min,
                            egui::pos2(rect.max.x, rect.max.y - 4.0),
                        );
                        ui.painter().text(
                            num_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            (display_row + 1).to_string(),
                            egui::FontId::monospace(10.0),
                            line_num,
                        );

                        // Row-select click (top 80% of gutter)
                        let click_rect = egui::Rect::from_min_max(
                            rect.min,
                            egui::pos2(rect.max.x, rect.min.y + row_h * 0.8),
                        );
                        let resp = ui.allocate_rect(click_rect, egui::Sense::click());
                        if resp.clicked() {
                            let shift = ctx.input(|i| i.modifiers.shift);
                            state.selection_type = Some(SelectionType::Row);
                            self.editing = None;
                            if !shift { state.selected_rows.clear(); state.selected_columns.clear(); }
                            if !state.selected_rows.contains(&display_row) {
                                state.selected_rows.push(display_row);
                            }
                        }

                        // Resize handle (bottom 4px of gutter)
                        let handle_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.min.x, rect.max.y - 4.0),
                            rect.max,
                        );
                        let handle_resp = ui.allocate_rect(handle_rect, egui::Sense::drag());
                        if handle_resp.hovered() || self.row_resize == Some(display_row) {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                            ui.painter().rect_filled(handle_rect, 0.0, accent);
                        } else {
                            ui.painter().rect_filled(handle_rect, 0.0, border);
                        }
                        if handle_resp.drag_started() {
                            self.row_resize = Some(display_row);
                            self.row_resize_start_y = handle_resp.interact_pointer_pos()
                                .map(|p| p.y)
                                .unwrap_or(0.0);
                            self.row_resize_start_h = row_h;
                        }
                        if handle_resp.dragged() {
                            if self.row_resize == Some(display_row) {
                                let dy = handle_resp.drag_delta().y;
                                let new_h = (self.row_resize_start_h + dy).max(16.0);
                                self.row_resize_start_h = new_h;
                                state.row_heights.insert(display_row, new_h);
                                state.invalidate_row_layout();
                            }
                        }
                        if handle_resp.drag_stopped() {
                            self.row_resize = None;
                        }
                    });

                    // ── Data cells ──
                    for col_idx in 0..col_count {
                        row.col(|ui| {
                            let cell_sel = state.is_cell_selected(display_row, col_idx);
                            let is_cursor = state.selection_anchor
                                .map(|a| a.row == display_row && a.col == col_idx)
                                .unwrap_or(false)
                                && state.selection_type == Some(SelectionType::Cell);
                            let is_editing = self.editing.as_ref()
                                .map(|(r, c, _)| *r == display_row && *c == col_idx)
                                .unwrap_or(false);
                            let has_edit = state.file.as_ref()
                                .map(|f| f.edits.contains_key(&(display_row, col_idx)))
                                .unwrap_or(false);

                            let bg = if cell_sel || row_sel { sel_color }
                                     else if has_edit { edit_color }
                                     else { surf };
                            ui.painter().rect_filled(ui.max_rect(), 0.0, bg);

                            if is_cursor && !is_editing {
                                ui.painter().rect_stroke(
                                    ui.max_rect(),
                                    0.0,
                                    egui::Stroke::new(1.5, accent),
                                    egui::StrokeKind::Inside,
                                );
                            }

                            if is_editing {
                                if let Some((_, _, ref mut buf)) = self.editing {
                                    let te = egui::TextEdit::singleline(buf)
                                        .font(egui::FontId::proportional(12.0))
                                        .desired_width(ui.available_width());
                                    let resp = ui.add(te);
                                    resp.request_focus();

                                    let cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
                                    let commit = resp.lost_focus() && !cancel;

                                    if commit {
                                        if let Some((r, c, new_val)) = self.editing.take() {
                                            commit_edit(state, r, c, new_val);
                                        }
                                    } else if cancel {
                                        self.editing = None;
                                    }
                                }
                            } else {
                                let value = get_cell(state, display_row, col_idx);
                                let cell_rect = ui.max_rect().shrink2(egui::vec2(4.0, 0.0));
                                ui.painter().text(
                                    egui::pos2(cell_rect.left(), cell_rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    &value,
                                    egui::FontId::proportional(12.0),
                                    text_pri,
                                );

                                let resp = ui.allocate_rect(ui.max_rect(), egui::Sense::click());
                                if resp.double_clicked() {
                                    self.editing = Some((display_row, col_idx, value.clone()));
                                    state.selection_type = Some(SelectionType::Cell);
                                    state.selection_anchor = Some(CellCoord { row: display_row, col: col_idx });
                                    state.selection_focus = Some(CellCoord { row: display_row, col: col_idx });
                                } else if resp.clicked() {
                                    let shift = ctx.input(|i| i.modifiers.shift);
                                    if !shift {
                                        state.selection_type = Some(SelectionType::Cell);
                                        state.selection_anchor = Some(CellCoord { row: display_row, col: col_idx });
                                        state.selection_focus = Some(CellCoord { row: display_row, col: col_idx });
                                        state.selected_rows.clear();
                                        state.selected_columns.clear();
                                    } else if state.selection_anchor.is_some() {
                                        state.selection_focus = Some(CellCoord { row: display_row, col: col_idx });
                                        state.selection_type = Some(SelectionType::Cell);
                                    }
                                    self.editing = None;
                                }

                                resp.context_menu(|ui| {
                                    if ui.button("Copy").clicked() {
                                        ui.ctx().copy_text(value.clone());
                                        ui.close_menu();
                                    }
                                    if ui.button("Clear cell").clicked() {
                                        commit_edit(state, display_row, col_idx, String::new());
                                        ui.close_menu();
                                    }
                                    ui.separator();
                                    if ui.button("Sort ↑").clicked() {
                                        state.pending_sort = Some((col_idx, true));
                                        ui.close_menu();
                                    }
                                    if ui.button("Sort ↓").clicked() {
                                        state.pending_sort = Some((col_idx, false));
                                        ui.close_menu();
                                    }
                                    if ui.button("Reset row height").clicked() {
                                        state.row_heights.remove(&display_row);
                                        state.invalidate_row_layout();
                                        ui.close_menu();
                                    }
                                });
                            }
                        });
                    }
                });
            });

        // Copy shortcut
        if self.editing.is_none() {
            let do_copy = ctx.input(|i| i.key_pressed(egui::Key::C) && i.modifiers.command);
            if do_copy {
                ctx.copy_text(selection_to_text(state));
            }
        }
    }

    fn handle_keyboard(&mut self, state: &mut AppState, ctx: &egui::Context) {
        if self.editing.is_some() { return; }

        let total_rows = state.display_row_count();
        let total_cols = state.col_count();
        if total_rows == 0 || total_cols == 0 { return; }

        let anchor = state.selection_anchor;
        let cur_row = anchor.map(|a| a.row).unwrap_or(0);
        let cur_col = anchor.map(|a| a.col).unwrap_or(0);

        let (moved, new_row, new_col, shift, do_enter, do_escape, do_delete, do_undo, do_redo) =
            ctx.input(|i| {
                let shift = i.modifiers.shift;
                let mut new_row = cur_row;
                let mut new_col = cur_col;
                let mut moved = false;

                if i.key_pressed(egui::Key::ArrowUp)    && cur_row > 0           { new_row -= 1; moved = true; }
                if i.key_pressed(egui::Key::ArrowDown)  && cur_row + 1 < total_rows { new_row += 1; moved = true; }
                if i.key_pressed(egui::Key::ArrowLeft)  && cur_col > 0           { new_col -= 1; moved = true; }
                if i.key_pressed(egui::Key::ArrowRight) && cur_col + 1 < total_cols { new_col += 1; moved = true; }

                let do_enter  = i.key_pressed(egui::Key::Enter);
                let do_escape = i.key_pressed(egui::Key::Escape);
                let do_delete = i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace);
                let do_undo   = i.key_pressed(egui::Key::Z) && i.modifiers.command && !i.modifiers.shift;
                let do_redo   = (i.key_pressed(egui::Key::Z) && i.modifiers.command && i.modifiers.shift)
                             || (i.key_pressed(egui::Key::Y) && i.modifiers.command);

                (moved, new_row, new_col, shift, do_enter, do_escape, do_delete, do_undo, do_redo)
            });

        if moved {
            let focus = CellCoord { row: new_row, col: new_col };
            if !shift || anchor.is_none() { state.selection_anchor = Some(focus); }
            state.selection_focus = Some(focus);
            state.selection_type = Some(SelectionType::Cell);
            state.selected_rows.clear();
            state.selected_columns.clear();
        }

        if do_enter {
            if let Some(a) = state.selection_anchor {
                let val = state.get_display_cell(a.row, a.col).unwrap_or_default();
                self.editing = Some((a.row, a.col, val));
            }
        }

        if do_escape {
            state.clear_selection();
        }

        if do_delete {
            delete_selection(state);
        }

        if do_undo { apply_undo(state); }
        if do_redo { apply_redo(state); }

        // Paste (Cmd+V / Ctrl+V)
        let paste_text = ctx.input(|i| {
            i.events.iter().find_map(|e| {
                if let egui::Event::Paste(text) = e { Some(text.clone()) } else { None }
            })
        });
        if let Some(text) = paste_text {
            if state.selection_type == Some(SelectionType::Cell) {
                apply_paste(state, &text);
            }
        }

        // Pick up Enter-initiated edits from state (e.g. from external triggers)
        if let Some((r, c, val)) = state.editing_cell.take() {
            self.editing = Some((r, c, val));
        }
    }
}

// ── Edit operations ───────────────────────────────────────────────────────────

fn commit_edit(state: &mut AppState, row: usize, col: usize, new_val: String) {
    let Some(ref mut file) = state.file else { return };
    let old_had_edit = file.edits.contains_key(&(row, col));
    let old_value = file.edits.get(&(row, col)).cloned()
        .or_else(|| state.row_cache.get(&row).and_then(|r| r.get(col).cloned()))
        .unwrap_or_default();

    if new_val == old_value && !old_had_edit { return; } // no-op

    file.edits.insert((row, col), new_val.clone());
    if let Some(cached) = state.row_cache.get_mut(&row) {
        if col < cached.len() { cached[col] = new_val.clone(); }
    }
    state.cache_version += 1;

    state.undo_stack.push(EditAction::CellEdit {
        row,
        col,
        old_had_edit,
        old_value,
        new_value: new_val,
    });
    state.redo_stack.clear();
}

fn delete_selection(state: &mut AppState) {
    let Some((min_r, max_r, min_c, max_c)) = state.selection_range() else { return };
    let mut edits_batch = Vec::new();
    {
        let Some(ref mut file) = state.file else { return };
        for r in min_r..=max_r {
            for c in min_c..=max_c {
                let old_had_edit = file.edits.contains_key(&(r, c));
                let old_value = file.edits.get(&(r, c)).cloned()
                    .or_else(|| state.row_cache.get(&r).and_then(|row| row.get(c).cloned()))
                    .unwrap_or_default();
                file.edits.insert((r, c), String::new());
                if let Some(row) = state.row_cache.get_mut(&r) {
                    if c < row.len() { row[c] = String::new(); }
                }
                edits_batch.push(BatchEditEntry { row: r, col: c, old_had_edit, old_value, new_value: String::new() });
            }
        }
    }
    if !edits_batch.is_empty() {
        state.undo_stack.push(EditAction::BatchCellEdit { edits: edits_batch });
        state.redo_stack.clear();
        state.cache_version += 1;
    }
}

fn apply_paste(state: &mut AppState, text: &str) {
    let Some(anchor) = state.selection_anchor else { return };
    let total_rows = state.display_row_count();
    let total_cols = state.col_count();
    let mut edits_batch: Vec<BatchEditEntry> = Vec::new();
    {
        let Some(ref mut file) = state.file else { return };
        let lines: Vec<&str> = text.split('\n').collect();
        for (row_offset, line) in lines.iter().enumerate() {
            let line = line.trim_end_matches('\r');
            // Skip the trailing empty entry produced by a newline-terminated string
            if line.is_empty() && row_offset + 1 == lines.len() { break; }
            let display_row = anchor.row + row_offset;
            if display_row >= total_rows { break; }
            for (col_offset, cell_val) in line.split('\t').enumerate() {
                let col = anchor.col + col_offset;
                if col >= total_cols { break; }
                let old_had_edit = file.edits.contains_key(&(display_row, col));
                let old_value = file.edits.get(&(display_row, col)).cloned()
                    .or_else(|| state.row_cache.get(&display_row).and_then(|r| r.get(col).cloned()))
                    .unwrap_or_default();
                let new_value = cell_val.to_string();
                file.edits.insert((display_row, col), new_value.clone());
                if let Some(row_data) = state.row_cache.get_mut(&display_row) {
                    if col < row_data.len() { row_data[col] = new_value.clone(); }
                }
                edits_batch.push(BatchEditEntry { row: display_row, col, old_had_edit, old_value, new_value });
            }
        }
    }
    if !edits_batch.is_empty() {
        state.undo_stack.push(EditAction::BatchCellEdit { edits: edits_batch });
        state.redo_stack.clear();
        state.cache_version += 1;
    }
}

fn apply_undo(state: &mut AppState) {
    let Some(action) = state.undo_stack.pop() else { return };
    let mut sel: Option<CellCoord> = None;

    match &action {
        EditAction::CellEdit { row, col, old_had_edit, old_value, .. } => {
            if let Some(ref mut file) = state.file {
                if *old_had_edit {
                    file.edits.insert((*row, *col), old_value.clone());
                } else {
                    file.edits.remove(&(*row, *col));
                }
            }
            if let Some(cached) = state.row_cache.get_mut(row) {
                if *col < cached.len() { cached[*col] = old_value.clone(); }
            }
            sel = Some(CellCoord { row: *row, col: *col });
        }
        EditAction::BatchCellEdit { edits } => {
            for e in edits {
                if let Some(ref mut file) = state.file {
                    if e.old_had_edit {
                        file.edits.insert((e.row, e.col), e.old_value.clone());
                    } else {
                        file.edits.remove(&(e.row, e.col));
                    }
                }
                if let Some(cached) = state.row_cache.get_mut(&e.row) {
                    if e.col < cached.len() { cached[e.col] = e.old_value.clone(); }
                }
            }
            sel = edits.first().map(|e| CellCoord { row: e.row, col: e.col });
        }
        EditAction::Structural { .. } => {}
    }

    if let Some(coord) = sel {
        state.selection_anchor = Some(coord);
        state.selection_focus = Some(coord);
        state.selection_type = Some(SelectionType::Cell);
    }
    state.redo_stack.push(action);
    state.cache_version += 1;
}

fn apply_redo(state: &mut AppState) {
    let Some(action) = state.redo_stack.pop() else { return };
    let mut sel: Option<CellCoord> = None;

    match &action {
        EditAction::CellEdit { row, col, new_value, .. } => {
            if let Some(ref mut file) = state.file {
                file.edits.insert((*row, *col), new_value.clone());
            }
            if let Some(cached) = state.row_cache.get_mut(row) {
                if *col < cached.len() { cached[*col] = new_value.clone(); }
            }
            sel = Some(CellCoord { row: *row, col: *col });
        }
        EditAction::BatchCellEdit { edits } => {
            for e in edits {
                if let Some(ref mut file) = state.file {
                    file.edits.insert((e.row, e.col), e.new_value.clone());
                }
                if let Some(cached) = state.row_cache.get_mut(&e.row) {
                    if e.col < cached.len() { cached[e.col] = e.new_value.clone(); }
                }
            }
            sel = edits.last().map(|e| CellCoord { row: e.row, col: e.col });
        }
        EditAction::Structural { .. } => {}
    }

    if let Some(coord) = sel {
        state.selection_anchor = Some(coord);
        state.selection_focus = Some(coord);
        state.selection_type = Some(SelectionType::Cell);
    }
    state.undo_stack.push(action);
    state.cache_version += 1;
}

// ── Row/cell helpers ──────────────────────────────────────────────────────────

fn get_cell(state: &mut AppState, display_row: usize, col: usize) -> String {
    if let Some(v) = state.get_display_cell(display_row, col) {
        return v;
    }
    if load_row(state, display_row) {
        return state.get_display_cell(display_row, col).unwrap_or_default();
    }
    String::new()
}

fn load_row(state: &mut AppState, display_row: usize) -> bool {
    let (path, byte_offset, actual_row, col_count, delimiter, row_edits) = {
        let file = match state.file.as_ref() { Some(f) => f, None => return false };
        let actual_row = match state.display_row_to_actual_row(display_row) { Some(r) => r, None => return false };
        let phys_row = file.virtual_to_actual_row(actual_row);
        let byte_offset = match file.row_offsets.get(phys_row).copied() { Some(o) => o, None => return false };
        let row_edits: Vec<(usize, String)> = file.edits.iter()
            .filter(|(&(r, _), _)| r == actual_row)
            .map(|(&(_, c), v)| (c, v.clone()))
            .collect();
        (file.file_path.clone(), byte_offset, actual_row, file.metadata.columns.len(), file.delimiter, row_edits)
    };

    let fh = match std::fs::File::open(&path) { Ok(f) => f, Err(_) => return false };
    let mut reader = BufReader::new(fh);
    if reader.seek(SeekFrom::Start(byte_offset)).is_err() { return false; }
    let mut csv_rdr = csv::ReaderBuilder::new()
        .has_headers(false).flexible(true).delimiter(delimiter).from_reader(reader);
    let record = match csv_rdr.records().next() { Some(Ok(r)) => r, _ => return false };

    let mut row: Vec<String> = (0..col_count).map(|c| record.get(c).unwrap_or("").to_string()).collect();
    for (c, v) in row_edits { if c < row.len() { row[c] = v; } }
    state.cache_row(actual_row, row);
    true
}

fn selection_to_text(state: &AppState) -> String {
    let Some((min_r, max_r, min_c, max_c)) = state.selection_range() else { return String::new() };
    (min_r..=max_r)
        .map(|r| {
            (min_c..=max_c)
                .map(|c| state.get_display_cell(r, c).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
