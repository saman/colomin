use gpui::Context;

use crate::csv_engine::parser;
use crate::state::{SortDirection, SortState};

use super::TableView;

pub(super) fn maybe_apply_pending_sort(view: &TableView, cx: &mut Context<TableView>) {
    let pending = view.state.update(cx, |s, _| s.pending_sort.take());
    let Some((column_index, ascending)) = pending else {
        return;
    };

    let (path, row_offsets, edits, delimiter) = {
        let state = view.state.read(cx);
        let Some(file) = &state.file else {
            return;
        };
        (
            file.file_path.clone(),
            file.row_offsets.clone(),
            file.edits.clone(),
            file.delimiter,
        )
    };

    match parser::sort_rows(
        &path,
        &row_offsets,
        &edits,
        column_index,
        ascending,
        delimiter,
    ) {
        Ok(permutation) => {
            view.state.update(cx, |s, _| {
                if let Some(file) = s.file.as_mut() {
                    file.sort_permutation = Some(permutation);
                    s.sort_state = Some(SortState {
                        column_index,
                        direction: if ascending {
                            SortDirection::Asc
                        } else {
                            SortDirection::Desc
                        },
                    });
                    s.clear_cache();
                }
            });
        }
        Err(e) => {
            view.state.update(cx, |s, _| {
                s.toast_message = Some(format!("sort failed: {}", e));
            });
        }
    }
    cx.notify();
}