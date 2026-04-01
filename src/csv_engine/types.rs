#[derive(Debug, Clone, PartialEq)]
pub enum ColumnType {
    String,
    Number,
    Boolean,
}

#[derive(Debug, Clone)]
pub struct CsvColumn {
    pub index: usize,
    pub name: String,
    pub inferred_type: ColumnType,
}

#[derive(Debug, Clone)]
pub struct CsvMetadata {
    pub path: String,
    pub columns: Vec<CsvColumn>,
    pub total_rows: usize,
    pub file_size_bytes: u64,
    pub delimiter: u8,
    pub has_headers: bool,
}

#[derive(Debug, Clone)]
pub struct RowChunk {
    pub start_index: usize,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct CellEdit {
    pub row_index: usize,
    pub col_index: usize,
    pub old_value: String,
    pub new_value: String,
}

#[derive(Debug, Clone)]
pub enum FilterOp {
    Contains,
    Equals,
    StartsWith,
    GreaterThan,
    LessThan,
}

#[derive(Debug, Clone)]
pub struct FilterCriteria {
    pub column_index: usize,
    pub operator: FilterOp,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub row_indices: Vec<usize>,
    pub total_matches: usize,
}

#[derive(Debug, Clone)]
pub struct ColumnStats {
    pub sum: Option<f64>,
    pub avg: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub count: usize,
    pub numeric_count: usize,
    pub min_length: Option<usize>,
    pub max_length: Option<usize>,
}
