use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::csv_engine;
use crate::state::{self, AppState};

pub struct LoadedFile {
    pub path: String,
    pub index_result: csv_engine::parser::IndexResult,
    pub first_chunk: Option<csv_engine::types::RowChunk>,
    pub initial_sort: Option<InitialSort>,
}

pub struct InitialSort {
    pub column_index: usize,
    pub ascending: bool,
    pub permutation: Vec<usize>,
}

pub struct LoadingHandle {
    pub rx: std::sync::mpsc::Receiver<Result<LoadedFile, String>>,
    pub progress: Arc<AtomicU32>,
}

/// Spawn a background thread to load a CSV file.
/// Poll `handle.rx.try_recv()` each frame, and read `handle.progress` for a 0.0–1.0 progress value.
///
/// If `initial_sort` is `Some((col, ascending))`, the sort permutation is computed in the
/// same background thread so the file arrives already sorted — avoiding a visible reshuffle
/// after a previously-saved sort is restored.
pub fn open_file_async(path: String, initial_sort: Option<(usize, bool)>) -> LoadingHandle {
    let progress = Arc::new(AtomicU32::new(0));
    let progress_for_thread = progress.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    crate::dlog!(Info, "FileIO", "open_file_async: {}", path);
    let path_clone = path.clone();
    std::thread::spawn(move || {
        let _t = crate::dspan!("FileIO", "index + first chunk");
        let result = (|| {
            let index_result = csv_engine::parser::index_file_with_progress(
                &PathBuf::from(&path_clone),
                move |done, total| {
                    if total > 0 {
                        let p = (done as f32 / total as f32).min(1.0);
                        progress_for_thread.store(p.to_bits(), Ordering::Relaxed);
                    }
                },
            )?;
            let initial_sort = initial_sort.and_then(|(col, ascending)| {
                if col >= index_result.columns.len() { return None; }
                let _t = crate::dspan!("Sort", "initial_sort during load");
                csv_engine::parser::sort_rows(
                    &PathBuf::from(&path_clone),
                    &index_result.row_offsets,
                    &std::collections::HashMap::new(),
                    col,
                    ascending,
                    index_result.delimiter,
                )
                .ok()
                .map(|permutation| InitialSort { column_index: col, ascending, permutation })
            });
            let first_chunk = csv_engine::parser::read_chunk_with_delim(
                &PathBuf::from(&path_clone),
                &index_result.row_offsets,
                &std::collections::HashMap::new(),
                0,
                200,
                index_result.columns.len(),
                index_result.delimiter,
            )
            .ok();
            Ok::<_, String>(LoadedFile {
                path: path_clone,
                index_result,
                first_chunk,
                initial_sort,
            })
        })();
        match &result {
            Ok(loaded) => crate::dlog!(
                Info,
                "FileIO",
                "loaded: rows={} cols={} bytes={}",
                loaded.index_result.total_rows,
                loaded.index_result.columns.len(),
                loaded.index_result.file_size_bytes
            ),
            Err(e) => crate::dlog!(Error, "FileIO", "load failed: {}", e),
        }
        let _ = tx.send(result);
    });

    LoadingHandle { rx, progress }
}

/// Apply a successfully loaded file to the app state.
pub fn apply_loaded_file(state: &mut AppState, loaded: LoadedFile) {
    let path = &loaded.path;
    let index_result = loaded.index_result;
    let first_chunk = loaded.first_chunk;
    let initial_sort = loaded.initial_sort;

    let file_path = PathBuf::from(path);
    let col_count = index_result.columns.len();
    let metadata = csv_engine::types::CsvMetadata {
        path: path.to_string(),
        columns: index_result.columns.clone(),
        total_rows: index_result.total_rows,
        file_size_bytes: index_result.file_size_bytes,
        delimiter: index_result.delimiter,
        has_headers: index_result.has_headers,
    };

    let (sort_permutation, sort_state) = match initial_sort {
        Some(s) => (
            Some(s.permutation),
            Some(state::SortState {
                column_index: s.column_index,
                direction: if s.ascending { state::SortDirection::Asc } else { state::SortDirection::Desc },
            }),
        ),
        None => (None, None),
    };

    state.file = Some(state::OpenFile {
        original_columns: metadata.columns.clone(),
        metadata,
        row_offsets: index_result.row_offsets,
        edits: std::collections::HashMap::new(),
        file_path,
        delimiter: index_result.delimiter,
        row_order: None,
        inserted_rows: Vec::new(),
        col_order: None,
        inserted_columns: Vec::new(),
        inserted_col_values: std::collections::HashMap::new(),
        original_col_count: col_count,
        sort_permutation,
        filter_indices: None,
        columns_renamed: false,
    });
    state.unfiltered_row_count = index_result.total_rows;
    state.sort_state = sort_state;
    state.has_filter = false;
    state.is_loading = false;
    state.loading_progress = 1.0;
    state.loading_message.clear();
    state.clear_cache();
    state.clear_selection();
    state.column_widths.clear();
    state.default_column_width = 150.0;
    state.row_heights.clear();
    state.invalidate_row_layout();
    state.invalidate_col_layout();

    if let Some(chunk) = first_chunk {
        for (i, row) in chunk.rows.into_iter().enumerate() {
            state.cache_row(chunk.start_index + i, row);
        }
        state.cache_version += 1;
    }
}
