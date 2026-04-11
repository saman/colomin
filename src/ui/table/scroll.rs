use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{point, px, Context, Pixels, Point, UniformListScrollHandle};

use super::TableView;

pub(super) fn apply_scrollbar_drag(
    scrollbar_drag: &Rc<Cell<Option<bool>>>,
    scroll_handle: &UniformListScrollHandle,
    horizontal_offset: &Rc<Cell<f32>>,
    mouse_pos: Point<Pixels>,
    content_width: f32,
) {
    let is_vertical = match scrollbar_drag.get() {
        Some(v) => v,
        None => return,
    };
    let sh = scroll_handle.0.borrow();
    let bounds = sh.base_handle.bounds();
    drop(sh);

    if is_vertical {
        let sh = scroll_handle.0.borrow();
        let max_off = sh.base_handle.max_offset();
        let current = sh.base_handle.offset();
        drop(sh);
        let vp_h = bounds.size.height.as_f32();
        let max_y = max_off.y.as_f32();
        if vp_h <= 0.0 || max_y <= 0.0 {
            return;
        }
        let relative_y = mouse_pos.y.as_f32() - bounds.origin.y.as_f32();
        let ratio = (relative_y / vp_h).clamp(0.0, 1.0);
        let new_offset = point(current.x, px(-(ratio * max_y)));
        scroll_handle.0.borrow().base_handle.set_offset(new_offset);
    } else {
        let vp_w = bounds.size.width.as_f32();
        let max_x = (content_width - vp_w).max(0.0);
        if vp_w <= 0.0 || max_x <= 0.0 {
            return;
        }
        let relative_x = mouse_pos.x.as_f32() - bounds.origin.x.as_f32();
        let ratio = (relative_x / vp_w).clamp(0.0, 1.0);
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
