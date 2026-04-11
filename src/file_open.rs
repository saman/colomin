use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use gpui::{Context, Entity};

use crate::csv_engine;
use crate::state::{self, AppState};

fn apply_loaded_file(
    state: &mut AppState,
    path: &str,
    index_result: csv_engine::parser::IndexResult,
    first_chunk: Option<csv_engine::types::RowChunk>,
) {
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
        sort_permutation: None,
        filter_indices: None,
    });
    state.unfiltered_row_count = index_result.total_rows;
    state.sort_state = None;
    state.has_filter = false;
    state.is_loading = false;
    state.loading_progress = 1.0;
    state.loading_message.clear();
    state.clear_cache();
    state.clear_selection();

    if let Some(chunk) = first_chunk {
        for (i, row) in chunk.rows.into_iter().enumerate() {
            state.cache_row(chunk.start_index + i, row);
        }
        state.cache_version += 1;
    }
}

pub fn open_file_async<T: 'static>(
    state_entity: Entity<AppState>,
    path: String,
    cx: &mut Context<T>,
) {
    let filename = std::path::Path::new(&path)
        .file_name()
        .and_then(|n| n.to_str().map(|s| s.to_string()))
        .unwrap_or_else(|| path.clone());

    let progress_atomic = Arc::new(AtomicU32::new(0));
    let progress_for_thread = progress_atomic.clone();
    let progress_for_poll = progress_atomic.clone();

    state_entity.update(cx, |s, _| {
        s.is_loading = true;
        s.loading_progress = 0.0;
        s.loading_message = filename.clone();
    });
    cx.notify();

    let se = state_entity.clone();
    let path_clone = path.clone();

    let se_poll = se.clone();
    cx.spawn(async move |_this, cx| {
        loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(50))
                .await;
            let raw = progress_for_poll.load(Ordering::Relaxed);
            let progress = f32::from_bits(raw);
            let still_loading = {
                let mut loading = true;
                let _ = se_poll.update(cx, |s, cx| {
                    if s.is_loading {
                        s.loading_progress = progress;
                        cx.notify();
                    } else {
                        loading = false;
                    }
                });
                loading
            };
            if !still_loading {
                break;
            }
        }
    })
    .detach();

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
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
            Ok::<_, String>((index_result, first_chunk))
        })();
        let _ = tx.send(result);
    });

    cx.spawn(async move |_this, cx| {
        let start = std::time::Instant::now();

        let result = loop {
            match rx.try_recv() {
                Ok(r) => break r,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(50))
                        .await;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    break Err("I/O thread panicked".into());
                }
            }
        };

        let elapsed = start.elapsed();
        let min_display = std::time::Duration::from_millis(400);
        if elapsed < min_display {
            cx.background_executor().timer(min_display - elapsed).await;
        }

        match result {
            Ok((index_result, first_chunk)) => {
                let _ = se.update(cx, |state, cx| {
                    apply_loaded_file(state, &path, index_result, first_chunk);
                    cx.notify();
                });
            }
            Err(e) => {
                eprintln!("Failed to open file: {}", e);
                let _ = se.update(cx, |s, cx| {
                    s.is_loading = false;
                    s.loading_progress = 0.0;
                    s.loading_message.clear();
                    cx.notify();
                });
            }
        }
    })
    .detach();
}
