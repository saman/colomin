use std::sync::atomic::Ordering;

use eframe::egui;

use crate::file_open::{self, LoadingHandle};
use crate::state::{AppState, SortDirection};
use crate::ui::stats::StatsSnapshot;
use crate::ui::table::TableView;

struct SortResult {
    permutation: Vec<usize>,
    column_index: usize,
    ascending: bool,
}

// ── Per-tab state ─────────────────────────────────────────────────────────────

struct TabState {
    state: AppState,
    table: TableView,
    loading: Option<LoadingHandle>,
    sorting_rx: Option<std::sync::mpsc::Receiver<Result<SortResult, String>>>,
    stats_rx: Option<std::sync::mpsc::Receiver<(String, crate::ui::stats::Stats)>>,
}

impl TabState {
    fn new() -> Self {
        Self {
            state: AppState::new(),
            table: TableView::new(),
            loading: None,
            sorting_rx: None,
            stats_rx: None,
        }
    }

    fn title(&self) -> String {
        if self.state.is_loading && !self.state.loading_message.is_empty() {
            return format!("Loading {}…", self.state.loading_message);
        }
        self.state.file.as_ref()
            .and_then(|f| f.file_path.file_name())
            .and_then(|n| n.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| "New Tab".to_string())
    }

    fn is_empty(&self) -> bool {
        self.state.file.is_none() && !self.state.is_loading
    }

    fn start_loading(&mut self, path: String) {
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

// ── App config (persisted) ────────────────────────────────────────────────────

fn default_tab_mode() -> bool { true }

#[derive(serde::Serialize, serde::Deserialize)]
struct AppConfig {
    #[serde(default)]
    selected_font: Option<String>,
    #[serde(default = "default_tab_mode")]
    tab_mode: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            selected_font: None,
            tab_mode: default_tab_mode(),
        }
    }
}

impl AppConfig {
    fn path() -> std::path::PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        std::path::Path::new(&home)
            .join(".config").join("colomin").join("settings.json")
    }

    fn load() -> Self {
        let path = Self::path();
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }
}

/// Read only the tab_mode flag from settings — used by main.rs before the app starts.
pub fn is_tab_mode() -> bool {
    AppConfig::load().tab_mode
}

// ── Main app struct ───────────────────────────────────────────────────────────

pub struct ColominApp {
    tabs: Vec<TabState>,
    active_tab: usize,
    /// When true, files open as additional tabs. When false, each window shows
    /// one file and additional launches start a new process (handled in main.rs).
    tab_mode: bool,
    ipc_rx: Option<std::sync::mpsc::Receiver<String>>,
    available_fonts: Vec<(String, std::path::PathBuf, u32)>,
    font_filter: String,
    /// In instance mode, when this process is spawned by another Colomin with a
    /// CLI arg, macOS may *also* re-deliver that path via Apple Events shortly
    /// after launch. Without this guard, the re-delivery would re-trigger
    /// `open_file_in_tab`, see the active tab is non-empty, and spawn yet
    /// another instance — an infinite loop. We dedup the first IPC arrival
    /// matching the CLI arg path within a short startup window.
    cli_arg_dedup: Option<String>,
    started_at: std::time::Instant,
    debug_log_enabled: bool,
}

