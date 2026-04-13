use gpui::{Context, KeyDownEvent, MouseMoveEvent};

use crate::state::{CellCoord, EditAction, SelectionType};

use super::TableView;

pub fn commit_edit(view: &mut TableView, cx: &mut Context<TableView>) {
    if let Some((row, col, ref text)) = view.editing {
        let new_value = text.clone();
        view.state.update(cx, |s, _| {
            // When header row is off, display row 0 is the metadata header row.
            // Edits go to column names instead of file.edits.
            if s.display_row_to_actual_row(row).is_none() {
                if let Some(ref mut file) = s.file {
                    if col < file.metadata.columns.len() {
                        let old_value = file.metadata.columns[col].name.clone();
                        if old_value != new_value {
                            file.metadata.columns[col].name = new_value;
                            file.columns_renamed = true;
                        }
                    }
                }
                s.cache_version += 1;
                s.editing_cell = None;
                return;
            }
            let actual_row = s.display_row_to_actual_row(row).unwrap();
            let old_had_edit = s
                .file
                .as_ref()
                .map_or(false, |f| f.edits.contains_key(&(actual_row, col)));
            // Capture old value before overwriting (from edits map, then cache)
            let old_value = s.file.as_ref()
                .and_then(|f| f.edits.get(&(actual_row, col)).cloned())
                .or_else(|| s.row_cache.get(&actual_row).and_then(|r| r.get(col)).cloned())
                .unwrap_or_default();
            if let Some(ref mut file) = s.file {
                file.edits.insert((actual_row, col), new_value.clone());
            }
               // Ensure row is cached (if not yet cached, fetch it and cache it)
               if !s.row_cache.contains_key(&actual_row) {
                   // Row not cached yet; fetch it from file with edits applied
                   if let Some(ref file) = s.file {
                       let col_count = file.metadata.columns.len();
                       let path = file.file_path.clone();
                       let row_offsets = file.row_offsets.clone();
                       // read_single_row_from_reader will be called synchronously here
                       // This is acceptable since we're within a state update and only reading one row  
                       match std::fs::File::open(&path) {
                           Ok(f) => {
                               let mut reader = std::io::BufReader::new(f);
                               match crate::csv_engine::parser::read_single_row_from_reader(
                                   &mut reader,
                                   &row_offsets,
                                   actual_row,
                                   col_count,
                                   file.delimiter,
                               ) {
                                   Ok(mut row_data) => {
                                       // Apply all current edits to this row
                                       for c in 0..col_count {
                                           if let Some(ed) = file.edits.get(&(actual_row, c)) {
                                               row_data[c].clone_from(ed);
                                           }
                                       }
                                       s.cache_row(actual_row, row_data);
                                   }
                                   Err(_) => {
                                       // If read fails, just update the existing cached row if present
                                       if let Some(cached) = s.row_cache.get_mut(&actual_row) {
                                           if col < cached.len() {
                                               cached[col] = new_value.clone();
                                           }
                                       }
                                   }
                               }
                           }
                           Err(_) => {
                               // If file open fails, just update the existing cached row if present
                               if let Some(cached) = s.row_cache.get_mut(&actual_row) {
                                   if col < cached.len() {
                                       cached[col] = new_value.clone();
                                   }
                               }
                           }
                       }
                   }
               } else {
                   // Row already cached; just update the edited cell
                   if let Some(cached) = s.row_cache.get_mut(&actual_row) {
                       if col < cached.len() {
                           cached[col] = new_value.clone();
                       }
                   }
               }
            s.cache_version += 1;
            s.editing_cell = None;
            // Push to undo stack only if value changed
            if old_value != new_value {
                s.undo_stack.push(EditAction::CellEdit {
                    row: actual_row,
                    col,
                    old_had_edit,
                    old_value,
                    new_value,
                });
                s.redo_stack.clear();
            }
        });
    }
    view.editing = None;
    cx.notify();
}

pub fn cancel_edit(view: &mut TableView, cx: &mut Context<TableView>) {
    view.editing = None;
    view.state.update(cx, |s, _| {
        s.editing_cell = None;
    });
    cx.notify();
}

pub fn start_edit_from_state(view: &mut TableView, cx: &mut Context<TableView>) {
    let editing_cell = view.state.read(cx).editing_cell.clone();

    // If view has a pending edit but editing_cell was cleared (e.g. user clicked away),
    // auto-commit the pending edit so changes are not silently lost.
    if view.editing.is_some() && editing_cell.is_none() {
        view.commit_edit(cx);
        return;
    }

    if let Some((r, c, _)) = editing_cell {
        let should_start = match &view.editing {
            Some((er, ec, _)) => *er != r || *ec != c,
            None => true,
        };
        if should_start {
            let current_val = view.state.read(cx)
                .get_display_cell(r, c)
                .unwrap_or_default();
            view.editing = Some((r, c, current_val));
        }
    }
}

