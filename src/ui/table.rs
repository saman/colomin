use std::io::{BufReader, Seek, SeekFrom};

use eframe::egui;
use egui_extras::{Column, TableBuilder};

use crate::state::{AppState, BatchEditEntry, CellCoord, EditAction, SelectionType};

pub struct TableView {
    pub editing: Option<(usize, usize, String)>,
    /// Persisted width of the cell-editor sidebar (survives open/close cycles).
    pub cell_editor_width: f32,
    /// Set to true when a double-click auto-fit changes column/row sizes, so app.rs can persist them.
    pub save_requested: bool,
    /// Index of row whose bottom edge is being dragged for resize, or usize::MAX for global resize.
    row_resize: Option<usize>,
    row_resize_start_y: f32,
    row_resize_start_h: f32,
    /// Index of data column whose right edge is being dragged for resize.
    col_resize: Option<usize>,
    col_resize_start_w: f32,
    /// Header bottom Y from the previous frame — used to offset the vertical scrollbar.
    header_bottom_y_last: f32,
    /// Column being renamed via double-click on header (col_index, buffer).
    renaming_col: Option<(usize, String)>,
    /// Column anchor for header drag-select (col index where drag started).
    col_drag_anchor: Option<usize>,
    /// Row anchor for gutter drag-select (display row index where drag started).
    row_drag_anchor: Option<usize>,
    /// Bounding rect of all visible selected cells — accumulated each frame for the dashed border.
    sel_rect: Option<egui::Rect>,
    /// Marching-ants dash phase offset, advanced each frame while a selection is active.
    dash_offset: f32,
    // Custom vertical scrollbar state (pinned to window right edge).
    v_scroll_y: f32,
    v_content_h: f32,
    v_viewport_h: f32,
    v_drag: Option<(f32, f32)>,
    v_alpha: f32,
    // Custom horizontal scrollbar state (pinned to window bottom edge).
    h_scroll_x: f32,
    h_content_w: f32,
    h_viewport_w: f32,
    h_drag: Option<(f32, f32)>,
    h_alpha: f32,
}

/// Paint a 1px horizontal line at the bottom edge of `rect` using the cell's painter.
/// Blend `fg` over `bg` at `alpha` (0–1). Produces a theme-aware separator
/// color that is visible without overpowering.
fn blend(fg: egui::Color32, bg: egui::Color32, alpha: f32) -> egui::Color32 {
    let a = alpha.clamp(0.0, 1.0);
    let b = 1.0 - a;
    egui::Color32::from_rgb(
        (fg.r() as f32 * a + bg.r() as f32 * b) as u8,
        (fg.g() as f32 * a + bg.g() as f32 * b) as u8,
        (fg.b() as f32 * a + bg.b() as f32 * b) as u8,
    )
}

fn paint_bottom_border(ui: &egui::Ui, rect: egui::Rect, color: egui::Color32) {
    let line = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.bottom() - 1.0),
        rect.max,
    );
    ui.painter().rect_filled(line, 0.0, color);
}

/// Draw a dashed rectangle border with an animated phase offset (marching ants).
/// Iteration is bounded by perimeter / period — no chance of an infinite loop
/// from f32 precision edge cases.
fn draw_dashed_rect(
    painter: &egui::Painter,
    rect: egui::Rect,
    stroke: egui::Stroke,
    dash: f32,
    gap: f32,
    offset: f32,
) {
    let period = dash + gap;
    if !rect.is_finite() || period < 1.0 || rect.width() < 2.0 || rect.height() < 2.0 {
        return;
    }
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    let phase = offset.rem_euclid(period);
    let mut accumulated = 0.0_f32;
    for pair in corners.windows(2) {
        let p0 = pair[0];
        let p1 = pair[1];
        let seg_len = (p1 - p0).length();
        if seg_len < 0.5 { continue; }
        let dir = (p1 - p0) / seg_len;
        let seg_start = accumulated;
        let seg_end = accumulated + seg_len;
        accumulated = seg_end;
        // Dash positions in absolute perimeter coords: [k*period - phase, k*period - phase + dash].
        let first_k = ((seg_start + phase) / period).floor() as i32;
        let last_k = ((seg_end + phase) / period).ceil() as i32;
        for k in first_k..=last_k {
            let ds = k as f32 * period - phase;
            let de = ds + dash;
            let s = ds.max(seg_start);
            let e = de.min(seg_end);
            if e <= s + 0.01 { continue; }
            let t0 = s - seg_start;
            let t1 = e - seg_start;
            painter.line_segment([p0 + dir * t0, p0 + dir * t1], stroke);
        }
    }
}

fn excel_col_label(mut index: usize) -> String {
    let mut s = String::new();
    loop {
        let rem = index % 26;
        s.push((b'A' + rem as u8) as char);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    s.chars().rev().collect()
}

fn col_name_for_display(state: &AppState, display_col: usize) -> String {
    let Some(f) = state.file.as_ref() else { return excel_col_label(display_col) };
    if state.header_row_enabled {
        match f.resolve_col(display_col) {
            crate::state::ColSource::Inserted(i) =>
                f.inserted_columns.get(i).cloned().unwrap_or_default(),
            crate::state::ColSource::Original(i) =>
                f.metadata.columns.iter()
                    .find(|c| c.index == i)
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| format!("Col {}", display_col + 1)),
        }
    } else {
        excel_col_label(display_col)
    }
}

fn selection_rows_and_cols(state: &AppState) -> Option<(Vec<usize>, Vec<usize>)> {
    use crate::state::SelectionType;
    match state.selection_type {
        Some(SelectionType::Cell) => {
            let (min_r, max_r, min_c, max_c) = state.selection_range()?;
            Some(((min_r..=max_r).collect(), (min_c..=max_c).collect()))
        }
        Some(SelectionType::Row) => {
            if state.selected_rows.is_empty() { return None; }
            let mut rows = state.selected_rows.clone();
            rows.sort_unstable();
            Some((rows, (0..state.col_count()).collect()))
        }
        Some(SelectionType::Column) => {
            if state.selected_columns.is_empty() { return None; }
            // Skip display row 0 when header is OFF — it is the synthetic letter-header row
            let start = if !state.header_row_enabled && state.file.is_some() { 1 } else { 0 };
            let mut cols = state.selected_columns.clone();
            cols.sort_unstable();
            Some(((start..state.display_row_count()).collect(), cols))
        }
        None => None,
    }
}

fn csv_escape(val: &str) -> String {
    if val.contains(',') || val.contains('"') || val.contains('\n') || val.contains('\r') {
        format!("\"{}\"", val.replace('"', "\"\""))
    } else {
        val.to_string()
    }
}

fn json_str(val: &str) -> String {
    let escaped = val
        .replace('\\', "\\\\")
        .replace('"',  "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{}\"", escaped)
}

fn format_selection(state: &mut AppState, mode: crate::state::CopyMode) -> String {
    use crate::state::CopyMode;
    let Some((rows, cols)) = selection_rows_and_cols(state) else { return String::new() };
    let headers: Vec<String> = cols.iter().map(|&c| col_name_for_display(state, c)).collect();

    match mode {
        CopyMode::Text => rows.iter().map(|&r|
            cols.iter()
                .map(|&c| get_cell(state, r, c))
                .collect::<Vec<_>>()
                .join("\t")
        ).collect::<Vec<_>>().join("\n"),

        CopyMode::Csv => {
            let mut out = vec![
                headers.iter().map(|h| csv_escape(h)).collect::<Vec<_>>().join(","),
            ];
            for &r in &rows {
                out.push(
                    cols.iter()
                        .map(|&c| csv_escape(&get_cell(state, r, c)))
                        .collect::<Vec<_>>()
                        .join(","),
                );
            }
            out.join("\n")
        }

        CopyMode::Json => {
            let mut objs: Vec<String> = Vec::with_capacity(rows.len());
            for &r in &rows {
                let pairs: Vec<String> = cols.iter().zip(headers.iter()).map(|(&c, h)| {
                    let v = get_cell(state, r, c);
                    format!("{}:{}", json_str(h), json_str(&v))
                }).collect();
                objs.push(format!("{{{}}}", pairs.join(",")));
            }
            format!("[{}]", objs.join(","))
        }

        CopyMode::Markdown => {
            let sep: Vec<String> = headers.iter().map(|_| "---".to_string()).collect();
            let mut lines = vec![
                format!("| {} |", headers.iter().map(|h| h.replace('|', "\\|")).collect::<Vec<_>>().join(" | ")),
                format!("| {} |", sep.join(" | ")),
            ];
            for &r in &rows {
                let cells: Vec<String> = cols.iter()
                    .map(|&c| get_cell(state, r, c).replace('|', "\\|"))
                    .collect();
                lines.push(format!("| {} |", cells.join(" | ")));
            }
            lines.join("\n")
        }
    }
}

impl TableView {
    pub fn new() -> Self {
        Self {
            editing: None,
            cell_editor_width: 300.0,
            save_requested: false,
            row_resize: None,
            row_resize_start_y: 0.0,
            row_resize_start_h: 0.0,
            col_resize: None,
            col_resize_start_w: 0.0,
            header_bottom_y_last: 0.0,
            renaming_col: None,
            col_drag_anchor: None,
            row_drag_anchor: None,
            sel_rect: None,
            dash_offset: 0.0,
            v_scroll_y: 0.0,
            v_content_h: 0.0,
            v_viewport_h: 0.0,
            v_drag: None,
            v_alpha: 0.0,
            h_scroll_x: 0.0,
            h_content_w: 0.0,
            h_viewport_w: 0.0,
            h_drag: None,
            h_alpha: 0.0,
        }
    }