impl ColominApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        ipc_rx: Option<std::sync::mpsc::Receiver<String>>,
    ) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        cc.egui_ctx.set_fonts(egui::FontDefinitions::default());

        let config = AppConfig::load();
        let tab_mode = config.tab_mode;

        let mut initial_tab = TabState::new();
        crate::ui::theme::apply_theme(&cc.egui_ctx, &initial_tab.state.current_theme());

        let available_fonts = Self::enumerate_system_fonts();

        if let Some(ref font_name) = config.selected_font {
            Self::apply_font(&cc.egui_ctx, Some(font_name), &available_fonts);
        }
        initial_tab.state.selected_font = config.selected_font;

        let cli_arg_path: Option<String> = std::env::args().nth(1)
            .filter(|a| !a.starts_with('-'));
        if let Some(ref path) = cli_arg_path {
            if std::path::Path::new(path).exists() {
                initial_tab.start_loading(path.clone());
            }
        }

        Self {
            tabs: vec![initial_tab],
            active_tab: 0,
            tab_mode,
            ipc_rx,
            available_fonts,
            font_filter: String::new(),
            cli_arg_dedup: cli_arg_path,
            started_at: std::time::Instant::now(),
            debug_log_enabled: false,
        }
    }

    /// Open a file. In tab mode: new tab (or reuse empty).
    /// In instance mode: load into this window if empty, otherwise spawn a new process.
    pub fn open_file_in_tab(&mut self, path: String) {
        if !self.tab_mode {
            if self.tabs[self.active_tab].is_empty() {
                self.tabs[self.active_tab].start_loading(path);
            } else {
                Self::launch_new_instance(&path);
            }
            return;
        }
        // Reuse empty active tab.
        if self.tabs[self.active_tab].is_empty() {
            self.tabs[self.active_tab].start_loading(path);
            return;
        }
        // Switch to already-open tab if it exists.
        if let Some(idx) = self.tabs.iter().position(|t| {
            t.state.file.as_ref()
                .map(|f| f.file_path.to_string_lossy() == path.as_str())
                .unwrap_or(false)
        }) {
            self.active_tab = idx;
            return;
        }
        let mut tab = TabState::new();
        // Inherit font from active tab.
        if let Some(ref font) = self.tabs[self.active_tab].state.selected_font.clone() {
            tab.state.selected_font = Some(font.clone());
        }
        tab.start_loading(path);
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
    }

    /// Spawn a new Colomin process that opens `path`.
    ///
    /// Always re-executes the binary directly rather than using `open -n`.
    /// Using `open -n` causes macOS to deliver a kAEOpenDocuments Apple Event
    /// to the new instance for the same path, which triggers another
    /// launch_new_instance call → infinite loop.
    /// Launching the binary directly passes the path as a CLI arg only —
    /// no Apple Event is generated, so no loop.
    fn launch_new_instance(path: &str) {
        let Ok(exe) = std::env::current_exe() else { return };
        let _ = std::process::Command::new(&exe).arg(path).spawn();
    }

    fn close_tab(&mut self, idx: usize) {
        if self.tabs.len() <= 1 {
            self.tabs[0] = TabState::new();
            self.active_tab = 0;
            return;
        }
        self.tabs.remove(idx);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
    }

    /// When tab mode is toggled off, keep only the active tab.
    fn collapse_to_single_tab(&mut self) {
        let active = self.active_tab;
        self.tabs = vec![self.tabs.remove(active)];
        self.active_tab = 0;
    }
}

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for ColominApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let tab = &mut self.tabs[self.active_tab];

        // ── Poll background file loading ──
        if let Some(handle) = &tab.loading {
            let progress = f32::from_bits(handle.progress.load(Ordering::Relaxed));
            tab.state.loading_progress = progress;
            match handle.rx.try_recv() {
                Ok(Ok(loaded)) => {
                    file_open::apply_loaded_file(&mut tab.state, loaded);
                    tab.loading = None;
                    ctx.request_repaint();
                }
                Ok(Err(e)) => {
                    eprintln!("Load error: {}", e);
                    tab.state.is_loading = false;
                    tab.state.loading_message.clear();
                    tab.loading = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    tab.state.is_loading = false;
                    tab.loading = None;
                }
            }
        }

        // ── Poll background sort ──
        if let Some(rx) = &tab.sorting_rx {
            match rx.try_recv() {
                Ok(Ok(result)) => {
                    if let Some(ref mut file) = tab.state.file {
                        file.sort_permutation = Some(result.permutation);
                        tab.state.sort_state = Some(crate::state::SortState {
                            column_index: result.column_index,
                            direction: if result.ascending { SortDirection::Asc } else { SortDirection::Desc },
                        });
                        tab.state.clear_cache();
                        tab.state.invalidate_row_layout();
                    }
                    tab.sorting_rx = None;
                    ctx.request_repaint();
                }
                Ok(Err(e)) => { eprintln!("Sort error: {}", e); tab.sorting_rx = None; }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => { tab.sorting_rx = None; }
            }
        }

        // ── Poll async stats ──
        if let Some(rx) = &tab.stats_rx {
            match rx.try_recv() {
                Ok((key, stats)) => {
                    if key == tab.state.selection_stats_key() {
                        let (count, num, sum, avg, min, max, len) = stats;
                        crate::dlog!(
                            Info, "Stats",
                            "async result count={} num={} sum={:.4} avg={:.4} min={:.4} max={:.4} len={}",
                            count, num, sum, avg, min, max, len
                        );
                        tab.state.computed_stats = Some(stats);
                        tab.state.stats_key = key;
                        tab.state.computing_stats = false;
                        ctx.request_repaint();
                    } else {
                        crate::dlog!(Debug, "Stats", "async stale (selection moved); discarding");
                    }
                    tab.stats_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    tab.state.computing_stats = false;
                    tab.stats_rx = None;
                }
            }
        }

        // ── Kick off stats computation when selection changes ──
        {
            use crate::ui::stats as st;
            const ASYNC_THRESHOLD: usize = 10_000;
            let key = tab.state.selection_stats_key();
            if !key.is_empty() && key != tab.state.stats_key && tab.stats_rx.is_none() {
                let cell_count = st::selection_cell_count(&tab.state);
                if cell_count <= ASYNC_THRESHOLD {
                    let _t = crate::dspan!("Stats", "compute_sync");
                    let result = st::compute_stats(&tab.state);
                    match &result {
                        Some((count, num, sum, avg, min, max, len)) => crate::dlog!(
                            Info, "Stats",
                            "sync result cells={} count={} num={} sum={:.4} avg={:.4} min={:.4} max={:.4} len={}",
                            cell_count, count, num, sum, avg, min, max, len
                        ),
                        None => crate::dlog!(Debug, "Stats", "sync result cells={} (no numeric data)", cell_count),
                    }
                    tab.state.computed_stats = result;
                    tab.state.stats_key = key;
                    tab.state.computing_stats = false;
                } else if tab.state.file.is_some() {
                    tab.state.computing_stats = true;
                    tab.state.computed_stats = None;
                    let key2 = key.clone();
                    let snap = StatsSnapshot::from(&tab.state);
                    let (tx, rx) = std::sync::mpsc::channel();
                    crate::dlog!(Info, "Stats", "spawn async cells={}", cell_count);
                    std::thread::spawn(move || {
                        let _t = crate::dspan!("Stats", "compute_async");
                        let result = crate::ui::stats::compute_stats_snapshot(&snap);
                        let _ = tx.send((key2, result));
                    });
                    tab.stats_rx = Some(rx);
                    ctx.request_repaint();
                }
            } else if key.is_empty() {
                tab.state.computed_stats = None;
                tab.state.stats_key.clear();
                tab.state.computing_stats = false;
                tab.stats_rx = None;
            }
        }

        // ── Dispatch pending sort to background thread ──
        let tab = &mut self.tabs[self.active_tab];
        if tab.sorting_rx.is_none() {
            if let Some((col_idx, ascending)) = tab.state.pending_sort.take() {
                if let Some(ref file) = tab.state.file {
                    crate::dlog!(
                        Info,
                        "Sort",
                        "dispatch col={} dir={} rows={}",
                        col_idx,
                        if ascending { "asc" } else { "desc" },
                        file.row_offsets.len()
                    );
                    let path = file.file_path.clone();
                    let row_offsets = file.row_offsets.clone();
                    let edits = file.edits.clone();
                    let delimiter = file.delimiter;
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let _t = crate::dspan!("Sort", "sort_rows");
                        let result = crate::csv_engine::parser::sort_rows(
                            &path, &row_offsets, &edits, col_idx, ascending, delimiter,
                        ).map(|perm| SortResult { permutation: perm, column_index: col_idx, ascending });
                        let _ = tx.send(result);
                    });
                    tab.sorting_rx = Some(rx);
                    ctx.request_repaint();
                }
            }
        }

        // ── IPC: Finder "Open With" / second process ──
        let ipc_path: Option<String> = self.ipc_rx.as_ref().and_then(|rx| {
            let mut last = None;
            while let Ok(p) = rx.try_recv() { last = Some(p); }
            last
        });
        if let Some(path) = ipc_path {
            // Drop a re-delivered Apple Event that matches our CLI arg during
            // the startup window. See `cli_arg_dedup` doc on ColominApp.
            let drop = self.cli_arg_dedup.as_ref().is_some_and(|d| {
                d == &path && self.started_at.elapsed() < std::time::Duration::from_secs(5)
            });
            if drop {
                crate::dlog!(Debug, "IPC", "drop dedup arg: {}", path);
                self.cli_arg_dedup = None;
            } else if std::path::Path::new(&path).exists() {
                crate::dlog!(Info, "IPC", "open via Apple Event/socket: {}", path);
                self.open_file_in_tab(path);
                ctx.request_repaint();
            } else {
                crate::dlog!(Warn, "IPC", "ignoring missing path: {}", path);
            }
        }

        // ── File drag-and-drop ──
        let drop_path = ctx.input(|i| {
            i.raw.dropped_files.first()
                .and_then(|f| f.path.as_ref())
                .map(|p| p.to_string_lossy().into_owned())
        });
        if let Some(path) = drop_path {
            self.open_file_in_tab(path);
        }

        // ── Global shortcuts ──
        let open_file = ctx.input(|i| i.key_pressed(egui::Key::O) && i.modifiers.command);
        if open_file && !self.tabs[self.active_tab].state.is_loading {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("CSV Files", &["csv", "tsv", "txt"])
                .add_filter("All Files", &["*"])
                .pick_file()
            {
                self.open_file_in_tab(path.to_string_lossy().into_owned());
            }
        }

        let close_tab_shortcut = ctx.input(|i| i.key_pressed(egui::Key::W) && i.modifiers.command);
        if close_tab_shortcut {
            let idx = self.active_tab;
            self.close_tab(idx);
        }

        // Cmd+Shift+T: new empty tab (tab mode only)
        let new_tab_shortcut = ctx.input(|i| {
            i.key_pressed(egui::Key::T) && i.modifiers.command && i.modifiers.shift
        });
        if new_tab_shortcut && self.tab_mode {
            self.tabs.push(TabState::new());
            self.active_tab = self.tabs.len() - 1;
        }

        let tab = &mut self.tabs[self.active_tab];

        let save_file = ctx.input(|i| i.key_pressed(egui::Key::S) && i.modifiers.command);
        if save_file { Self::handle_save_tab(tab); }

        let cycle_theme = ctx.input(|i| {
            i.key_pressed(egui::Key::T) && i.modifiers.command && !i.modifiers.shift
        });
        if cycle_theme { tab.state.cycle_theme(); }

        // ── Apply theme ──
        crate::ui::theme::apply_theme(ctx, &tab.state.current_theme());

        // ── Window title ──
        let title = {
            let tab = &self.tabs[self.active_tab];
            if tab.state.is_loading {
                format!("Loading {} — {:.0}%", tab.state.loading_message, tab.state.loading_progress * 100.0)
            } else if let Some(ref f) = tab.state.file {
                let name = f.file_path.file_name()
                    .and_then(|n| n.to_str()).unwrap_or("Colomin").to_string();
                let ch = tab.state.total_changes();
                if ch > 0 { format!("{} ({})", name, ch) } else { name }
            } else {
                "Colomin".to_string()
            }
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        // ── Tab bar (only in tab mode with 2+ tabs) ──
        let show_tab_bar = self.tab_mode && self.tabs.len() > 1;
        let mut tab_action: Option<TabAction> = None;
        if show_tab_bar {
            // Snapshot tab display info to avoid borrow conflicts during rendering.
            let tab_infos: Vec<(String, bool)> = self.tabs.iter()
                .map(|t| (t.title(), t.state.total_changes() > 0))
                .collect();
            let active_idx = self.active_tab;
            let colors = self.tabs[self.active_tab].state.current_theme();
            // bg is always slightly darker/muted vs surface (e.g. #FAFAFA vs #FFFFFF in light,
            // #141414 vs #1B1B1B in dark), giving the active tab a clear "raised" appearance.
            let tab_bar_fill  = colors.bg;
            let active_fill   = colors.surface;
            let text_pri      = colors.text_primary;
            let text_sec      = colors.text_secondary;
            let accent        = colors.accent;

            const TAB_H: f32 = 28.0;

            egui::TopBottomPanel::top("tab_bar")
                .frame(egui::Frame::NONE
                    .fill(tab_bar_fill)
                    .inner_margin(egui::Margin { left: 4, right: 4, top: 2, bottom: 0 }))
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 1.0;

                        for (i, (title, has_changes)) in tab_infos.iter().enumerate() {
                            let is_active = i == active_idx;
                            let (bg, fg) = if is_active {
                                (active_fill, text_pri)
                            } else {
                                (tab_bar_fill, text_sec)
                            };

                            // Measure text width.
                            let text_w = ui.fonts(|f| {
                                f.layout_no_wrap(
                                    title.clone(),
                                    egui::FontId::proportional(12.0),
                                    egui::Color32::WHITE,
                                ).size().x
                            });

                            const LPAD:       f32 = 10.0;
                            const DOT_SZ:     f32 = 10.0;
                            const CLOSE_ZONE: f32 = 28.0; // wide hit area

                            let dot_w = if *has_changes { DOT_SZ + 4.0 } else { 0.0 };
                            let tab_w = (LPAD + dot_w + text_w + 8.0 + CLOSE_ZONE).max(72.0);

                            let (tab_rect, tab_resp) = ui.allocate_exact_size(
                                egui::vec2(tab_w, TAB_H),
                                egui::Sense::click(),
                            );

                            if ui.is_rect_visible(tab_rect) {
                                let painter = ui.painter();
                                let cr = egui::CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 };
                                painter.rect_filled(tab_rect, cr, bg);

                                // Active tab: 2px accent underline flush with the bottom edge.
                                if is_active {
                                    painter.rect_filled(
                                        egui::Rect::from_min_max(
                                            egui::pos2(tab_rect.left(), tab_rect.bottom() - 2.0),
                                            tab_rect.max,
                                        ),
                                        egui::CornerRadius::ZERO,
                                        accent,
                                    );
                                }

                                let cy = tab_rect.center().y;
                                let mut x = tab_rect.left() + LPAD;

                                // Unsaved-changes dot (SVG icon).
                                if *has_changes {
                                    let dot_rect = egui::Rect::from_center_size(
                                        egui::pos2(x + DOT_SZ / 2.0, cy),
                                        egui::vec2(DOT_SZ, DOT_SZ),
                                    );
                                    crate::ui::icons::icon("modified", accent)
                                        .paint_at(ui, dot_rect);
                                    x += DOT_SZ + 4.0;
                                }

                                // File name (non-interactive painted text).
                                painter.text(
                                    egui::pos2(x, cy),
                                    egui::Align2::LEFT_CENTER,
                                    title.as_str(),
                                    egui::FontId::proportional(12.0),
                                    fg,
                                );

                                // Close button — right side of tab, full height.
                                let close_rect = egui::Rect::from_min_max(
                                    egui::pos2(tab_rect.right() - CLOSE_ZONE, tab_rect.top()),
                                    tab_rect.max,
                                );
                                let close_id = egui::Id::new(("tab_close", i));
                                let close_resp = ui.interact(
                                    close_rect,
                                    close_id,
                                    egui::Sense::click(),
                                );
                                // × always visible on active tab; appears on hover for others.
                                let x_color = if close_resp.hovered() || is_active {
                                    text_sec
                                } else {
                                    bg // invisible
                                };
                                painter.text(
                                    close_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "×",
                                    egui::FontId::proportional(15.0),
                                    x_color,
                                );

                                // close takes priority over switch.
                                if close_resp.clicked() {
                                    tab_action = Some(TabAction::Close(i));
                                } else if tab_resp.clicked() {
                                    tab_action = Some(TabAction::Switch(i));
                                }
                            }
                        }

                        // "+" new tab button.
                        let (new_rect, new_resp) = ui.allocate_exact_size(
                            egui::vec2(28.0, TAB_H),
                            egui::Sense::click(),
                        );
                        if ui.is_rect_visible(new_rect) {
                            let c = if new_resp.hovered() { text_pri } else { text_sec };
                            ui.painter().text(
                                new_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "+",
                                egui::FontId::proportional(16.0),
                                c,
                            );
                        }
                        if new_resp.clicked() {
                            tab_action = Some(TabAction::New);
                        }
                    });
                });
        }

        // Apply tab actions.
        match tab_action {
            Some(TabAction::Switch(i)) => { self.active_tab = i; }
            Some(TabAction::Close(i))  => { self.close_tab(i); }
            Some(TabAction::New) => {
                self.tabs.push(TabState::new());
                self.active_tab = self.tabs.len() - 1;
            }
            None => {}
        }

        let tab = &mut self.tabs[self.active_tab];

        // ── Loading screen ──
        if tab.state.is_loading {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label(format!(
                        "Loading {}… {:.0}%",
                        tab.state.loading_message,
                        tab.state.loading_progress * 100.0
                    ));
                });
            });
            return;
        }

        // ── Empty state ──
        if tab.state.file.is_none() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.label("Open a CSV file with ⌘O, or drag and drop a file here");
                });
            });
            return;
        }

        // ── Status bar ──
        let mut tab_mode_toggle: Option<bool> = None;
        let mut font_choice: Option<Option<String>> = None;
        let mut debug_toggle: Option<bool> = None;
        let mut reveal_log: bool = false;
        let mut open_log: bool = false;
        Self::show_status_bar_for(
            tab,
            &self.available_fonts,
            &mut self.font_filter,
            self.tab_mode,
            &mut tab_mode_toggle,
            &mut font_choice,
            self.debug_log_enabled,
            &mut debug_toggle,
            &mut reveal_log,
            &mut open_log,
            ctx,
        );

        if let Some(choice) = font_choice {
            let tab = &mut self.tabs[self.active_tab];
            tab.state.selected_font = choice.clone();
            Self::apply_font(ctx, choice.as_deref(), &self.available_fonts);
            AppConfig {
                selected_font: choice,
                tab_mode: self.tab_mode,
            }.save();
        }

        // Apply tab_mode change.
        if let Some(new_mode) = tab_mode_toggle {
            self.tab_mode = new_mode;
            if !new_mode {
                self.collapse_to_single_tab();
            }
            let config = AppConfig {
                selected_font: self.tabs[self.active_tab].state.selected_font.clone(),
                tab_mode: new_mode,
            };
            config.save();
        }

        // Apply debug-log toggle. Session-only — never persisted.
        if let Some(on) = debug_toggle {
            self.debug_log_enabled = on;
            if on {
                crate::debug_log::enable();
                crate::dlog!(Info, "App", "debug logging enabled by user");
            } else {
                crate::dlog!(Info, "App", "debug logging disabled by user");
                crate::debug_log::disable();
            }
        }

        if reveal_log {
            if let Some(path) = crate::debug_log::current_log_path() {
                Self::reveal_in_finder(&path);
            }
        }

        if open_log {
            if let Some(path) = crate::debug_log::current_log_path() {
                let _ = std::process::Command::new("open").arg(&path).spawn();
            }
        }

        // ── Main table ──
        let tab = &mut self.tabs[self.active_tab];
        let panel_fill = tab.state.current_theme().bg;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(panel_fill))
            .show(ctx, |ui| {
                tab.table.show(ui, &mut tab.state, ctx);
            });
    }
}

