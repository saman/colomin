use gpui::*;
use std::cell::Cell;
use std::rc::Rc;

use crate::state::AppState;

mod body;
mod cache;
mod clipboard;
mod editing;
mod file_commands;
mod hit_test;
mod navigation;
mod render_header;
mod render_row;
mod row_interaction;
mod scroll;
mod sorting;
mod stats;
mod undo_redo;
mod views;

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
    scroll_handle: ScrollHandle,
    /// Shared scrollbar drag state: Some(true) = vertical, Some(false) = horizontal.
    scrollbar_drag: Rc<Cell<Option<bool>>>,
    /// Offset from mouse position to thumb top/left when drag started.
    scrollbar_drag_anchor: Rc<Cell<f32>>,
    /// Manual horizontal scroll offset (positive = scrolled right).
    horizontal_offset: Rc<Cell<f32>>,
    scrollbar_initialized: bool,
    /// Column being resized (index), shared with canvas drag listeners.
    column_resize: Rc<Cell<Option<usize>>>,
    /// (start_mouse_x_in_window, start_column_width) captured on resize mouse-down.
    column_resize_start: Rc<Cell<Option<(f32, f32)>>>,
    /// Row-height resize target: None = not resizing, Some(usize::MAX) = global, Some(n) = per-row.
    row_resize: Rc<Cell<Option<usize>>>,
    /// (start_mouse_y_in_window, start_row_height) captured on resize mouse-down.
    row_resize_start: Rc<Cell<Option<(f32, f32)>>>,
}

impl TableView {
    fn hit_test_col_from_content_x(state: &AppState, x_content: f32) -> usize {
        hit_test::hit_test_col_from_content_x(state, x_content, state.row_number_width())
    }

    fn hit_test_row_from_window_y(&self, y_window: f32, total_rows: usize, cx: &App) -> Option<usize> {
        let state = self.state.read(cx);
        hit_test::hit_test_row_from_window_y(
            &self.scroll_handle,
            y_window,
            total_rows,
            HEADER_HEIGHT,
            state,
        )
    }

