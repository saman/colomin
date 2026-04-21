use std::io::{BufReader, Seek, SeekFrom};

use eframe::egui;
use egui_extras::{Column, TableBuilder};

use crate::state::{AppState, BatchEditEntry, CellCoord, EditAction, SelectionType};

pub struct TableView {
    pub editing: Option<(usize, usize, String)>,
    /// Index of row whose bottom edge is being dragged for resize, or usize::MAX for global resize.
    row_resize: Option<usize>,
    row_resize_start_y: f32,
    row_resize_start_h: f32,
    /// Column being renamed via double-click on header (col_index, buffer).
    renaming_col: Option<(usize, String)>,
    /// Bounding rect of all visible selected cells — accumulated each frame for the dashed border.
    sel_rect: Option<egui::Rect>,
    /// Marching-ants dash phase offset, advanced each frame while a selection is active.
    dash_offset: f32,
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

impl TableView {
    pub fn new() -> Self {
        Self {
            editing: None,
            row_resize: None,
            row_resize_start_y: 0.0,
            row_resize_start_h: 0.0,
            renaming_col: None,
            sel_rect: None,
            dash_offset: 0.0,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut AppState, ctx: &egui::Context) {
        let Some(_) = state.file else { return };

        // Reset selection bounding rect; it's re-accumulated during cell rendering.
        self.sel_rect = None;

        self.handle_keyboard(state, ctx);

        let colors = state.current_theme();
        let row_count = state.display_row_count();
        let col_count = state.col_count();
        let gutter_width = state.row_number_width();

        let header_bg  = colors.gutter_bg;
        let gutter_bg  = colors.gutter_bg;
        let line_num   = colors.line_number;
        let text_pri   = colors.text_primary;
        let text_sec   = colors.text_secondary;
        let sel_color  = colors.accent_subtle;
        let edit_color = colors.edited;
        let surf       = colors.surface;
        let accent     = colors.accent;
        let border     = colors.border;
        // Header/body separator: 40% of text_secondary over surface — visible
        // in every theme without being heavy (border color is too close to surface).
        let header_sep = blend(text_sec, surf, 0.4);

        let header_enabled = state.header_row_enabled;
        let col_names: Vec<String> = if let Some(f) = state.file.as_ref() {
            let n = f.current_col_count();
            (0..n).map(|display_col| {
                if header_enabled {
                    let phys = state.display_to_physical_col(display_col);
                    f.metadata.columns.iter()
                        .find(|c| c.index == phys)
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| format!("Col {}", display_col + 1))
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

        // Zero inter-cell spacing so columns are flush against each other.
        let total_width: f32 = gutter_width
            + (0..col_count).map(|c| state.column_width(c)).sum::<f32>()
            + 8.0; // small right padding


        // Cells capture values across closure boundaries without &mut conflicts.
        let header_bottom_y = std::cell::Cell::new(0.0_f32);
        let header_left_x   = std::cell::Cell::new(f32::MAX);
        let header_right_x  = std::cell::Cell::new(0.0_f32);
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

        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(total_width);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                // Suppress idle vertical column separators drawn by egui_extras.
                // Hovered/dragged strokes remain so the resize affordance is still visible.
                ui.visuals_mut().widgets.noninteractive.bg_stroke = egui::Stroke::NONE;

                let mut table = TableBuilder::new(ui)
                    .striped(false)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::exact(gutter_width))
                    .min_scrolled_height(0.0);

                for col in 0..col_count {
                    let w = state.column_width(col);
                    table = table.column(Column::initial(w).at_least(40.0).resizable(true));
                }

                // ── Header ──
                table
            .header(28.0, |mut header| {
                header.col(|ui| {
                    let rect = ui.max_rect();
                    ui.painter().rect_filled(rect, 0.0, header_bg);
                    header_bottom_y.set(rect.bottom());
                    if rect.left() < header_left_x.get() { header_left_x.set(rect.left()); }
                });
                for (col_idx, name) in col_names.iter().enumerate() {
                    header.col(|ui| {
                        let rect = ui.max_rect();
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
                                .font(egui::FontId::proportional(12.0))
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
                            // Allocate interaction FIRST with stable ID, then paint on top.
                            let hdr_id = egui::Id::new(("hdr", col_idx as u64));
                            let resp = ui.interact(rect, hdr_id, egui::Sense::click());

                            // Text label — leave room for sort icon on the right.
                            let label_max_x = if is_sorted_here { rect.right() - 20.0 } else { rect.right() - 4.0 };
                            let galley = ui.painter().layout_no_wrap(
                                name.clone(),
                                egui::FontId::proportional(12.0),
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
                                    ui.close_menu();
                                }
                                if icon_menu_item(ui, "insert-column-right", "Insert Column Right", ic, tc, false).clicked() {
                                    insert_col(state, col_idx + 1);
                                    ui.close_menu();
                                }
                                ui.separator();
                                let is_asc = sort_asc == Some(true) && is_sorted_here;
                                if icon_menu_item(ui, "sort-asc", "Sort Ascending",
                                    if is_sorted_here && is_asc { accent } else { ic }, tc,
                                    is_sorted_here && is_asc).clicked()
                                {
                                    state.pending_sort = Some((phys_col, true));
                                    ui.close_menu();
                                }
                                if icon_menu_item(ui, "sort-desc", "Sort Descending",
                                    if is_sorted_here && !is_asc { accent } else { ic }, tc,
                                    is_sorted_here && !is_asc).clicked()
                                {
                                    state.pending_sort = Some((phys_col, false));
                                    ui.close_menu();
                                }
                                ui.separator();
                                if col_idx > 0 {
                                    if icon_menu_item(ui, "chevron-left", "Move Left", ic, tc, false).clicked() {
                                        move_column(state, col_idx, col_idx - 1);
                                        ui.close_menu();
                                    }
                                }
                                if col_idx + 1 < col_count {
                                    if icon_menu_item(ui, "chevron-right", "Move Right", ic, tc, false).clicked() {
                                        move_column(state, col_idx, col_idx + 1);
                                        ui.close_menu();
                                    }
                                }
                                if header_enabled {
                                    ui.separator();
                                    if icon_menu_item(ui, "pen", "Rename", ic, tc, false).clicked() {
                                        if let Some((er, ec, ev)) = self.editing.take() {
                                            commit_edit(state, er, ec, ev);
                                        }
                                        self.renaming_col = Some((col_idx, name.clone()));
                                        ui.close_menu();
                                    }
                                }
                                ui.separator();
                                let danger = colors.danger;
                                if icon_menu_item(ui, "delete-column", "Delete Column", danger, danger, false).clicked() {
                                    delete_col(state, col_idx);
                                    ui.close_menu();
                                }
                            });
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
                    let row_h = state.row_height_for(display_row);

                    let row_sel = state.selection_type == Some(SelectionType::Row)
                        && state.selected_rows.contains(&display_row);

                    // ── Gutter cell ──
                    row.col(|ui| {
                        let rect = ui.max_rect();
                        let bg = if row_sel { sel_color } else { gutter_bg };
                        ui.painter().rect_filled(rect, 0.0, bg);

                        // Row number
                        let num_rect = egui::Rect::from_min_max(
                            rect.min,
                            egui::pos2(rect.max.x, rect.max.y - 4.0),
                        );
                        ui.painter().text(
                            num_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            (display_row + 1).to_string(),
                            egui::FontId::monospace(10.0),
                            line_num,
                        );

                        // Row-select click (top 80% of gutter)
                        let click_rect = egui::Rect::from_min_max(
                            rect.min,
                            egui::pos2(rect.max.x, rect.min.y + row_h * 0.8),
                        );
                        let resp = ui.allocate_rect(click_rect, egui::Sense::click());
                        if resp.clicked() {
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
                        // Right-click context menu on row gutter.
                        let total_rows_ctx = row_count;
                        resp.context_menu(|ui| {
                            ui.set_min_width(130.0);
                            ui.set_max_width(150.0);
                            let ic = colors.text_secondary;
                            let tc = colors.text_primary;
                            if icon_menu_item(ui, "insert-row-above", "Insert Row Above", ic, tc, false).clicked() {
                                insert_row(state, display_row);
                                ui.close_menu();
                            }
                            if icon_menu_item(ui, "insert-row-below", "Insert Row Below", ic, tc, false).clicked() {
                                insert_row(state, display_row + 1);
                                ui.close_menu();
                            }
                            ui.separator();
                            if display_row > 0 {
                                if icon_menu_item(ui, "chevron-up", "Move Up", ic, tc, false).clicked() {
                                    move_row(state, display_row, display_row - 1);
                                    ui.close_menu();
                                }
                            }
                            if display_row + 1 < total_rows_ctx {
                                if icon_menu_item(ui, "chevron-down", "Move Down", ic, tc, false).clicked() {
                                    move_row(state, display_row, display_row + 1);
                                    ui.close_menu();
                                }
                            }
                            ui.separator();
                            let danger = colors.danger;
                            if icon_menu_item(ui, "delete-row", "Delete Row", danger, danger, false).clicked() {
                                delete_row(state, display_row);
                                ui.close_menu();
                            }
                        });

                        // Resize handle (bottom 4px of gutter)
                        let handle_rect = egui::Rect::from_min_max(
                            egui::pos2(rect.min.x, rect.max.y - 4.0),
                            rect.max,
                        );
                        let handle_resp = ui.allocate_rect(handle_rect, egui::Sense::drag());
                        if handle_resp.hovered() || self.row_resize == Some(display_row) {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
                            ui.painter().rect_filled(handle_rect, 0.0, accent);
                        } else {
                            // Draw only a 1px line at the very bottom instead of the full 4px block
                            let line_rect = egui::Rect::from_min_max(
                                egui::pos2(handle_rect.min.x, handle_rect.max.y - 1.0),
                                handle_rect.max,
                            );
                            ui.painter().rect_filled(line_rect, 0.0, border);
                        }
                        if handle_resp.drag_started() {
                            self.row_resize = Some(display_row);
                            self.row_resize_start_y = handle_resp.interact_pointer_pos()
                                .map(|p| p.y)
                                .unwrap_or(0.0);
                            self.row_resize_start_h = row_h;
                        }
                        if handle_resp.dragged() {
                            if self.row_resize == Some(display_row) {
                                let dy = handle_resp.drag_delta().y;
                                let new_h = (self.row_resize_start_h + dy).max(16.0);
                                self.row_resize_start_h = new_h;
                                state.row_heights.insert(display_row, new_h);
                                state.invalidate_row_layout();
                            }
                        }
                        if handle_resp.drag_stopped() {
                            self.row_resize = None;
                        }
                    });

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

                            let bg = if cell_sel || row_sel { sel_color }
                                     else if has_edit { edit_color }
                                     else { surf };

                            if is_editing {
                                // In-place editing: same background and border as a normal cell.
                                let edit_rect = ui.max_rect();
                                ui.painter().rect_filled(edit_rect, 0.0, bg);
                                paint_bottom_border(ui, edit_rect, border);
                                // Restore spacing so the TextEdit renders at normal height.
                                ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
                                let desired_w = ui.available_width() - 4.0;
                                let buf = &mut self.editing.as_mut().unwrap().2;
                                // Stable explicit ID prevents ID churn when the editing cell
                                // changes row/col, which would otherwise drop focus.
                                let edit_id = egui::Id::new(("edit", display_row as u64, col_idx as u64));
                                let te = egui::TextEdit::singleline(buf)
                                    .id(edit_id)
                                    .font(egui::FontId::proportional(12.0))
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
                                paint_bottom_border(ui, resp.rect, border);
                                let _ = is_cursor;
                                let value = get_cell(state, display_row, col_idx);
                                ui.painter().text(
                                    egui::pos2(resp.rect.left() + 4.0, resp.rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    &value,
                                    egui::FontId::proportional(12.0),
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
                                    ui.set_min_width(130.0);
                                    ui.set_max_width(150.0);
                                    let ic = colors.text_secondary;
                                    let tc = colors.text_primary;
                                    if icon_menu_item(ui, "copy", "Copy", ic, tc, false).clicked() {
                                        ui.ctx().copy_text(value.clone());
                                        ui.close_menu();
                                    }
                                    if icon_menu_item(ui, "edit", "Clear Cell", ic, tc, false).clicked() {
                                        commit_edit(state, display_row, col_idx, String::new());
                                        ui.close_menu();
                                    }
                                    ui.separator();
                                    if icon_menu_item(ui, "sort-asc", "Sort Ascending", ic, tc, false).clicked() {
                                        state.pending_sort = Some((col_idx, true));
                                        ui.close_menu();
                                    }
                                    if icon_menu_item(ui, "sort-desc", "Sort Descending", ic, tc, false).clicked() {
                                        state.pending_sort = Some((col_idx, false));
                                        ui.close_menu();
                                    }
                                    ui.separator();
                                    if icon_menu_item(ui, "chevron-up", "Reset Row Height", ic, tc, false).clicked() {
                                        state.row_heights.remove(&display_row);
                                        state.invalidate_row_layout();
                                        ui.close_menu();
                                    }
                                });
                            }
                        });
                    }
                });
            }); // closes .body()

                // Paint the header border in Order::Middle so it renders on top
                // of body cells but below context menus and tooltips.
                let y = header_bottom_y.get();
                let l = header_left_x.get();
                let r = header_right_x.get();
                if y > 0.0 && r > l {
                    let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::new("header_sep"));
                    let body_clip = ui.clip_rect();
                    let sb = ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_outer_margin + 2.0;
                    let clip = egui::Rect::from_min_max(
                        body_clip.min,
                        egui::pos2(body_clip.max.x - sb, body_clip.max.y - sb),
                    );
                    ui.ctx().layer_painter(layer).with_clip_rect(clip).rect_filled(
                        egui::Rect::from_min_max(egui::pos2(l, y - 4.0), egui::pos2(r, y - 3.0)),
                        0.0,
                        header_sep,
                    );
                }

