#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;

use crate::csv_engine::types::CsvMetadata;

#[derive(Debug, Clone, Copy)]
pub enum RowSource {
    Original(usize),
    Inserted(usize),
}

#[derive(Debug, Clone, Copy)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferredStat {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Length,
}

impl PreferredStat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Count => "Count",
            Self::Sum => "Sum",
            Self::Avg => "Avg",
            Self::Min => "Min",
            Self::Max => "Max",
            Self::Length => "Length",
        }
    }

    pub fn icon_path(self) -> &'static str {
        match self {
            Self::Count => "assets/icons/stat-count.svg",
            Self::Sum => "assets/icons/stat-sum.svg",
            Self::Avg => "assets/icons/stat-avg.svg",
            Self::Min => "assets/icons/stat-min.svg",
            Self::Max => "assets/icons/stat-max.svg",
            Self::Length => "assets/icons/stat-length.svg",
        }
    }

    pub const ALL: [PreferredStat; 6] = [
        Self::Count, Self::Sum, Self::Avg, Self::Min, Self::Max, Self::Length,
    ];
}

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

#[derive(Debug, Clone)]
pub enum EditAction {
    CellEdit {
        row: usize,
        /// Display column index (used to restore cursor on undo/redo).
        col: usize,
        /// Physical CSV column index (used for `file.edits` key lookup).
        physical_col: usize,
        old_had_edit: bool,
        old_value: String,
        new_value: String,
    },
    BatchCellEdit {
        edits: Vec<BatchEditEntry>,
    },
    RenameColumn {
        col: usize,
        old_name: String,
        new_name: String,
    },
    /// Swap two display columns (col_order entries). Undo = swap back.
    MoveColumn {
        from_col: usize,
        to_col: usize,
    },
    /// Swap two display rows (sort_permutation entries). Undo = swap back.
    MoveRow {
        from_row: usize,
        to_row: usize,
    },
    Structural {
        description: String,
    },
}

#[derive(Debug, Clone)]
pub struct BatchEditEntry {
    pub row: usize,
    /// Display column index.
    pub col: usize,
    /// Physical CSV column index.
    pub physical_col: usize,
    pub old_had_edit: bool,
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
    pub columns_renamed: bool,
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
            Some(order) => order[virtual_idx],
            None => RowSource::Original(virtual_idx),
        }
    }

    pub fn resolve_col(&self, virtual_idx: usize) -> ColSource {
        match &self.col_order {
            Some(order) => order[virtual_idx],
            None => ColSource::Original(virtual_idx),
        }
    }

    pub fn ensure_row_order(&mut self) {
        if self.row_order.is_none() {
            self.row_order = Some((0..self.metadata.total_rows).map(RowSource::Original).collect());
        }
    }

    pub fn ensure_col_order(&mut self) {
        if self.col_order.is_none() {
            self.col_order = Some((0..self.original_col_count).map(ColSource::Original).collect());
        }
    }
}