    pub fn is_resizing(&self) -> bool {
        self.col_resize.is_some() || self.row_resize.is_some()
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut AppState, ctx: &egui::Context) {
        let Some(_) = state.file else { return };

        // Reset selection bounding rect; it's re-accumulated during cell rendering.
        self.sel_rect = None;

        self.handle_keyboard(state, ctx);

        // ── Custom scrollbar geometry (vertical pinned to right, horizontal pinned to bottom) ──
        let panel_rect = ui.max_rect();
        let scroll_spc = ui.spacing().scroll;
        let scroll_w = scroll_spc.bar_width + scroll_spc.bar_inner_margin + scroll_spc.bar_outer_margin;
        let scroll_h = scroll_w; // use same thickness for both axes
        // Table fills the entire panel — scrollbars are overlaid on top (macOS style).
        let _table_rect = panel_rect;
        // Interaction zones for the overlay scrollbars (right strip and bottom strip).
        // The vertical bar starts below the sticky header so the thumb never overlaps it.
        let v_bar_top = if self.header_bottom_y_last > 0.0 {
            self.header_bottom_y_last
        } else {
            panel_rect.min.y
        };
        let bar_rect = egui::Rect::from_min_max(
            egui::pos2(panel_rect.max.x - scroll_w, v_bar_top),
            egui::pos2(panel_rect.max.x, panel_rect.max.y - scroll_h),
        );
        // Gutter width needed early for h_bar_rect.
        let gutter_width_early = state.row_number_width();
        let h_bar_rect = egui::Rect::from_min_max(
            egui::pos2(panel_rect.min.x + gutter_width_early, panel_rect.max.y - scroll_h),
            egui::pos2(panel_rect.max.x - scroll_w, panel_rect.max.y),
        );
        let v_content_h = self.v_content_h;
        let v_viewport_h = self.v_viewport_h;
        let max_scroll = (v_content_h - v_viewport_h).max(0.0);
        let track_h = bar_rect.height();
        let thumb_h = if v_content_h > 0.0 {
            (v_viewport_h / v_content_h * track_h).max(24.0).min(track_h)
        } else { track_h };

        // Apply vertical drag that started in a previous frame.
        let mut drag_scroll_y: Option<f32> = None;
        if let Some((start_cursor_y, start_scroll_y)) = self.v_drag {
            if ui.ctx().input(|i| i.pointer.button_down(egui::PointerButton::Primary)) {
                if let Some(pos) = ui.ctx().pointer_latest_pos() {
                    let track_scroll_h = (track_h - thumb_h).max(1.0);
                    let new_y = (start_scroll_y + (pos.y - start_cursor_y) / track_scroll_h * max_scroll)
                        .clamp(0.0, max_scroll);
                    drag_scroll_y = Some(new_y);
                }
            } else {
                self.v_drag = None;
            }
        }

        // Horizontal drag state.
        let h_content_w = self.h_content_w;
        let h_viewport_w = self.h_viewport_w;
        let h_max_scroll = (h_content_w - h_viewport_w).max(0.0);
        let h_track_w = h_bar_rect.width();
        let h_thumb_w = if h_content_w > 0.0 {
            (h_viewport_w / h_content_w * h_track_w).max(24.0).min(h_track_w)
        } else { h_track_w };

        let mut h_drag_scroll_x: Option<f32> = None;
        if let Some((start_cursor_x, start_scroll_x)) = self.h_drag {
            if ui.ctx().input(|i| i.pointer.button_down(egui::PointerButton::Primary)) {
                if let Some(pos) = ui.ctx().pointer_latest_pos() {
                    let track_scroll_w = (h_track_w - h_thumb_w).max(1.0);
                    let new_x = (start_scroll_x + (pos.x - start_cursor_x) / track_scroll_w * h_max_scroll)
                        .clamp(0.0, h_max_scroll);
                    h_drag_scroll_x = Some(new_x);
                }
            } else {
                self.h_drag = None;
            }
        }

        let colors = state.current_theme();
        let row_count = state.display_row_count();
        let col_count = state.col_count();
        let gutter_width = state.row_number_width();
        // Gutter is sticky — the scrollable area starts right of it.
        let data_rect = egui::Rect::from_min_max(
            egui::pos2(panel_rect.left() + gutter_width, panel_rect.top()),
            panel_rect.max,
        );

        let header_bg  = colors.gutter_bg;
        let gutter_bg  = colors.gutter_bg;
        let line_num   = colors.line_number;
        let text_pri   = colors.text_primary;
        let text_sec   = colors.text_secondary;
        let sel_color  = colors.accent_subtle;
        let edit_color = colors.edited;
        let surf       = colors.surface;
        let accent          = colors.accent;
        let border          = colors.border;
        let hover_row_color = colors.hover_row;
        let header_sep = border;
        let gutter_sep = border;

        let header_enabled = state.header_row_enabled;
        let col_names: Vec<String> = if let Some(f) = state.file.as_ref() {
            let n = f.current_col_count();
            (0..n).map(|display_col| {
                if header_enabled {
                    match f.resolve_col(display_col) {
                        crate::state::ColSource::Inserted(ins_idx) =>
                            f.inserted_columns.get(ins_idx).cloned().unwrap_or_default(),
                        crate::state::ColSource::Original(orig_idx) =>
                            f.metadata.columns.iter()
                                .find(|c| c.index == orig_idx)
                                .map(|c| c.name.clone())
                                .unwrap_or_else(|| format!("Col {}", display_col + 1)),
                    }
                } else {
                    excel_col_label(display_col)
                }
            }).collect()
        } else {
            Vec::new()
        };

        // Sort state for header arrows
        let sort_col = state.sort_state.as_ref().map(|s| s.column_index);
        let sort_asc = state.sort_state.as_ref().map(|s| {
            matches!(s.direction, crate::state::SortDirection::Asc)
        });

        // Content width covers only data columns — gutter is outside the scroll area.
        let total_width: f32 = (0..col_count).map(|c| state.column_width(c)).sum::<f32>()
            + 8.0; // small right padding


        // Cells capture values across closure boundaries without &mut conflicts.
        let header_bottom_y = std::cell::Cell::new(0.0_f32);
        let header_left_x   = std::cell::Cell::new(f32::MAX);
        let header_right_x  = std::cell::Cell::new(0.0_f32);
        // Header column rects captured during header rendering for the overlay repaint.
        let header_col_rects: std::cell::Cell<Vec<(egui::Rect, usize)>> = std::cell::Cell::new(Vec::new());
        // Which column's resize handle is hovered this frame (for overlay painting).
        let col_resize_hovered: std::cell::Cell<Option<usize>> = std::cell::Cell::new(None);
        // Selection rect accumulated during body rendering; drawn after .body().
        let sel_rect_acc    = std::cell::Cell::<Option<egui::Rect>>::new(None);
        let current_dash    = self.dash_offset;

        // Single-cell selection gets a solid border; multi-cell gets marching ants.
        let is_single_cell = match state.selection_type {
            Some(SelectionType::Cell) => state
                .selection_range()
                .map(|(mr, xr, mc, xc)| mr == xr && mc == xc)
                .unwrap_or(false),
            _ => false,
        };

        // Cells to capture scroll state from inside the body closure.
        let scroll_y_out    = std::cell::Cell::new(self.v_scroll_y);
        let content_h_out   = std::cell::Cell::new(self.v_content_h);
        let viewport_h_out  = std::cell::Cell::new(self.v_viewport_h);

        // Precompute cell-selection range for border highlights (gutter right, header bottom).
        let cell_sel_range = if state.selection_type == Some(SelectionType::Cell) {
            state.selection_range()
        } else {
            None
        };

        // Table renders in the data rect (gutter is sticky, painted separately).
        let mut table_ui = ui.new_child(egui::UiBuilder::new().max_rect(data_rect));

        let mut h_sa = egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
        if let Some(dx) = h_drag_scroll_x {
            h_sa = h_sa.scroll_offset(egui::vec2(dx, 0.0));
        }
        let h_scroll_out = h_sa.show(&mut table_ui, |ui| {
                ui.set_min_width(total_width);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                // Suppress all vertical column separators — resize is handled manually
                // via header-only handles (like row resize).
                ui.visuals_mut().widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
                ui.visuals_mut().widgets.hovered.bg_stroke = egui::Stroke::NONE;
                ui.visuals_mut().widgets.active.bg_stroke = egui::Stroke::NONE;

                let mut table = TableBuilder::new(ui)
                    .striped(false)
                    .resizable(false)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .min_scrolled_height(0.0)
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);

                for col in 0..col_count {
                    let w = state.column_width(col);
                    table = table.column(Column::initial(w).at_least(40.0).resizable(false));
                }

                if let Some(dy) = drag_scroll_y {
                    table = table.vertical_scroll_offset(dy);
                }

                // ── Header ──
                let body_out = table
            .header(30.0, |mut header| {
                for (col_idx, name) in col_names.iter().enumerate() {
                    header.col(|ui| {
                        let rect = ui.max_rect();
                        // Capture bottom Y and leftmost X from the first column.
                        if col_idx == 0 {
                            header_bottom_y.set(rect.bottom());
                            if rect.left() < header_left_x.get() { header_left_x.set(rect.left()); }
                        }
                        // Capture rect for the header overlay repaint.
                        { let mut v = header_col_rects.take(); v.push((rect, col_idx)); header_col_rects.set(v); }
                        // Highlight header when the column is selected.
                        let col_selected = state.selection_type == Some(SelectionType::Column)
                            && state.selected_columns.contains(&col_idx);
                        let effective_hdr_bg = if col_selected { sel_color } else { header_bg };
                        ui.painter().rect_filled(rect, 0.0, effective_hdr_bg);
                        if rect.right() > header_right_x.get() { header_right_x.set(rect.right()); }
                        let _is_sorted = sort_col == Some(col_idx); // superseded by is_sorted_here below
                        let is_renaming = self
                            .renaming_col
                            .as_ref()
                            .map(|(c, _)| *c == col_idx)
                            .unwrap_or(false);

                        if is_renaming {
                            // Render a TextEdit overlay; renaming only possible when header_row_enabled.
                            ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
                            let buf = &mut self.renaming_col.as_mut().unwrap().1;
                            let te = egui::TextEdit::singleline(buf)
                                .font(egui::FontId::proportional(state.font_size))
                                .desired_width(rect.width() - 8.0);
                            let resp = ui.add(te);
                            if !resp.has_focus() {
                                resp.request_focus();
                            }
                            let enter   = ctx.input(|i| i.key_pressed(egui::Key::Enter));
                            let escaped = ctx.input(|i| i.key_pressed(egui::Key::Escape));
                            let commit  = (resp.lost_focus() || enter) && !escaped;
                            let _ = buf;
                            if commit {
                                if let Some((c, new_name)) = self.renaming_col.take() {
                                    rename_column(state, c, new_name);
                                }
                            } else if escaped {
                                self.renaming_col = None;
                            }
                        } else {
                            let phys_col = state.display_to_physical_col(col_idx);
                            let is_sorted_here = sort_col == Some(phys_col);
                            let color = if is_sorted_here { accent } else { text_sec };
                            // Reserve the right 5px for the resize handle so it gets
                            // exclusive hover/drag priority (no rect overlap).
                            let hdr_id = egui::Id::new(("hdr", col_idx as u64));
                            let click_rect = egui::Rect::from_min_max(
                                rect.min,
                                egui::pos2(rect.max.x - 5.0, rect.max.y),
                            );
                            let resp = ui.interact(click_rect, hdr_id, egui::Sense::click_and_drag());

                            // Drag-select columns.
                            if resp.drag_started() {
                                if let Some((er, ec, ev)) = self.editing.take() {
                                    commit_edit(state, er, ec, ev);
                                }
                                self.col_drag_anchor = Some(col_idx);
                                state.selection_type = Some(SelectionType::Column);
                                state.selected_columns = vec![col_idx];
                                state.selected_rows.clear();
                                state.selection_anchor = None;
                                state.selection_focus = None;
                            }
                            if self.col_drag_anchor.is_some()
                                && ctx.input(|i| i.pointer.button_down(egui::PointerButton::Primary))
                            {
                                if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
                                    if click_rect.x_range().contains(pos.x) {
                                        let anchor = self.col_drag_anchor.unwrap();
                                        let lo = anchor.min(col_idx);
                                        let hi = anchor.max(col_idx);
                                        state.selected_columns = (lo..=hi).collect();
                                        state.selection_type = Some(SelectionType::Column);
                                    }
                                }
                            }
                            if resp.drag_stopped() {
                                self.col_drag_anchor = None;
                            }

                            // Text label — leave room for sort icon on the right.
                            let label_max_x = if is_sorted_here { rect.right() - 20.0 } else { rect.right() - 4.0 };
                            let galley = ui.painter().layout_no_wrap(
                                name.clone(),
                                egui::FontId::proportional(state.font_size),
                                color,
                            );
                            let text_pos = egui::pos2(rect.left() + 6.0, rect.center().y - galley.size().y * 0.5);
                            // Clip text so it doesn't overlap the sort icon.
                            let clip = egui::Rect::from_min_max(rect.min, egui::pos2(label_max_x, rect.max.y));
                            ui.painter().with_clip_rect(clip).galley(text_pos, galley, color);

                            // Sort icon painted as SVG (no glyph needed).
                            if is_sorted_here {
                                let icon_name = if sort_asc == Some(true) { "sort-asc" } else { "sort-desc" };
                                let icon_rect = egui::Rect::from_center_size(
                                    egui::pos2(rect.right() - 11.0, rect.center().y),
                                    egui::vec2(14.0, 14.0),
                                );
                                crate::ui::icons::icon(icon_name, color).paint_at(ui, icon_rect);
                            }

                            if resp.double_clicked() && header_enabled {
                                if let Some((er, ec, ev)) = self.editing.take() {
                                    commit_edit(state, er, ec, ev);
                                }
                                self.renaming_col = Some((col_idx, name.clone()));
                            } else if resp.clicked() {
                                if let Some((er, ec, ev)) = self.editing.take() {
                                    commit_edit(state, er, ec, ev);
                                }
                                let shift = ctx.input(|i| i.modifiers.shift);
                                state.selection_type = Some(SelectionType::Column);
                                if shift {
                                    if !state.selected_columns.contains(&col_idx) {
                                        state.selected_columns.push(col_idx);
                                    }
                                } else {
                                    state.selected_columns.clear();
                                    state.selected_columns.push(col_idx);
                                    state.selected_rows.clear();
                                    state.selection_anchor = None;
                                    state.selection_focus = None;
                                }
                            }

                            // Right-click context menu on column header.
                            resp.context_menu(|ui| {
                                ui.set_min_width(140.0);
                                ui.set_max_width(160.0);
                                let ic = colors.text_secondary;
                                let tc = colors.text_primary;
                                if icon_menu_item(ui, "insert-column-left", "Insert Column Left", ic, tc, false).clicked() {
                                    insert_col(state, col_idx);
                                    ui.close();
                                }
                                if icon_menu_item(ui, "insert-column-right", "Insert Column Right", ic, tc, false).clicked() {
                                    insert_col(state, col_idx + 1);
                                    ui.close();
                                }
                                ui.separator();
                                let is_asc = sort_asc == Some(true) && is_sorted_here;
                                if icon_menu_item(ui, "sort-asc", "Sort Ascending",
                                    if is_sorted_here && is_asc { accent } else { ic }, tc,
                                    is_sorted_here && is_asc).clicked()
                                {
                                    state.pending_sort = Some((phys_col, true));
                                    ui.close();
                                }
                                if icon_menu_item(ui, "sort-desc", "Sort Descending",
                                    if is_sorted_here && !is_asc { accent } else { ic }, tc,
                                    is_sorted_here && !is_asc).clicked()
                                {
                                    state.pending_sort = Some((phys_col, false));
                                    ui.close();
                                }
                                ui.separator();
                                if col_idx > 0 {
                                    if icon_menu_item(ui, "chevron-left", "Move Left", ic, tc, false).clicked() {
                                        move_selected_columns(state, col_idx, -1, col_count);
                                        ui.close();
                                    }
                                }
                                if col_idx + 1 < col_count {
                                    if icon_menu_item(ui, "chevron-right", "Move Right", ic, tc, false).clicked() {
                                        move_selected_columns(state, col_idx, 1, col_count);
                                        ui.close();
                                    }
                                }
                                if header_enabled {
                                    ui.separator();
                                    if icon_menu_item(ui, "pen", "Rename", ic, tc, false).clicked() {
                                        if let Some((er, ec, ev)) = self.editing.take() {
                                            commit_edit(state, er, ec, ev);
                                        }
                                        self.renaming_col = Some((col_idx, name.clone()));
                                        ui.close();
                                    }
                                }
                                ui.separator();
                                let danger = colors.danger;
                                if icon_menu_item(ui, "delete-column", "Delete Column", danger, danger, false).clicked() {
                                    delete_col(state, col_idx);
                                    ui.close();
                                }
                            });
                        }

                        // ── Column resize handle (right 5px, no overlap with click_rect) ──
                        let handle_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.max.x - 5.0, rect.min.y),
                            rect.max,
                        );
                        let ch_id = egui::Id::new(("col_resize_handle", col_idx as u64));
                        let handle_resp = ui.interact(handle_rect, ch_id, egui::Sense::click_and_drag());
                        let handle_active = handle_resp.hovered()
                            || handle_resp.dragged()
                            || self.col_resize == Some(col_idx);
                        if handle_active {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                            col_resize_hovered.set(Some(col_idx));
                        }
                        if handle_resp.drag_started() {
                            self.col_resize = Some(col_idx);
                            self.col_resize_start_w = state.column_width(col_idx);
                        }
                        if handle_resp.dragged() && self.col_resize == Some(col_idx) {
                            let dx = handle_resp.drag_delta().x;
                            let new_w = (self.col_resize_start_w + dx).max(40.0);
                            self.col_resize_start_w = new_w;
                            state.column_widths.insert(col_idx, new_w);
                            state.invalidate_col_layout();
                        }
                        if handle_resp.drag_stopped() {
                            self.col_resize = None;
                        }
                        // Double-click the resize handle → auto-fit width to content.
                        if handle_resp.double_clicked() {
                            let font_id = egui::FontId::proportional(state.font_size);
                            // Minimum: the column header name.
                            let col_label = col_name_for_display(state, col_idx);
                            let mut best_w = ui.fonts(|f| {
                                f.layout_no_wrap(col_label, font_id.clone(), egui::Color32::WHITE).size().x
                            }) + 28.0; // header padding + resize-handle zone

                            // Scan rows: load uncached ones on the fly (with a cap for large files).
                            let total = state.display_row_count();
                            let scan_limit = total.min(2000);
                            for display_row in 0..scan_limit {
                                // Only read from disk if not already cached.
                                if let Some(ar) = state.display_row_to_actual_row(display_row) {
                                    if !state.row_cache.contains_key(&ar) {
                                        load_row(state, display_row);
                                    }
                                }
                                let Some(val) = state.get_display_cell(display_row, col_idx)
                                    else { continue };
                                if val.is_empty() { continue; }
                                let w = ui.fonts(|f| {
                                    f.layout_no_wrap(val, font_id.clone(), egui::Color32::WHITE).size().x
                                }) + 12.0; // cell l+r padding
                                if w > best_w { best_w = w; }
                            }

                            state.column_widths.insert(col_idx, best_w.max(40.0));
                            state.invalidate_col_layout();
                            self.save_requested = true;
                        }
                    });
                }
            })
            // ── Body ──
            .body(|body| {
                // Use heterogeneous_rows so per-row heights work
                let heights: Vec<f32> = (0..row_count)
                    .map(|r| state.row_height_for(r))
                    .collect();
                body.heterogeneous_rows(heights.into_iter(), |mut row| {
                    let display_row = row.index();
                    let _row_h = state.row_height_for(display_row);

                    let row_sel = state.selection_type == Some(SelectionType::Row)
                        && state.selected_rows.contains(&display_row);

                    // ── Data cells ──
                    for col_idx in 0..col_count {
                        row.col(|ui| {
                            let cell_sel = state.is_cell_selected(display_row, col_idx);
                            let is_cursor = state.selection_anchor
                                .map(|a| a.row == display_row && a.col == col_idx)
                                .unwrap_or(false)
                                && state.selection_type == Some(SelectionType::Cell);
                            let is_editing = self.editing.as_ref()
                                .map(|(r, c, _)| *r == display_row && *c == col_idx)
                                .unwrap_or(false);
                            let has_edit = state.file.as_ref()
                                .map(|f| f.edits.contains_key(&(display_row, col_idx)))
                                .unwrap_or(false);

                            let row_hov = state.row_highlight_on_hover && !row_sel
                                && ctx.input(|i| i.pointer.hover_pos()
                                    .map(|p| ui.max_rect().y_range().contains(p.y) && panel_rect.contains(p))
                                    .unwrap_or(false));
                            let bg = if cell_sel || row_sel { sel_color }
                                     else if row_hov { hover_row_color }
                                     else if has_edit { edit_color }
                                     else { surf };

                            let col_resize_line = self.col_resize == Some(col_idx);

                            if is_editing {
                                // In-place editing: same background and border as a normal cell.
                                let edit_rect = ui.max_rect();
                                ui.painter().rect_filled(edit_rect, 0.0, bg);
                                let bottom_color = if self.row_resize == Some(display_row) { accent } else { border };
                                paint_bottom_border(ui, edit_rect, bottom_color);
                                if col_resize_line {
                                    ui.painter().rect_filled(
                                        egui::Rect::from_min_max(
                                            egui::pos2(edit_rect.right() - 1.0, edit_rect.min.y),
                                            edit_rect.max,
                                        ),
                                        0.0,
                                        accent,
                                    );
                                }
                                // Restore spacing so the TextEdit renders at normal height.
                                ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
                                let desired_w = ui.available_width() - 4.0;
                                let buf = &mut self.editing.as_mut().unwrap().2;
                                // Stable explicit ID prevents ID churn when the editing cell
                                // changes row/col, which would otherwise drop focus.
                                let edit_id = egui::Id::new(("edit", display_row as u64, col_idx as u64));
                                let te = egui::TextEdit::singleline(buf)
                                    .id(edit_id)
                                    .font(egui::FontId::proportional(state.font_size))
                                    .desired_width(desired_w)
                                    .frame(false)
                                    .margin(egui::Margin::symmetric(4, 0));
                                let resp = ui.add(te);
                                // Only request focus when not yet focused; calling every
                                // frame interferes with lost_focus() detection.
                                if !resp.has_focus() {
                                    resp.request_focus();
                                }
                                let enter   = ctx.input(|i| i.key_pressed(egui::Key::Enter));
                                let tab     = ctx.input(|i| i.key_pressed(egui::Key::Tab));
                                let escaped = ctx.input(|i| i.key_pressed(egui::Key::Escape));
                                // Commit on Enter/Tab OR when egui surrenders focus (click-away).
                                let commit  = (resp.lost_focus() || enter || tab) && !escaped;
                                let _ = buf; // end borrow before take()
                                if commit {
                                    if let Some((r, c, new_val)) = self.editing.take() {
                                        commit_edit(state, r, c, new_val);
                                        // Move selection: Enter → down, Tab → right
                                        let total_rows = state.display_row_count();
                                        let total_cols = state.col_count();
                                        let next = if enter && r + 1 < total_rows {
                                            Some(CellCoord { row: r + 1, col: c })
                                        } else if tab {
                                            let nc = c + 1;
                                            let (nr, nc) = if nc < total_cols {
                                                (r, nc)
                                            } else {
                                                ((r + 1).min(total_rows.saturating_sub(1)), 0)
                                            };
                                            Some(CellCoord { row: nr, col: nc })
                                        } else {
                                            None
                                        };
                                        if let Some(coord) = next {
                                            state.selection_anchor = Some(coord);
                                            state.selection_focus  = Some(coord);
                                            state.selection_type   = Some(SelectionType::Cell);
                                            state.selected_rows.clear();
                                            state.selected_columns.clear();
                                        }
                                    }
                                } else if escaped {
                                    self.editing = None;
                                }
                            } else {
                                // Allocate the interaction rect FIRST with a stable explicit ID
                                // before any painting — this is the correct egui pattern and
                                // ensures hit-testing works reliably inside table cells.
                                let cell_id = egui::Id::new(("cell", display_row as u64, col_idx as u64));
                                let resp = ui.interact(ui.max_rect(), cell_id, egui::Sense::click_and_drag());

                                // Paint after interaction registration.
                                ui.painter().rect_filled(resp.rect, 0.0, bg);
                                let bottom_color = if self.row_resize == Some(display_row) { accent } else { border };
                                paint_bottom_border(ui, resp.rect, bottom_color);
                                if col_resize_line {
                                    ui.painter().rect_filled(
                                        egui::Rect::from_min_max(
                                            egui::pos2(resp.rect.right() - 1.0, resp.rect.min.y),
                                            resp.rect.max,
                                        ),
                                        0.0,
                                        accent,
                                    );
                                }
                                let _ = is_cursor;
                                let value = get_cell(state, display_row, col_idx);
                                let cell_clip = egui::Rect::from_min_max(
                                    egui::pos2(resp.rect.left() + 4.0, resp.rect.min.y),
                                    egui::pos2(resp.rect.right() - 4.0, resp.rect.max.y),
                                );
                                ui.painter().with_clip_rect(cell_clip).text(
                                    egui::pos2(resp.rect.left() + 4.0, resp.rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    &value,
                                    egui::FontId::proportional(state.font_size),
                                    text_pri,
                                );

                                // Accumulate selected cells for the marching-ants border.
                                if cell_sel {
                                    sel_rect_acc.set(Some(match sel_rect_acc.get() {
                                        Some(r) => r.union(resp.rect),
                                        None    => resp.rect,
                                    }));
                                }

                                if resp.double_clicked() {
                                    // Commit any edit on another cell before starting a new one.
                                    if let Some((er, ec, ev)) = self.editing.take() {
                                        commit_edit(state, er, ec, ev);
                                    }
                                    self.editing = Some((display_row, col_idx, value.clone()));
                                    state.selection_type = Some(SelectionType::Cell);
                                    state.selection_anchor = Some(CellCoord { row: display_row, col: col_idx });
                                    state.selection_focus = Some(CellCoord { row: display_row, col: col_idx });
                                } else if resp.drag_started() {
                                    // Commit any pending edit — ui.interact() never steals keyboard
                                    // focus from the TextEdit, so lost_focus() won't fire on its own.
                                    if let Some((er, ec, ev)) = self.editing.take() {
                                        commit_edit(state, er, ec, ev);
                                    }
                                    state.is_dragging = true;
                                    state.selection_type = Some(SelectionType::Cell);
                                    state.selection_anchor = Some(CellCoord { row: display_row, col: col_idx });
                                    state.selection_focus = Some(CellCoord { row: display_row, col: col_idx });
                                    state.selected_rows.clear();
                                    state.selected_columns.clear();
                                } else if resp.clicked() {
                                    // Same: commit before changing selection.
                                    if let Some((er, ec, ev)) = self.editing.take() {
                                        commit_edit(state, er, ec, ev);
                                    }
                                    let shift = ctx.input(|i| i.modifiers.shift);
                                    if !shift {
                                        state.selection_type = Some(SelectionType::Cell);
                                        state.selection_anchor = Some(CellCoord { row: display_row, col: col_idx });
                                        state.selection_focus = Some(CellCoord { row: display_row, col: col_idx });
                                        state.selected_rows.clear();
                                        state.selected_columns.clear();
                                    } else if state.selection_anchor.is_some() {
                                        state.selection_focus = Some(CellCoord { row: display_row, col: col_idx });
                                        state.selection_type = Some(SelectionType::Cell);
                                    }
                                }

                                // Extend selection while dragging.
                                // NOTE: egui locks pointer events to the widget that started the
                                // drag, so resp.contains_pointer() returns false for other cells.
                                // We use the raw hover position to detect which cell the mouse
                                // is currently over, bypassing the egui widget ownership system.
                                if state.is_dragging
                                    && ctx.input(|i| i.pointer.button_down(egui::PointerButton::Primary))
                                {
                                    if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
                                        if resp.rect.contains(pos) {
                                            state.selection_focus = Some(CellCoord { row: display_row, col: col_idx });
                                            state.selection_type = Some(SelectionType::Cell);
                                        }
                                    }
                                }

                                resp.context_menu(|ui| {
                                    ui.set_min_width(140.0);
                                    ui.set_max_width(160.0);
                                    let ic = colors.text_secondary;
                                    let tc = colors.text_primary;
                                    if icon_menu_item(ui, "pen", "Edit Cell", ic, tc, false).clicked() {
                                        state.cell_editor = Some((display_row, col_idx, value.clone()));
                                        ui.close();
                                    }
                                    ui.separator();
                                    if icon_menu_item(ui, "copy", "Copy", ic, tc, false).clicked() {
                                        ui.ctx().copy_text(value.clone());
                                        ui.close();
                                    }
                                    if icon_menu_item(ui, "edit", "Clear Cell", ic, tc, false).clicked() {
                                        commit_edit(state, display_row, col_idx, String::new());
                                        ui.close();
                                    }
                                    ui.separator();
                                    if icon_menu_item(ui, "sort-asc", "Sort Ascending", ic, tc, false).clicked() {
                                        state.pending_sort = Some((col_idx, true));
                                        ui.close();
                                    }
                                    if icon_menu_item(ui, "sort-desc", "Sort Descending", ic, tc, false).clicked() {
                                        state.pending_sort = Some((col_idx, false));
                                        ui.close();
                                    }
                                    ui.separator();
                                    if icon_menu_item(ui, "chevron-up", "Reset Row Height", ic, tc, false).clicked() {
                                        state.row_heights.remove(&display_row);
                                        state.invalidate_row_layout();
                                        ui.close();
                                    }
                                });
                            }
                        });
                    }
                });
            }); // closes .body() — body_out captured above

                // Capture scroll state for the custom scrollbar.
                scroll_y_out.set(body_out.state.offset.y);
                content_h_out.set(body_out.content_size.y);
                viewport_h_out.set(body_out.inner_rect.height());

                // Repaint the full header as a Foreground overlay so body rows
                // scrolling into the header area are always covered.
                let y = header_bottom_y.get();
                // Extend l all the way to the panel left so the gutter header area is
                // covered by the overlay (gutter is outside the scroll area now).
                let l = panel_rect.left();
                // Clamp r to the viewport's right edge so the header overlay never
                // bleeds into the cell-editor sidebar (or any other right-side panel).
                let r = header_right_x.get().min(ui.clip_rect().right());
                if y > 0.0 && r > l {
                    let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::new("header_overlay"));
                    let clip = ui.clip_rect();
                    // Clip strictly to the header strip so this layer can never
                    // cover context menus or popups that appear below the header.
                    let header_clip = egui::Rect::from_min_max(
                        egui::pos2(l, clip.min.y),
                        egui::pos2(r, y),
                    );
                    let painter = ui.ctx().layer_painter(layer).with_clip_rect(header_clip);

                    // Repaint each column's background and name on top.
                    // Skip the column being renamed — its TextEdit must remain visible.
                    let renaming_col = self.renaming_col.as_ref().map(|(c, _)| *c);
                    let col_rects = header_col_rects.take();

                    // Find the renaming column's screen rect so we can punch a hole in
                    // the blanket background fill, letting the TextEdit show through.
                    let renaming_rect = renaming_col.and_then(|rc| {
                        col_rects.iter().find(|(_, ci)| *ci == rc).map(|(r, _)| *r)
                    });

                    // Fill header background, skipping the renaming column's area.
                    if let Some(rr) = renaming_rect {
                        // Left section (gutter + columns left of the rename field).
                        if rr.left() > l {
                            painter.rect_filled(
                                egui::Rect::from_min_max(egui::pos2(l, clip.min.y), egui::pos2(rr.left(), y)),
                                0.0,
                                header_bg,
                            );
                        }
                        // Right section (columns right of the rename field).
                        if r > rr.right() {
                            painter.rect_filled(
                                egui::Rect::from_min_max(egui::pos2(rr.right(), clip.min.y), egui::pos2(r, y)),
                                0.0,
                                header_bg,
                            );
                        }
                    } else {
                        // No active rename (or column not visible) — fill everything.
                        painter.rect_filled(
                            egui::Rect::from_min_max(egui::pos2(l, clip.min.y), egui::pos2(r, y)),
                            0.0,
                            header_bg,
                        );
                    }

                    for (rect, col_idx) in &col_rects {
                        if renaming_col == Some(*col_idx) { continue; }
                        let col_selected = state.selection_type == Some(SelectionType::Column)
                            && state.selected_columns.contains(col_idx);
                        painter.rect_filled(*rect, 0.0, if col_selected { sel_color } else { header_bg });
                        if let Some(name) = col_names.get(*col_idx) {
                            if !name.is_empty() {
                                painter.text(
                                    egui::pos2(rect.left() + 8.0, rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    name,
                                    egui::FontId::proportional(state.font_size - 1.0),
                                    text_sec,
                                );
                            }
                        }
                    }

                    // Gutter separator — 1px at the left edge of the first data column header.
                    if let Some((first_rect, _)) = col_rects.first() {
                        painter.rect_filled(
                            egui::Rect::from_min_max(
                                first_rect.min,
                                egui::pos2(first_rect.left() + 1.0, y),
                            ),
                            0.0,
                            gutter_sep,
                        );
                    }

                    // Right-edge resize border on each column header.
                    // Idle: 1px border color. Hovered/active: 1px accent.
                    let hovered_col = col_resize_hovered.get();
                    for (rect, col_idx) in &col_rects {
                        let line_color = if hovered_col == Some(*col_idx) { accent } else { border };
                        painter.rect_filled(
                            egui::Rect::from_min_max(
                                egui::pos2(rect.max.x - 1.0, rect.min.y),
                                rect.max,
                            ),
                            0.0,
                            line_color,
                        );
                    }

                    // Separator line at the very bottom edge of the header.
                    painter.rect_filled(
                        egui::Rect::from_min_max(egui::pos2(l, y - 1.0), egui::pos2(r, y)),
                        0.0,
                        header_sep,
                    );

                    // Bottom accent border on column headers within the cell selection range.
                    if let Some((_, _, min_c, max_c)) = cell_sel_range {
                        for (rect, col_idx) in &col_rects {
                            if *col_idx >= min_c && *col_idx <= max_c {
                                painter.rect_filled(
                                    egui::Rect::from_min_max(
                                        egui::pos2(rect.min.x, y - 1.0),
                                        egui::pos2(rect.max.x, y),
                                    ),
                                    0.0,
                                    accent,
                                );
                            }
                        }
                    }
                }

                // Marching-ants selection border drawn on Order::Middle so it
                // sits on top of cells but below context menus and tooltips.
                if let Some(sr) = sel_rect_acc.get() {
                    if sr.is_finite() && sr.width() > 1.0 && sr.height() > 1.0 {
                        let layer = egui::LayerId::new(
                            egui::Order::Middle,
                            egui::Id::new("sel_dashed"),
                        );
                        let body_clip = ui.clip_rect();
                        let clip = egui::Rect::from_min_max(
                            egui::pos2(body_clip.min.x, header_bottom_y.get()),
                            body_clip.max,
                        );
                        let painter = ui.ctx().layer_painter(layer).with_clip_rect(clip);
                        if is_single_cell {
                            painter.rect_stroke(
                                sr,
                                0.0,
                                egui::Stroke::new(1.0, accent),
                                egui::StrokeKind::Inside,
                            );
                        } else {
                            draw_dashed_rect(
                                &painter,
                                sr,
                                egui::Stroke::new(1.0, accent),
                                5.0, 4.0,
                                current_dash,
                            );
                        }
                    }
                }

        }); // closes h_sa.show() — ScrollArea::horizontal

        // ── Update scroll state ──
        // Persist header bottom Y so the vertical scrollbar can avoid overlapping it.
        if header_bottom_y.get() > 0.0 {
            self.header_bottom_y_last = header_bottom_y.get();
        }
        let prev_v = self.v_scroll_y;
        let prev_h = self.h_scroll_x;
        self.v_scroll_y   = scroll_y_out.get();
        self.v_content_h  = content_h_out.get();
        self.v_viewport_h = viewport_h_out.get();
        self.h_scroll_x   = h_scroll_out.state.offset.x;
        self.h_content_w  = h_scroll_out.content_size.x;
        self.h_viewport_w = h_scroll_out.inner_rect.width();

        // ── Scrollbar fade (macOS-style overlay) ──
        let dt = ctx.input(|i| i.unstable_dt).min(0.1);
        const FADE_IN: f32  = 8.0;   // alpha/s when appearing
        const FADE_OUT: f32 = 1.5;   // alpha/s when disappearing
        const LINGER: f32   = 0.6;   // extra seconds after scroll stops

        // Detect scroll or mouse-over-the-table to trigger fade-in.
        let mouse_in_table = ctx.input(|i| i.pointer.hover_pos())
            .map(|p| panel_rect.contains(p))
            .unwrap_or(false);
        let scrolled_v = (self.v_scroll_y - prev_v).abs() > 0.1;
        let scrolled_h = (self.h_scroll_x - prev_h).abs() > 0.1;

        // ── Vertical scrollbar interaction ──
        let bar_id = egui::Id::new("table_v_scrollbar");
        let bar_resp = ui.interact(bar_rect, bar_id, egui::Sense::click_and_drag());

        if bar_resp.drag_started() && self.v_drag.is_none() {
            if let Some(pos) = bar_resp.interact_pointer_pos() {
                self.v_drag = Some((pos.y, self.v_scroll_y));
            }
        }
        if bar_resp.clicked() && self.v_drag.is_none() && max_scroll > 0.0 {
            if let Some(pos) = bar_resp.interact_pointer_pos() {
                let ratio = ((pos.y - bar_rect.min.y - thumb_h / 2.0) / (track_h - thumb_h).max(1.0))
                    .clamp(0.0, 1.0);
                self.v_scroll_y = ratio * max_scroll;
            }
        }

        // ── Horizontal scrollbar interaction ──
        let h_bar_id = egui::Id::new("table_h_scrollbar");
        let h_bar_resp = ui.interact(h_bar_rect, h_bar_id, egui::Sense::click_and_drag());
        if h_bar_resp.drag_started() && self.h_drag.is_none() {
            if let Some(pos) = h_bar_resp.interact_pointer_pos() {
                self.h_drag = Some((pos.x, self.h_scroll_x));
            }
        }
        if h_bar_resp.clicked() && self.h_drag.is_none() && h_max_scroll > 0.0 {
            if let Some(pos) = h_bar_resp.interact_pointer_pos() {
                let ratio = ((pos.x - h_bar_rect.min.x - h_thumb_w / 2.0)
                    / (h_track_w - h_thumb_w).max(1.0))
                    .clamp(0.0, 1.0);
                self.h_scroll_x = ratio * h_max_scroll;
            }
        }

        // Update alpha: fade in when active, fade out when idle.
        let v_active = scrolled_v || bar_resp.hovered() || bar_resp.dragged() || self.v_drag.is_some();
        let h_active = scrolled_h || h_bar_resp.hovered() || h_bar_resp.dragged() || self.h_drag.is_some();
        // Mouse over the table area keeps the relevant bar faintly visible while moving.
        let v_idle_show = mouse_in_table && max_scroll > 0.0;
        let h_idle_show = mouse_in_table && h_max_scroll > 0.0;

        if v_active {
            self.v_alpha = (self.v_alpha + dt * FADE_IN).min(1.0);
        } else if v_idle_show {
            // Drift toward a reduced opacity, not zero, while mouse is anywhere in the table.
            let target = 0.35;
            if self.v_alpha > target {
                self.v_alpha = (self.v_alpha - dt * FADE_OUT).max(target);
            } else {
                self.v_alpha = (self.v_alpha + dt * FADE_IN).min(target);
            }
        } else {
            self.v_alpha = (self.v_alpha - dt / LINGER).max(0.0);
        }

        if h_active {
            self.h_alpha = (self.h_alpha + dt * FADE_IN).min(1.0);
        } else if h_idle_show {
            let target = 0.35;
            if self.h_alpha > target {
                self.h_alpha = (self.h_alpha - dt * FADE_OUT).max(target);
            } else {
                self.h_alpha = (self.h_alpha + dt * FADE_IN).min(target);
            }
        } else {
            self.h_alpha = (self.h_alpha - dt / LINGER).max(0.0);
        }

        // Request repaint while fading so the animation is smooth.
        if self.v_alpha > 0.0 || self.h_alpha > 0.0 {
            ctx.request_repaint();
        }

        // Helper: apply alpha to a color (multiplies the alpha channel).
        let with_alpha = |c: egui::Color32, a: f32| -> egui::Color32 {
            egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a * 255.0) as u8)
        };

        // ── Draw vertical scrollbar ──
        let painter = ui.painter();
        // Only fill the track background when the bar is at least somewhat visible.
        if self.v_alpha > 0.01 && max_scroll > 0.0 {
            let display_y  = drag_scroll_y.unwrap_or(self.v_scroll_y);
            let thumb_top  = (display_y / max_scroll * (track_h - thumb_h)).max(0.0);
            let thumb_rect = egui::Rect::from_min_max(
                egui::pos2(bar_rect.min.x + 3.0, bar_rect.min.y + thumb_top + 2.0),
                egui::pos2(bar_rect.max.x - 3.0, bar_rect.min.y + thumb_top + thumb_h - 2.0),
            );
            let base_color = if bar_resp.dragged() || bar_resp.is_pointer_button_down_on() {
                colors.text_secondary
            } else if bar_resp.hovered() {
                blend(colors.text_secondary, colors.text_tertiary, 0.5)
            } else {
                colors.text_tertiary
            };
            painter.rect_filled(thumb_rect, 4.0, with_alpha(base_color, self.v_alpha));
        }

        // ── Draw horizontal scrollbar ──
        if self.h_alpha > 0.01 && h_max_scroll > 0.0 {
            let display_x = h_drag_scroll_x.unwrap_or(self.h_scroll_x);
            let thumb_left = (display_x / h_max_scroll * (h_track_w - h_thumb_w)).max(0.0);
            let h_thumb_rect = egui::Rect::from_min_max(
                egui::pos2(h_bar_rect.min.x + thumb_left + 2.0, h_bar_rect.min.y + 3.0),
                egui::pos2(h_bar_rect.min.x + thumb_left + h_thumb_w - 2.0, h_bar_rect.max.y - 3.0),
            );
            let h_base_color = if h_bar_resp.dragged() || h_bar_resp.is_pointer_button_down_on() {
                colors.text_secondary
            } else if h_bar_resp.hovered() {
                blend(colors.text_secondary, colors.text_tertiary, 0.5)
            } else {
                colors.text_tertiary
            };
            painter.rect_filled(h_thumb_rect, 4.0, with_alpha(h_base_color, self.h_alpha));
        }


        // ── Sticky gutter overlay ──
        // The gutter lives outside the horizontal scroll area, so it never scrolls away.
        // We paint it and register interactions after the data table renders so it
        // appears on top of any data cell that might overlap its screen area.
        {
            let v_scroll   = self.v_scroll_y;           // previous-frame scroll (1-frame lag is imperceptible)
            let header_h   = self.header_bottom_y_last.max(30.0);
            let gutter_rect = egui::Rect::from_min_max(
                panel_rect.min,
                egui::pos2(panel_rect.left() + gutter_width, panel_rect.max.y),
            );
            let body_rect  = egui::Rect::from_min_max(
                egui::pos2(panel_rect.left(), panel_rect.top() + header_h),
                egui::pos2(panel_rect.left() + gutter_width, panel_rect.max.y),
            );
            let painter = ui.painter().with_clip_rect(gutter_rect);

            state.ensure_row_layout();
            let body_h   = body_rect.height();
            let first_row = state.row_at_y(v_scroll, row_count);
            let last_row  = state.row_at_y((v_scroll + body_h + state.row_height).min(state.row_layout.total_height), row_count)
                .min(row_count.saturating_sub(1));

            for display_row in first_row..=last_row {
                let row_h    = state.row_height_for(display_row);
                let screen_y = body_rect.top() + state.row_top(display_row) - v_scroll;
                if screen_y >= body_rect.bottom() { break; }
                if screen_y + row_h <= body_rect.top() { continue; }

                let cell_rect = egui::Rect::from_min_size(
                    egui::pos2(panel_rect.left(), screen_y),
                    egui::vec2(gutter_width, row_h),
                );

                let row_sel = state.selection_type == Some(SelectionType::Row)
                    && state.selected_rows.contains(&display_row);
                let is_hov  = state.row_highlight_on_hover && !row_sel
                    && ctx.input(|i| i.pointer.hover_pos()
                        .map(|p| cell_rect.y_range().contains(p.y) && panel_rect.contains(p))
                        .unwrap_or(false));
                let bg = if row_sel { sel_color }
                         else if is_hov { hover_row_color }
                         else { gutter_bg };

                // Background
                painter.rect_filled(cell_rect, 0.0, bg);


                // Row number
                let num_rect = egui::Rect::from_min_max(
                    cell_rect.min,
                    egui::pos2(cell_rect.max.x - 1.0, cell_rect.max.y - 4.0),
                );
                painter.text(
                    num_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    (display_row + 1).to_string(),
                    egui::FontId::monospace((state.font_size - 2.0).max(8.0)),
                    line_num,
                );

                // Bottom border — check handle hover directly so it's always current.
                let handle_rect_for_border = egui::Rect::from_min_max(
                    egui::pos2(cell_rect.min.x, cell_rect.max.y - 4.0),
                    cell_rect.max,
                ).intersect(body_rect);
                let resize_active = self.row_resize == Some(display_row)
                    || ctx.input(|i| i.pointer.hover_pos()
                        .map(|p| handle_rect_for_border.contains(p))
                        .unwrap_or(false));
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(cell_rect.min.x, cell_rect.max.y - 1.0),
                        cell_rect.max,
                    ),
                    0.0,
                    if resize_active { accent } else { border },
                );

                // ── Row-select click (upper 80% of cell) ──
                let click_rect = egui::Rect::from_min_max(
                    cell_rect.min,
                    egui::pos2(cell_rect.max.x, cell_rect.min.y + row_h * 0.8),
                ).intersect(body_rect);
                let click_resp = ui.interact(
                    click_rect,
                    egui::Id::new(("gutter_click", display_row as u64)),
                    egui::Sense::click_and_drag(),
                );
                // Drag-select rows.
                if click_resp.drag_started() {
                    if let Some((er, ec, ev)) = self.editing.take() {
                        commit_edit(state, er, ec, ev);
                    }
                    self.row_drag_anchor = Some(display_row);
                    state.selection_type = Some(SelectionType::Row);
                    state.selected_rows = vec![display_row];
                    state.selected_columns.clear();
                    state.selection_anchor = None;
                    state.selection_focus = None;
                }
                if self.row_drag_anchor.is_some()
                    && ctx.input(|i| i.pointer.button_down(egui::PointerButton::Primary))
                {
                    if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
                        if cell_rect.y_range().contains(pos.y) {
                            let anchor = self.row_drag_anchor.unwrap();
                            let lo = anchor.min(display_row);
                            let hi = anchor.max(display_row);
                            state.selected_rows = (lo..=hi).collect();
                            state.selection_type = Some(SelectionType::Row);
                        }
                    }
                }
                if click_resp.drag_stopped() {
                    self.row_drag_anchor = None;
                }
                if click_resp.clicked() {
                    if let Some((er, ec, ev)) = self.editing.take() {
                        commit_edit(state, er, ec, ev);
                    }
                    let shift = ctx.input(|i| i.modifiers.shift);
                    state.selection_type = Some(SelectionType::Row);
                    if !shift { state.selected_rows.clear(); state.selected_columns.clear(); }
                    if !state.selected_rows.contains(&display_row) {
                        state.selected_rows.push(display_row);
                    }
                }

                // Right-click context menu
                let actual_row_for_ctx = state.display_row_to_actual_row(display_row)
                    .unwrap_or(display_row);
                let total_rows_ctx = row_count;
                click_resp.context_menu(|ui| {
                    ui.set_min_width(130.0);
                    ui.set_max_width(150.0);
                    let ic = colors.text_secondary;
                    let tc = colors.text_primary;
                    if icon_menu_item(ui, "insert-row-above", "Insert Row Above", ic, tc, false).clicked() {
                        insert_row(state, actual_row_for_ctx);
                        ui.close();
                    }
                    if icon_menu_item(ui, "insert-row-below", "Insert Row Below", ic, tc, false).clicked() {
                        insert_row(state, actual_row_for_ctx + 1);
                        ui.close();
                    }
                    ui.separator();
                    if actual_row_for_ctx > 0 {
                        if icon_menu_item(ui, "chevron-up", "Move Up", ic, tc, false).clicked() {
                            move_selected_rows(state, display_row, -1, total_rows_ctx);
                            ui.close();
                        }
                    }
                    if display_row + 1 < total_rows_ctx {
                        if icon_menu_item(ui, "chevron-down", "Move Down", ic, tc, false).clicked() {
                            move_selected_rows(state, display_row, 1, total_rows_ctx);
                            ui.close();
                        }
                    }
                    ui.separator();
                    let danger = colors.danger;
                    if icon_menu_item(ui, "delete-row", "Delete Row", danger, danger, false).clicked() {
                        delete_row(state, actual_row_for_ctx);
                        ui.close();
                    }
                });

                // ── Row-resize handle (bottom 4px) ──
                let handle_rect = egui::Rect::from_min_max(
                    egui::pos2(cell_rect.min.x, cell_rect.max.y - 4.0),
                    cell_rect.max,
                ).intersect(body_rect);
                let handle_resp = ui.interact(
                    handle_rect,
                    egui::Id::new(("gutter_resize", display_row as u64)),
                    egui::Sense::click_and_drag(),
                );
                if handle_resp.hovered() || handle_resp.dragged() || resize_active {
                    ctx.set_cursor_icon(egui::CursorIcon::ResizeVertical);
                }
                if handle_resp.drag_started() {
                    self.row_resize = Some(display_row);
                    self.row_resize_start_y = handle_resp.interact_pointer_pos()
                        .map(|p| p.y).unwrap_or(0.0);
                    self.row_resize_start_h = row_h;
                }
                if handle_resp.dragged() && self.row_resize == Some(display_row) {
                    let dy = handle_resp.drag_delta().y;
                    let new_h = (self.row_resize_start_h + dy).max(16.0);
                    self.row_resize_start_h = new_h;
                    state.row_heights.insert(display_row, new_h);
                    state.invalidate_row_layout();
                }
                if handle_resp.drag_stopped() {
                    self.row_resize = None;
                }
                // Double-click the resize handle → auto-fit height to cell content.
                if handle_resp.double_clicked() {
                    // Ensure this row is in cache.
                    if let Some(ar) = state.display_row_to_actual_row(display_row) {
                        if !state.row_cache.contains_key(&ar) {
                            load_row(state, display_row);
                        }
                    }
                    let mut max_lines = 1usize;
                    for c in 0..col_count {
                        if let Some(val) = state.get_display_cell(display_row, c) {
                            let lines = val.lines().count().max(1);
                            if lines > max_lines { max_lines = lines; }
                        }
                    }
                    if max_lines <= 1 {
                        // Single-line content: reset to default height.
                        state.row_heights.remove(&display_row);
                    } else {
                        let line_h = state.font_size + 4.0;
                        let new_h = (max_lines as f32 * line_h + 8.0).max(state.row_height);
                        state.row_heights.insert(display_row, new_h);
                    }
                    state.invalidate_row_layout();
                    self.save_requested = true;
                }
            }

        }

        // Gutter separator — 1px line at the left edge of the first data column.
        let sep_x = panel_rect.left() + gutter_width;
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(sep_x, panel_rect.min.y),
                egui::pos2(sep_x + 1.0, panel_rect.max.y),
            ),
            0.0,
            gutter_sep,
        );

        // Sync sel_rect and advance the dash animation for the next frame.
        self.sel_rect = sel_rect_acc.get();
        if self.sel_rect.is_some() && !is_single_cell {
            let dt = ctx.input(|i| i.unstable_dt);
            self.dash_offset = (self.dash_offset - dt * 12.0).rem_euclid(9.0);
            ctx.request_repaint();
        }

        // Copy shortcut — detect Event::Copy because egui-winit intercepts Cmd+C and emits
        // Event::Copy instead of Event::Key { Key::C }, so key_pressed(Key::C) never fires.
        if self.editing.is_none() {
            let do_copy = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
            if do_copy {
                ctx.copy_text(format_selection(state, state.copy_mode));
            }
        }
    }

    fn handle_keyboard(&mut self, state: &mut AppState, ctx: &egui::Context) {
        // Clear drag flag when button is released
        if state.is_dragging && ctx.input(|i| !i.pointer.button_down(egui::PointerButton::Primary)) {
            state.is_dragging = false;
        }

        if self.editing.is_some() { return; }
        // While a column rename TextEdit is active, let it own all key events.
        if self.renaming_col.is_some() { return; }

        let total_rows = state.display_row_count();
        let total_cols = state.col_count();
        if total_rows == 0 || total_cols == 0 { return; }

        let anchor = state.selection_anchor;
        let cur_row = anchor.map(|a| a.row).unwrap_or(0);
        let cur_col = anchor.map(|a| a.col).unwrap_or(0);

        let (moved, new_row, new_col, shift, do_enter, do_escape, do_delete, do_undo, do_redo) =
            ctx.input(|i| {
                let shift = i.modifiers.shift;
                let mut new_row = cur_row;
                let mut new_col = cur_col;
                let mut moved = false;

                if i.key_pressed(egui::Key::ArrowUp)    && cur_row > 0           { new_row -= 1; moved = true; }
                if i.key_pressed(egui::Key::ArrowDown)  && cur_row + 1 < total_rows { new_row += 1; moved = true; }
                if i.key_pressed(egui::Key::ArrowLeft)  && cur_col > 0           { new_col -= 1; moved = true; }
                if i.key_pressed(egui::Key::ArrowRight) && cur_col + 1 < total_cols { new_col += 1; moved = true; }

                let do_enter  = i.key_pressed(egui::Key::Enter);
                let do_escape = i.key_pressed(egui::Key::Escape);
                let do_delete = i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace);
                let do_undo   = i.key_pressed(egui::Key::Z) && i.modifiers.command && !i.modifiers.shift;
                let do_redo   = (i.key_pressed(egui::Key::Z) && i.modifiers.command && i.modifiers.shift)
                             || (i.key_pressed(egui::Key::Y) && i.modifiers.command);

                (moved, new_row, new_col, shift, do_enter, do_escape, do_delete, do_undo, do_redo)
            });

        if moved {
            let focus = CellCoord { row: new_row, col: new_col };
            if !shift || anchor.is_none() { state.selection_anchor = Some(focus); }
            state.selection_focus = Some(focus);
            state.selection_type = Some(SelectionType::Cell);
            state.selected_rows.clear();
            state.selected_columns.clear();
        }

        if do_enter {
            if let Some(a) = state.selection_anchor {
                let val = state.get_display_cell(a.row, a.col).unwrap_or_default();
                self.editing = Some((a.row, a.col, val));
            }
        }

        if do_escape {
            state.clear_selection();
        }

        if do_delete {
            delete_selection(state);
        }

        if do_undo { apply_undo(state); }
        if do_redo { apply_redo(state); }

        // Paste (Cmd+V / Ctrl+V)
        let paste_text = ctx.input(|i| {
            i.events.iter().find_map(|e| {
                if let egui::Event::Paste(text) = e { Some(text.clone()) } else { None }
            })
        });
        if let Some(text) = paste_text {
            if state.selection_type == Some(SelectionType::Cell) {
                apply_paste(state, &text);
            }
        }

        // Pick up Enter-initiated edits from state (e.g. from external triggers)
        if let Some((r, c, val)) = state.editing_cell.take() {
            self.editing = Some((r, c, val));
        }
    }
}