    fn is_in_scrollbar_hit_region(
        scroll_handle: &ScrollHandle,
        state: &AppState,
        mouse_pos: Point<Pixels>,
    ) -> bool {
        hit_test::is_in_scrollbar_hit_region(scroll_handle, state, mouse_pos)
    }

    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self {
            state,
            focus_handle: cx.focus_handle(),
            editing: None,
            needs_focus: true,
            scroll_handle: ScrollHandle::new(),
            scrollbar_drag: Rc::new(Cell::new(None)),
            scrollbar_drag_anchor: Rc::new(Cell::new(0.0)),
            horizontal_offset: Rc::new(Cell::new(0.0)),
            scrollbar_initialized: false,
            column_resize: Rc::new(Cell::new(None)),
            column_resize_start: Rc::new(Cell::new(None)),
            row_resize: Rc::new(Cell::new(None)),
            row_resize_start: Rc::new(Cell::new(None)),
        }
    }

    fn ensure_rows_cached(&self, start: usize, count: usize, cx: &mut Context<Self>) {
        cache::ensure_rows_cached(&self.state, start, count, cx);
    }

    /// Check if we need to compute stats and spawn a background task if so
    fn maybe_compute_stats(&self, cx: &mut Context<Self>) {
        stats::maybe_compute_stats(&self.state, cx);
    }

    fn maybe_apply_pending_sort(&self, cx: &mut Context<Self>) {
        sorting::maybe_apply_pending_sort(self, cx);
    }

    fn commit_edit(&mut self, cx: &mut Context<Self>) {
        editing::commit_edit(self, cx);
    }

    fn on_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        undo_redo::on_undo(self, cx);
    }

    fn on_redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        undo_redo::on_redo(self, cx);
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        editing::cancel_edit(self, cx);
    }

    /// Map a mouse position to a scroll offset and apply it (for scrollbar click/drag).
    /// This is a static helper that works with shared Rc state for use from window-level listeners.
    fn apply_scrollbar_drag(
        scrollbar_drag: &Rc<Cell<Option<bool>>>,
        scrollbar_drag_anchor: &Rc<Cell<f32>>,
        scroll_handle: &ScrollHandle,
        horizontal_offset: &Rc<Cell<f32>>,
        mouse_pos: Point<Pixels>,
        content_width: f32,
        content_height: f32,
    ) {
        scroll::apply_scrollbar_drag(
            scrollbar_drag,
            scrollbar_drag_anchor,
            scroll_handle,
            horizontal_offset,
            mouse_pos,
            content_width,
            content_height,
        );
    }

    fn start_edit_from_state(&mut self, cx: &mut Context<Self>) {
        editing::start_edit_from_state(self, cx);
    }

    fn move_selection(&mut self, dr: i32, dc: i32, cx: &mut Context<Self>) {
        editing::move_selection(self, dr, dc, cx);
    }

    fn on_move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) { self.move_selection(-1, 0, cx); }
    fn on_move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) { self.move_selection(1, 0, cx); }
    fn on_move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) { self.move_selection(0, -1, cx); }
    fn on_move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) { self.move_selection(0, 1, cx); }

    fn extend_selection(&mut self, dr: i32, dc: i32, cx: &mut Context<Self>) {
        editing::extend_selection(self, dr, dc, cx);
    }

    fn on_select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) { self.extend_selection(-1, 0, cx); }
    fn on_select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) { self.extend_selection(1, 0, cx); }
    fn on_select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) { self.extend_selection(0, -1, cx); }
    fn on_select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) { self.extend_selection(0, 1, cx); }

    fn on_enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        editing::on_enter(self, cx);
    }

    fn on_escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        navigation::on_escape(self, cx);
    }

    fn on_copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        clipboard::on_copy(self, cx);
    }

    fn on_delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        clipboard::on_delete(self, cx);
    }

    fn on_t_open_file(&mut self, _: &TOpenFile, _window: &mut Window, cx: &mut Context<Self>) {
        file_commands::on_t_open_file(self, cx);
    }

    fn on_t_save_file(&mut self, _: &TSaveFile, _window: &mut Window, cx: &mut Context<Self>) {
        file_commands::on_t_save_file(self, cx);
    }

    fn on_t_cycle_theme(&mut self, _: &TCycleTheme, _window: &mut Window, cx: &mut Context<Self>) {
        file_commands::on_t_cycle_theme(self, cx);
    }

    fn on_t_quit(&mut self, _: &TQuit, _window: &mut Window, cx: &mut Context<Self>) {
        file_commands::on_t_quit(cx);
    }

    fn handle_key_input(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        editing::handle_key_input(self, event, cx);
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rnw = self.state.read(cx).row_number_width();
        render_header::render_header(self, cx, rnw, HEADER_HEIGHT)
    }

    fn render_row_el(&self, display_ri: usize, actual_ri: Option<usize>, display_num: usize, cx: &App) -> Stateful<Div> {
        let state = self.state.read(cx);
        let row_height = state.row_height_for(display_ri);
        let rnw = state.row_number_width();
        render_row::render_row_el(self, display_ri, actual_ri, display_num, cx, rnw, row_height)
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

        if self.state.read(cx).is_loading {
            let focus_handle = self.focus_handle.clone();
            return views::render_loading(self, &focus_handle, cx);
        }

        if self.state.read(cx).file.is_none() {
            let focus_handle = self.focus_handle.clone();
            return views::render_empty(self, &focus_handle, cx);
        }

        let display_rows = self.state.read(cx).display_row_count();

        // Pick up pending edit from double-click
        self.start_edit_from_state(cx);

        // Trigger async stats computation if needed
        self.maybe_compute_stats(cx);

        // Apply pending sort requests from context menu actions.
        self.maybe_apply_pending_sort(cx);

        // Render body first — it clamps horizontal_offset based on viewport/content width.
        // Header must be rendered AFTER so it reads the clamped value.
        let rnw = self.state.read(cx).row_number_width();
        let body = body::render_body(self, display_rows, cx, rnw);
        let header = self.render_header(cx);
        let colors = self.state.read(cx).current_theme();

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
                editing::handle_drag_selection(this, ev, cx);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                // Cell selection drag release (scrollbar mouse-up is handled globally)
                this.state.update(cx, |s, _| { s.is_dragging = false; });
            }))
            // Header row (static, clipped, synced with horizontal scroll)
            .child(header)
            .child(body)
            .into_any_element()
    }
}
