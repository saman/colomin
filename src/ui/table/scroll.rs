use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{point, px, Context, Pixels, Point, ScrollHandle};

use super::TableView;

const SCROLLBAR_MIN_THUMB: f32 = 24.0;

pub(super) fn apply_scrollbar_drag(
    scrollbar_drag: &Rc<Cell<Option<bool>>>,
    scrollbar_drag_anchor: &Rc<Cell<f32>>,
    scroll_handle: &ScrollHandle,
    horizontal_offset: &Rc<Cell<f32>>,
    mouse_pos: Point<Pixels>,
    content_width: f32,
    content_height: f32,
) {
    let is_vertical = match scrollbar_drag.get() {
        Some(v) => v,
        None => return,
    };
    let bounds = scroll_handle.bounds();
    let current = scroll_handle.offset();

    let vp_h = bounds.size.height.as_f32();
    let vp_w = bounds.size.width.as_f32();
    let max_y = (content_height - vp_h).max(0.0);
    let max_x = (content_width - vp_w).max(0.0);
    let anchor = scrollbar_drag_anchor.get();

    // Account for corner gap when both bars present
    let has_v_bar = max_y > 0.0;
    let has_h_bar = max_x > 0.0;
    let corner_gap = if has_v_bar && has_h_bar { 12.0 } else { 0.0 }; // SCROLLBAR_SIZE + SCROLLBAR_MARGIN * 2

    if is_vertical {
        if vp_h <= 0.0 || max_y <= 0.0 {
            return;
        }
        let bar_h = vp_h - corner_gap;
        let content_h = vp_h + max_y;
        let thumb_h = (vp_h / content_h * bar_h).max(SCROLLBAR_MIN_THUMB);
        let track_h = bar_h - thumb_h;
        let relative_y = mouse_pos.y.as_f32() - bounds.origin.y.as_f32() - anchor;
        let ratio = (relative_y / track_h).clamp(0.0, 1.0);
        let new_offset = point(current.x, px(-(ratio * max_y)));
        scroll_handle.set_offset(new_offset);
    } else {
        if vp_w <= 0.0 || max_x <= 0.0 {
            return;
        }
        let bar_w = vp_w - corner_gap;
        let thumb_w = (vp_w / content_width * bar_w).max(SCROLLBAR_MIN_THUMB);
        let track_w = bar_w - thumb_w;
        let relative_x = mouse_pos.x.as_f32() - bounds.origin.x.as_f32() - anchor;
        let ratio = (relative_x / track_w).clamp(0.0, 1.0);
        horizontal_offset.set(ratio * max_x);
    }
}

pub(super) fn ensure_scrollbar_initialized(
    scrollbar_initialized: &mut bool,
    cx: &mut Context<TableView>,
    vp_h: f32,
    vp_w: f32,
) {
    // Scroll handle bounds are 0x0 on first frame; schedule a deferred notify
    // so scrollbar geometry appears after initial layout.
    if *scrollbar_initialized || (vp_h != 0.0 && vp_w != 0.0) {
        return;
    }

    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(Duration::from_millis(100)).await;
        let _ = this.update(cx, |this, cx| {
            this.scrollbar_initialized = true;
            cx.notify();
        });
    })
    .detach();
}
