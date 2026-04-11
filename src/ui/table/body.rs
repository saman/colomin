use std::ops::Range;

use gpui::*;

use super::{row_interaction, scroll, TableView};

pub(super) fn render_body(
    view: &mut TableView,
    total_rows: usize,
    cx: &mut Context<TableView>,
    row_number_width: f32,
) -> AnyElement {
    let row_list = uniform_list(
        "rows",
        total_rows,
        cx.processor(
            move |this: &mut TableView,
             range: Range<usize>,
             _: &mut Window,
             cx: &mut Context<TableView>| {
                this.ensure_rows_cached(range.start, range.end - range.start, cx);
                let mut items = Vec::with_capacity(range.end - range.start);
                for ri in range {
                    let se = this.state.clone();
                    let h_off_rc = this.horizontal_offset.clone();
                    let sh_for_rows = this.scroll_handle.clone();
                    let sb_drag_for_rows = this.scrollbar_drag.clone();
                    items.push(
                        this.render_row_el(ri, cx)
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, {
                                let se = se.clone();
                                let h_off_rc = h_off_rc.clone();
                                let sh_for_rows = sh_for_rows.clone();
                                let sb_drag_for_rows = sb_drag_for_rows.clone();
                                move |ev, _, cx| {
                                    row_interaction::on_row_left_mouse_down(
                                        &se,
                                        &h_off_rc,
                                        &sh_for_rows,
                                        &sb_drag_for_rows,
                                        ri,
                                        ev,
                                        cx,
                                        row_number_width,
                                    );
                                }
                            })
                            .on_mouse_down(MouseButton::Right, {
                                let se = se.clone();
                                let h_off_rc = h_off_rc.clone();
                                let sh_for_rows = sh_for_rows.clone();
                                let sb_drag_for_rows = sb_drag_for_rows.clone();
                                move |ev, _, cx| {
                                    row_interaction::on_row_right_mouse_down(
                                        &se,
                                        &h_off_rc,
                                        &sh_for_rows,
                                        &sb_drag_for_rows,
                                        ri,
                                        ev,
                                        cx,
                                    );
                                }
                            })
                            .on_click({
                                let se = se.clone();
                                move |event, _, cx| {
                                    row_interaction::on_row_click(&se, ri, event, cx);
                                }
                            }),
                    );
                }
                items
            },
        ),
    )
    .size_full()
    .flex_grow()
    .track_scroll(&view.scroll_handle);

    const SCROLLBAR_SIZE: f32 = 8.0;
    const SCROLLBAR_MIN_THUMB: f32 = 24.0;
    const SCROLLBAR_MARGIN: f32 = 2.0;

    let sh = view.scroll_handle.0.borrow();
    let s_off = sh.base_handle.offset();
    let s_max = sh.base_handle.max_offset();
    let s_bounds = sh.base_handle.bounds();
    drop(sh);

    let vp_h = s_bounds.size.height.as_f32();
    let vp_w = s_bounds.size.width.as_f32();
    let off_y = s_off.y.as_f32();
    let max_y = s_max.y.as_f32();

    // Horizontal scroll: computed from our manual state and column widths.
    let state_ref = view.state.read(cx);
    let content_w: f32 = if let Some(file) = &state_ref.file {
        row_number_width
            + file
                .metadata
                .columns
                .iter()
                .map(|c| state_ref.column_width(c.index))
                .sum::<f32>()
    } else {
        0.0
    };
    let _ = state_ref;
    let max_x = (content_w - vp_w).max(0.0);
    let h_off = view.horizontal_offset.get().clamp(0.0, max_x);
    view.horizontal_offset.set(h_off);

    let thumb_color = hsla(0., 0., 0.5, 0.45);

    scroll::ensure_scrollbar_initialized(&mut view.scrollbar_initialized, cx, vp_h, vp_w);

    let has_v_bar = max_y > 0.0 && vp_h > 0.0;
    let has_h_bar = max_x > 0.0 && vp_w > 0.0;
    let corner_gap = SCROLLBAR_SIZE + SCROLLBAR_MARGIN * 2.0;

    let v_bar: Option<Stateful<Div>> = if has_v_bar {
        let v_bar_h = if has_h_bar { vp_h - corner_gap } else { vp_h };
        let content_h = vp_h + max_y;
        let thumb_h = (vp_h / content_h * v_bar_h).max(SCROLLBAR_MIN_THUMB);
        let track_h = v_bar_h - thumb_h;
        let scroll_pos = -off_y;
        let thumb_top = if max_y > 0.0 {
            scroll_pos / max_y * track_h
        } else {
            0.0
        };
        let drag = view.scrollbar_drag.clone();
        let sh = view.scroll_handle.clone();
        let h_off_rc = view.horizontal_offset.clone();
        Some(
            div()
                .id("v-scrollbar")
                .absolute()
                .top(px(0.))
                .right(px(SCROLLBAR_MARGIN))
                .w(px(SCROLLBAR_SIZE))
                .h(px(v_bar_h))
                .cursor(CursorStyle::Arrow)
                .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, _, _| {
                    drag.set(Some(true));
                    TableView::apply_scrollbar_drag(&drag, &sh, &h_off_rc, ev.position, content_w);
                })
                .child(
                    div()
                        .absolute()
                        .top(px(thumb_top))
                        .w(px(SCROLLBAR_SIZE))
                        .h(px(thumb_h))
                        .rounded(px(SCROLLBAR_SIZE / 2.0))
                        .bg(thumb_color),
                ),
        )
    } else {
        None
    };

    let h_bar: Option<Stateful<Div>> = if has_h_bar {
        let h_bar_w = if has_v_bar { vp_w - corner_gap } else { vp_w };
        let thumb_w = (vp_w / content_w * h_bar_w).max(SCROLLBAR_MIN_THUMB);
        let track_w = h_bar_w - thumb_w;
        let thumb_left = if max_x > 0.0 { h_off / max_x * track_w } else { 0.0 };
        let drag = view.scrollbar_drag.clone();
        let sh = view.scroll_handle.clone();
        let h_off_rc = view.horizontal_offset.clone();
        Some(
            div()
                .id("h-scrollbar")
                .absolute()
                .bottom(px(SCROLLBAR_MARGIN))
                .left(px(0.))
                .h(px(SCROLLBAR_SIZE))
                .w(px(h_bar_w))
                .cursor(CursorStyle::Arrow)
                .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, _, _| {
                    drag.set(Some(false));
                    TableView::apply_scrollbar_drag(&drag, &sh, &h_off_rc, ev.position, content_w);
                })
                .child(
                    div()
                        .absolute()
                        .left(px(thumb_left))
                        .h(px(SCROLLBAR_SIZE))
                        .w(px(thumb_w))
                        .rounded(px(SCROLLBAR_SIZE / 2.0))
                        .bg(thumb_color),
                ),
        )
    } else {
        None
    };

    // Global window-level mouse listeners for scrollbar drag (via canvas paint callback).
    let drag_for_canvas = view.scrollbar_drag.clone();
    let sh_for_canvas = view.scroll_handle.clone();
    let h_off_for_canvas = view.horizontal_offset.clone();
    let cw_for_canvas = content_w;
    let col_resize_for_canvas = view.column_resize.clone();
    let col_resize_start_for_canvas = view.column_resize_start.clone();
    let state_for_resize = view.state.clone();
    let scrollbar_canvas = canvas(|_, _, _| {}, move |_, _, window, _| {
        let drag_down = drag_for_canvas.clone();
        let sh_down = sh_for_canvas.clone();
        let h_off_down = h_off_for_canvas.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, _| {
            if phase != DispatchPhase::Capture {
                return;
            }
            if event.button != MouseButton::Left {
                return;
            }

            let sh = sh_down.0.borrow();
            let bounds = sh.base_handle.bounds();
            let max_off = sh.base_handle.max_offset();
            drop(sh);

            let vp_w = bounds.size.width.as_f32();
            let vp_h = bounds.size.height.as_f32();
            if vp_w <= 0.0 || vp_h <= 0.0 {
                return;
            }

            let max_x = (cw_for_canvas - vp_w).max(0.0);
            let max_y = max_off.y.as_f32();

            let has_v_bar = max_y > 0.0;
            let has_h_bar = max_x > 0.0;
            if !has_v_bar && !has_h_bar {
                return;
            }

            const SCROLLBAR_SIZE: f32 = 8.0;
            const SCROLLBAR_MARGIN: f32 = 2.0;
            let corner_gap = SCROLLBAR_SIZE + SCROLLBAR_MARGIN * 2.0;

            let ox = bounds.origin.x.as_f32();
            let oy = bounds.origin.y.as_f32();
            let mx = event.position.x.as_f32();
            let my = event.position.y.as_f32();

            let in_v_bar = if has_v_bar {
                let track_top = oy;
                let track_bottom = oy + if has_h_bar { vp_h - corner_gap } else { vp_h };
                let bar_left = ox + vp_w - SCROLLBAR_MARGIN - SCROLLBAR_SIZE;
                let bar_right = ox + vp_w - SCROLLBAR_MARGIN;
                mx >= bar_left && mx <= bar_right && my >= track_top && my <= track_bottom
            } else {
                false
            };

            let in_h_bar = if has_h_bar {
                let track_left = ox;
                let track_right = ox + if has_v_bar { vp_w - corner_gap } else { vp_w };
                let bar_top = oy + vp_h - SCROLLBAR_MARGIN - SCROLLBAR_SIZE;
                let bar_bottom = oy + vp_h - SCROLLBAR_MARGIN;
                mx >= track_left && mx <= track_right && my >= bar_top && my <= bar_bottom
            } else {
                false
            };

            if in_v_bar {
                drag_down.set(Some(true));
                TableView::apply_scrollbar_drag(
                    &drag_down,
                    &sh_down,
                    &h_off_down,
                    event.position,
                    cw_for_canvas,
                );
                window.refresh();
            } else if in_h_bar {
                drag_down.set(Some(false));
                TableView::apply_scrollbar_drag(
                    &drag_down,
                    &sh_down,
                    &h_off_down,
                    event.position,
                    cw_for_canvas,
                );
                window.refresh();
            }
        });

        let cr_move = col_resize_for_canvas.clone();
        let crs_move = col_resize_start_for_canvas.clone();
        let state_move = state_for_resize.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let col_idx = match cr_move.get() {
                Some(c) => c,
                None => return,
            };
            if !event.dragging() {
                return;
            }
            let (start_x, start_w) = match crs_move.get() {
                Some(s) => s,
                None => return,
            };
            let delta = event.position.x.as_f32() - start_x;
            let new_w = (start_w + delta).max(30.0).round();
            state_move.update(cx, |s, _| {
                s.column_widths.insert(col_idx, new_w);
            });
            window.refresh();
        });

        let cr_up = col_resize_for_canvas.clone();
        let crs_up = col_resize_start_for_canvas.clone();
        window.on_mouse_event(move |_event: &MouseUpEvent, phase, window, _| {
            if phase != DispatchPhase::Capture {
                return;
            }
            if cr_up.get().is_none() {
                return;
            }
            cr_up.set(None);
            crs_up.set(None);
            window.refresh();
        });

        let drag_move = drag_for_canvas.clone();
        let sh_move = sh_for_canvas.clone();
        let h_off_move = h_off_for_canvas.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _| {
            if phase != DispatchPhase::Capture {
                return;
            }
            if drag_move.get().is_none() {
                return;
            }
            if !event.dragging() {
                return;
            }
            TableView::apply_scrollbar_drag(&drag_move, &sh_move, &h_off_move, event.position, cw_for_canvas);
            window.refresh();
        });

        let drag_up = drag_for_canvas.clone();
        window.on_mouse_event(move |_event: &MouseUpEvent, phase, window, _| {
            if phase != DispatchPhase::Capture {
                return;
            }
            if drag_up.get().is_none() {
                return;
            }
            drag_up.set(None);
            window.refresh();
        });

        let state_sel_up = state_for_resize.clone();
        window.on_mouse_event(move |_event: &MouseUpEvent, phase, _window, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let _ = state_sel_up.update(cx, |s, cx| {
                if s.is_dragging {
                    s.is_dragging = false;
                    cx.notify();
                }
            });
        });

        let h_off_scroll = h_off_for_canvas.clone();
        let max_x_for_scroll = max_x;
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, _| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let delta_x = match event.delta {
                ScrollDelta::Pixels(pt) => pt.x.as_f32(),
                ScrollDelta::Lines(pt) => pt.x * 20.0,
            };
            if delta_x == 0.0 {
                return;
            }
            let cur = h_off_scroll.get();
            let new_val = (cur - delta_x).clamp(0.0, max_x_for_scroll);
            if new_val != cur {
                h_off_scroll.set(new_val);
                window.refresh();
            }
        });
    })
    .w(px(0.))
    .h(px(0.))
    .absolute();

    div()
        .flex_grow()
        .w_full()
        .relative()
        .overflow_hidden()
        .child(row_list)
        .child(scrollbar_canvas)
        .children(v_bar)
        .children(h_bar)
        .into_any_element()
}