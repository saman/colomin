#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::{CellCoord, EditAction, OpenFile, PreferredStat, SelectionType, SortState};

/// Cached row position layout. Avoids O(N) prefix-sum recomputation per frame.
pub struct RowLayout {
    /// When true, all rows share the same height (`row_heights` is empty).
    /// Positions are computed with simple multiplication — no Vec needed.
    pub uniform: bool,
    /// Prefix sums of row heights. Only populated when `uniform == false`.
    /// `row_tops[i]` = sum of heights of rows `0..i`. Length = `total_rows + 1`.
    pub row_tops: Vec<f32>,
    /// Total content height (sum of all row heights).
    pub total_height: f32,
    /// Matches `row_layout_version` when this cache is up to date.
    version: u64,
}

/// Cached column layout (total content width).
pub struct ColumnLayout {
    /// Total content width including row-number gutter.
    pub total_width: f32,
    /// The `row_number_width` used when this was last computed.
    row_number_width: f32,
    /// Matches `col_layout_version` when this cache is up to date.
    version: u64,
}

/// The main application state, held as a GPUI Model
pub struct AppState {
    pub file: Option<OpenFile>,
    // Selection
    pub selection_type: Option<SelectionType>,
    pub selected_rows: Vec<usize>,
    pub selected_columns: Vec<usize>,
    pub selection_anchor: Option<CellCoord>,
    pub selection_focus: Option<CellCoord>,
    // Sort/filter
    pub sort_state: Option<SortState>,
    pub has_filter: bool,
    pub unfiltered_row_count: usize,
    // Undo/redo
    pub undo_stack: Vec<EditAction>,
    pub redo_stack: Vec<EditAction>,
    // UI state
    pub column_widths: HashMap<usize, f32>,
    pub default_column_width: f32,
    pub row_height: f32,
    pub row_heights: HashMap<usize, f32>,
    pub search_query: String,
    pub search_results: Vec<usize>,
    pub show_search: bool,
    pub show_command_palette: bool,
    pub toast_message: Option<String>,
    /// Pending sort request (column index, ascending) set by UI actions.
    pub pending_sort: Option<(usize, bool)>,
    /// Loading state for file open
    pub is_loading: bool,
    pub loading_message: String,
    /// Loading progress 0.0–1.0 (updated from background thread via shared atomic)
    pub loading_progress: f32,
    /// Index into the available themes list
    pub theme_index: usize,
    /// Pending edit request from double-click (row, col, value)
    pub editing_cell: Option<(usize, usize, String)>,
    /// Context menu position and target cell (screen x, screen y, row, col)
    pub context_menu: Option<(f32, f32, usize, usize)>,
    /// Settings menu open state
    pub settings_menu: bool,
    /// Whether the settings menu is currently showing the theme list submenu
    pub settings_theme_submenu: bool,
    /// View-only header mode toggle. When false, header labels use Excel letters.
    pub header_row_enabled: bool,
    /// Whether the user is currently dragging to select cells
    pub is_dragging: bool,
    /// Which stat to show by default in the center zone when a range is selected
    pub preferred_stat: PreferredStat,
    /// Whether the stats picker menu is open
    pub stats_menu: bool,
    /// Screen-space X center of the stat badge (for anchoring the stats menu)
    pub stat_badge_center_x: f32,
    /// Async-computed stats for large selections (count, numeric_count, sum, avg, min, max, char_len)
    pub computed_stats: Option<(usize, usize, f64, f64, f64, f64, usize)>,
    /// Whether stats are currently being computed
    pub computing_stats: bool,
    /// Key that identifies the current stats computation (to avoid stale results)
    pub stats_key: String,
    // Row cache
    pub row_cache: HashMap<usize, Vec<String>>,
    row_cache_order: RefCell<VecDeque<usize>>,
    pub cache_version: u64,
    // Layout caches (avoid O(N) recomputation per frame)
    pub row_layout: RowLayout,
    pub col_layout: ColumnLayout,
    row_layout_version: u64,
    col_layout_version: u64,
}

