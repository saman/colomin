//! Selection statistics for the status bar badge.
//!
//! Returns `(count, numeric_count, sum, avg, min, max, char_len)` for the
//! current selection. Computation is synchronous; the caller decides whether
//! to run it inline or on a background thread.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::state::{AppState, SelectionType};

pub type Stats = (usize, usize, f64, f64, f64, f64, usize);

pub fn compute_stats(state: &AppState) -> Option<Stats> {
    match state.selection_type.as_ref()? {
        SelectionType::Cell => compute_cell_stats(state),
        SelectionType::Row => compute_row_stats(state),
        SelectionType::Column => compute_col_stats(state),
    }
}

fn compute_cell_stats(state: &AppState) -> Option<Stats> {
    let (mr, xr, mc, xc) = state.selection_range()?;
    compute_range_stats(state, mr, xr, mc, xc)
}

fn compute_row_stats(state: &AppState) -> Option<Stats> {
    if state.selected_rows.is_empty() {
        return None;
    }
    let cols = state.col_count();
    if cols == 0 {
        return None;
    }
    let mut acc = Acc::default();
    for &r in &state.selected_rows {
        if let Some(row) = state.get_display_row(r) {
            for c in 0..cols {
                accumulate(row.get(c), &mut acc);
            }
        }
    }
    finalize(acc)
}

fn compute_col_stats(state: &AppState) -> Option<Stats> {
    if state.selected_columns.is_empty() {
        return None;
    }
    let total = state.display_row_count();
    let mut acc = Acc::default();
    for r in 0..total {
        if let Some(row) = state.get_display_row(r) {
            for &c in &state.selected_columns {
                let phys = state.display_to_physical_col(c);
                accumulate(row.get(phys), &mut acc);
            }
        }
    }
    finalize(acc)
}

fn compute_range_stats(
    state: &AppState,
    mr: usize,
    xr: usize,
    mc: usize,
    xc: usize,
) -> Option<Stats> {
    let mut acc = Acc::default();
    for r in mr..=xr {
        if let Some(row) = state.get_display_row(r) {
            for c in mc..=xc {
                let phys = state.display_to_physical_col(c);
                accumulate(row.get(phys), &mut acc);
            }
        }
    }
    finalize(acc)
}

struct Acc {
    count: usize,
    num_count: usize,
    sum: f64,
    min: f64,
    max: f64,
    char_len: usize,
}

impl Default for Acc {
    fn default() -> Self {
        Self {
            count: 0,
            num_count: 0,
            sum: 0.0,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            char_len: 0,
        }
    }
}

fn accumulate(val: Option<&String>, acc: &mut Acc) {
    if let Some(val) = val {
        acc.count += 1;
        acc.char_len += val.chars().count();
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            if let Ok(n) = trimmed.parse::<f64>() {
                if n.is_finite() {
                    acc.num_count += 1;
                    acc.sum += n;
                    if n < acc.min {
                        acc.min = n;
                    }
                    if n > acc.max {
                        acc.max = n;
                    }
                }
            }
        }
    }
}

fn finalize(acc: Acc) -> Option<Stats> {
    if acc.count == 0 {
        return None;
    }
    let avg = if acc.num_count > 0 {
        acc.sum / acc.num_count as f64
    } else {
        0.0
    };
    let min = if acc.num_count > 0 && acc.min.is_finite() {
        acc.min
    } else {
        0.0
    };
    let max = if acc.num_count > 0 && acc.max.is_finite() {
        acc.max
    } else {
        0.0
    };
    Some((acc.count, acc.num_count, acc.sum, avg, min, max, acc.char_len))
}

// ── Formatters ────────────────────────────────────────────────────────────────

pub fn format_compact(n: usize) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

pub fn format_num(n: f64) -> String {
    if n == n.floor() && n.abs() < 1e12 {
        format!("{}", n as i64)
    } else {
        format!("{:.2}", n)
    }
}

/// Size in cells of the current selection (used to decide sync vs async compute).
pub fn selection_cell_count(state: &AppState) -> usize {
    match state.selection_type.as_ref() {
        Some(SelectionType::Cell) => state
            .selection_range()
            .map(|(mr, xr, mc, xc)| (xr - mr + 1) * (xc - mc + 1))
            .unwrap_or(0),
        Some(SelectionType::Row) => state.selected_rows.len() * state.col_count(),
        Some(SelectionType::Column) => state.selected_columns.len() * state.display_row_count(),
        None => 0,
    }
}

