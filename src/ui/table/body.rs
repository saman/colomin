use gpui::*;

use super::{row_interaction, scroll, TableView};

pub(super) fn render_body(
    view: &mut TableView,
    total_rows: usize,
    cx: &mut Context<TableView>,
    row_number_width: f32,
) -> AnyElement {
    // ── Ensure layout caches are up to date (no-op if already valid) ──
    view.state.update(cx, |s, _| {
        s.ensure_row_layout();
        s.ensure_col_layout(row_number_width);
    });

    let state_ref = view.state.read(cx);
    let total_h = state_ref.row_layout.total_height;
    let content_w = state_ref.col_layout.total_width;
    let header_off = !state_ref.header_row_enabled;
    let _ = state_ref;

    // ── Scroll info from ScrollHandle ──
    let s_off = view.scroll_handle.offset();
    let s_bounds = view.scroll_handle.bounds();

    let vp_h = s_bounds.size.height.as_f32();
    let vp_w = s_bounds.size.width.as_f32();
    let off_y = s_off.y.as_f32();
    // Compute max_y directly from content height for clarity.
    let max_y = (total_h - vp_h).max(0.0);
    let scroll_y = (-off_y).clamp(0.0, max_y); // positive = how far scrolled down

    // Horizontal scroll
    let max_x = (content_w - vp_w).max(0.0);
    let h_off = view.horizontal_offset.get().clamp(0.0, max_x);
    view.horizontal_offset.set(h_off);

    // ── Compute visible row range ──
    let overdraw = if vp_h > 0.0 { vp_h * 0.5 } else { 200.0 };
    let view_top = (scroll_y - overdraw).max(0.0);
    let view_bottom = scroll_y + vp_h + overdraw;

    let state_ref = view.state.read(cx);
    let vis_start = state_ref.row_at_y(view_top, total_rows).min(total_rows);
    let vis_end = (state_ref.row_at_y(view_bottom, total_rows) + 1).min(total_rows);
    let _ = state_ref;

    // ── Cache visible rows ──
    if vis_start < vis_end {
        let (cache_start, cache_count) = if header_off {
            let ds = vis_start.saturating_sub(1);
            let de = vis_end.saturating_sub(1);
            (ds, de.saturating_sub(ds))
        } else {
            (vis_start, vis_end - vis_start)
        };
        if cache_count > 0 {
            view.ensure_rows_cached(cache_start, cache_count, cx);
        }
    }

    // ── Render visible rows absolutely positioned ──
    // Rows are placed at VIEWPORT-RELATIVE coordinates (y - scroll_y) rather than
    // document-absolute coordinates. This keeps rendered positions as small numbers
    // (0 .. vp_h) and avoids f32 precision loss at large scroll offsets (e.g. row
    // 2,000,000 → absolute y ≈ 56,000,000 px → ULP = 4 on Retina = visible gaps).
    let mut content_inner = div().relative().size_full();
    for ri in vis_start..vis_end {
        let (data_ri, y) = {
            let s = view.state.read(cx);
            (s.display_row_to_actual_row(ri), s.row_top(ri))
        };
        let viewport_y = y - scroll_y; // small number, no precision loss
        let display_num = ri + 1;

        let se = view.state.clone();
        let h_off_rc = view.horizontal_offset.clone();
        let sh_for_rows = view.scroll_handle.clone();
        let sb_drag_for_rows = view.scrollbar_drag.clone();

        let row_el = view
            .render_row_el(ri, data_ri, display_num, cx)
            .absolute()
            .top(px(viewport_y))
            .w_full()
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
                let h_off_rc = h_off_rc.clone();
                move |event, _, cx| {
                    row_interaction::on_row_click(
                        &se,
                        &h_off_rc,
                        ri,
                        event,
                        cx,
                        row_number_width,
                    );
                }
            });

        content_inner = content_inner.child(row_el);
    }

    // ── Scroll container + row overlay ──
    // The scroll_container holds a phantom div of the full content height so that
    // track_scroll can record the viewport bounds and clamp the scroll offset
    // correctly — but it does NOT contain the actual rows.
    //
    // Rows live in row_overlay (an absolute sibling) at VIEWPORT-RELATIVE positions.
    // This separates scroll-offset tracking from rendering, so the rendered row
    // positions are always small numbers (0..vp_h) with no f32 precision loss.
    let scroll_container = div()
        .id("rows")
        .size_full()
        .flex_grow()
        .overflow_hidden()
        .track_scroll(&view.scroll_handle)
        // Phantom div: gives track_scroll the correct content height for clamping.
        .child(div().h(px(total_h)).w_full());

    // Row overlay: absolutely fills the same region as scroll_container.
    // Rows inside are clipped to the viewport; no scroll transform is applied here.
    let row_overlay = div()
        .absolute()
        .top_0()
        .left_0()
        .w_full()
        .h_full()
        .overflow_hidden()
        .child(content_inner);

    // ── Scrollbar constants ──
    const SCROLLBAR_SIZE: f32 = 8.0;
    const SCROLLBAR_MIN_THUMB: f32 = 24.0;
    const SCROLLBAR_MARGIN: f32 = 2.0;
    let thumb_color = hsla(0., 0., 0.5, 0.45);

    scroll::ensure_scrollbar_initialized(&mut view.scrollbar_initialized, cx, vp_h, vp_w);

    let has_v_bar = max_y > 0.0 && vp_h > 0.0;
    let has_h_bar = max_x > 0.0 && vp_w > 0.0;
    let corner_gap = SCROLLBAR_SIZE + SCROLLBAR_MARGIN * 2.0;

    // ── Vertical scrollbar ──
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
        let drag_anchor = view.scrollbar_drag_anchor.clone();
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
                    let bar_origin_y = sh.bounds().origin.y.as_f32();
                    let click_in_track = ev.position.y.as_f32() - bar_origin_y;
                    if click_in_track >= thumb_top && click_in_track <= thumb_top + thumb_h {
                        drag_anchor.set(click_in_track - thumb_top);
                    } else {
                        drag_anchor.set(thumb_h / 2.0);
                    }
                    TableView::apply_scrollbar_drag(
                        &drag,
                        &drag_anchor,
                        &sh,
                        &h_off_rc,
                        ev.position,
                        content_w,
                        total_h,
                    );
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

    // ── Horizontal scrollbar ──
    let h_bar: Option<Stateful<Div>> = if has_h_bar {
        let h_bar_w = if has_v_bar { vp_w - corner_gap } else { vp_w };
        let thumb_w = (vp_w / content_w * h_bar_w).max(SCROLLBAR_MIN_THUMB);
        let track_w = h_bar_w - thumb_w;
        let thumb_left = if max_x > 0.0 { h_off / max_x * track_w } else { 0.0 };
        let drag = view.scrollbar_drag.clone();
        let drag_anchor = view.scrollbar_drag_anchor.clone();
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
                    let bar_origin_x = sh.bounds().origin.x.as_f32();
                    let click_in_track = ev.position.x.as_f32() - bar_origin_x;
                    if click_in_track >= thumb_left && click_in_track <= thumb_left + thumb_w {
                        drag_anchor.set(click_in_track - thumb_left);
                    } else {
                        drag_anchor.set(thumb_w / 2.0);
                    }
                    TableView::apply_scrollbar_drag(
                        &drag,
                        &drag_anchor,
                        &sh,
                        &h_off_rc,
                        ev.position,
                        content_w,
                        total_h,
                    );
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

    // ── Global window-level mouse listeners (via canvas paint callback) ──
    let drag_for_canvas = view.scrollbar_drag.clone();
    let drag_anchor_for_canvas = view.scrollbar_drag_anchor.clone();
    let sh_for_canvas = view.scroll_handle.clone();
    let h_off_for_canvas = view.horizontal_offset.clone();
    let cw_for_canvas = content_w;
    let ch_for_canvas = total_h;
    let col_resize_for_canvas = view.column_resize.clone();
    let col_resize_start_for_canvas = view.column_resize_start.clone();
    let row_resize_for_canvas = view.row_resize.clone();
    let row_resize_start_for_canvas = view.row_resize_start.clone();
    let state_for_resize = view.state.clone();
    let scrollbar_canvas = canvas(|_, _, _| {}, move |_, _, window, _| {
        // ── Scrollbar mouse-down (capture phase) ──
        let drag_down = drag_for_canvas.clone();
        let drag_anchor_down = drag_anchor_for_canvas.clone();
        let sh_down = sh_for_canvas.clone();
        let h_off_down = h_off_for_canvas.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, _| {
            if phase != DispatchPhase::Capture {
                return;
            }
            if event.button != MouseButton::Left {
                return;
            }

            let bounds = sh_down.bounds();
            let cur_off = sh_down.offset();

            let vp_w = bounds.size.width.as_f32();
            let vp_h = bounds.size.height.as_f32();
            if vp_w <= 0.0 || vp_h <= 0.0 {
                return;
            }

            let max_x = (cw_for_canvas - vp_w).max(0.0);
            let max_y = (ch_for_canvas - vp_h).max(0.0);

            let has_v_bar = max_y > 0.0;
            let has_h_bar = max_x > 0.0;
            if !has_v_bar && !has_h_bar {
                return;
            }

            const SCROLLBAR_SIZE: f32 = 8.0;
            const SCROLLBAR_MARGIN: f32 = 2.0;
            const SCROLLBAR_MIN_THUMB: f32 = 24.0;
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
                let v_bar_h = if has_h_bar { vp_h - corner_gap } else { vp_h };
                let content_h = vp_h + max_y;
                let thumb_h = (vp_h / content_h * v_bar_h).max(SCROLLBAR_MIN_THUMB);
                let track_h = v_bar_h - thumb_h;
                let scroll_pos = -cur_off.y.as_f32();
                let thumb_top = if max_y > 0.0 { scroll_pos / max_y * track_h } else { 0.0 };
                let click_in_track = my - oy;
                if click_in_track >= thumb_top && click_in_track <= thumb_top + thumb_h {
                    drag_anchor_down.set(click_in_track - thumb_top);
                } else {
                    drag_anchor_down.set(thumb_h / 2.0);
                }
                TableView::apply_scrollbar_drag(
                    &drag_down,
                    &drag_anchor_down,
                    &sh_down,
                    &h_off_down,
                    event.position,
                    cw_for_canvas,
                    ch_for_canvas,
                );
                window.refresh();
            } else if in_h_bar {
                drag_down.set(Some(false));
                let h_bar_w = if has_v_bar { vp_w - corner_gap } else { vp_w };
                let thumb_w = (vp_w / cw_for_canvas * h_bar_w).max(SCROLLBAR_MIN_THUMB);
                let track_w = h_bar_w - thumb_w;
                let cur_h_off = h_off_down.get();
                let thumb_left = if max_x > 0.0 { cur_h_off / max_x * track_w } else { 0.0 };
                let click_in_track = mx - ox;
                if click_in_track >= thumb_left && click_in_track <= thumb_left + thumb_w {
                    drag_anchor_down.set(click_in_track - thumb_left);
                } else {
                    drag_anchor_down.set(thumb_w / 2.0);
                }
                TableView::apply_scrollbar_drag(
                    &drag_down,
                    &drag_anchor_down,
                    &sh_down,
                    &h_off_down,
                    event.position,
                    cw_for_canvas,
                    ch_for_canvas,
                );
                window.refresh();
            }
        });

        // ── Column resize drag: move ──
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
                if col_idx == usize::MAX {
                    // Global column resize: update default width
                    s.default_column_width = new_w;
                } else {
                    s.column_widths.insert(col_idx, new_w);
                }
                s.invalidate_col_layout();
            });
            window.refresh();
        });

        // ── Column resize drag: up ──
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

        // ── Row resize drag: move ──
        let rr_move = row_resize_for_canvas.clone();
        let rrs_move = row_resize_start_for_canvas.clone();
        let state_rr_move = state_for_resize.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let target = match rr_move.get() {
                Some(t) => t,
                None => return,
            };
            if !event.dragging() {
                return;
            }
            let (start_y, start_h) = match rrs_move.get() {
                Some(s) => s,
                None => return,
            };
            let delta = event.position.y.as_f32() - start_y;
            let new_h = (start_h + delta).clamp(16.0, 120.0).round();
            state_rr_move.update(cx, |s, _| {
                if target == usize::MAX {
                    // Global resize: update default row height
                    s.row_height = new_h;
                } else {
                    // Per-row resize
                    s.row_heights.insert(target, new_h);
                }
                s.invalidate_row_layout();
            });
            window.refresh();
        });

        // ── Row resize drag: up ──
        let rr_up = row_resize_for_canvas.clone();
        let rrs_up = row_resize_start_for_canvas.clone();
        window.on_mouse_event(move |_event: &MouseUpEvent, phase, window, _| {
            if phase != DispatchPhase::Capture {
                return;
            }
            if rr_up.get().is_none() {
                return;
            }
            rr_up.set(None);
            rrs_up.set(None);
            window.refresh();
        });

        // ── Scrollbar drag: move ──
        let drag_move = drag_for_canvas.clone();
        let drag_anchor_move = drag_anchor_for_canvas.clone();
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
            TableView::apply_scrollbar_drag(
                &drag_move,
                &drag_anchor_move,
                &sh_move,
                &h_off_move,
                event.position,
                cw_for_canvas,
                ch_for_canvas,
            );
            window.refresh();
        });

        // ── Scrollbar drag: up ──
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

        // ── Selection drag: up ──
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

        // ── Scroll wheel: both axes (capture phase) ──
        // With overflow_hidden, GPUI's built-in scroll handler is inactive.
        // We manage horizontal (horizontal_offset) and vertical (ScrollHandle)
        // ourselves — same approach as Zed's editor.
        let h_off_scroll = h_off_for_canvas.clone();
        let sh_scroll = sh_for_canvas.clone();
        let max_x_for_scroll = max_x;
        let max_y_for_scroll = max_y;
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, _| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let (delta_x, delta_y) = match event.delta {
                ScrollDelta::Pixels(pt) => (pt.x.as_f32(), pt.y.as_f32()),
                ScrollDelta::Lines(pt) => (pt.x * 20.0, pt.y * 20.0),
            };
            if delta_x == 0.0 && delta_y == 0.0 {
                return;
            }
            let mut changed = false;

            // Horizontal
            if delta_x != 0.0 {
                let cur = h_off_scroll.get();
                let new_val = (cur - delta_x).clamp(0.0, max_x_for_scroll);
                if new_val != cur {
                    h_off_scroll.set(new_val);
                    changed = true;
                }
            }

            // Vertical
            if delta_y != 0.0 {
                let cur_y = -sh_scroll.offset().y.as_f32(); // positive = scrolled
                let new_y = (cur_y - delta_y).clamp(0.0, max_y_for_scroll);
                if new_y != cur_y {
                    sh_scroll.set_offset(point(px(0.0), px(-new_y)));
                    changed = true;
                }
            }

            if changed {
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
        .child(scroll_container)
        .child(row_overlay)
        .child(scrollbar_canvas)
        .children(v_bar)
        .children(h_bar)
        .into_any_element()
}