const ROW_CACHE_LIMIT: usize = 5000;
static THEME_MEMORY_INDEX: AtomicUsize = AtomicUsize::new(0);
static THEME_EVER_SET: AtomicBool = AtomicBool::new(false);

/// Detect if macOS is in dark mode via `defaults read`.
fn system_is_dark_mode() -> bool {
    std::process::Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().eq_ignore_ascii_case("dark"))
        .unwrap_or(false)
}

impl AppState {
    pub fn new() -> Self {
        let themes = crate::ui::theme::bundled_themes();
        let theme_count = themes.len().max(1);

        let remembered_theme = if THEME_EVER_SET.load(Ordering::Relaxed) {
            THEME_MEMORY_INDEX.load(Ordering::Relaxed) % theme_count
        } else if system_is_dark_mode() {
            // First launch + system dark mode → pick first dark theme
            let dark_idx = themes.iter().position(|t| {
                matches!(t.appearance, crate::ui::theme::ThemeAppearance::Dark)
            }).unwrap_or(0);
            THEME_MEMORY_INDEX.store(dark_idx, Ordering::Relaxed);
            THEME_EVER_SET.store(true, Ordering::Relaxed);
            dark_idx
        } else {
            0 // Colomin Light
        };
        Self {
            file: None,
            selection_type: None,
            selected_rows: Vec::new(),
            selected_columns: Vec::new(),
            selection_anchor: None,
            selection_focus: None,
            sort_state: None,
            has_filter: false,
            unfiltered_row_count: 0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            column_widths: HashMap::new(),
            default_column_width: 150.0,
            row_height: 28.0,
            row_heights: HashMap::new(),
            search_query: String::new(),
            search_results: Vec::new(),
            show_search: false,
            show_command_palette: false,
            toast_message: None,
            pending_sort: None,
            is_loading: false,
            loading_message: String::new(),
            loading_progress: 0.0,
            theme_index: remembered_theme,
            editing_cell: None,
            context_menu: None,
            settings_menu: false,
            settings_theme_submenu: false,
            header_row_enabled: true,
            is_dragging: false,
            preferred_stat: PreferredStat::Count,
            stats_menu: false,
            stat_badge_center_x: 0.0,
            computed_stats: None,
            computing_stats: false,
            stats_key: String::new(),
            row_cache: HashMap::new(),
            row_cache_order: RefCell::new(VecDeque::new()),
            cache_version: 0,
            row_layout: RowLayout {
                uniform: true,
                row_tops: Vec::new(),
                total_height: 0.0,
                version: 0,
            },
            col_layout: ColumnLayout {
                total_width: 0.0,
                row_number_width: 0.0,
                version: 0,
            },
            row_layout_version: 1, // start at 1 so first ensure_* always computes
            col_layout_version: 1,
        }
    }

    pub fn effective_row_count(&self) -> usize {
        self.file.as_ref().map_or(0, |f| f.effective_row_count())
    }

    pub fn display_row_count(&self) -> usize {
        let data_rows = self.effective_row_count();
        if !self.header_row_enabled && self.file.is_some() {
            data_rows + 1
        } else {
            data_rows
        }
    }

    pub fn display_row_to_actual_row(&self, display_row: usize) -> Option<usize> {
        if !self.header_row_enabled && self.file.is_some() {
            if display_row == 0 {
                None
            } else {
                Some(display_row - 1)
            }
        } else {
            Some(display_row)
        }
    }

    pub fn actual_row_to_display_row(&self, actual_row: usize) -> usize {
        if !self.header_row_enabled && self.file.is_some() {
            actual_row + 1
        } else {
            actual_row
        }
    }

    pub fn get_display_row(&self, display_row: usize) -> Option<Vec<String>> {
        let file = self.file.as_ref()?;
        match self.display_row_to_actual_row(display_row) {
            Some(actual_row) => self.get_cached_row(actual_row).map(|row| row.to_vec()),
            None => Some(file.metadata.columns.iter().map(|c| c.name.clone()).collect()),
        }
    }

    pub fn get_display_cell(&self, display_row: usize, col: usize) -> Option<String> {
        self.get_display_row(display_row)
            .and_then(|row| row.get(col).cloned())
    }

