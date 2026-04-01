mod csv_engine;
mod state;
mod ui;

use std::path::PathBuf;

use gpui::*;
use state::AppState;
use ui::status_bar::StatusBar;
use ui::table::{self, TableView};
use ui::theme::ThemeColors;

actions!(colomin, [OpenFile, Quit, SaveFile, CycleTheme]);

struct Colomin {
    state: Entity<AppState>,
    table_view: Entity<TableView>,
    status_bar: Entity<StatusBar>,
}

impl Colomin {
    /// Apply an index result to the app state. Called from both sync and async paths.
    fn apply_index_result_to_state(
        state: &mut AppState,
        path: &str,
        index_result: csv_engine::parser::IndexResult,
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

        // Pre-load first chunk before setting state (so it's available on first render)
        let first_chunk = csv_engine::parser::read_chunk_with_delim(
            &file_path, &index_result.row_offsets, &std::collections::HashMap::new(),
            0, 200, col_count, index_result.delimiter,
        );

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
        state.loading_message.clear();
        state.clear_cache();
        state.clear_selection();

        if let Ok(chunk) = first_chunk {
            for (i, row) in chunk.rows.into_iter().enumerate() {
                state.cache_row(chunk.start_index + i, row);
            }
            state.cache_version += 1;
        }
    }

    /// Open file synchronously (for small files / initial load from CLI)
    fn open_file_sync(&mut self, path: &str, cx: &mut Context<Self>) {
        let file_path = PathBuf::from(path);
        let result = csv_engine::parser::index_file(&file_path);
        match result {
            Ok(index_result) => {
                let p = path.to_string();
                self.state.update(cx, |state, _| {
                    Self::apply_index_result_to_state(state, &p, index_result);
                });
                cx.notify();
            }
            Err(e) => eprintln!("Failed to open file: {}", e),
        }
    }

    /// Open file asynchronously — ALL I/O on background thread
    fn open_file_async(&mut self, path: String, cx: &mut Context<Self>) {
        self.state.update(cx, |s, _| {
            s.is_loading = true;
            s.loading_message = format!("Opening {}...",
                std::path::Path::new(&path).file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.clone()));
        });
        cx.notify();

        let se = self.state.clone();
        let path_clone = path.clone();

        cx.spawn(async move |_this, cx| {
            // Do ALL I/O on the background thread: index + first chunk
            let result = std::thread::spawn(move || {
                let index_result = csv_engine::parser::index_file(&PathBuf::from(&path_clone))?;
                let first_chunk = csv_engine::parser::read_chunk_with_delim(
                    &PathBuf::from(&path_clone),
                    &index_result.row_offsets,
                    &std::collections::HashMap::new(),
                    0, 200,
                    index_result.columns.len(),
                    index_result.delimiter,
                ).ok();
                Ok::<_, String>((index_result, first_chunk))
            })
            .join()
            .unwrap_or_else(|_| Err("Thread panicked".into()));

            match result {
                Ok((index_result, first_chunk)) => {
                    let _ = se.update(cx, |state, cx| {
                        // Just apply data — no I/O here
                        let file_path = PathBuf::from(&path);
                        let col_count = index_result.columns.len();
                        let metadata = csv_engine::types::CsvMetadata {
                            path: path.clone(),
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
                        state.loading_message.clear();
                        state.clear_cache();
                        state.clear_selection();

                        if let Some(chunk) = first_chunk {
                            for (i, row) in chunk.rows.into_iter().enumerate() {
                                state.cache_row(chunk.start_index + i, row);
                            }
                            state.cache_version += 1;
                        }

                        cx.notify();
                    });
                }
                Err(e) => {
                    eprintln!("Failed to open file: {}", e);
                    let _ = se.update(cx, |s, cx| {
                        s.is_loading = false;
                        s.loading_message.clear();
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn on_open_file(&mut self, _: &OpenFile, _window: &mut Window, cx: &mut Context<Self>) {
        // rfd must run on the main thread on macOS
        let path = rfd::FileDialog::new()
            .add_filter("CSV Files", &["csv", "tsv", "txt"])
            .add_filter("All Files", &["*"])
            .pick_file();

        if let Some(path) = path {
            let path_str = path.to_string_lossy().to_string();
            self.open_file_async(path_str, cx);
        }
    }

    fn on_save(&mut self, _: &SaveFile, _window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        if !state.has_unsaved_changes() {
            return;
        }
        let file = match &state.file {
            Some(f) => f,
            None => return,
        };
        let target_path = file.file_path.clone();
        let headers: Vec<String> = file.metadata.columns.iter().map(|c| c.name.clone()).collect();
        let save_result = csv_engine::writer::save_file(file, &target_path, &headers);
        drop(state);

        match save_result {
            Ok(()) => {
                self.open_file_sync(&target_path.to_string_lossy(), cx);
            }
            Err(e) => eprintln!("Save failed: {}", e),
        }
    }

    fn on_quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    fn on_cycle_theme(&mut self, _: &CycleTheme, _window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |s, _| s.cycle_theme());
        cx.notify();
    }
}

actions!(context_menu, [CmCopy, CmDelete, CmSortAsc, CmSortDesc, CmInsertRowAbove, CmInsertRowBelow]);

impl Colomin {
    fn render_context_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let state = self.state.read(cx);
        let (mx, my, row, col) = state.context_menu?;
        let colors = state.current_theme();
        let has_selection = state.selection_type.is_some();
        let col_name = state.file.as_ref()
            .and_then(|f| f.metadata.columns.get(col))
            .map(|c| c.name.clone())
            .unwrap_or_default();
        drop(state);

        let se = self.state.clone();

        let menu_item = |id: &str, label: String, se: Entity<AppState>, action: Box<dyn Fn(&mut AppState)>| {
            let se2 = se.clone();
            div()
                .id(SharedString::from(id.to_string()))
                .px(px(12.0))
                .py(px(6.0))
                .text_size(px(12.0))
                .text_color(colors.text_primary)
                .cursor_pointer()
                .rounded(px(4.0))
                .hover(|s| s.bg(colors.accent_subtle))
                .child(label)
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    se2.update(cx, |s, _| {
                        action(s);
                        s.context_menu = None;
                    });
                })
        };

        let menu = div()
            .absolute()
            .left(px(mx))
            .top(px(my))
            .w(px(180.0))
            .py(px(4.0))
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border)
            .rounded(px(8.0))
            .shadow_lg()
            .text_color(colors.text_primary)
            .child(menu_item("cm-copy", "Copy".into(), se.clone(), Box::new(|_| {})))
            .child(menu_item("cm-delete", "Clear cells".into(), se.clone(), Box::new(|s| {
                if let Some((mr, xr, mc, xc)) = s.selection_range() {
                    if let Some(ref mut f) = s.file {
                        for r in mr..=xr { for c in mc..=xc {
                            f.edits.insert((r, c), String::new());
                            if let Some(row) = s.row_cache.get_mut(&r) { if c < row.len() { row[c] = String::new(); } }
                        }}
                        s.cache_version += 1;
                    }
                }
            })))
            .child(div().h(px(1.0)).my(px(4.0)).bg(colors.border)) // separator
            .child(menu_item("cm-sort-asc", format!("Sort {} \u{2191}", col_name), se.clone(), Box::new(move |s| {
                // Sort will be triggered after menu closes
                s.toast_message = Some(format!("sort-asc:{}", col));
            })))
            .child(menu_item("cm-sort-desc", format!("Sort {} \u{2193}", col_name), se.clone(), Box::new(move |s| {
                s.toast_message = Some(format!("sort-desc:{}", col));
            })));

        Some(menu)
    }
}

impl Render for Colomin {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        cx.observe(&self.state, |_this, _state, cx| cx.notify()).detach();

        let has_menu = self.state.read(cx).context_menu.is_some();
        let se = self.state.clone();

        let mut root = div()
            .id("colomin-root")
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::on_open_file))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_quit))
            .on_action(cx.listener(Self::on_cycle_theme))
            .child(
                div().flex_1().min_h_0().child(self.table_view.clone()),
            )
            .child(self.status_bar.clone());

        // Context menu overlay
        if let Some(menu) = self.render_context_menu(cx) {
            root = root.child(
                // Full-screen click-away backdrop
                div()
                    .id("ctx-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_mouse_down(MouseButton::Left, {
                        let se = se.clone();
                        move |_, _, cx| { se.update(cx, |s, _| { s.context_menu = None; }); }
                    })
                    .on_mouse_down(MouseButton::Right, {
                        let se = se.clone();
                        move |_, _, cx| { se.update(cx, |s, _| { s.context_menu = None; }); }
                    })
                    .child(menu)
            );
        }

        root
    }
}