/// Pick the value field for the currently preferred stat. Returns a display
/// string and a boolean indicating whether the value is meaningful (e.g. Sum
/// of a non-numeric range returns `("-", false)`).
pub fn format_stat(stats: Stats, preferred: crate::state::PreferredStat) -> (String, bool) {
    let (count, num_count, sum, avg, min, max, char_len) = stats;
    use crate::state::PreferredStat::*;
    match preferred {
        Count => (format_compact(count), true),
        Sum if num_count > 0 => (format_num(sum), true),
        Avg if num_count > 0 => (format_num(avg), true),
        Min if num_count > 0 => (format_num(min), true),
        Max if num_count > 0 => (format_num(max), true),
        Length => (format_compact(char_len), true),
        _ => ("-".to_string(), false),
    }
}


// ── Async computation ─────────────────────────────────────────────────────────

/// A cheap-to-clone snapshot of the state fields needed for stats computation
/// on a background thread. Does NOT hold the row cache (too expensive to clone
/// for large files) — instead reads directly from disk like the sort does.
pub struct StatsSnapshot {
    pub selection_type: SelectionType,
    pub selected_rows: Vec<usize>,
    pub selected_columns: Vec<usize>,
    pub cell_range: Option<(usize, usize, usize, usize)>,
    pub file_path: PathBuf,
    pub row_offsets: Vec<u64>,
    pub sort_permutation: Option<Vec<usize>>,
    pub filter_indices: Option<Vec<usize>>,
    pub edits: HashMap<(usize, usize), String>,
    pub col_count: usize,
    pub total_rows: usize,
    pub delimiter: u8,
    pub header_row_enabled: bool,
    /// display_col → physical_col mapping (empty vec = identity).
    pub display_to_physical: Vec<usize>,
}

impl StatsSnapshot {
    pub fn from(state: &AppState) -> Self {
        let file = state.file.as_ref().expect("stats snapshot requires open file");
        let phys_col_count = file.metadata.columns.len();
        let display_to_physical: Vec<usize> = (0..file.current_col_count())
            .map(|d| state.display_to_physical_col(d))
            .collect();
        StatsSnapshot {
            selection_type: state
                .selection_type
                .clone()
                .unwrap_or(SelectionType::Cell),
            selected_rows: state.selected_rows.clone(),
            selected_columns: state.selected_columns.clone(),
            cell_range: state.selection_range(),
            file_path: file.file_path.clone(),
            row_offsets: file.row_offsets.clone(),
            sort_permutation: file.sort_permutation.clone(),
            filter_indices: file.filter_indices.clone(),
            edits: file.edits.clone(),
            col_count: phys_col_count,
            total_rows: file.metadata.total_rows,
            delimiter: file.delimiter,
            header_row_enabled: state.header_row_enabled,
            display_to_physical,
        }
    }

    fn snap_physical_col(&self, display_col: usize) -> usize {
        self.display_to_physical.get(display_col).copied().unwrap_or(display_col)
    }
}

fn display_to_actual(snap: &StatsSnapshot, display_row: usize) -> Option<usize> {
    if !snap.header_row_enabled {
        if display_row == 0 {
            return None;
        }
        return Some(display_row - 1);
    }
    Some(display_row)
}

fn display_row_count(snap: &StatsSnapshot) -> usize {
    let data = if let Some(ref fi) = snap.filter_indices {
        fi.len()
    } else {
        snap.total_rows
    };
    if !snap.header_row_enabled {
        data + 1
    } else {
        data
    }
}

/// Map an actual (logical) row index to the physical row offset in the file.
fn actual_to_file_offset(snap: &StatsSnapshot, actual_row: usize) -> Option<u64> {
    let phys_row = snap
        .sort_permutation
        .as_ref()
        .map(|p| p.get(actual_row).copied().unwrap_or(actual_row))
        .unwrap_or(actual_row);
    snap.row_offsets.get(phys_row).copied()
}

