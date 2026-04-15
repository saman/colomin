use std::sync::atomic::Ordering;

use eframe::egui;

use crate::file_open::{self, LoadingHandle};
use crate::state::{AppState, SelectionType, SortDirection};
use crate::ui::table::TableView;

struct SortResult {
    permutation: Vec<usize>,
    column_index: usize,
    ascending: bool,
}

pub struct ColominApp {
    pub state: AppState,
    pub loading: Option<LoadingHandle>,
    pub table: TableView,
    sorting_rx: Option<std::sync::mpsc::Receiver<Result<SortResult, String>>>,
    ipc_rx: Option<std::sync::mpsc::Receiver<String>>,
}

impl ColominApp {
    pub fn new(cc: &eframe::CreationContext<'_>, ipc_rx: Option<std::sync::mpsc::Receiver<String>>) -> Self {
        let fonts = egui::FontDefinitions::default();
        cc.egui_ctx.set_fonts(fonts);

        let state = AppState::new();
        crate::ui::theme::apply_theme(&cc.egui_ctx, &state.current_theme());

        let mut app = Self {
            state,
            loading: None,
            table: TableView::new(),
            sorting_rx: None,
            ipc_rx,
        };

        // Open file from command-line arg
        if let Some(path) = std::env::args().nth(1).filter(|a| !a.starts_with('-')) {
            if std::path::Path::new(&path).exists() {
                app.start_loading(path);
            }
        }

        app
    }

    pub fn start_loading(&mut self, path: String) {
        let filename = std::path::Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&path)
            .to_string();
        self.state.is_loading = true;
        self.state.loading_progress = 0.0;
        self.state.loading_message = filename;
        self.loading = Some(file_open::open_file_async(path));
    }
}

impl eframe::App for ColominApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ── Poll background file loading ──
        if let Some(handle) = &self.loading {
            let progress = f32::from_bits(handle.progress.load(Ordering::Relaxed));
            self.state.loading_progress = progress;
            match handle.rx.try_recv() {
                Ok(Ok(loaded)) => {
                    file_open::apply_loaded_file(&mut self.state, loaded);
                    self.loading = None;
                    ctx.request_repaint();
                }
                Ok(Err(e)) => {
                    eprintln!("Load error: {}", e);
                    self.state.is_loading = false;
                    self.state.loading_message.clear();
                    self.loading = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.state.is_loading = false;
                    self.loading = None;
                }
            }
        }

        // ── Poll background sort ──
        if let Some(rx) = &self.sorting_rx {
            match rx.try_recv() {
                Ok(Ok(result)) => {
                    if let Some(ref mut file) = self.state.file {
                        file.sort_permutation = Some(result.permutation);
                        self.state.sort_state = Some(crate::state::SortState {
                            column_index: result.column_index,
                            direction: if result.ascending { SortDirection::Asc } else { SortDirection::Desc },
                        });
                        self.state.clear_cache();
                        self.state.invalidate_row_layout();
                    }
                    self.sorting_rx = None;
                    ctx.request_repaint();
                }
                Ok(Err(e)) => {
                    eprintln!("Sort error: {}", e);
                    self.sorting_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.sorting_rx = None;
                }
            }
        }

        // ── Dispatch pending sort to background thread ──
        if self.sorting_rx.is_none() {
            if let Some((col_idx, ascending)) = self.state.pending_sort.take() {
                if let Some(ref file) = self.state.file {
                    let path = file.file_path.clone();
                    let row_offsets = file.row_offsets.clone();
                    let edits = file.edits.clone();
                    let delimiter = file.delimiter;
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let result = crate::csv_engine::parser::sort_rows(
                            &path, &row_offsets, &edits, col_idx, ascending, delimiter,
                        ).map(|perm| SortResult { permutation: perm, column_index: col_idx, ascending });
                        let _ = tx.send(result);
                    });
                    self.sorting_rx = Some(rx);
                    ctx.request_repaint();
                }
            }
        }

        // ── IPC: Finder "Open With" when already running ──
        let ipc_path: Option<String> = self.ipc_rx.as_ref().and_then(|rx| {
            // Drain all pending, use the last one (most recent open request)
            let mut last = None;
            while let Ok(p) = rx.try_recv() { last = Some(p); }
            last
        });
        if let Some(path) = ipc_path {
            if std::path::Path::new(&path).exists() {
                self.start_loading(path);
                ctx.request_repaint();
            }
        }

        // ── File drag-and-drop ──
        ctx.input(|i| {
            if let Some(file) = i.raw.dropped_files.first() {
                if let Some(path) = file.path.as_ref() {
                    let path_str = path.to_string_lossy().into_owned();
                    // Store for opening outside the input closure borrow
                    self.state.loading_message = path_str; // reuse field as temp
                    self.state.is_loading = false; // signal "pending open"
                }
            }
        });
        // Drain the pending path stored above (hack: we use loading_message as temp)
        // Proper: check if there's a pending drop path
        let drop_path = ctx.input(|i| {
            i.raw.dropped_files.first()
                .and_then(|f| f.path.as_ref())
                .map(|p| p.to_string_lossy().into_owned())
        });
        if let Some(path) = drop_path {
            if !self.state.is_loading {
                self.start_loading(path);
            }
        }

        // ── Global shortcuts ──
        let open_file = ctx.input(|i| i.key_pressed(egui::Key::O) && i.modifiers.command);
        if open_file && !self.state.is_loading {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("CSV Files", &["csv", "tsv", "txt"])
                .add_filter("All Files", &["*"])
                .pick_file()
            {
                self.start_loading(path.to_string_lossy().into_owned());
            }
        }

        let save_file = ctx.input(|i| i.key_pressed(egui::Key::S) && i.modifiers.command);
        if save_file { self.handle_save(); }

        let cycle_theme = ctx.input(|i| i.key_pressed(egui::Key::T) && i.modifiers.command);
        if cycle_theme { self.state.cycle_theme(); }

        // ── Apply theme every frame ──
        crate::ui::theme::apply_theme(ctx, &self.state.current_theme());

        // ── Window title ──
        let title = if self.state.is_loading && !self.state.loading_message.is_empty() {
            format!("Loading {} — {:.0}%", self.state.loading_message, self.state.loading_progress * 100.0)
        } else if let Some(ref f) = self.state.file {
            let name = f.file_path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Colomin")
                .to_string();
            let changes = self.state.total_changes();
            if changes > 0 { format!("{} ({})", name, changes) } else { name }
        } else {
            "Colomin".to_string()
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        // ── Loading screen ──
        if self.state.is_loading {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(format!(
                        "Loading {}… {:.0}%",
                        self.state.loading_message,
                        self.state.loading_progress * 100.0
                    ));
                });
            });
            return;
        }

        // ── Empty state ──
        if self.state.file.is_none() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label("Open a CSV file with ⌘O, or drag and drop a file here");
                });
            });
            return;
        }

        // ── Status bar (must be added before CentralPanel) ──
        self.show_status_bar(ctx);

        // ── Main table ──
        egui::CentralPanel::default().show(ctx, |ui| {
            self.table.show(ui, &mut self.state, ctx);
        });
    }
}