fn main() {
    let file_to_open = std::env::args().nth(1).and_then(|arg| {
        if !arg.starts_with('-') {
            let path = PathBuf::from(&arg);
            if path.exists() { Some(arg) } else { None }
        } else { None }
    });

    gpui_platform::application().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("up", table::MoveUp, Some("TableView")),
            KeyBinding::new("down", table::MoveDown, Some("TableView")),
            KeyBinding::new("left", table::MoveLeft, Some("TableView")),
            KeyBinding::new("right", table::MoveRight, Some("TableView")),
            KeyBinding::new("shift-up", table::SelectUp, Some("TableView")),
            KeyBinding::new("shift-down", table::SelectDown, Some("TableView")),
            KeyBinding::new("shift-left", table::SelectLeft, Some("TableView")),
            KeyBinding::new("shift-right", table::SelectRight, Some("TableView")),
            KeyBinding::new("escape", table::Escape, Some("TableView")),
            KeyBinding::new("enter", table::Enter, Some("TableView")),
            KeyBinding::new("delete", table::Delete, Some("TableView")),
            KeyBinding::new("backspace", table::Delete, Some("TableView")),
            KeyBinding::new("cmd-c", table::Copy, Some("TableView")),
            KeyBinding::new("cmd-o", table::TOpenFile, Some("TableView")),
            KeyBinding::new("cmd-s", table::TSaveFile, Some("TableView")),
            KeyBinding::new("cmd-t", table::TCycleTheme, Some("TableView")),
            KeyBinding::new("cmd-q", table::TQuit, Some("TableView")),
        ]);

        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Colomin".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let app_state = cx.new(|_| AppState::new());

                let state_for_table = app_state.clone();
                let table_view = cx.new(|cx| {
                    let sc = state_for_table.clone();
                    cx.observe(&sc, |_this, _state, cx| cx.notify()).detach();
                    TableView::new(state_for_table, cx)
                });

                // Focus the table view so keybindings work immediately
                let tv_focus = table_view.read(cx).focus_handle(cx);
                tv_focus.focus(window, cx);

                let state_for_status = app_state.clone();
                let status_bar = cx.new(|cx| {
                    let sc = state_for_status.clone();
                    cx.observe(&sc, |_this, _state, cx| cx.notify()).detach();
                    StatusBar { state: state_for_status }
                });

                let colomin = cx.new(|cx| {
                    let mut c = Colomin {
                        state: app_state,
                        table_view,
                        status_bar,
                    };
                    if let Some(ref path) = file_to_open {
                        c.open_file_sync(path, cx);
                    }
                    c
                });

                colomin
            },
        )
        .unwrap();

        cx.on_window_closed(|cx, _| {
            cx.quit();
        }).detach();

        cx.activate(true);
    });
}