// ── Edit operations ───────────────────────────────────────────────────────────

fn action_label(a: &EditAction) -> String {
    match a {
        EditAction::CellEdit { row, col, .. } => format!("CellEdit r={} c={}", row, col),
        EditAction::BatchCellEdit { edits } => format!("BatchCellEdit n={}", edits.len()),
        EditAction::RenameColumn { col, .. } => format!("RenameColumn c={}", col),
        EditAction::MoveColumn { from_col, to_col } => format!("MoveColumn {}↔{}", from_col, to_col),
        EditAction::MoveRow { from_row, to_row } => format!("MoveRow {}↔{}", from_row, to_row),
        EditAction::Structural { description } => format!("Structural {}", description),
    }
}

fn commit_edit(state: &mut AppState, row: usize, display_col: usize, new_val: String) {
    let physical_col = state.display_to_physical_col(display_col);
    let Some(ref mut file) = state.file else { return };
    let old_had_edit = file.edits.contains_key(&(row, physical_col));
    let old_value = file.edits.get(&(row, physical_col)).cloned()
        .or_else(|| state.row_cache.get(&row).and_then(|r| r.get(physical_col).cloned()))
        .unwrap_or_default();

    if new_val == old_value && !old_had_edit { return; } // no-op

    crate::dlog!(Debug, "Edit", "commit row={} col={} (phys={}) len={}→{}",
        row, display_col, physical_col, old_value.len(), new_val.len());

    file.edits.insert((row, physical_col), new_val.clone());
    if let Some(cached) = state.row_cache.get_mut(&row) {
        if physical_col < cached.len() { cached[physical_col] = new_val.clone(); }
    }
    state.cache_version += 1;

    state.undo_stack.push(EditAction::CellEdit {
        row,
        col: display_col,
        physical_col,
        old_had_edit,
        old_value,
        new_value: new_val,
    });
    state.redo_stack.clear();
}

