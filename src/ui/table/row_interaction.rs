use std::cell::Cell;
use std::rc::Rc;

use gpui::{App, ClickEvent, Entity, MouseDownEvent, ScrollHandle};

use crate::state::{AppState, CellCoord, SelectionType};

use super::TableView;

pub(super) fn on_row_left_mouse_down(
    state_entity: &Entity<AppState>,
    horizontal_offset: &Rc<Cell<f32>>,
    scroll_handle: &ScrollHandle,
    scrollbar_drag: &Rc<Cell<Option<bool>>>,
    row_index: usize,
    event: &MouseDownEvent,
    cx: &mut App,
    row_number_width: f32,
) {
    if scrollbar_drag.get().is_some() {
        return;
    }
    let state = state_entity.read(cx);
    if state.has_open_menu() {
        return;
    }
    if TableView::is_in_scrollbar_hit_region(scroll_handle, &state, event.position) {
        return;
    }

    // Adjust screen x by horizontal scroll offset to get content-space x
    let x = event.position.x.as_f32() + horizontal_offset.get();
    if x < row_number_width {
        let _ = state;
        state_entity.update(cx, |s, _| {
            if event.modifiers.platform {
                if s.selected_rows.contains(&row_index) {
                    s.selected_rows.retain(|&r| r != row_index);
                } else {
                    s.selected_rows.push(row_index);
                }
            } else {
                s.selected_rows.clear();
                s.selected_rows.push(row_index);
            }
            s.selection_type = Some(SelectionType::Row);
            s.selection_anchor = None;
            s.selection_focus = None;
            s.selected_columns.clear();
            s.context_menu = None;
        });
        return;
    }

    let col_index = TableView::hit_test_col_from_content_x(&state, x);
    let _ = state;
    state_entity.update(cx, |s, _| {
        s.selection_type = Some(SelectionType::Cell);
        if event.modifiers.shift {
            s.selection_focus = Some(CellCoord {
                row: row_index,
                col: col_index,
            });
        } else {
            s.selection_anchor = Some(CellCoord {
                row: row_index,
                col: col_index,
            });
            s.selection_focus = Some(CellCoord {
                row: row_index,
                col: col_index,
            });
            s.is_dragging = true;
        }
        s.selected_rows.clear();
        s.selected_columns.clear();
        s.context_menu = None;
        s.editing_cell = None;
    });
}

pub(super) fn on_row_right_mouse_down(
    state_entity: &Entity<AppState>,
    horizontal_offset: &Rc<Cell<f32>>,
    scroll_handle: &ScrollHandle,
    scrollbar_drag: &Rc<Cell<Option<bool>>>,
    row_index: usize,
    event: &MouseDownEvent,
    cx: &mut App,
) {
    if scrollbar_drag.get().is_some() {
        return;
    }
    let state = state_entity.read(cx);
    if state.has_open_menu() {
        return;
    }
    if TableView::is_in_scrollbar_hit_region(scroll_handle, &state, event.position) {
        return;
    }

    // Adjust screen x by horizontal scroll offset to get content-space x
    let x = event.position.x.as_f32() + horizontal_offset.get();
    let y = event.position.y.as_f32();
    let col_index = TableView::hit_test_col_from_content_x(&state, x);
    let already_selected = state.is_cell_selected(row_index, col_index);
    let _ = state;

    state_entity.update(cx, |s, _| {
        if !already_selected {
            s.selection_type = Some(SelectionType::Cell);
            s.selection_anchor = Some(CellCoord {
                row: row_index,
                col: col_index,
            });
            s.selection_focus = Some(CellCoord {
                row: row_index,
                col: col_index,
            });
            s.selected_rows.clear();
            s.selected_columns.clear();
        }
        s.context_menu = Some((x, y, row_index, col_index));
    });
}

pub(super) fn on_row_click(
    state_entity: &Entity<AppState>,
    horizontal_offset: &Rc<Cell<f32>>,
    row_index: usize,
    event: &ClickEvent,
    cx: &mut App,
    row_number_width: f32,
) {
    if event.click_count() < 2 {
        return;
    }

    let state = state_entity.read(cx);
    let x = event.position().x.as_f32() + horizontal_offset.get();
    let col = if x < row_number_width {
        state.selection_focus.map(|f| f.col).unwrap_or(0)
    } else {
        TableView::hit_test_col_from_content_x(&state, x)
    };
    let value = state
        .get_display_cell(row_index, col)
        .unwrap_or_default();
    let _ = state;

    state_entity.update(cx, |s, _| {
        s.editing_cell = Some((row_index, col, value));
    });
}