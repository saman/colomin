use gpui::{ClipboardItem, Context};

use crate::state::{BatchEditEntry, EditAction, SelectionType};

use super::TableView;

pub fn on_copy(view: &mut TableView, cx: &mut Context<TableView>) {
    let preload_range = {
        let state = view.state.read(cx);
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
                let rows = state.display_row_count();
                if rows == 0 {
                    None
                } else {
                    Some((0, rows - 1))
                }
            }
            None => None,
        }
    };

    if let Some((start, end)) = preload_range {
        let all_cached = {
            let state = view.state.read(cx);
            (start..=end).all(|r| state.display_row_to_actual_row(r).is_none() || state.get_cached_row(state.display_row_to_actual_row(r).unwrap()).is_some())
        };
        if !all_cached {
            view.ensure_rows_cached(start, end.saturating_sub(start) + 1, cx);
            view.state.update(cx, |s, _| {
                s.toast_message = Some("Selection is still loading. Try copy again in a moment.".to_string());
            });
            cx.notify();
            return;
        }
    }

    let state = view.state.read(cx);
    let mut lines = Vec::new();
    match state.selection_type {
        Some(SelectionType::Cell) => {
            if let Some((mr, xr, mc, xc)) = state.selection_range() {
                for r in mr..=xr {
                    let mut cells = Vec::with_capacity(xc - mc + 1);
                    for c in mc..=xc {
                        cells.push(state.get_display_cell(r, c).unwrap_or_default());
                    }
                    lines.push(cells.join("\t"));
                }
            }
        }
        Some(SelectionType::Row) => {
            for &r in &state.selected_rows {
                if let Some(row) = state.get_display_row(r) {
                    lines.push(row.join("\t"));
                }
            }
        }
        Some(SelectionType::Column) => {
            for r in 0..state.display_row_count() {
                let mut cells = Vec::new();
                for &c in &state.selected_columns {
                    cells.push(state.get_display_cell(r, c).unwrap_or_default());
                }
                lines.push(cells.join("\t"));
            }
        }
        None => {}
    }
    if !lines.is_empty() {
        cx.write_to_clipboard(ClipboardItem::new_string(lines.join("\n")));
    }
}

pub fn on_delete(view: &mut TableView, cx: &mut Context<TableView>) {
    if view.editing.is_some() {
        // Backspace in edit mode
        if let Some((_, _, ref mut text)) = view.editing {
            text.pop();
        }
        cx.notify();
        return;
    }
    let range = view.state.read(cx).selection_range();
    if let Some((mr, xr, mc, xc)) = range {
        view.state.update(cx, |s, _| {
            if s.file.is_none() {
                return;
            }

            let header_off = !s.header_row_enabled;
            let mut planned_edits = Vec::new();
            for r in mr..=xr {
                let actual_row = if header_off {
                    if r == 0 {
                        continue;
                    }
                    r - 1
                } else {
                    r
                };

                for c in mc..=xc {
                    let old_had_edit = s
                        .file
                        .as_ref()
                        .map_or(false, |file| file.edits.contains_key(&(actual_row, c)));
                    let old_value = s
                        .file
                        .as_ref()
                        .and_then(|file| file.edits.get(&(actual_row, c)).cloned())
                        .or_else(|| {
                            s.row_cache
                                .get(&actual_row)
                                .and_then(|row| row.get(c))
                                .cloned()
                        })
                        .unwrap_or_default();

                    planned_edits.push((actual_row, c, old_had_edit, old_value));
                }
            }

            let mut batch_edits = Vec::new();
            for (actual_row, c, old_had_edit, old_value) in planned_edits {
                if let Some(file) = s.file.as_mut() {
                    file.edits.insert((actual_row, c), String::new());
                }
                if let Some(row) = s.row_cache.get_mut(&actual_row) {
                    if c < row.len() {
                        row[c] = String::new();
                    }
                }

                if !old_value.is_empty() {
                    batch_edits.push(BatchEditEntry {
                        row: actual_row,
                        col: c,
                        old_had_edit,
                        old_value,
                        new_value: String::new(),
                    });
                }
            }
            if !batch_edits.is_empty() {
                s.undo_stack.push(EditAction::BatchCellEdit { edits: batch_edits });
                s.redo_stack.clear();
            }
            s.cache_version += 1;
        });
        cx.notify();
    }
}