fn rename_column(state: &mut AppState, col: usize, new_name: String) {
    let Some(ref mut file) = state.file else { return };
    let Some(column) = file.metadata.columns.get_mut(col) else { return };
    if column.name == new_name { return; }
    let old_name = std::mem::replace(&mut column.name, new_name.clone());
    file.columns_renamed = true;
    crate::dlog!(Info, "Edit", "rename col={} '{}' → '{}'", col, old_name, new_name);
    state.undo_stack.push(EditAction::RenameColumn { col, old_name, new_name });
    state.redo_stack.clear();
}

fn delete_selection(state: &mut AppState) {
    let Some((min_r, max_r, min_c, max_c)) = state.selection_range() else { return };
    let _t = crate::dspan!("Edit", "delete_selection");
    crate::dlog!(Info, "Edit", "delete rows={}..={} cols={}..={}",
        min_r, max_r, min_c, max_c);
    let mut edits_batch = Vec::new();
    {
        let Some(ref mut file) = state.file else { return };
        for r in min_r..=max_r {
            for c in min_c..=max_c {
                let physical_c = {
                    if let Some(ref order) = file.col_order {
                        match order.get(c) {
                            Some(crate::state::ColSource::Original(idx)) => *idx,
                            _ => c,
                        }
                    } else { c }
                };
                let old_had_edit = file.edits.contains_key(&(r, physical_c));
                let old_value = file.edits.get(&(r, physical_c)).cloned()
                    .or_else(|| state.row_cache.get(&r).and_then(|row| row.get(physical_c).cloned()))
                    .unwrap_or_default();
                file.edits.insert((r, physical_c), String::new());
                if let Some(row) = state.row_cache.get_mut(&r) {
                    if physical_c < row.len() { row[physical_c] = String::new(); }
                }
                edits_batch.push(BatchEditEntry { row: r, col: c, physical_col: physical_c, old_had_edit, old_value, new_value: String::new() });
            }
        }
    }
    if !edits_batch.is_empty() {
        state.undo_stack.push(EditAction::BatchCellEdit { edits: edits_batch });
        state.redo_stack.clear();
        state.cache_version += 1;
    }
}