                // Marching-ants selection border drawn on Order::Middle so it
                // sits on top of cells but below context menus and tooltips.
                if let Some(sr) = sel_rect_acc.get() {
                    if sr.is_finite() && sr.width() > 1.0 && sr.height() > 1.0 {
                        let layer = egui::LayerId::new(
                            egui::Order::Middle,
                            egui::Id::new("sel_dashed"),
                        );
                        // Clip to the table body region so the border can never
                        // bleed over the sticky header above, the status bar
                        // below, or the scrollbars at the edges.
                        let body_clip = ui.clip_rect();
                        let sb = ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_outer_margin + 2.0;
                        let clip = egui::Rect::from_min_max(
                            egui::pos2(body_clip.min.x, header_bottom_y.get()),
                            egui::pos2(body_clip.max.x - sb, body_clip.max.y - sb),
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

        }); // closes ScrollArea::horizontal().show()

        // Sync sel_rect and advance the dash animation for the next frame.
        self.sel_rect = sel_rect_acc.get();
        if self.sel_rect.is_some() && !is_single_cell {
            let dt = ctx.input(|i| i.unstable_dt);
            self.dash_offset = (self.dash_offset - dt * 12.0).rem_euclid(9.0);
            ctx.request_repaint();
        }

        // Copy shortcut
        if self.editing.is_none() {
            let do_copy = ctx.input(|i| i.key_pressed(egui::Key::C) && i.modifiers.command);
            if do_copy {
                ctx.copy_text(selection_to_text(state));
            }
        }
    }

