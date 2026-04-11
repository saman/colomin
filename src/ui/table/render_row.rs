use gpui::*;

use super::TableView;

pub(super) fn render_row_el(
    this: &TableView,
    ri: usize,
    cx: &App,
    row_number_width: f32,
    row_height: f32,
) -> Stateful<Div> {
    let state = this.state.read(cx);
    let colors = state.current_theme();
    let file = state.file.as_ref().expect("file should exist when rendering row");
    let columns = &file.metadata.columns;
    let is_row_sel = state.selected_rows.contains(&ri);
    let row_bg = if is_row_sel {
        colors.accent_subtle
    } else {
        colors.surface
    };
    let rn_color = if is_row_sel {
        colors.accent
    } else {
        colors.text_tertiary
    };
    let cached = state.get_cached_row(ri);
    let total_w: f32 = row_number_width
        + columns
            .iter()
            .map(|c| state.column_width(c.index))
            .sum::<f32>();

    // Horizontal offset (positive = scrolled right)
    let h_off = this.horizontal_offset.get();

    let hover_bg = colors.hover_row;
    // Outer row: relative positioning context, clips overflow, fixed height
    let row_outer = div()
        .id(ElementId::NamedInteger("r".into(), ri as u64))
        .h(px(row_height))
        .w_full()
        .relative()
        .overflow_hidden()
        .bg(row_bg)
        .border_b_1()
        .border_color(colors.border)
        .text_size(px(13.0))
        .text_color(colors.text_primary)
        .hover(move |s| s.bg(hover_bg));

    // Inner content: absolutely positioned so negative left doesn't affect parent layout
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
            .flex()
            .items_center()
            .justify_end()
            .pr(px(8.0))
            .border_r_1()
            .border_color(colors.border)
            .text_size(px(10.0))
            .text_color(rn_color)
            .child(format!("{}", ri + 1)),
    );

    // If data not yet cached, render placeholder cells
    if cached.is_none() {
        for col in columns.iter() {
            let w = state.column_width(col.index);
            let is_sel = state.is_cell_selected(ri, col.index);
            let cell_bg = if is_sel { colors.accent_subtle } else { row_bg };
            inner = inner.child(
                div()
                    .flex_shrink_0()
                    .w(px(w))
                    .h_full()
                    .bg(cell_bg)
                    .flex()
                    .items_center()
                    .pl(px(8.0))
                    .text_color(colors.text_tertiary)
                    .child("\u{2026}"),
            );
        }
        return row_outer.child(inner);
    }

    // Get selection range for border drawing
    let sel_range = state.selection_range();

    for col in columns.iter() {
        let w = state.column_width(col.index);
        let ci = col.index;

        let is_editing = this
            .editing
            .as_ref()
            .map_or(false, |(r, c, _)| *r == ri && *c == ci);
        let val = if is_editing {
            this.editing
                .as_ref()
                .map(|(_, _, t)| t.clone())
                .unwrap_or_default()
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

        let display = if is_editing { format!("{}|", val) } else { val };

        let mut cell = div()
            .flex_shrink_0()
            .w(px(w))
            .h_full()
            .flex()
            .items_center()
            .pl(px(8.0))
            .bg(cell_bg)
            .truncate()
            .child(display);

        if is_editing {
            cell = cell.border_1().border_color(colors.accent);
        } else if is_sel {
            if let Some((mr, xr, mc, xc)) = sel_range {
                let sel_border = colors.accent;
                let is_top = ri == mr;
                let is_bottom = ri == xr;
                let is_left = ci == mc;
                let is_right = ci == xc;

                if is_top || is_bottom || is_left || is_right {
                    let mut border_cell = div().size_full().absolute().top_0().left_0();
                    if is_top {
                        border_cell = border_cell.border_t_1();
                    }
                    if is_bottom {
                        border_cell = border_cell.border_b_1();
                    }
                    if is_left {
                        border_cell = border_cell.border_l_1();
                    }
                    if is_right {
                        border_cell = border_cell.border_r_1();
                    }
                    border_cell = border_cell.border_color(sel_border).border_dashed();
                    cell = cell.relative().child(border_cell);
                }
            }
        }

        inner = inner.child(cell);
    }
    row_outer.child(inner)
}
