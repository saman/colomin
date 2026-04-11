use gpui::*;

use crate::state::{SelectionType, SortDirection};

use super::TableView;

pub(super) fn render_header(
    this: &TableView,
    cx: &mut Context<TableView>,
    row_number_width: f32,
    header_height: f32,
) -> Stateful<Div> {
    let state = this.state.read(cx);
    let colors = state.current_theme();
    let file = state.file.as_ref().expect("file should exist when rendering header");
    let columns = file.metadata.columns.clone();
    let total_w: f32 =
        row_number_width + columns.iter().map(|c| state.column_width(c.index)).sum::<f32>();

    // Horizontal offset (positive = scrolled right)
    let h_off = this.horizontal_offset.get();
    let col_resize = this.column_resize.clone();
    let col_resize_start = this.column_resize_start.clone();

    // Inner row: absolutely positioned so negative left doesn't affect parent layout
    let mut inner = div()
        .flex()
        .flex_shrink_0()
        .h_full()
        .absolute()
        .top_0()
        .left(px(-h_off))
        .w(px(total_w));
    inner = inner.child(
        div()
            .flex_shrink_0()
            .w(px(row_number_width))
            .h_full()
            .border_r_1()
            .border_color(colors.border),
    );

    for col in columns.iter() {
        let w = state.column_width(col.index);
        let ci = col.index;
        let is_sorted = state.sort_state.as_ref().map_or(false, |s| s.column_index == ci);
        let is_sel = state.selected_columns.contains(&ci);
        let mut name = col.name.clone();
        if is_sorted {
            name.push(' ');
            if state.sort_state.as_ref().expect("sort_state checked above").direction == SortDirection::Asc {
                name.push('\u{2191}');
            } else {
                name.push('\u{2193}');
            }
        }
        let tc = if is_sel {
            colors.accent
        } else {
            colors.text_secondary
        };
        let bg = if is_sel {
            colors.accent_subtle
        } else {
            colors.surface
        };
        let resize_bar_hover = colors.accent;
        let se = this.state.clone();
        let cr = col_resize.clone();
        let cr_click = col_resize.clone();
        let crs = col_resize_start.clone();
        // Resize handle: wider hit area, visible only on hover.
        // Resize handle owns the column right border; hover changes its color.
        let border_col = colors.border;
        let resize_handle = div()
            .id(ElementId::NamedInteger("rh".into(), ci as u64))
            .absolute()
            .right(px(0.))
            .top_0()
            .h_full()
            .w(px(8.0))
            .border_r_1()
            .border_color(border_col)
            .cursor_col_resize()
            .hover(move |s| s.border_r_3().border_color(resize_bar_hover))
            .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, _, _| {
                cr.set(Some(ci));
                crs.set(Some((ev.position.x.as_f32(), w)));
            });
        let hdr_cell = div()
            .id(ElementId::NamedInteger("h".into(), ci as u64))
            .relative() // needed for absolute resize handle
            .flex_shrink_0()
            .w(px(w))
            .h_full()
            .flex()
            .items_center()
            .pl(px(8.0))
            .bg(bg)
            .text_color(tc)
            .cursor_pointer()
            .truncate();
        let hdr_cell = hdr_cell.child(name);
        inner = inner.child(
            hdr_cell
                // on_mouse_down fires after child resize_handle.on_mouse_down (bubble order);
                // if column_resize is already set the click started on the resize handle.
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    if cr_click.get().is_some() {
                        return;
                    }
                    se.update(cx, |s, _| {
                        s.selected_columns.clear();
                        s.selected_columns.push(ci);
                        s.selection_type = Some(SelectionType::Column);
                        s.selection_anchor = None;
                        s.selection_focus = None;
                        s.selected_rows.clear();
                    });
                })
                .child(resize_handle),
        );
    }

    // Outer container: relative for absolute child, clips overflow
    div()
        .id("hdr")
        .flex_shrink_0()
        .h(px(header_height))
        .bg(colors.surface)
        .border_b_1()
        .border_color(colors.border)
        .text_size(px(11.0))
        .text_color(colors.text_secondary)
        .relative()
        .overflow_hidden()
        .child(inner)
}
