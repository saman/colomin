use gpui::Context;

use crate::{csv_engine, file_open};

use super::TableView;

pub fn on_t_open_file(view: &mut TableView, cx: &mut Context<TableView>) {
    let path = rfd::FileDialog::new()
        .add_filter("CSV Files", &["csv", "tsv", "txt"])
        .add_filter("All Files", &["*"])
        .pick_file();

    if let Some(path) = path {
        let path_str = path.to_string_lossy().into_owned();
        file_open::open_file_async(view.state.clone(), path_str, cx);
    }
}

pub fn on_t_save_file(view: &mut TableView, cx: &mut Context<TableView>) {
    let state = view.state.read(cx);
    if !state.has_unsaved_changes() {
        return;
    }
    let file = match &state.file {
        Some(f) => f,
        None => return,
    };
    let target_path = file.file_path.clone();
    let save_result = csv_engine::writer::save_file(file, &target_path);
    let _ = state;

    match save_result {
        Ok(()) => {
            // Re-open file asynchronously using the same flow as all other open paths.
            file_open::open_file_async(view.state.clone(), target_path.to_string_lossy().into_owned(), cx);
        }
        Err(e) => eprintln!("Save failed: {}", e),
    }
}

pub fn on_t_cycle_theme(view: &mut TableView, cx: &mut Context<TableView>) {
    view.state.update(cx, |s, _| s.cycle_theme());
    cx.notify();
}

pub fn on_t_quit(cx: &mut Context<TableView>) {
    cx.quit();
}