fn apply_paste(state: &mut AppState, text: &str) {
    let Some(anchor) = state.selection_anchor else { return };
    let _t = crate::dspan!("Edit", "apply_paste");
    crate::dlog!(Info, "Edit", "paste anchor=({},{}) bytes={}",
        anchor.row, anchor.col, text.len());
    let total_rows = state.display_row_count();
    let total_cols = state.col_count();
    let mut edits_batch: Vec<BatchEditEntry> = Vec::new();
    {
        let Some(ref mut file) = state.file else { return };
        let lines: Vec<&str> = text.split('\n').collect();
        for (row_offset, line) in lines.iter().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() && row_offset + 1 == lines.len() { break; }
            let display_row = anchor.row + row_offset;
            if display_row >= total_rows { break; }
            for (col_offset, cell_val) in line.split('\t').enumerate() {
                let display_col = anchor.col + col_offset;
                if display_col >= total_cols { break; }
                let physical_col = match &file.col_order {
                    Some(order) => match order.get(display_col) {
                        Some(crate::state::ColSource::Original(idx)) => *idx,
                        _ => display_col,
                    },
                    None => display_col,
                };
                let old_had_edit = file.edits.contains_key(&(display_row, physical_col));
                let old_value = file.edits.get(&(display_row, physical_col)).cloned()
                    .or_else(|| state.row_cache.get(&display_row).and_then(|r| r.get(physical_col).cloned()))
                    .unwrap_or_default();
                let new_value = cell_val.to_string();
                file.edits.insert((display_row, physical_col), new_value.clone());
                if let Some(row_data) = state.row_cache.get_mut(&display_row) {
                    if physical_col < row_data.len() { row_data[physical_col] = new_value.clone(); }
                }
                edits_batch.push(BatchEditEntry { row: display_row, col: display_col, physical_col, old_had_edit, old_value, new_value });
            }
        }
    }
    if !edits_batch.is_empty() {
        state.undo_stack.push(EditAction::BatchCellEdit { edits: edits_batch });
        state.redo_stack.clear();
        state.cache_version += 1;
    }
}

