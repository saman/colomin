use std::collections::HashMap;
use std::path::PathBuf;

use crate::csv_engine::types::CsvMetadata;

#[derive(Debug, Clone)]
pub enum RowSource {
    Original(usize),
    Inserted(usize),
}

#[derive(Debug, Clone)]
pub enum ColSource {
    Original(usize),
    Inserted(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone)]
pub struct SortState {
    pub column_index: usize,
    pub direction: SortDirection,
}

/// Selection types
#[derive(Debug, Clone, PartialEq)]
pub enum SelectionType {
    Cell,
    Row,
    Column,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellCoord {
    pub row: usize,
    pub col: usize,
}

/// Undo/redo action types
#[derive(Debug, Clone)]
pub enum EditAction {
    CellEdit {
        row: usize,
        col: usize,
        old_value: String,
        new_value: String,
    },
    BatchCellEdit {
        edits: Vec<BatchEditEntry>,
    },
    Structural {
        description: String,
    },
}

#[derive(Debug, Clone)]
pub struct BatchEditEntry {
    pub row: usize,
    pub col: usize,
    pub old_value: String,
    pub new_value: String,
}

pub struct OpenFile {
    pub metadata: CsvMetadata,
    pub row_offsets: Vec<u64>,
    pub edits: HashMap<(usize, usize), String>,
    pub file_path: PathBuf,
    pub delimiter: u8,
    pub row_order: Option<Vec<RowSource>>,
    pub inserted_rows: Vec<Vec<String>>,
    pub col_order: Option<Vec<ColSource>>,
    pub inserted_columns: Vec<String>,
    pub inserted_col_values: HashMap<(usize, usize), String>,
    pub original_col_count: usize,
    pub original_columns: Vec<crate::csv_engine::types::CsvColumn>,
    pub sort_permutation: Option<Vec<usize>>,
    pub filter_indices: Option<Vec<usize>>,
}

impl OpenFile {
    pub fn effective_row_count(&self) -> usize {
        if let Some(ref indices) = self.filter_indices {
            return indices.len();
        }
        match &self.row_order {
            Some(order) => order.len(),
            None => self.metadata.total_rows,
        }
    }

    pub fn virtual_to_actual_row(&self, virtual_idx: usize) -> usize {
        if let Some(ref indices) = self.filter_indices {
            if virtual_idx < indices.len() {
                return indices[virtual_idx];
            }
            return virtual_idx;
        }
        if let Some(ref perm) = self.sort_permutation {
            if virtual_idx < perm.len() {
                return perm[virtual_idx];
            }
            return virtual_idx;
        }
        virtual_idx
    }

    pub fn current_col_count(&self) -> usize {
        match &self.col_order {
            Some(order) => order.len(),
            None => self.original_col_count,
        }
    }

    pub fn resolve_row(&self, virtual_idx: usize) -> RowSource {
        match &self.row_order {
            Some(order) => order[virtual_idx].clone(),
            None => RowSource::Original(virtual_idx),
        }
    }

    pub fn resolve_col(&self, virtual_idx: usize) -> ColSource {
        match &self.col_order {
            Some(order) => order[virtual_idx].clone(),
            None => ColSource::Original(virtual_idx),
        }
    }

    pub fn ensure_row_order(&mut self) {
        if self.row_order.is_none() {
            self.row_order = Some(
                (0..self.metadata.total_rows)
                    .map(RowSource::Original)
                    .collect(),
            );
        }
    }

    pub fn ensure_col_order(&mut self) {
        if self.col_order.is_none() {
            self.col_order = Some(
                (0..self.original_col_count)
                    .map(ColSource::Original)
                    .collect(),
            );
        }
    }
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
    pub search_query: String,
    pub search_results: Vec<usize>,
    pub show_search: bool,
    pub show_command_palette: bool,
    pub toast_message: Option<String>,
    /// Loading state for file open
    pub is_loading: bool,
    pub loading_message: String,
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
    pub cache_version: u64,
}

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
            is_loading: false,
            loading_message: String::new(),
            theme_index: 0,
            editing_cell: None,
            context_menu: None,
            is_dragging: false,
            computed_stats: None,
            computing_stats: false,
            stats_key: String::new(),
            row_cache: HashMap::new(),
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
        self.file.as_ref().map_or(false, |f| {
            !f.edits.is_empty() || f.row_order.is_some() || f.col_order.is_some()
        })
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
        self.cache_version += 1;
    }

    pub fn get_cached_row(&self, index: usize) -> Option<&Vec<String>> {
        self.row_cache.get(&index)
    }

    pub fn cache_row(&mut self, index: usize, data: Vec<String>) {
        self.row_cache.insert(index, data);
    }
}