    pub fn col_count(&self) -> usize {
        self.file.as_ref().map_or(0, |f| f.metadata.columns.len())
    }

    pub fn column_width(&self, col: usize) -> f32 {
        self.column_widths.get(&col).copied().unwrap_or(self.default_column_width)
    }

    pub fn row_height_for(&self, display_row: usize) -> f32 {
        self.row_heights.get(&display_row).copied().unwrap_or(self.row_height)
    }

    /// Width of the row-number gutter, scaled to fit the widest row number.
    /// Uses 10px font: ~7px per digit + 20px padding, minimum 36px.
    pub fn row_number_width(&self) -> f32 {
        let total = self.display_row_count().max(1);
        let digits = (total as f64).log10().floor() as usize + 1;
        (digits as f32 * 7.0 + 20.0).max(36.0)
    }

    // ── Layout cache methods ──

    /// Recompute the row layout cache if stale. Fast path for uniform heights.
    pub fn ensure_row_layout(&mut self) {
        if self.row_layout.version == self.row_layout_version {
            return;
        }
        let total = self.display_row_count();
        if self.row_heights.is_empty() {
            self.row_layout = RowLayout {
                uniform: true,
                row_tops: Vec::new(),
                total_height: total as f32 * self.row_height,
                version: self.row_layout_version,
            };
        } else {
            let mut tops = Vec::with_capacity(total + 1);
            tops.push(0.0);
            for ri in 0..total {
                let rh = self.row_height_for(ri);
                tops.push(tops[ri] + rh);
            }
            let total_h = *tops.last().unwrap_or(&0.0);
            self.row_layout = RowLayout {
                uniform: false,
                row_tops: tops,
                total_height: total_h,
                version: self.row_layout_version,
            };
        }
    }

    /// Recompute the column layout cache if stale.
    pub fn ensure_col_layout(&mut self, row_number_width: f32) {
        if self.col_layout.version == self.col_layout_version
            && self.col_layout.row_number_width == row_number_width
        {
            return;
        }
        let total_w = if let Some(file) = &self.file {
            row_number_width
                + file
                    .metadata
                    .columns
                    .iter()
                    .map(|c| self.column_width(c.index))
                    .sum::<f32>()
        } else {
            0.0
        };
        self.col_layout = ColumnLayout {
            total_width: total_w,
            row_number_width,
            version: self.col_layout_version,
        };
    }

    pub fn invalidate_row_layout(&mut self) {
        self.row_layout_version += 1;
    }

    pub fn invalidate_col_layout(&mut self) {
        self.col_layout_version += 1;
    }

    /// O(1) for uniform heights, O(1) Vec lookup for variable.
    pub fn row_top(&self, row: usize) -> f32 {
        if self.row_layout.uniform {
            row as f32 * self.row_height
        } else {
            self.row_layout.row_tops.get(row).copied().unwrap_or(0.0)
        }
    }

    /// Find the row index at a given y-coordinate.
    /// O(1) division for uniform heights, O(log N) binary search for variable.
    pub fn row_at_y(&self, y: f32, total_rows: usize) -> usize {
        if total_rows == 0 {
            return 0;
        }
        if self.row_layout.uniform {
            if self.row_height <= 0.0 {
                return 0;
            }
            ((y / self.row_height) as usize).min(total_rows.saturating_sub(1))
        } else {
            match self
                .row_layout
                .row_tops
                .binary_search_by(|t| t.partial_cmp(&y).unwrap())
            {
                Ok(i) => i.min(total_rows),
                Err(i) => i.saturating_sub(1).min(total_rows),
            }
        }
    }

    pub fn selection_range(&self) -> Option<(usize, usize, usize, usize)> {
        if self.selection_type != Some(SelectionType::Cell) {
            return None;
        }
        let anchor = self.selection_anchor?;
        let focus = self.selection_focus?;
        Some((
            anchor.row.min(focus.row),
            anchor.row.max(focus.row),
            anchor.col.min(focus.col),
            anchor.col.max(focus.col),
        ))
    }