    fn handle_keyboard(&mut self, state: &mut AppState, ctx: &egui::Context) {
        // Clear drag flag when button is released
        if state.is_dragging && ctx.input(|i| !i.pointer.button_down(egui::PointerButton::Primary)) {
            state.is_dragging = false;
        }

        if self.editing.is_some() { return; }

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
    let (path, byte_offset, actual_row, col_count, delimiter, row_edits) = {
        let file = match state.file.as_ref() { Some(f) => f, None => return false };
        let actual_row = match state.display_row_to_actual_row(display_row) { Some(r) => r, None => return false };
        let phys_row = file.virtual_to_actual_row(actual_row);
        let byte_offset = match file.row_offsets.get(phys_row).copied() { Some(o) => o, None => return false };
        let row_edits: Vec<(usize, String)> = file.edits.iter()
            .filter(|(&(r, _), _)| r == actual_row)
            .map(|(&(_, c), v)| (c, v.clone()))
            .collect();
        (file.file_path.clone(), byte_offset, actual_row, file.metadata.columns.len(), file.delimiter, row_edits)
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

fn selection_to_text(state: &AppState) -> String {
    let Some((min_r, max_r, min_c, max_c)) = state.selection_range() else { return String::new() };
    (min_r..=max_r)
        .map(|r| {
            (min_c..=max_c)
                .map(|c| state.get_display_cell(r, c).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    file.inserted_columns.push(format!("Column {}", ins_idx + 1));
    let at = at.min(file.col_order.as_ref().map(|o| o.len()).unwrap_or(0));
    if let Some(ref mut order) = file.col_order {
        order.insert(at, crate::state::ColSource::Inserted(ins_idx));
    }
    // Also add a metadata entry so the header label renders correctly.
    let new_col_idx = file.metadata.columns.len();
    file.metadata.columns.push(crate::csv_engine::types::CsvColumn {
        index: new_col_idx,
        name: format!("Column {}", ins_idx + 1),
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
