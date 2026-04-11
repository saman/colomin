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
                let rows = state.effective_row_count();
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
            (start..=end).all(|r| state.get_cached_row(r).is_some())
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
                        cells.push(
                            state
                                .get_cached_row(r)
                                .and_then(|row| row.get(c))
                                .cloned()
                                .unwrap_or_default(),
                        );
                    }
                    lines.push(cells.join("\t"));
                }
            }
        }
        Some(SelectionType::Row) => {
            for &r in &state.selected_rows {
                if let Some(row) = state.get_cached_row(r) {
                    lines.push(row.join("\t"));
                }
            }
        }
        Some(SelectionType::Column) => {
            for r in 0..state.effective_row_count() {
                let mut cells = Vec::new();
                for &c in &state.selected_columns {
                    cells.push(
                        state
                            .get_cached_row(r)
                            .and_then(|row| row.get(c))
                            .cloned()
                            .unwrap_or_default(),
                    );
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
            if let Some(ref mut file) = s.file {
                let mut batch_edits = Vec::new();
                for r in mr..=xr {
                    for c in mc..=xc {
                        let old_had_edit = file.edits.contains_key(&(r, c));
                        let old_value = file
                            .edits
                            .get(&(r, c))
                            .cloned()
                            .or_else(|| s.row_cache.get(&r).and_then(|row| row.get(c)).cloned())
                            .unwrap_or_default();

                        file.edits.insert((r, c), String::new());
                        if let Some(row) = s.row_cache.get_mut(&r) {
                            if c < row.len() {
                                row[c] = String::new();
                            }
                        }

                        if !old_value.is_empty() {
                            batch_edits.push(BatchEditEntry {
                                row: r,
                                col: c,
                                old_had_edit,
                                old_value,
                                new_value: String::new(),
                            });
                        }
                    }
                }
                if !batch_edits.is_empty() {
                    s.undo_stack.push(EditAction::BatchCellEdit { edits: batch_edits });
                    s.redo_stack.clear();
                }
                s.cache_version += 1;
            }
        });
        cx.notify();
    }
}