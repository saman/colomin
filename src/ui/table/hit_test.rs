use gpui::{Pixels, Point, ScrollHandle};

use crate::state::AppState;

pub(super) fn hit_test_col_from_content_x(
    state: &AppState,
    x_content: f32,
    row_number_width: f32,
) -> usize {
    if state.col_count() == 0 {
        return 0;
    }
    let mut col_x = row_number_width;
    let mut col = 0usize;
    for c in 0..state.col_count() {
        let w = state.column_width(c);
        if x_content < col_x + w {
            col = c;
            break;
        }
        col_x += w;
        col = c;
    }
    col
}

pub(super) fn hit_test_row_from_window_y(
    scroll_handle: &ScrollHandle,
    y_window: f32,
    total_rows: usize,
    header_height: f32,
    state: &AppState,
) -> Option<usize> {
    if total_rows == 0 {
        return None;
    }

    let viewport_h = scroll_handle.bounds().size.height.as_f32();
    let scroll_y = -scroll_handle.offset().y.as_f32();

    if viewport_h <= 0.0 {
        return None;
    }

    // Convert from window space to table-body local y, then add scroll offset
    let local_y = (y_window - header_height).clamp(0.0, (viewport_h - 1.0).max(0.0));
    let abs_y = local_y + scroll_y;

    // Walk through rows with variable heights to find which row abs_y falls in
    let mut y_acc = 0.0;
    for ri in 0..total_rows {
        let rh = state.row_height_for(ri);
        if abs_y < y_acc + rh {
            return Some(ri);
        }
        y_acc += rh;
    }
    Some(total_rows.saturating_sub(1))
}

pub(super) fn is_in_scrollbar_hit_region(
    scroll_handle: &ScrollHandle,
    state: &AppState,
    mouse_pos: Point<Pixels>,
    row_number_width: f32,
) -> bool {
    const SCROLLBAR_SIZE: f32 = 8.0;
    const SCROLLBAR_MARGIN: f32 = 2.0;

    let bounds = scroll_handle.bounds();
    let max_y = scroll_handle.max_offset().y.as_f32();

    let vp_w = bounds.size.width.as_f32();
    let vp_h = bounds.size.height.as_f32();
    if vp_w <= 0.0 || vp_h <= 0.0 {
        return false;
    }

    let content_w = if let Some(file) = &state.file {
        row_number_width
            + file
                .metadata
                .columns
                .iter()
                .map(|c| state.column_width(c.index))
                .sum::<f32>()
    } else {
        0.0
    };
    let max_x = (content_w - vp_w).max(0.0);

    let has_v_bar = max_y > 0.0;
    let has_h_bar = max_x > 0.0;
    if !has_v_bar && !has_h_bar {
        return false;
    }

    let corner_gap = SCROLLBAR_SIZE + SCROLLBAR_MARGIN * 2.0;
    let ox = bounds.origin.x.as_f32();
    let oy = bounds.origin.y.as_f32();
    let mx = mouse_pos.x.as_f32();
    let my = mouse_pos.y.as_f32();

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

    in_v_bar || in_h_bar
}