fn apply_undo(state: &mut AppState) {
    let Some(action) = state.undo_stack.pop() else { return };
    crate::dlog!(Info, "Undo", "undo {}", action_label(&action));
    let mut sel: Option<CellCoord> = None;

    match &action {
        EditAction::CellEdit { row, col, physical_col, old_had_edit, old_value, .. } => {
            if let Some(ref mut file) = state.file {
                if *old_had_edit {
                    file.edits.insert((*row, *physical_col), old_value.clone());
                } else {
                    file.edits.remove(&(*row, *physical_col));
                }
            }
            if let Some(cached) = state.row_cache.get_mut(row) {
                if *physical_col < cached.len() { cached[*physical_col] = old_value.clone(); }
            }
            sel = Some(CellCoord { row: *row, col: *col });
        }
        EditAction::BatchCellEdit { edits } => {
            for e in edits {
                if let Some(ref mut file) = state.file {
                    if e.old_had_edit {
                        file.edits.insert((e.row, e.physical_col), e.old_value.clone());
                    } else {
                        file.edits.remove(&(e.row, e.physical_col));
                    }
                }
                if let Some(cached) = state.row_cache.get_mut(&e.row) {
                    if e.physical_col < cached.len() { cached[e.physical_col] = e.old_value.clone(); }
                }
            }
            sel = edits.first().map(|e| CellCoord { row: e.row, col: e.col });
        }
        EditAction::RenameColumn { col, old_name, .. } => {
            if let Some(ref mut file) = state.file {
                if let Some(column) = file.metadata.columns.get_mut(*col) {
                    column.name = old_name.clone();
                }
            }
        }
        EditAction::MoveColumn { from_col, to_col } => {
            // Undo = swap back.
            move_column_impl(state, *to_col, *from_col);
        }
        EditAction::MoveRow { from_row, to_row } => {
            move_row_impl(state, *to_row, *from_row);
        }
        EditAction::Structural { .. } => {}
    }

    if let Some(coord) = sel {
        state.selection_anchor = Some(coord);
        state.selection_focus = Some(coord);
        state.selection_type = Some(SelectionType::Cell);
    }
    state.redo_stack.push(action);
    state.cache_version += 1;
}

