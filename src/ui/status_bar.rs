#![allow(dead_code)]

use gpui::*;

use crate::state::{AppState, SelectionType, SortDirection};
use crate::ui::theme::ThemeColors;

pub struct StatusBar {
    pub state: Entity<AppState>,
}

impl StatusBar {
    fn format_size(bytes: u64) -> String {
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else if bytes < 1024 * 1024 * 1024 {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    }

    fn format_compact(n: usize) -> String {
        if n < 1_000 {
            n.to_string()
        } else if n < 1_000_000 {
            format!("{:.1}K", n as f64 / 1_000.0)
        } else {
            format!("{:.1}M", n as f64 / 1_000_000.0)
        }
    }

    fn format_num(n: f64) -> String {
        if n == n.floor() && n.abs() < 1e12 {
            format!("{}", n as i64)
        } else {
            format!("{:.2}", n)
        }
    }

    fn dot(colors: &ThemeColors) -> Div {
        div()
            .text_color(colors.text_tertiary.opacity(0.3))
            .child("\u{00B7}")
    }

    /// Compute stats for cell range selection (data rows only, never header)
    fn compute_cell_stats(state: &AppState) -> Option<(usize, usize, f64, f64, f64, f64)> {
        let (mr, xr, mc, xc) = state.selection_range()?;
        Self::compute_range_stats(state, mr, xr, mc, xc)
    }

    /// Compute stats for row selection
    fn compute_row_stats(state: &AppState) -> Option<(usize, usize, f64, f64, f64, f64)> {
        if state.selected_rows.is_empty() {
            return None;
        }
        let cols = state.col_count();
        if cols == 0 {
            return None;
        }
        let mut count = 0usize;
        let mut num_count = 0usize;
        let mut sum = 0.0f64;
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for &r in &state.selected_rows {
            if let Some(row) = state.get_cached_row(r) {
                for c in 0..cols {
                    Self::accumulate(
                        row.get(c),
                        &mut count,
                        &mut num_count,
                        &mut sum,
                        &mut min,
                        &mut max,
                    );
                }
            }
        }
        Self::finalize(count, num_count, sum, min, max)
    }

    /// Compute stats for column selection (all data rows, excluding header)
    fn compute_col_stats(state: &AppState) -> Option<(usize, usize, f64, f64, f64, f64)> {
        if state.selected_columns.is_empty() {
            return None;
        }
        let total = state.effective_row_count();
        let mut count = 0usize;
        let mut num_count = 0usize;
        let mut sum = 0.0f64;
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for r in 0..total {
            if let Some(row) = state.get_cached_row(r) {
                for &c in &state.selected_columns {
                    Self::accumulate(
                        row.get(c),
                        &mut count,
                        &mut num_count,
                        &mut sum,
                        &mut min,
                        &mut max,
                    );
                }
            }
        }
        Self::finalize(count, num_count, sum, min, max)
    }

    fn compute_range_stats(
        state: &AppState,
        mr: usize,
        xr: usize,
        mc: usize,
        xc: usize,
    ) -> Option<(usize, usize, f64, f64, f64, f64)> {
        let mut count = 0usize;
        let mut num_count = 0usize;
        let mut sum = 0.0f64;
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        for r in mr..=xr {
            if let Some(row) = state.get_cached_row(r) {
                for c in mc..=xc {
                    Self::accumulate(
                        row.get(c),
                        &mut count,
                        &mut num_count,
                        &mut sum,
                        &mut min,
                        &mut max,
                    );
                }
            }
        }
        Self::finalize(count, num_count, sum, min, max)
    }

    fn accumulate(
        val: Option<&String>,
        count: &mut usize,
        num_count: &mut usize,
        sum: &mut f64,
        min: &mut f64,
        max: &mut f64,
    ) {
        if let Some(val) = val {
            *count += 1;
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                if let Ok(n) = trimmed.parse::<f64>() {
                    if n.is_finite() {
                        *num_count += 1;
                        *sum += n;
                        if n < *min {
                            *min = n;
                        }
                        if n > *max {
                            *max = n;
                        }
                    }
                }
            }
        }
    }

