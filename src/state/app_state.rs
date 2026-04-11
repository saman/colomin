use std::collections::{HashMap, VecDeque};

use super::{CellCoord, EditAction, OpenFile, SelectionType, SortState};

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
    /// Whether the user is currently dragging to select cells
    pub is_dragging: bool,
    /// Async-computed stats for large selections (count, numeric_count, sum, avg, min, max)
    pub computed_stats: Option<(usize, usize, f64, f64, f64, f64)>,
    /// Whether stats are currently being computed
    pub computing_stats: bool,
    /// Key that identifies the current stats computation (to avoid stale results)
    pub stats_key: String,
    // Row cache
    pub row_cache: HashMap<usize, Vec<String>>,
    row_cache_order: VecDeque<usize>,
    pub cache_version: u64,
}

const ROW_CACHE_LIMIT: usize = 5000;

impl AppState {
    pub fn new() -> Self {
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
            search_query: String::new(),
            search_results: Vec::new(),
            show_search: false,
            show_command_palette: false,
            toast_message: None,
            pending_sort: None,
            is_loading: false,
            loading_message: String::new(),
            loading_progress: 0.0,
            theme_index: 0,
            editing_cell: None,
            context_menu: None,
            is_dragging: false,
            computed_stats: None,
            computing_stats: false,
            stats_key: String::new(),
            row_cache: HashMap::new(),
            row_cache_order: VecDeque::new(),
            cache_version: 0,
        }
    }

    pub fn effective_row_count(&self) -> usize {
        self.file.as_ref().map_or(0, |f| f.effective_row_count())
    }

    pub fn col_count(&self) -> usize {
        self.file.as_ref().map_or(0, |f| f.metadata.columns.len())
    }

    pub fn column_width(&self, col: usize) -> f32 {
        self.column_widths.get(&col).copied().unwrap_or(150.0)
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
            .map_or(false, |f| !f.edits.is_empty() || f.row_order.is_some() || f.col_order.is_some())
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
    }

    pub fn clear_cache(&mut self) {
        self.row_cache.clear();
        self.row_cache_order.clear();
        self.cache_version += 1;
    }

    pub fn get_cached_row(&self, index: usize) -> Option<&[String]> {
        self.row_cache.get(&index).map(Vec::as_slice)
    }

    pub fn cache_row(&mut self, index: usize, data: Vec<String>) {
        if !self.row_cache.contains_key(&index) {
            self.row_cache_order.push_back(index);
            while self.row_cache.len() >= ROW_CACHE_LIMIT {
                if let Some(oldest) = self.row_cache_order.pop_front() {
                    self.row_cache.remove(&oldest);
                } else {
                    break;
                }
            }
        }
        self.row_cache.insert(index, data);
    }
}
