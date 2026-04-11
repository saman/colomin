use gpui::Context;

use crate::state::{CellCoord, EditAction, SelectionType};

use super::TableView;

enum EditApplyMode {
    Remove,
    Set,
}

fn apply_cell_value(
    view: &mut TableView,
    row: usize,
    col: usize,
    value: &str,
    mode: EditApplyMode,
    cx: &mut Context<TableView>,
) {
    view.state.update(cx, |s, _| {
        if let Some(ref mut file) = s.file {
            match mode {
                EditApplyMode::Remove => {
                    file.edits.remove(&(row, col));
                }
                EditApplyMode::Set => {
                    file.edits.insert((row, col), value.to_string());
                }
            }
        }
        if let Some(cached) = s.row_cache.get_mut(&row) {
            if col < cached.len() {
                cached[col] = value.to_string();
            }
        }
    });
}

pub fn on_undo(view: &mut TableView, cx: &mut Context<TableView>) {
    if view.editing.is_some() {
        view.cancel_edit(cx);
        return;
    }
    let action = view
        .state
        .update(cx, |s, _| s.undo_stack.pop());
    let Some(action) = action else {
        return;
    };

    let mut selection: Option<CellCoord> = None;
    match &action {
        EditAction::CellEdit {
            row,
            col,
            old_had_edit,
            old_value,
            ..
        } => {
            let mode = if *old_had_edit {
                EditApplyMode::Set
            } else {
                EditApplyMode::Remove
            };
            apply_cell_value(view, *row, *col, old_value, mode, cx);
            selection = Some(CellCoord {
                row: *row,
                col: *col,
            });
        }
        EditAction::BatchCellEdit { edits } => {
            for edit in edits {
                let mode = if edit.old_had_edit {
                    EditApplyMode::Set
                } else {
                    EditApplyMode::Remove
                };
                apply_cell_value(view, edit.row, edit.col, &edit.old_value, mode, cx);
            }
            if let Some(last) = edits.last() {
                selection = Some(CellCoord {
                    row: last.row,
                    col: last.col,
                });
            }
        }
        EditAction::Structural { .. } => {
            // Intentionally ignored: undo/redo scope is value changes only.
        }
    }

    view.state.update(cx, |s, _| {
        if let Some(coord) = selection {
            s.selection_anchor = Some(coord);
            s.selection_focus = Some(coord);
            s.selection_type = Some(SelectionType::Cell);
        }
        s.redo_stack.push(action);
        s.cache_version += 1;
    });
    cx.notify();
}

pub fn on_redo(view: &mut TableView, cx: &mut Context<TableView>) {
    if view.editing.is_some() {
        view.cancel_edit(cx);
        return;
    }
    let action = view
        .state
        .update(cx, |s, _| s.redo_stack.pop());
    let Some(action) = action else {
        return;
    };

    let mut selection: Option<CellCoord> = None;
    match &action {
        EditAction::CellEdit {
            row,
            col,
            new_value,
            ..
        } => {
            apply_cell_value(view, *row, *col, new_value, EditApplyMode::Set, cx);
            selection = Some(CellCoord {
                row: *row,
                col: *col,
            });
        }
        EditAction::BatchCellEdit { edits } => {
            for edit in edits {
                apply_cell_value(view, edit.row, edit.col, &edit.new_value, EditApplyMode::Set, cx);
            }
            if let Some(last) = edits.last() {
                selection = Some(CellCoord {
                    row: last.row,
                    col: last.col,
                });
            }
        }
        EditAction::Structural { .. } => {
            // Intentionally ignored: undo/redo scope is value changes only.
        }
    }

    view.state.update(cx, |s, _| {
        if let Some(coord) = selection {
            s.selection_anchor = Some(coord);
            s.selection_focus = Some(coord);
            s.selection_type = Some(SelectionType::Cell);
        }
        s.undo_stack.push(action);
        s.cache_version += 1;
    });
    cx.notify();
}
