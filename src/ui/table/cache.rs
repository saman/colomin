use gpui::{Context, Entity};

use crate::csv_engine::parser;
use crate::state::AppState;

use super::TableView;

pub(super) fn ensure_rows_cached(
    state_entity: &Entity<AppState>,
    start: usize,
    count: usize,
    cx: &mut Context<TableView>,
) {
    let state = state_entity.read(cx);
    let file = match &state.file {
        Some(f) => f,
        None => return,
    };
    let effective_rows = file.effective_row_count();
    let col_count = file.metadata.columns.len();
    let path = file.file_path.clone();
    let row_offsets = file.row_offsets.clone();
    let edits = file.edits.clone();
    let delimiter = file.delimiter;
    let has_sf = file.sort_permutation.is_some() || file.filter_indices.is_some();

    let mut ns = None;
    let mut ne = start;
    for i in start..(start + count).min(effective_rows) {
        if state.get_cached_row(i).is_none() {
            if ns.is_none() {
                ns = Some(i);
            }
            ne = i + 1;
        }
    }
    let ns = match ns {
        Some(s) => s,
        None => return,
    };
    let se = state_entity.clone();

    if has_sf {
        let sp = file.sort_permutation.clone();
        let fi = file.filter_indices.clone();
        cx.spawn(async move |_, cx| {
            let rows = std::thread::spawn(move || {
                let f = std::fs::File::open(&path).map_err(|e| format!("{e}"))?;
                let mut rdr = std::io::BufReader::new(f);
                let mut res = Vec::new();
                for vr in ns..ne {
                    let ar = fi
                        .as_ref()
                        .and_then(|i| i.get(vr).copied())
                        .or_else(|| sp.as_ref().and_then(|p| p.get(vr).copied()))
                        .unwrap_or(vr);
                    let mut rd = parser::read_single_row_from_reader(
                        &mut rdr,
                        &row_offsets,
                        ar,
                        col_count,
                        delimiter,
                    )?;
                    for c in 0..col_count {
                        if let Some(ed) = edits.get(&(ar, c)) {
                            rd[c].clone_from(ed);
                        }
                    }
                    res.push((vr, rd));
                }
                Ok::<_, String>(res)
            })
            .join()
            .map_err(|e| format!("cache worker panicked: {e:?}"))
            .and_then(|r| r);
            if let Ok(data) = rows {
                let _ = se.update(cx, |s, cx| {
                    for (i, r) in data {
                        s.cache_row(i, r);
                    }
                    s.cache_version += 1;
                    cx.notify();
                });
            }
        })
        .detach();
    } else {
        cx.spawn(async move |_, cx| {
            let chunk = std::thread::spawn(move || {
                parser::read_chunk_with_delim(
                    &path,
                    &row_offsets,
                    &edits,
                    ns,
                    ne - ns,
                    col_count,
                    delimiter,
                )
            })
            .join()
            .map_err(|e| format!("chunk worker panicked: {e:?}"))
            .and_then(|r| r);
            if let Ok(chunk) = chunk {
                let _ = se.update(cx, |s, cx| {
                    for (i, r) in chunk.rows.into_iter().enumerate() {
                        s.cache_row(chunk.start_index + i, r);
                    }
                    s.cache_version += 1;
                    cx.notify();
                });
            }
        })
        .detach();
    }
}