    pub fn selection_stats_key(&self) -> String {
        match &self.selection_type {
            Some(SelectionType::Column) => {
                let mut cols: Vec<usize> = self.selected_columns.to_vec();
                cols.sort_unstable();
                format!("col:{:?}", cols)
            }
            Some(SelectionType::Row) => {
                let mut rows: Vec<usize> = self.selected_rows.to_vec();
                rows.sort_unstable();
                format!("row:{:?}", rows)
            }
            Some(SelectionType::Cell) => {
                if let Some((mr, xr, mc, xc)) = self.selection_range() {
                    format!("cell:{}-{}-{}-{}", mr, xr, mc, xc)
                } else {
                    String::new()
                }
            }
            None => String::new(),
        }
    }

    pub fn is_cell_selected(&self, row: usize, col: usize) -> bool {
        match self.selection_type {
            Some(SelectionType::Cell) => {
                if let Some((min_row, max_row, min_col, max_col)) = self.selection_range() {
                    row >= min_row && row <= max_row && col >= min_col && col <= max_col
                } else {
                    false
                }
            }
            Some(SelectionType::Row) => self.selected_rows.contains(&row),
            Some(SelectionType::Column) => self.selected_columns.contains(&col),
            None => false,
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection_type = None;
        self.selected_rows.clear();
        self.selected_columns.clear();
        self.selection_anchor = None;
        self.selection_focus = None;
        self.computed_stats = None;
        self.computing_stats = false;
        self.stats_key.clear();
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.file
            .as_ref()
            .map_or(false, |f| !f.edits.is_empty() || f.columns_renamed || f.row_order.is_some() || f.col_order.is_some())
    }

    /// Returns true if any popup menu is currently open (settings, stats, or context menu).
    pub fn has_open_menu(&self) -> bool {
        self.settings_menu || self.stats_menu || self.context_menu.is_some()
    }

    pub fn total_changes(&self) -> usize {
        self.file.as_ref().map_or(0, |f| f.edits.len())
    }

    pub fn current_theme(&self) -> crate::ui::theme::ThemeColors {
        let themes = crate::ui::theme::bundled_themes();
        let idx = self.theme_index.min(themes.len().saturating_sub(1));
        themes[idx].colors.clone()
    }

    pub fn theme_name(&self) -> String {
        let themes = crate::ui::theme::bundled_themes();
        let idx = self.theme_index.min(themes.len().saturating_sub(1));
        themes[idx].name.clone()
    }

    pub fn cycle_theme(&mut self) {
        let count = crate::ui::theme::bundled_themes().len();
        self.theme_index = (self.theme_index + 1) % count;
        THEME_MEMORY_INDEX.store(self.theme_index, Ordering::Relaxed);
        THEME_EVER_SET.store(true, Ordering::Relaxed);
    }

    pub fn set_theme_index(&mut self, idx: usize) {
        let count = crate::ui::theme::bundled_themes().len().max(1);
        self.theme_index = idx.min(count - 1);
        THEME_MEMORY_INDEX.store(self.theme_index, Ordering::Relaxed);
        THEME_EVER_SET.store(true, Ordering::Relaxed);
    }

    pub fn clear_cache(&mut self) {
        self.row_cache.clear();
        self.row_cache_order.borrow_mut().clear();
        self.cache_version += 1;
    }

    pub fn get_cached_row(&self, index: usize) -> Option<&[String]> {
        let row = self.row_cache.get(&index)?;
        self.touch_cache_key(index);
        Some(row.as_slice())
    }

    pub fn cache_row(&mut self, index: usize, data: Vec<String>) {
        self.row_cache.insert(index, data);
        self.touch_cache_key(index);

        while self.row_cache.len() > ROW_CACHE_LIMIT {
            let oldest = self.row_cache_order.borrow_mut().pop_front();
            let Some(oldest) = oldest else {
                break;
            };
            self.row_cache.remove(&oldest);
        }
    }

    fn touch_cache_key(&self, index: usize) {
        let mut order = self.row_cache_order.borrow_mut();
        if let Some(pos) = order.iter().position(|&k| k == index) {
            order.remove(pos);
        }
        order.push_back(index);
    }
}