fn apply_redo(state: &mut AppState) {
    let Some(action) = state.redo_stack.pop() else { return };
    crate::dlog!(Info, "Undo", "redo {}", action_label(&action));
    let mut sel: Option<CellCoord> = None;

    match &action {
        EditAction::CellEdit { row, col, physical_col, new_value, .. } => {
            if let Some(ref mut file) = state.file {
                file.edits.insert((*row, *physical_col), new_value.clone());
            }
            if let Some(cached) = state.row_cache.get_mut(row) {
                if *physical_col < cached.len() { cached[*physical_col] = new_value.clone(); }
            }
            sel = Some(CellCoord { row: *row, col: *col });
        }
        EditAction::BatchCellEdit { edits } => {
            for e in edits {
                if let Some(ref mut file) = state.file {
                    file.edits.insert((e.row, e.physical_col), e.new_value.clone());
                }
                if let Some(cached) = state.row_cache.get_mut(&e.row) {
                    if e.physical_col < cached.len() { cached[e.physical_col] = e.new_value.clone(); }
                }
            }
            sel = edits.last().map(|e| CellCoord { row: e.row, col: e.col });
        }
        EditAction::RenameColumn { col, new_name, .. } => {
            if let Some(ref mut file) = state.file {
                if let Some(column) = file.metadata.columns.get_mut(*col) {
                    column.name = new_name.clone();
                }
            }
        }
        EditAction::MoveColumn { from_col, to_col } => {
            move_column_impl(state, *from_col, *to_col);
        }
        EditAction::MoveRow { from_row, to_row } => {
            move_row_impl(state, *from_row, *to_row);
        }
        EditAction::Structural { .. } => {}
    }

    if let Some(coord) = sel {
        state.selection_anchor = Some(coord);
        state.selection_focus = Some(coord);
        state.selection_type = Some(SelectionType::Cell);
    }
    state.undo_stack.push(action);
    state.cache_version += 1;
}

// ── Column / row reorder ──────────────────────────────────────────────────────

/// Core swap logic (no undo entry). Call from `move_column` and undo/redo.
fn move_column_impl(state: &mut AppState, from: usize, to: usize) {
    if from == to { return; }
    let Some(ref mut file) = state.file else { return };
    file.ensure_col_order();
    if let Some(ref mut order) = file.col_order {
        order.swap(from, to);
    }
    // Swap recorded column widths so the visual widths follow the columns.
    let from_w = state.column_widths.remove(&from);
    let to_w   = state.column_widths.remove(&to);
    if let Some(w) = to_w   { state.column_widths.insert(from, w); }
    if let Some(w) = from_w { state.column_widths.insert(to,   w); }
    state.row_cache.clear();
    state.cache_version += 1;
}

/// Public entry: swap display columns `from` ↔ `to`, push undo entry.
fn move_column(state: &mut AppState, from: usize, to: usize) {
    if from == to { return; }
    move_column_impl(state, from, to);
    state.undo_stack.push(EditAction::MoveColumn { from_col: from, to_col: to });
    state.redo_stack.clear();
}

/// Move all selected columns left or right by one step.
/// `delta` is -1 (left) or +1 (right). `ctx_col` is the right-clicked column.
fn move_selected_columns(state: &mut AppState, ctx_col: usize, delta: i32, col_count: usize) {
    let cols: Vec<usize> = if state.selected_columns.contains(&ctx_col) && !state.selected_columns.is_empty() {
        let mut v = state.selected_columns.clone();
        v.sort_unstable();
        v
    } else {
        vec![ctx_col]
    };
    if delta < 0 {
        if cols[0] == 0 { return; }
        for &c in &cols {
            move_column_impl(state, c, (c as i32 + delta) as usize);
            state.undo_stack.push(EditAction::MoveColumn { from_col: c, to_col: (c as i32 + delta) as usize });
        }
    } else {
        if *cols.last().unwrap() + 1 >= col_count { return; }
        for &c in cols.iter().rev() {
            move_column_impl(state, c, (c as i32 + delta) as usize);
            state.undo_stack.push(EditAction::MoveColumn { from_col: c, to_col: (c as i32 + delta) as usize });
        }
    }
    state.redo_stack.clear();
    for sel in state.selected_columns.iter_mut() {
        if cols.contains(sel) {
            *sel = (*sel as i32 + delta) as usize;
        }
    }
}

