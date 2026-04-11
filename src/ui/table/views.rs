use gpui::*;

use super::{TOpenFile, TableView};

pub fn render_loading(
    view: &TableView,
    focus_handle: &FocusHandle,
    cx: &mut Context<TableView>,
) -> AnyElement {
    let state = view.state.read(cx);
    let filename = state.loading_message.clone();
    let bg = state.current_theme().bg;
    let text_color = state.current_theme().text_secondary;
    let accent = state.current_theme().accent;

    div()
        .id("loading-screen")
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(bg)
        .key_context("TableView")
        .track_focus(focus_handle)
        .on_key_down(cx.listener(TableView::handle_key_input))
        .flex_col()
        .gap(px(16.0))
        .child(
            svg()
                .path("assets/spinner.svg")
                .w(px(40.0))
                .h(px(40.0))
                .text_color(accent)
                .with_animation(
                    "spinner-rotate",
                    Animation::new(std::time::Duration::from_millis(800)).repeat(),
                    |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
                ),
        )
        .child(
            div()
                .id("loading-filename")
                .text_size(px(14.0))
                .text_color(text_color)
                .child(if filename.is_empty() {
                    "Loading...".to_string()
                } else {
                    filename
                }),
        )
        .into_any_element()
}

pub fn render_empty(
    view: &mut TableView,
    focus_handle: &FocusHandle,
    cx: &mut Context<TableView>,
) -> AnyElement {
    let colors = view.state.read(cx).current_theme();

    div()
        .id("empty-state")
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(colors.bg)
        .text_size(px(16.0))
        .key_context("TableView")
        .track_focus(focus_handle)
        .on_key_down(cx.listener(TableView::handle_key_input))
        .on_action(cx.listener(TableView::on_t_open_file))
        .on_action(cx.listener(TableView::on_t_quit))
        .on_action(cx.listener(TableView::on_t_cycle_theme))
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
                })),
        )
        .child(
            div()
                .text_color(colors.text_tertiary)
                .text_size(px(14.0))
                .child("or press Cmd+O"),
        )
        .into_any_element()
}