/// Compute stats by opening the file **once** and seeking forward through it.
///
/// `rows` must be sorted by `file_offset` (ascending) so we only ever seek
/// forward — backward seeks flush the BufReader and negate its benefit.
/// Each entry is `(actual_row_index, file_offset)`.
fn compute_stats_streaming(
    snap: &StatsSnapshot,
    mut rows: Vec<(usize, u64)>,
    col_selector: &ColSelector,
) -> Acc {
    use std::io::{BufReader, Seek, SeekFrom};

    // Sort by file offset for forward-only sequential I/O.
    rows.sort_unstable_by_key(|&(_, off)| off);

    let mut acc = Acc::default();
    let Ok(fh) = std::fs::File::open(&snap.file_path) else { return acc };
    // 256 KB read-ahead: enough for many rows per chunk, not wasteful on seeks.
    let mut reader = BufReader::with_capacity(256 * 1024, fh);
    let mut prev_offset: Option<u64> = None;

    for (actual_row, offset) in rows {
        // Only seek if we're not already at the right position.
        // For contiguous rows the BufReader keeps the data hot.
        if prev_offset.map_or(true, |p| p != offset) {
            if reader.seek(SeekFrom::Start(offset)).is_err() {
                continue;
            }
        }

        // Parse exactly one CSV record from the current position.
        let mut csv_rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .delimiter(snap.delimiter)
            .from_reader(&mut reader);
        let record = match csv_rdr.records().next() {
            Some(Ok(r)) => r,
            _ => continue,
        };
        // Track approximate next position (csv_rdr consumed bytes from reader).
        // We drop csv_rdr here; the BufReader cursor has advanced past this row.
        drop(csv_rdr);

        match col_selector {
            ColSelector::All => {
                for c in 0..snap.col_count {
                    let val = snap.edits.get(&(actual_row, c))
                        .map(|s| s.as_str())
                        .or_else(|| record.get(c));
                    accumulate(val.map(String::from).as_ref(), &mut acc);
                }
            }
            ColSelector::Range(mc, xc) => {
                for c in *mc..=*xc {
                    let phys = snap.snap_physical_col(c);
                    let val = snap.edits.get(&(actual_row, phys))
                        .map(|s| s.as_str())
                        .or_else(|| record.get(phys));
                    accumulate(val.map(String::from).as_ref(), &mut acc);
                }
            }
            ColSelector::Set(cols) => {
                for &c in cols {
                    let phys = snap.snap_physical_col(c);
                    let val = snap.edits.get(&(actual_row, phys))
                        .map(|s| s.as_str())
                        .or_else(|| record.get(phys));
                    accumulate(val.map(String::from).as_ref(), &mut acc);
                }
            }
        }

        // Next iteration: we don't know the exact offset after the record
        // because BufReader's internal cursor has advanced an unknown amount.
        // Setting prev_offset to None forces a seek on the next row unless
        // rows happen to be contiguous (handled by the sort ordering).
        prev_offset = None; // will seek next time (safe / correct)
    }
    acc
}

enum ColSelector {
    All,
    Range(usize, usize),
    Set(Vec<usize>),
}