/// Move all selected rows up or down by one step.
/// `delta` is -1 (up) or +1 (down). `ctx_display_row` is the right-clicked display row.
fn move_selected_rows(state: &mut AppState, ctx_display_row: usize, delta: i32, total_rows: usize) {
    let rows: Vec<usize> = if state.selected_rows.contains(&ctx_display_row) && !state.selected_rows.is_empty() {
        let mut v = state.selected_rows.clone();
        v.sort_unstable();
        v
    } else {
        vec![ctx_display_row]
    };
    if delta < 0 {
        if rows[0] == 0 { return; }
        for &dr in &rows {
            let ar = state.display_row_to_actual_row(dr).unwrap_or(dr);
            move_row_impl(state, ar, (ar as i32 + delta) as usize);
            state.undo_stack.push(EditAction::MoveRow { from_row: ar, to_row: (ar as i32 + delta) as usize });
        }
    } else {
        if *rows.last().unwrap() + 1 >= total_rows { return; }
        for &dr in rows.iter().rev() {
            let ar = state.display_row_to_actual_row(dr).unwrap_or(dr);
            move_row_impl(state, ar, (ar as i32 + delta) as usize);
            state.undo_stack.push(EditAction::MoveRow { from_row: ar, to_row: (ar as i32 + delta) as usize });
        }
    }
    state.redo_stack.clear();
    for sel in state.selected_rows.iter_mut() {
        if rows.contains(sel) {
            *sel = (*sel as i32 + delta) as usize;
        }
    }
}

/// Core swap logic for rows (no undo entry).
fn move_row_impl(state: &mut AppState, from: usize, to: usize) {
    if from == to { return; }
    let Some(ref mut file) = state.file else { return };
    // Materialise sort_permutation if not already set.
    if file.sort_permutation.is_none() {
        let n = file.metadata.total_rows;
        file.sort_permutation = Some((0..n).collect());
    }
    if let Some(ref mut perm) = file.sort_permutation {
        perm.swap(from, to);
    }
    // Swap per-row heights.
    let from_h = state.row_heights.remove(&from);
    let to_h   = state.row_heights.remove(&to);
    if let Some(h) = to_h   { state.row_heights.insert(from, h); }
    if let Some(h) = from_h { state.row_heights.insert(to,   h); }
    // Swap any edits keyed by the two display rows.
    let affected: Vec<(usize, usize)> = state.file.as_ref()
        .map(|f| f.edits.keys().filter(|&&(r, _)| r == from || r == to).copied().collect())
        .unwrap_or_default();
    for (r, c) in affected {
        if let Some(ref mut f) = state.file {
            let val = f.edits.remove(&(r, c)).unwrap_or_default();
            let new_r = if r == from { to } else { from };
            f.edits.insert((new_r, c), val);
        }
    }
    state.row_cache.remove(&from);
    state.row_cache.remove(&to);
    state.cache_version += 1;
    state.invalidate_row_layout();
}

/// Public entry: swap display rows `from` ↔ `to`, push undo entry.
fn move_row(state: &mut AppState, from: usize, to: usize) {
    if from == to { return; }
    move_row_impl(state, from, to);
    state.undo_stack.push(EditAction::MoveRow { from_row: from, to_row: to });
    state.redo_stack.clear();
}

// ── Row/cell helpers ──────────────────────────────────────────────────────────

fn get_cell(state: &mut AppState, display_row: usize, col: usize) -> String {
    if let Some(v) = state.get_display_cell(display_row, col) {
        return v;
    }
    if load_row(state, display_row) {
        return state.get_display_cell(display_row, col).unwrap_or_default();
    }
    String::new()
}

fn load_row(state: &mut AppState, display_row: usize) -> bool {
    let actual_row = match state.display_row_to_actual_row(display_row) { Some(r) => r, None => return false };

    // Resolve the row source via row_order (respects inserts/deletes/moves).
    // Returns None for inserted rows (handled inline), Some(orig_row) for original CSV rows.
    enum RowResolution { Inserted(Vec<String>), Original(usize) }
    let resolution: RowResolution = {
        let file = match state.file.as_ref() { Some(f) => f, None => return false };
        if let Some(ref order) = file.row_order {
            match order.get(actual_row) {
                Some(&crate::state::RowSource::Inserted(ins_idx)) =>
                    RowResolution::Inserted(file.inserted_rows.get(ins_idx).cloned().unwrap_or_default()),
                Some(&crate::state::RowSource::Original(orig_row)) =>
                    RowResolution::Original(orig_row),
                None => return false,
            }
        } else {
            // No row_order: fall back to sort/filter permutation.
            RowResolution::Original(file.virtual_to_actual_row(actual_row))
        }
    };

    if let RowResolution::Inserted(data) = resolution {
        state.cache_row(actual_row, data);
        return true;
    }
    let orig_row = match resolution { RowResolution::Original(r) => r, _ => unreachable!() };

    let (path, byte_offset, col_count, delimiter, row_edits) = {
        let file = match state.file.as_ref() { Some(f) => f, None => return false };
        // Use orig_row (from row_order) to seek to the correct CSV offset.
        let byte_offset = match file.row_offsets.get(orig_row).copied() { Some(o) => o, None => return false };
        let row_edits: Vec<(usize, String)> = file.edits.iter()
            .filter(|(&(r, _), _)| r == actual_row)
            .map(|(&(_, c), v)| (c, v.clone()))
            .collect();
        (file.file_path.clone(), byte_offset, file.metadata.columns.len(), file.delimiter, row_edits)
    };

    let fh = match std::fs::File::open(&path) { Ok(f) => f, Err(_) => return false };
    let mut reader = BufReader::new(fh);
    if reader.seek(SeekFrom::Start(byte_offset)).is_err() { return false; }
    let mut csv_rdr = csv::ReaderBuilder::new()
        .has_headers(false).flexible(true).delimiter(delimiter).from_reader(reader);
    let record = match csv_rdr.records().next() { Some(Ok(r)) => r, _ => return false };

    let mut row: Vec<String> = (0..col_count).map(|c| record.get(c).unwrap_or("").to_string()).collect();
    for (c, v) in row_edits { if c < row.len() { row[c] = v; } }
    state.cache_row(actual_row, row);
    true
}

// ── Insert row / column ───────────────────────────────────────────────────────

/// Insert a blank row at display position `at` (rows at `at` and beyond shift down).
fn insert_row(state: &mut AppState, at: usize) {
    let Some(ref mut file) = state.file else { return };
    let col_count = file.current_col_count();
    file.ensure_row_order();
    let ins_idx = file.inserted_rows.len();
    file.inserted_rows.push(vec![String::new(); col_count]);
    let at = at.min(file.row_order.as_ref().map(|o| o.len()).unwrap_or(0));
    if let Some(ref mut order) = file.row_order {
        order.insert(at, crate::state::RowSource::Inserted(ins_idx));
    }
    state.row_cache.clear();
    state.cache_version += 1;
    state.invalidate_row_layout();
    state.undo_stack.push(crate::state::EditAction::Structural {
        description: format!("Insert row at {}", at),
    });
    state.redo_stack.clear();
}

/// Insert a blank column at display position `at` (columns at `at` and beyond shift right).
fn insert_col(state: &mut AppState, at: usize) {
    let Some(ref mut file) = state.file else { return };
    file.ensure_col_order();
    let ins_idx = file.inserted_columns.len();
    file.inserted_columns.push(String::new());
    let at = at.min(file.col_order.as_ref().map(|o| o.len()).unwrap_or(0));
    if let Some(ref mut order) = file.col_order {
        order.insert(at, crate::state::ColSource::Inserted(ins_idx));
    }
    // Also add a metadata entry so the header label renders correctly.
    let new_col_idx = file.metadata.columns.len();
    file.metadata.columns.push(crate::csv_engine::types::CsvColumn {
        index: new_col_idx,
        name: String::new(),
        inferred_type: crate::csv_engine::types::ColumnType::String,
    });
    state.row_cache.clear();
    state.cache_version += 1;
    state.undo_stack.push(crate::state::EditAction::Structural {
        description: format!("Insert column at {}", at),
    });
    state.redo_stack.clear();
}

/// Delete a column at display position `at`.
fn delete_col(state: &mut AppState, at: usize) {
    let Some(ref mut file) = state.file else { return };
    file.ensure_col_order();
    if let Some(ref mut order) = file.col_order {
        if at < order.len() && order.len() > 1 {
            order.remove(at);
        } else {
            return;
        }
    }
    // Clear selection if it references the deleted column.
    state.selected_columns.retain(|&c| c != at);
    if let Some(anchor) = state.selection_anchor {
        if anchor.col == at { state.selection_anchor = None; state.selection_focus = None; }
    }
    state.row_cache.clear();
    state.cache_version += 1;
    state.undo_stack.push(crate::state::EditAction::Structural {
        description: format!("Delete column at {}", at),
    });
    state.redo_stack.clear();
}

/// Delete a row at display position `at`.
fn delete_row(state: &mut AppState, at: usize) {
    let Some(ref mut file) = state.file else { return };
    // Ensure both ordering systems exist.
    file.ensure_row_order();
    if file.sort_permutation.is_none() {
        let n = file.metadata.total_rows;
        file.sort_permutation = Some((0..n).collect());
    }
    // Guard: must have more than 1 row, and `at` must be in range.
    let order_len = file.row_order.as_ref().map(|o| o.len()).unwrap_or(0);
    if at >= order_len || order_len <= 1 { return; }
    // Remove from row_order (structural — used by save path).
    if let Some(ref mut order) = file.row_order {
        order.remove(at);
    }
    // Remove from sort_permutation (display — used by render/cache path).
    if let Some(ref mut perm) = file.sort_permutation {
        if at < perm.len() {
            perm.remove(at);
        }
    }
    // Clear selection if it references the deleted row.
    state.selected_rows.retain(|&r| r != at);
    if let Some(anchor) = state.selection_anchor {
        if anchor.row == at { state.selection_anchor = None; state.selection_focus = None; }
    }
    state.row_cache.clear();
    state.cache_version += 1;
    state.invalidate_row_layout();
    state.undo_stack.push(crate::state::EditAction::Structural {
        description: format!("Delete row at {}", at),
    });
    state.redo_stack.clear();
}

// ── Context menu helper ───────────────────────────────────────────────────────

/// A selectable menu row with a leading SVG icon.
/// Returns the egui `Response` so the caller can check `.clicked()`.
fn icon_menu_item(
    ui: &mut egui::Ui,
    icon_name: &str,
    label: &str,
    icon_color: egui::Color32,
    text_color: egui::Color32,
    _active: bool,
) -> egui::Response {
    let row_h = 26.0;
    let avail_w = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(avail_w, row_h),
        egui::Sense::click(),
    );
    let bg = if response.hovered() {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };
    if bg != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 4.0, bg);
    }
    let inner = rect.shrink2(egui::vec2(8.0, 0.0));
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.add(crate::ui::icons::icon(icon_name, icon_color));
            ui.add_space(6.0);
            ui.add(egui::Label::new(
                egui::RichText::new(label).size(12.0).color(text_color),
            ).selectable(false));
        },
    );
    response
}

/// Public wrapper used by app.rs to commit a sidebar cell edit.
pub fn commit_edit_pub(state: &mut AppState, row: usize, display_col: usize, new_val: String) {
    commit_edit(state, row, display_col, new_val);
}

/// Public wrapper for column display name — used by app.rs sidebar header.
pub fn col_name_for_display_pub(state: &AppState, display_col: usize) -> String {
    col_name_for_display(state, display_col)
}