impl ColominApp {
    fn handle_save(&mut self) {
        if !self.state.has_unsaved_changes() { return; }
        let Some(ref file) = self.state.file else { return };
        let target_path = file.file_path.clone();
        match crate::csv_engine::writer::save_file(file, &target_path) {
            Ok(()) => self.start_loading(target_path.to_string_lossy().into_owned()),
            Err(e) => eprintln!("Save failed: {}", e),
        }
    }

    fn show_status_bar(&mut self, ctx: &egui::Context) {
        let colors = self.state.current_theme();
        let fill = colors.status_bar_bg;
        let text_pri = colors.text_primary;
        let text_sec = colors.text_secondary;

        egui::TopBottomPanel::bottom("status_bar")
            .frame(egui::Frame::NONE.fill(fill).inner_margin(egui::Margin::symmetric(8, 4)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Left: row/col count + file size + sort
                    if let Some(ref f) = self.state.file {
                        let rows = self.state.effective_row_count();
                        let cols = self.state.col_count();
                        let bytes = f.metadata.file_size_bytes;

                        ui.colored_label(text_pri, format!("{} rows × {} cols", rows, cols));
                        ui.colored_label(text_sec, "·");
                        ui.colored_label(text_sec, Self::format_size(bytes));

                        if let Some(ref sort) = self.state.sort_state {
                            let dir = match sort.direction {
                                SortDirection::Asc => "↑",
                                SortDirection::Desc => "↓",
                            };
                            let col_name = f.metadata.columns.get(sort.column_index)
                                .map(|c| c.name.as_str())
                                .unwrap_or("?");
                            ui.colored_label(text_sec, "·");
                            ui.colored_label(text_sec, format!("Sorted {} {}", col_name, dir));
                        }
                    }

                    // Right-aligned: selection info + theme button
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Theme picker popup
                        let theme_name = self.state.theme_name();
                        let popup_id = egui::Id::new("theme_picker_popup");
                        let btn_resp = ui.small_button(format!("◑ {}", theme_name));
                        if btn_resp.clicked() {
                            if ui.memory(|m| m.is_popup_open(popup_id)) {
                                ui.memory_mut(|m| m.close_popup());
                            } else {
                                ui.memory_mut(|m| m.open_popup(popup_id));
                            }
                        }
                        let current_idx = self.state.theme_index;
                        let themes = crate::ui::theme::bundled_themes();
                        egui::popup_above_or_below_widget(
                            ui, popup_id, &btn_resp,
                            egui::AboveOrBelow::Above,
                            egui::PopupCloseBehavior::CloseOnClickOutside,
                            |ui| {
                                ui.set_min_width(160.0);
                                for (i, theme) in themes.iter().enumerate() {
                                    if ui.selectable_label(current_idx == i, &theme.name).clicked() {
                                        self.state.set_theme_index(i);
                                        ui.memory_mut(|m| m.close_popup());
                                    }
                                }
                            },
                        );

                        // Selection stats
                        if let Some(sel_text) = self.selection_stat_text() {
                            ui.colored_label(text_sec, "·");
                            ui.colored_label(text_sec, sel_text);
                        }
                    });
                });
            });
    }

    fn selection_stat_text(&self) -> Option<String> {
        let state = &self.state;
        match state.selection_type.as_ref()? {
            SelectionType::Cell => {
                let (min_r, max_r, min_c, max_c) = state.selection_range()?;
                let rows = max_r - min_r + 1;
                let cols = max_c - min_c + 1;
                if rows == 1 && cols == 1 {
                    // Single cell: show value length
                    let val = state.get_display_cell(min_r, min_c).unwrap_or_default();
                    if val.is_empty() { return None; }
                    return Some(format!("{} chars", val.len()));
                }
                Some(format!("{} × {} selected", rows, cols))
            }
            SelectionType::Row => {
                let n = state.selected_rows.len();
                if n == 0 { return None; }
                Some(format!("{} rows selected", n))
            }
            SelectionType::Column => {
                let n = state.selected_columns.len();
                if n == 0 { return None; }
                Some(format!("{} cols selected", n))
            }
        }
    }

    fn format_size(bytes: u64) -> String {
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else if bytes < 1024 * 1024 * 1024 {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        } else {
            format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
        }
    }
}