/// Fast streaming path used when rows are in ascending file order with no gaps.
/// Opens the file once, seeks to the first row, and reads sequentially — no
/// per-row seek overhead. This is the hot path for full-column or full-range
/// stats on unsorted files.
fn compute_stats_sequential(
    snap: &StatsSnapshot,
    first_actual: usize,
    last_actual: usize,
    col_selector: &ColSelector,
) -> Acc {
    use std::io::{BufReader, Seek, SeekFrom};

    let Some(&start_offset) = snap.row_offsets.get(first_actual) else { return Acc::default() };

    let Ok(fh) = std::fs::File::open(&snap.file_path) else { return Acc::default() };
    let mut reader = BufReader::with_capacity(256 * 1024, fh);
    if reader.seek(SeekFrom::Start(start_offset)).is_err() {
        return Acc::default();
    }

    let mut csv_rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(snap.delimiter)
        .from_reader(reader);

    let mut acc = Acc::default();

    for (row_idx, result) in csv_rdr.records().enumerate() {
        let actual_row = first_actual + row_idx;
        if actual_row > last_actual {
            break;
        }
        let Ok(record) = result else { continue };

        match col_selector {
            ColSelector::All => {
                for c in 0..snap.col_count {
                    let val = snap.edits.get(&(actual_row, c))
                        .cloned()
                        .or_else(|| record.get(c).map(str::to_string));
                    accumulate(val.as_ref(), &mut acc);
                }
            }
            ColSelector::Range(mc, xc) => {
                for c in *mc..=*xc {
                    let phys = snap.snap_physical_col(c);
                    let val = snap.edits.get(&(actual_row, phys))
                        .cloned()
                        .or_else(|| record.get(phys).map(str::to_string));
                    accumulate(val.as_ref(), &mut acc);
                }
            }
            ColSelector::Set(cols) => {
                for &c in cols {
                    let phys = snap.snap_physical_col(c);
                    let val = snap.edits.get(&(actual_row, phys))
                        .cloned()
                        .or_else(|| record.get(phys).map(str::to_string));
                    accumulate(val.as_ref(), &mut acc);
                }
            }
        }
    }
    acc
}

pub fn compute_stats_snapshot(snap: &StatsSnapshot) -> Stats {
    let acc = match snap.selection_type {
        SelectionType::Cell => {
            let Some((mr, xr, mc, xc)) = snap.cell_range else {
                return (0, 0, 0.0, 0.0, 0.0, 0.0, 0);
            };
            // Collect actual rows in display order.
            let actuals: Vec<usize> = (mr..=xr)
                .filter_map(|r| display_to_actual(snap, r))
                .collect();
            // If no sort permutation the actual rows are consecutive file rows.
            if snap.sort_permutation.is_none() && !actuals.is_empty() {
                compute_stats_sequential(snap, *actuals.first().unwrap(), *actuals.last().unwrap(), &ColSelector::Range(mc, xc))
            } else {
                let rows: Vec<(usize, u64)> = actuals.into_iter()
                    .filter_map(|a| actual_to_file_offset(snap, a).map(|off| (a, off)))
                    .collect();
                compute_stats_streaming(snap, rows, &ColSelector::Range(mc, xc))
            }
        }
        SelectionType::Row => {
            let mut actuals: Vec<usize> = snap.selected_rows.iter()
                .filter_map(|&r| display_to_actual(snap, r))
                .collect();
            actuals.sort_unstable();
            if snap.sort_permutation.is_none() && !actuals.is_empty() {
                // Rows may not be contiguous — use scatter path (one file open, seek per row).
                let rows: Vec<(usize, u64)> = actuals.into_iter()
                    .filter_map(|a| actual_to_file_offset(snap, a).map(|off| (a, off)))
                    .collect();
                compute_stats_streaming(snap, rows, &ColSelector::All)
            } else {
                let rows: Vec<(usize, u64)> = actuals.into_iter()
                    .filter_map(|a| actual_to_file_offset(snap, a).map(|off| (a, off)))
                    .collect();
                compute_stats_streaming(snap, rows, &ColSelector::All)
            }
        }
        SelectionType::Column => {
            let total = display_row_count(snap);
            // Fast path: no permutation → rows 0..total are consecutive in the file.
            if snap.sort_permutation.is_none() {
                let first_actual = display_to_actual(snap, if snap.header_row_enabled { 0 } else { 1 });
                let last_actual  = display_to_actual(snap, total.saturating_sub(1));
                if let (Some(first), Some(last)) = (first_actual, last_actual) {
                    compute_stats_sequential(snap, first, last, &ColSelector::Set(snap.selected_columns.clone()))
                } else {
                    Acc::default()
                }
            } else {
                // Permuted: rows are scattered — use scatter path.
                let rows: Vec<(usize, u64)> = (0..total)
                    .filter_map(|r| {
                        let actual = display_to_actual(snap, r)?;
                        let off = actual_to_file_offset(snap, actual)?;
                        Some((actual, off))
                    })
                    .collect();
                compute_stats_streaming(snap, rows, &ColSelector::Set(snap.selected_columns.clone()))
            }
        }
    };
    finalize(acc).unwrap_or((0, 0, 0.0, 0.0, 0.0, 0.0, 0))
}
