use gpui::{Context, Entity};

use crate::csv_engine::parser;
use crate::state::{AppState, SelectionType};

use super::TableView;

pub(super) fn selection_stats_key(state: &AppState) -> String {
    match &state.selection_type {
        Some(SelectionType::Column) => {
            let mut cols: Vec<usize> = state.selected_columns.iter().copied().collect();
            cols.sort();
            format!("col:{:?}", cols)
        }
        Some(SelectionType::Row) => {
            let mut rows: Vec<usize> = state.selected_rows.iter().copied().collect();
            rows.sort();
            format!("row:{:?}", rows)
        }
        Some(SelectionType::Cell) => {
            if let Some((mr, xr, mc, xc)) = state.selection_range() {
                format!("cell:{}-{}-{}-{}", mr, xr, mc, xc)
            } else {
                String::new()
            }
        }
        None => String::new(),
    }
}

pub(super) fn maybe_compute_stats(state_entity: &Entity<AppState>, cx: &mut Context<TableView>) {
    // Do the entire check-and-set in a single update to prevent race conditions
    // that spawn thousands of background threads
    let task_data = state_entity.update(cx, |state, _| {
        let key = selection_stats_key(state);

        // No selection or single cell — no async stats needed
        if key.is_empty() {
            return None;
        }
        if let Some(SelectionType::Cell) = &state.selection_type {
            if let Some((mr, xr, mc, xc)) = state.selection_range() {
                if mr == xr && mc == xc {
                    return None;
                }
                let total_cells = (xr - mr + 1) * (xc - mc + 1);
                if total_cells < 500 {
                    let all_cached = (mr..=xr).all(|r| state.get_cached_row(r).is_some());
                    if all_cached {
                        return None;
                    }
                }
            }
        }

        // If already computing/computed for this exact key, skip
        if state.stats_key == key && (state.computing_stats || state.computed_stats.is_some()) {
            return None;
        }

        let file = match &state.file {
            Some(f) => f,
            None => return None,
        };

        let data = (
            file.file_path.clone(),
            file.row_offsets.clone(),
            file.edits.clone(),
            file.delimiter,
            file.metadata.columns.len(),
            state.selection_type.clone(),
            state.selected_columns.clone(),
            state.selected_rows.clone(),
            state.selection_range(),
        );

        // Mark as computing NOW, atomically with the check
        state.computing_stats = true;
        state.computed_stats = None;
        state.stats_key = key.clone();

        Some((key, data))
    });

    let (key, (path, row_offsets, edits, delimiter, col_count, sel_type, sel_cols, sel_rows, sel_range)) =
        match task_data {
            Some((k, d)) => (k, d),
            None => return,
        };

    let se = state_entity.clone();
    let key_for_task = key.clone();

    cx.spawn(async move |_, cx| {
        let result = std::thread::spawn(move || match sel_type {
            Some(SelectionType::Column) => {
                let mut count = 0usize;
                let mut num_count = 0usize;
                let mut sum = 0.0f64;
                let mut min = f64::INFINITY;
                let mut max = f64::NEG_INFINITY;

                for &col_idx in &sel_cols {
                    if let Ok(stats) =
                        parser::aggregate_column(&path, col_idx, &row_offsets, &edits, delimiter)
                    {
                        count += stats.count;
                        num_count += stats.numeric_count;
                        if let Some(s) = stats.sum {
                            sum += s;
                        }
                        if let Some(m) = stats.min {
                            if m < min {
                                min = m;
                            }
                        }
                        if let Some(m) = stats.max {
                            if m > max {
                                max = m;
                            }
                        }
                    }
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
            Some(SelectionType::Row) | Some(SelectionType::Cell) => {
                // Stream the entire file sequentially and pick out the rows we need
                let is_row_sel = matches!(sel_type, Some(SelectionType::Row));
                let selected_row_set: std::collections::HashSet<usize> = if is_row_sel {
                    sel_rows.iter().copied().collect()
                } else {
                    std::collections::HashSet::new()
                };
                let (mr, xr, mc, xc) = if is_row_sel {
                    (
                        0,
                        row_offsets.len().saturating_sub(1),
                        0,
                        col_count.saturating_sub(1),
                    )
                } else {
                    match sel_range {
                        Some(r) => r,
                        None => return None,
                    }
                };

                let mut count = 0usize;
                let mut num_count = 0usize;
                let mut sum = 0.0f64;
                let mut min = f64::INFINITY;
                let mut max = f64::NEG_INFINITY;

                let f = std::fs::File::open(&path).ok()?;
                let mut buf_reader = std::io::BufReader::new(f);
                if !row_offsets.is_empty() {
                    use std::io::Seek;
                    buf_reader
                        .seek(std::io::SeekFrom::Start(row_offsets[mr]))
                        .ok()?;
                }
                let mut csv_rdr = csv::ReaderBuilder::new()
                    .has_headers(false)
                    .flexible(true)
                    .delimiter(delimiter)
                    .from_reader(buf_reader);

                for (i, result) in csv_rdr.records().enumerate() {
                    let row_idx = mr + i;
                    if row_idx > xr {
                        break;
                    }
                    let record = match result {
                        Ok(r) => r,
                        Err(_) => continue,
                    };

                    // Check if this row is in our selection
                    let in_selection = if is_row_sel {
                        selected_row_set.contains(&row_idx)
                    } else {
                        true
                    };
                    if !in_selection {
                        continue;
                    }

                    let col_start = if is_row_sel { 0 } else { mc };
                    let col_end = if is_row_sel {
                        col_count.saturating_sub(1)
                    } else {
                        xc
                    };

                    for ci in col_start..=col_end {
                        let val = if let Some(edited) = edits.get(&(row_idx, ci)) {
                            edited.as_str()
                        } else {
                            record.get(ci).unwrap_or("")
                        };
                        count += 1;
                        let trimmed = val.trim();
                        if !trimmed.is_empty() {
                            if let Ok(n) = trimmed.parse::<f64>() {
                                if n.is_finite() {
                                    num_count += 1;
                                    sum += n;
                                    if n < min {
                                        min = n;
                                    }
                                    if n > max {
                                        max = n;
                                    }
                                }
                            }
                        }
                    }
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
            None => None,
        })
        .join()
        .ok()
        .flatten();

        let _ = se.update(cx, |s, cx| {
            // Only apply if the key still matches (selection didn't change while computing)
            if s.stats_key == key_for_task {
                s.computed_stats = result;
                s.computing_stats = false;
                cx.notify();
            }
        });
    })
    .detach();
}