pub fn move_selection(view: &mut TableView, dr: i32, dc: i32, cx: &mut Context<TableView>) {
    if view.editing.is_some() {
        view.commit_edit(cx);
    }
    view.state.update(cx, |s, _| {
        let (rows, cols) = (s.display_row_count(), s.col_count());
        if rows == 0 || cols == 0 {
            return;
        }
        let (cr, cc) = s
            .selection_focus
            .map(|c| (c.row as i32, c.col as i32))
            .unwrap_or((0, 0));
        let nr = (cr + dr).clamp(0, rows as i32 - 1) as usize;
        let nc = (cc + dc).clamp(0, cols as i32 - 1) as usize;
        s.selection_type = Some(SelectionType::Cell);
        s.selection_anchor = Some(CellCoord { row: nr, col: nc });
        s.selection_focus = Some(CellCoord { row: nr, col: nc });
        s.selected_rows.clear();
        s.selected_columns.clear();
    });
    cx.notify();
}

pub fn extend_selection(view: &mut TableView, dr: i32, dc: i32, cx: &mut Context<TableView>) {
    if view.editing.is_some() {
        view.commit_edit(cx);
    }
    view.state.update(cx, |s, _| {
        let (rows, cols) = (s.display_row_count(), s.col_count());
        if rows == 0 || cols == 0 {
            return;
        }
        // Keep anchor, move focus
        if s.selection_anchor.is_none() {
            s.selection_anchor = s.selection_focus;
        }
        let (cr, cc) = s
            .selection_focus
            .map(|c| (c.row as i32, c.col as i32))
            .unwrap_or((0, 0));
        let nr = (cr + dr).clamp(0, rows as i32 - 1) as usize;
        let nc = (cc + dc).clamp(0, cols as i32 - 1) as usize;
        s.selection_type = Some(SelectionType::Cell);
        s.selection_focus = Some(CellCoord { row: nr, col: nc });
        s.selected_rows.clear();
        s.selected_columns.clear();
    });
    cx.notify();
}

pub fn on_enter(view: &mut TableView, cx: &mut Context<TableView>) {
    if view.editing.is_some() {
        view.commit_edit(cx);
        // Move down after commit
        view.move_selection(1, 0, cx);
    } else {
        // Start editing the selected cell
        let next_edit = {
            let state = view.state.read(cx);
            state.selection_focus.map(|focus| {
                let val = state
                    .get_display_cell(focus.row, focus.col)
                    .unwrap_or_default();
                (focus.row, focus.col, val)
            })
        };
        if let Some((row, col, val)) = next_edit {
            view.editing = Some((row, col, val));
            cx.notify();
        }
    }
}

pub fn handle_key_input(view: &mut TableView, event: &KeyDownEvent, cx: &mut Context<TableView>) {
    if view.editing.is_none() {
        // If not editing and a printable key is pressed, start editing
        if let Some(ref ch) = event.keystroke.key_char {
            if !event.keystroke.modifiers.platform && !event.keystroke.modifiers.control {
                let focus = {
                    let state = view.state.read(cx);
                    state.selection_focus
                };
                if let Some(focus) = focus {
                    // Start editing with this character (replace mode)
                    view.editing = Some((focus.row, focus.col, ch.clone()));
                    cx.notify();
                }
            }
        }
        return;
    }

    // We're in edit mode — handle input
    if let Some(ref ch) = event.keystroke.key_char {
        if !event.keystroke.modifiers.platform && !event.keystroke.modifiers.control {
            if let Some((_, _, ref mut text)) = view.editing {
                text.push_str(ch);
            }
            cx.notify();
        }
    }
}

pub fn handle_drag_selection(view: &mut TableView, event: &MouseMoveEvent, cx: &mut Context<TableView>) {
    // Cell selection drag (scrollbar drag is handled by global canvas listeners)
    if view.scrollbar_drag.get().is_some() {
        return;
    }

    let state = view.state.read(cx);
    if !state.is_dragging {
        return;
    }

    // Only drag-select while mouse button is actively held.
    // This prevents scroll gestures from moving selection under the cursor.
    if !event.dragging() {
        let _ = state;
        view.state.update(cx, |s, _| {
            s.is_dragging = false;
        });
        return;
    }

    // Adjust screen x by horizontal scroll offset to get content-space x
    let x = event.position.x.as_f32() + view.horizontal_offset.get();
    let display_rows = state.display_row_count();
    let row_index = match view.hit_test_row_from_window_y(event.position.y.as_f32(), display_rows, cx) {
        Some(row) => row,
        None => return,
    };
    let col_index = TableView::hit_test_col_from_content_x(&state, x);
    let current_focus = state.selection_focus;
    let _ = state;

    let new_focus = CellCoord {
        row: row_index,
        col: col_index,
    };
    if current_focus != Some(new_focus) {
        view.state.update(cx, |s, _| {
            s.selection_focus = Some(new_focus);
            s.selection_type = Some(SelectionType::Cell);
        });
    }
}