    fn finalize(
        count: usize,
        num_count: usize,
        sum: f64,
        min: f64,
        max: f64,
    ) -> Option<(usize, usize, f64, f64, f64, f64)> {
        if count == 0 {
            return None;
        }
        let avg = if num_count > 0 {
            sum / num_count as f64
        } else {
            0.0
        };
        let min = if num_count > 0 && min.is_finite() {
            min
        } else {
            0.0
        };
        let max = if num_count > 0 && max.is_finite() {
            max
        } else {
            0.0
        };
        Some((count, num_count, sum, avg, min, max))
    }

    fn render_stats(colors: &ThemeColors, stats: (usize, usize, f64, f64, f64, f64)) -> Div {
        let (count, num_count, sum, avg, min, max) = stats;
        let mut center = div().flex().items_center().gap(px(10.0));

        if num_count > 0 {
            center = center
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .child(div().text_color(colors.text_tertiary).child("sum"))
                        .child(
                            div()
                                .text_color(colors.text_secondary)
                                .child(Self::format_num(sum)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .child(div().text_color(colors.text_tertiary).child("avg"))
                        .child(
                            div()
                                .text_color(colors.text_secondary)
                                .child(Self::format_num(avg)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .child(div().text_color(colors.text_tertiary).child("min"))
                        .child(
                            div()
                                .text_color(colors.text_secondary)
                                .child(Self::format_num(min)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(3.0))
                        .child(div().text_color(colors.text_tertiary).child("max"))
                        .child(
                            div()
                                .text_color(colors.text_secondary)
                                .child(Self::format_num(max)),
                        ),
                );
        }

        center = center.child(
            div()
                .flex()
                .items_center()
                .gap(px(3.0))
                .child(div().text_color(colors.text_tertiary).child("count"))
                .child(
                    div()
                        .text_color(colors.text_secondary)
                        .child(Self::format_compact(count)),
                ),
        );

        if num_count > 0 && num_count < count {
            center = center.child(div().text_color(colors.text_tertiary).child(format!(
                "\u{00B7} {} numeric",
                Self::format_compact(num_count)
            )));
        }

        center
    }
}

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let colors = state.current_theme();

        if state.file.is_none() {
            return div()
                .h(px(26.0))
                .flex_shrink_0()
                .border_t_1()
                .border_color(colors.border)
                .bg(colors.bg)
                .into_any_element();
        }

        let file = state.file.as_ref().expect("file should exist when rendering status bar");
        let row_count = state.effective_row_count();
        let col_count = state.col_count();
        let file_size = file.metadata.file_size_bytes;

        let row_text = if state.has_filter {
            format!(
                "{} / {} rows",
                Self::format_compact(row_count),
                Self::format_compact(state.unfiltered_row_count)
            )
        } else {
            format!("{} rows", Self::format_compact(row_count))
        };

        let zone_left = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .flex_shrink_0()
            .child(if state.has_filter {
                div().text_color(colors.accent).child(row_text)
            } else {
                div().child(row_text)
            })
            .child(Self::dot(&colors))
            .child(format!("{} cols", col_count))
            .child(Self::dot(&colors))
            .child(Self::format_size(file_size));

        // ── Zone 2: Center — selection context ──
        // Use async computed_stats when available, otherwise try cache-based stats
        let zone_center = match &state.selection_type {
            Some(SelectionType::Cell) => {
                if let Some((mr, xr, mc, xc)) = state.selection_range() {
                    if mr == xr && mc == xc {
                        // Single cell info
                        let col_name = file
                            .metadata
                            .columns
                            .get(mc)
                            .map(|c| c.name.clone())
                            .unwrap_or_default();
                        let truncated = if col_name.len() > 20 {
                            format!("{}\u{2026}", &col_name[..19])
                        } else {
                            col_name
                        };
                        let row_num = mr + 1;
                        let char_count = state
                            .get_cached_row(mr)
                            .and_then(|r| r.get(mc))
                            .map(|v| v.len())
                            .unwrap_or(0);

                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_color(colors.text_secondary)
                            .child(truncated)
                            .child(Self::dot(&colors))
                            .child(format!("row {}", row_num))
                            .child(Self::dot(&colors))
                            .child(format!("{} chars", char_count))
                    } else if state.computing_stats {
                        div()
                            .text_color(colors.text_tertiary)
                            .child("Computing\u{2026}")
                    } else if let Some(stats) = state.computed_stats {
                        Self::render_stats(&colors, stats)
                    } else {
                        // Try cache-based stats as fallback for small ranges
                        match Self::compute_cell_stats(state) {
                            Some(stats) => Self::render_stats(&colors, stats),
                            None => div(),
                        }
                    }
                } else {
                    div()
                }
            }
            Some(SelectionType::Row) => {
                let n = state.selected_rows.len();
                if n > 0 {
                    let mut row_center = div().flex().items_center().gap(px(10.0)).child(
                        div().text_color(colors.text_secondary).child(format!(
                            "{} row{}",
                            n,
                            if n > 1 { "s" } else { "" }
                        )),
                    );

                    if state.computing_stats {
                        row_center = row_center.child(Self::dot(&colors)).child(
                            div()
                                .text_color(colors.text_tertiary)
                                .child("Computing\u{2026}"),
                        );
                    } else if let Some(stats) = state.computed_stats {
                        row_center = row_center
                            .child(Self::dot(&colors))
                            .child(Self::render_stats(&colors, stats));
                    } else if let Some(stats) = Self::compute_row_stats(state) {
                        row_center = row_center
                            .child(Self::dot(&colors))
                            .child(Self::render_stats(&colors, stats));
                    }
                    row_center
                } else {
                    div()
                }
            }
            Some(SelectionType::Column) => {
                let n = state.selected_columns.len();
                if n > 0 {
                    let mut col_center = div().flex().items_center().gap(px(10.0)).child(
                        div().text_color(colors.text_secondary).child(format!(
                            "{} col{}",
                            n,
                            if n > 1 { "s" } else { "" }
                        )),
                    );

                    if state.computing_stats {
                        col_center = col_center.child(Self::dot(&colors)).child(
                            div()
                                .text_color(colors.text_tertiary)
                                .child("Computing\u{2026}"),
                        );
                    } else if let Some(stats) = state.computed_stats {
                        col_center = col_center
                            .child(Self::dot(&colors))
                            .child(Self::render_stats(&colors, stats));
                    }
                    col_center
                } else {
                    div()
                }
            }
            None => div(),
        };

        // ── Zone 3: Right — app state ──
        let mut zone_right = div().flex().items_center().gap(px(8.0)).flex_shrink_0();

        if let Some(ref sort) = state.sort_state {
            let arrow = if sort.direction == SortDirection::Asc {
                "\u{2191}"
            } else {
                "\u{2193}"
            };
            let col_name = file
                .metadata
                .columns
                .get(sort.column_index)
                .map(|c| {
                    if c.name.len() > 15 {
                        format!("{}\u{2026}", &c.name[..14])
                    } else {
                        c.name.clone()
                    }
                })
                .unwrap_or_default();

            zone_right = zone_right.child(
                div()
                    .px(px(7.0))
                    .py(px(1.0))
                    .bg(colors.border)
                    .rounded(px(4.0))
                    .text_size(px(10.0))
                    .text_color(colors.text_secondary)
                    .child(format!("{} {}", arrow, col_name)),
            );
        }

        if state.has_filter {
            zone_right = zone_right.child(
                div()
                    .px(px(7.0))
                    .py(px(1.0))
                    .bg(colors.border)
                    .rounded(px(4.0))
                    .text_size(px(10.0))
                    .text_color(colors.accent)
                    .child("\u{2298} filtered"),
            );
        }

        div()
            .flex()
            .items_center()
            .justify_between()
            .px(px(12.0))
            .h(px(26.0))
            .flex_shrink_0()
            .border_t_1()
            .border_color(colors.border)
            .bg(colors.bg)
            .text_size(px(11.0))
            .text_color(colors.text_tertiary)
            .child(zone_left)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_center()
                    .min_w_0()
                    .child(zone_center),
            )
            .child(zone_right)
            .into_any_element()
    }
}