enum TabAction { Switch(usize), Close(usize), New }

// ── ColominApp helpers ────────────────────────────────────────────────────────

impl ColominApp {
    fn enumerate_system_fonts() -> Vec<(String, std::path::PathBuf, u32)> {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let mut map: std::collections::BTreeMap<String, (std::path::PathBuf, u32)> =
            std::collections::BTreeMap::new();
        for face in db.faces() {
            let Some((family, _)) = face.families.first() else { continue };
            if family.starts_with('.') { continue; }
            let fontdb::Source::File(ref path) = face.source else { continue };
            let is_regular = face.weight == fontdb::Weight::NORMAL && face.style == fontdb::Style::Normal;
            if is_regular || !map.contains_key(family) {
                map.insert(family.clone(), (path.clone(), face.index));
            }
        }
        map.into_iter().map(|(name, (path, idx))| (name, path, idx)).collect()
    }

    fn apply_font(ctx: &egui::Context, family: Option<&str>, available: &[(String, std::path::PathBuf, u32)]) {
        let Some(name) = family else {
            ctx.set_fonts(egui::FontDefinitions::default());
            return;
        };
        let Some((_, path, idx)) = available.iter().find(|(n, _, _)| n == name) else { return };
        let Ok(bytes) = std::fs::read(path) else { return };
        let mut fonts = egui::FontDefinitions::default();
        let mut fd = egui::FontData::from_owned(bytes);
        fd.index = *idx;
        fonts.font_data.insert(name.to_string(), fd.into());
        fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, name.to_string());
        ctx.set_fonts(fonts);
    }

    fn reveal_in_finder(path: &std::path::Path) {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .arg("-R")
                .arg(path)
                .spawn();
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = path; // silence unused on non-mac
        }
    }

    /// Render a single full-width settings menu row.
    ///
    /// The whole row is the click target — hovering paints a subtle background,
    /// and `selected` rows get an accent tint. `trailing` is rendered right-aligned
    /// (e.g. chevron, "On"/"Off" status, or "Active" badge).
    fn menu_row(
        ui: &mut egui::Ui,
        icon_name: &str,
        icon_color: egui::Color32,
        label: egui::RichText,
        selected: bool,
        accent: egui::Color32,
        trailing: impl FnOnce(&mut egui::Ui),
    ) -> egui::Response {
        let row_h = 26.0;
        let avail_w = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(avail_w, row_h),
            egui::Sense::click(),
        );

        let bg = if selected {
            // Soft accent tint for the active item.
            egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 28)
        } else if response.hovered() {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else {
            egui::Color32::TRANSPARENT
        };
        if bg != egui::Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, 4.0, bg);
        }

        let inner = rect.shrink2(egui::vec2(8.0, 0.0));
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.add(crate::ui::icons::icon(icon_name, icon_color));
                ui.add_space(8.0);
                ui.add(egui::Label::new(label).selectable(false));
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    trailing,
                );
            },
        );

        response
    }

    fn handle_save_tab(tab: &mut TabState) {
        if !tab.state.has_unsaved_changes() { return; }
        let Some(ref file) = tab.state.file else { return };
        let target_path = file.file_path.clone();
        let _t = crate::dspan!("FileIO", "save");
        crate::dlog!(Info, "FileIO", "save → {}", target_path.display());
        match crate::csv_engine::writer::save_file(file, &target_path) {
            Ok(()) => {
                crate::dlog!(Info, "FileIO", "save ok");
                tab.start_loading(target_path.to_string_lossy().into_owned());
            }
            Err(e) => {
                crate::dlog!(Error, "FileIO", "save failed: {}", e);
                eprintln!("Save failed: {}", e);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn show_status_bar_for(
        tab: &mut TabState,
        available_fonts: &[(String, std::path::PathBuf, u32)],
        font_filter: &mut String,
        tab_mode: bool,
        tab_mode_toggle: &mut Option<bool>,
        font_choice: &mut Option<Option<String>>,
        debug_log_enabled: bool,
        debug_toggle: &mut Option<bool>,
        reveal_log: &mut bool,
        open_log: &mut bool,
        ctx: &egui::Context,
    ) {
        use crate::ui::stats as st;

        let colors   = tab.state.current_theme();
        let fill     = colors.status_bar_bg;
        let text_pri = colors.text_primary;
        let text_sec = colors.text_secondary;

        let has_file    = tab.state.file.is_some();
        let rows        = tab.state.effective_row_count();
        let cols        = tab.state.col_count();
        let bytes       = tab.state.file.as_ref().map(|f| f.metadata.file_size_bytes).unwrap_or(0);
        let has_filter  = tab.state.has_filter;
        let unfiltered  = tab.state.unfiltered_row_count;
        let sort_label: Option<String> = tab.state.sort_state.as_ref().and_then(|s| {
            let dir = if matches!(s.direction, SortDirection::Asc) { "↑" } else { "↓" };
            let name = tab.state.file.as_ref()
                .and_then(|f| f.metadata.columns.get(s.column_index))
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "?".to_string());
            Some(format!("Sorted {} {}", name, dir))
        });

        let stats_key    = tab.state.selection_stats_key();
        let current_stats = if tab.state.stats_key == stats_key { tab.state.computed_stats } else { None };
        let computing    = tab.state.computing_stats;
        let pref         = tab.state.preferred_stat;
        let shape_text   = Self::selection_shape_text_for(&tab.state);

        let theme_name        = tab.state.theme_name();
        let current_theme_idx = tab.state.theme_index;
        let header_on         = tab.state.header_row_enabled;
        let themes            = crate::ui::theme::bundled_themes();
        let theme_submenu     = tab.state.settings_theme_submenu;
        let font_submenu      = tab.state.settings_font_submenu;
        let debug_submenu     = tab.state.settings_debug_submenu;
        let selected_font     = tab.state.selected_font.clone();

        let mut toggle_header    = false;
        let mut new_theme_idx:   Option<usize>                       = None;
        let mut new_pref_stat:   Option<crate::state::PreferredStat> = None;
        let mut new_theme_sub:   Option<bool>                        = None;
        let mut new_font_sub:    Option<bool>                        = None;
        let mut new_debug_sub:   Option<bool>                        = None;
        let mut toggle_tab_mode: Option<bool>                        = None;
        let mut new_debug_toggle: Option<bool>                       = None;
        let mut new_reveal_log:  bool                                = false;
        let mut new_open_log:    bool                                = false;

        egui::TopBottomPanel::bottom("status_bar")
            .frame(egui::Frame::NONE.fill(fill).inner_margin(egui::Margin::symmetric(8, 4)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let bar_rect = ui.max_rect();

                    // ── Left: file info ──
                    if has_file {
                        let row_text = if has_filter {
                            format!("{} / {} rows", st::format_compact(rows), st::format_compact(unfiltered))
                        } else {
                            format!("{} rows", st::format_compact(rows))
                        };
                        ui.colored_label(text_pri, row_text);
                        ui.colored_label(text_sec, "·");
                        ui.colored_label(text_sec, format!("{} cols", cols));
                        ui.colored_label(text_sec, "·");
                        ui.colored_label(text_sec, Self::format_size(bytes));
                        if let Some(ref s) = sort_label {
                            ui.colored_label(text_sec, "·");
                            ui.colored_label(text_sec, s);
                        }
                        if let Some(ref shape) = shape_text {
                            ui.colored_label(text_sec, "·");
                            ui.colored_label(text_pri, shape);
                        }
                    }
                    let left_edge = ui.min_rect().right();

                    // ── Center: stats badge ──
                    if !stats_key.is_empty() {
                        let badge_label = if let Some(s) = current_stats {
                            let (val, _) = st::format_stat(s, pref);
                            Some(format!("{}: {}", pref.label(), val))
                        } else if computing {
                            Some("Computing…".to_string())
                        } else {
                            None
                        };

                        if let Some(ref label) = badge_label {
                            const BADGE_HALF_W: f32 = 55.0;
                            let space = (bar_rect.center().x - BADGE_HALF_W - left_edge).max(8.0);
                            ui.add_space(space);

                            if let Some(s) = current_stats {
                                let badge_id = egui::Id::new("stats_picker_popup");
                                let badge_btn = ui.small_button(label);
                                if badge_btn.clicked() {
                                    if ui.memory(|m| m.is_popup_open(badge_id)) {
                                        ui.memory_mut(|m| m.close_popup());
                                    } else {
                                        ui.memory_mut(|m| m.open_popup(badge_id));
                                    }
                                }
                                egui::popup_above_or_below_widget(
                                    ui, badge_id, &badge_btn,
                                    egui::AboveOrBelow::Above,
                                    egui::PopupCloseBehavior::CloseOnClickOutside,
                                    |ui| {
                                        ui.set_min_width(160.0);
                                        for &stat in crate::state::PreferredStat::ALL.iter() {
                                            let (sv, _) = st::format_stat(s, stat);
                                            let is_active = pref == stat;
                                            let icon_color = if is_active { colors.accent } else { text_sec };
                                            let text_color = if is_active { colors.accent } else { text_pri };
                                            ui.horizontal(|ui| {
                                                ui.add(crate::ui::icons::icon(crate::ui::icons::stat_icon_name(stat), icon_color));
                                                let lbl = egui::RichText::new(format!("{}   {}", stat.label(), sv)).size(12.0).color(text_color);
                                                if ui.selectable_label(is_active, lbl).clicked() {
                                                    new_pref_stat = Some(stat);
                                                    ui.memory_mut(|m| m.close_popup());
                                                }
                                            });
                                        }
                                    },
                                );
                            } else {
                                ui.colored_label(text_sec, label);
                            }
                        }
                    }

                    // ── Right: settings gear ──
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let settings_id = egui::Id::new("settings_popover");
                        let settings_btn = {
                            let gear = crate::ui::icons::icon("gear", text_sec);
                            ui.add(egui::Button::image(gear).small())
                        };
                        if settings_btn.clicked() {
                            if ui.memory(|m| m.is_popup_open(settings_id)) {
                                ui.memory_mut(|m| m.close_popup());
                                new_theme_sub = Some(false);
                                new_font_sub = Some(false);
                                new_debug_sub = Some(false);
                            } else {
                                ui.memory_mut(|m| m.open_popup(settings_id));
                            }
                        }
                        // Custom popover anchored at the gear's right edge with an
                        // 8px window-edge gutter. (Built directly on egui::Area so we
                        // can set our own pivot/offset; the convenience
                        // popup_above_or_below_widget pins LEFT_BOTTOM and clamps to
                        // the screen, leaving the popup flush against the right edge.)
                        if ui.memory(|m| m.is_popup_open(settings_id)) {
                            // Anchor the popup's right edge to the gear's right edge,
                            // so it inherits whatever margin the gear has from the window.
                            let anchor = settings_btn.rect.right_top();
                            let area_resp = egui::Area::new(settings_id)
                                .kind(egui::UiKind::Popup)
                                .order(egui::Order::Foreground)
                                .fixed_pos(anchor)
                                .pivot(egui::Align2::RIGHT_BOTTOM)
                                .interactable(true)
                                .show(ui.ctx(), |ui| {
                                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                                        ui.with_layout(
                                            egui::Layout::top_down_justified(egui::Align::LEFT),
                                            |ui| {
                                        ui.set_min_width(180.0);
                                        ui.set_max_width(200.0);
                                if font_submenu {
                                    // Font submenu — same layout idiom as Theme submenu.
                                    if Self::menu_row(
                                        ui, "chevron-left", text_sec,
                                        egui::RichText::new("Back").size(12.0).color(text_sec),
                                        false, colors.accent,
                                        |_ui| {},
                                    ).clicked() { new_font_sub = Some(false); }
                                    ui.separator();
                                    let te = egui::TextEdit::singleline(font_filter)
                                        .id(egui::Id::new("settings_font_filter"))
                                        .hint_text("Search fonts…")
                                        .desired_width(f32::INFINITY)
                                        .font(egui::FontId::proportional(12.0));
                                    ui.add(te);
                                    ui.add_space(2.0);
                                    let filter_lower = font_filter.to_lowercase();
                                    let avail_h = ctx.screen_rect().height();
                                    // Reserve ~140px above for the gear button, back row, and search field.
                                    let list_h = (avail_h - 140.0).clamp(240.0, 720.0);
                                    egui::ScrollArea::vertical()
                                        .id_salt("settings_font_scroll")
                                        .min_scrolled_height(list_h)
                                        .max_height(list_h)
                                        .show(ui, |ui| {
                                            ui.set_min_width(ui.available_width());
                                            let def_active = selected_font.is_none();
                                            let def_color = if def_active { colors.accent } else { text_pri };
                                            let def_ic    = if def_active { colors.accent } else { text_sec };
                                            if Self::menu_row(
                                                ui, "font", def_ic,
                                                egui::RichText::new("Default").size(12.0).color(def_color).italics(),
                                                def_active, colors.accent,
                                                |ui| {
                                                    if def_active {
                                                        ui.add(egui::Label::new(egui::RichText::new("Active").size(11.0).color(colors.accent)).selectable(false));
                                                    }
                                                },
                                            ).clicked() {
                                                *font_choice = Some(None);
                                                new_font_sub = Some(false);
                                            }
                                            for (name, _, _) in available_fonts {
                                                if !filter_lower.is_empty() && !name.to_lowercase().contains(&filter_lower) { continue; }
                                                let is_active = selected_font.as_deref() == Some(name.as_str());
                                                let tc = if is_active { colors.accent } else { text_pri };
                                                let ic = if is_active { colors.accent } else { text_sec };
                                                if Self::menu_row(
                                                    ui, "font", ic,
                                                    egui::RichText::new(name.as_str()).size(12.0).color(tc),
                                                    is_active, colors.accent,
                                                    |ui| {
                                                        if is_active {
                                                            ui.add(egui::Label::new(egui::RichText::new("Active").size(11.0).color(colors.accent)).selectable(false));
                                                        }
                                                    },
                                                ).clicked() {
                                                    *font_choice = Some(Some(name.clone()));
                                                }
                                            }
                                        });
                                } else if debug_submenu {
                                    // Debug submenu — toggle + log details.
                                    if Self::menu_row(
                                        ui, "chevron-left", text_sec,
                                        egui::RichText::new("Back").size(12.0).color(text_sec),
                                        false, colors.accent,
                                        |_ui| {},
                                    ).clicked() { new_debug_sub = Some(false); }
                                    ui.separator();

                                    let dbg_ic = if debug_log_enabled { colors.accent } else { text_sec };
                                    if Self::menu_row(
                                        ui, "debug", dbg_ic,
                                        egui::RichText::new("Enable Logging").size(12.0).color(text_pri),
                                        debug_log_enabled, colors.accent,
                                        |ui| {
                                            let status = if debug_log_enabled { "On" } else { "Off" };
                                            let c = if debug_log_enabled { colors.accent } else { text_sec };
                                            ui.add(egui::Label::new(egui::RichText::new(status).size(11.0).color(c)).selectable(false));
                                        },
                                    ).clicked() { new_debug_toggle = Some(!debug_log_enabled); }

                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(
                                            "Logs file open, edits, sort, save, and IPC events with timing. \
                                             Zero overhead when off.",
                                        )
                                        .size(10.0)
                                        .color(text_sec),
                                    );

                                    ui.add_space(6.0);
                                    ui.separator();
                                    ui.label(
                                        egui::RichText::new("Session log")
                                            .size(10.0)
                                            .color(text_sec)
                                            .text_style(egui::TextStyle::Small),
                                    );

                                    if debug_log_enabled {
                                        if let Some(path) = crate::debug_log::current_log_path() {
                                            let file_name = path
                                                .file_name()
                                                .map(|f| f.to_string_lossy().into_owned())
                                                .unwrap_or_default();
                                            let dir = path
                                                .parent()
                                                .map(|p| p.display().to_string())
                                                .unwrap_or_default();
                                            let size_kb = std::fs::metadata(&path)
                                                .ok()
                                                .map(|m| m.len() / 1024)
                                                .unwrap_or(0);

                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new(file_name)
                                                        .size(11.0)
                                                        .color(text_pri)
                                                        .monospace(),
                                                );
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    ui.label(
                                                        egui::RichText::new(format!("{} KB", size_kb))
                                                            .size(10.0)
                                                            .color(text_sec),
                                                    );
                                                });
                                            });
                                            ui.label(
                                                egui::RichText::new(dir)
                                                    .size(10.0)
                                                    .color(text_sec)
                                                    .monospace(),
                                            );
                                            ui.add_space(4.0);
                                            if Self::menu_row(
                                                ui, "edit", colors.accent,
                                                egui::RichText::new("Open Log File").size(12.0).color(colors.accent),
                                                false, colors.accent,
                                                |_ui| {},
                                            ).clicked() {
                                                new_open_log = true;
                                            }
                                            if Self::menu_row(
                                                ui, "search", colors.accent,
                                                egui::RichText::new("Reveal in Finder").size(12.0).color(colors.accent),
                                                false, colors.accent,
                                                |_ui| {},
                                            ).clicked() {
                                                new_reveal_log = true;
                                            }
                                        }
                                    } else {
                                        ui.label(
                                            egui::RichText::new(
                                                "Inactive. Enable logging above to start a new session log.",
                                            )
                                            .size(10.0)
                                            .color(text_sec),
                                        );
                                    }
                                } else if theme_submenu {
                                    // Theme submenu
                                    if Self::menu_row(
                                        ui, "chevron-left", text_sec,
                                        egui::RichText::new("Back").size(12.0).color(text_sec),
                                        false, colors.accent,
                                        |_ui| {},
                                    ).clicked() { new_theme_sub = Some(false); }
                                    ui.separator();
                                    for (i, theme) in themes.iter().enumerate() {
                                        let is_active  = i == current_theme_idx;
                                        let tc = if is_active { colors.accent } else { text_pri };
                                        let ic = if is_active { colors.accent } else { text_sec };
                                        if Self::menu_row(
                                            ui, "theme", ic,
                                            egui::RichText::new(&theme.name).size(12.0).color(tc),
                                            is_active, colors.accent,
                                            |ui| {
                                                if is_active {
                                                    ui.add(egui::Label::new(egui::RichText::new("Active").size(11.0).color(colors.accent)).selectable(false));
                                                }
                                            },
                                        ).clicked() { new_theme_idx = Some(i); }
                                    }
                                } else {
                                    // Root settings
                                    if Self::menu_row(
                                        ui, "theme", colors.accent,
                                        egui::RichText::new(format!("Theme: {}", theme_name)).size(12.0).color(text_pri),
                                        false, colors.accent,
                                        |ui| { ui.add(crate::ui::icons::icon("chevron-right", text_sec)); },
                                    ).clicked() { new_theme_sub = Some(true); }

                                    let font_label = selected_font.as_deref().unwrap_or("Default");
                                    if Self::menu_row(
                                        ui, "font", colors.accent,
                                        egui::RichText::new(format!("Font: {}", font_label)).size(12.0).color(text_pri),
                                        false, colors.accent,
                                        |ui| { ui.add(crate::ui::icons::icon("chevron-right", text_sec)); },
                                    ).clicked() { new_font_sub = Some(true); }

                                    ui.separator();

                                    // Header row toggle
                                    let hdr_ic = if header_on { colors.accent } else { text_sec };
                                    if Self::menu_row(
                                        ui, "header-toggle", hdr_ic,
                                        egui::RichText::new("Header Row").size(12.0).color(text_pri),
                                        header_on, colors.accent,
                                        |ui| {
                                            let s = if header_on { "On" } else { "Off" };
                                            let c = if header_on { colors.accent } else { text_sec };
                                            ui.add(egui::Label::new(egui::RichText::new(s).size(11.0).color(c)).selectable(false));
                                        },
                                    ).clicked() { toggle_header = true; }

                                    // Tab mode toggle
                                    let tab_ic = if tab_mode { colors.accent } else { text_sec };
                                    if Self::menu_row(
                                        ui, "tabs", tab_ic,
                                        egui::RichText::new("Tab Mode").size(12.0).color(text_pri),
                                        tab_mode, colors.accent,
                                        |ui| {
                                            let s = if tab_mode { "On" } else { "Off" };
                                            let c = if tab_mode { colors.accent } else { text_sec };
                                            ui.add(egui::Label::new(egui::RichText::new(s).size(11.0).color(c)).selectable(false));
                                        },
                                    ).clicked() { toggle_tab_mode = Some(!tab_mode); }

                                    ui.separator();

                                    // Debug submenu link
                                    let dbg_color = if debug_log_enabled { colors.accent } else { text_sec };
                                    if Self::menu_row(
                                        ui, "debug", dbg_color,
                                        egui::RichText::new("Debug").size(12.0).color(text_pri),
                                        false, colors.accent,
                                        |ui| {
                                            ui.add(crate::ui::icons::icon("chevron-right", text_sec));
                                            let s = if debug_log_enabled { "On" } else { "Off" };
                                            let c = if debug_log_enabled { colors.accent } else { text_sec };
                                            ui.add(egui::Label::new(egui::RichText::new(s).size(11.0).color(c)).selectable(false));
                                            ui.add_space(4.0);
                                        },
                                    ).clicked() { new_debug_sub = Some(true); }
                                }
                                        });
                                    });
                                });
                            // Close on click outside (mirrors PopupCloseBehavior::CloseOnClickOutside).
                            if ui.input(|i| i.pointer.any_click()) {
                                if let Some(p) = ui.input(|i| i.pointer.interact_pos()) {
                                    if !area_resp.response.rect.contains(p)
                                        && !settings_btn.rect.contains(p)
                                    {
                                        ui.memory_mut(|m| m.close_popup());
                                        new_theme_sub = Some(false);
                                        new_font_sub = Some(false);
                                        new_debug_sub = Some(false);
                                    }
                                }
                            }
                        }
                    });
                });
            });

        // Apply mutations.
        if toggle_header {
            tab.state.header_row_enabled = !tab.state.header_row_enabled;
            tab.state.clear_cache();
            tab.state.invalidate_row_layout();
            tab.state.computed_stats = None;
            tab.state.stats_key.clear();
        }
        if let Some(idx) = new_theme_idx  { tab.state.set_theme_index(idx); }
        if let Some(s)   = new_pref_stat  { tab.state.preferred_stat = s; }
        if let Some(sub) = new_theme_sub  {
            tab.state.settings_theme_submenu = sub;
            if sub {
                tab.state.settings_font_submenu = false;
                tab.state.settings_debug_submenu = false;
            }
        }
        if let Some(sub) = new_font_sub {
            tab.state.settings_font_submenu = sub;
            if sub {
                tab.state.settings_theme_submenu = false;
                tab.state.settings_debug_submenu = false;
            }
        }
        if let Some(sub) = new_debug_sub {
            tab.state.settings_debug_submenu = sub;
            if sub {
                tab.state.settings_theme_submenu = false;
                tab.state.settings_font_submenu = false;
            }
        }
        if let Some(new_mode) = toggle_tab_mode {
            *tab_mode_toggle = Some(new_mode);
        }
        if let Some(on) = new_debug_toggle {
            *debug_toggle = Some(on);
        }
        if new_reveal_log {
            *reveal_log = true;
        }
        if new_open_log {
            *open_log = true;
        }
    }

    fn selection_shape_text_for(state: &AppState) -> Option<String> {
        use crate::state::SelectionType;
        match state.selection_type.as_ref()? {
            SelectionType::Cell => {
                let (min_r, max_r, min_c, max_c) = state.selection_range()?;
                let rows = max_r - min_r + 1;
                let cols = max_c - min_c + 1;
                if rows == 1 && cols == 1 { return None; }
                Some(format!("{} × {}", rows, cols))
            }
            SelectionType::Row => {
                let n = state.selected_rows.len();
                if n == 0 { return None; }
                Some(format!("{} rows", n))
            }
            SelectionType::Column => {
                let n = state.selected_columns.len();
                if n == 0 { return None; }
                Some(format!("{} cols", n))
            }
        }
    }

    fn format_size(bytes: u64) -> String {
        if bytes < 1024 { format!("{} B", bytes) }
        else if bytes < 1024 * 1024 { format!("{:.1} KB", bytes as f64 / 1024.0) }
        else if bytes < 1024 * 1024 * 1024 { format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)) }
        else { format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0)) }
    }
}
