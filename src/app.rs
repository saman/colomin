use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    /// Stored per-file settings stashed at load-start so we read disk once.
    /// Consumed (minus the sort, which is applied in the background loader) once the file finishes loading.
    pending_settings: Option<FileSettings>,
    sorting_rx: Option<std::sync::mpsc::Receiver<Result<SortResult, String>>>,
    searching_rx: Option<std::sync::mpsc::Receiver<Result<(String, bool, bool, u64, crate::csv_engine::types::SearchResult), String>>>,
    stats_rx: Option<std::sync::mpsc::Receiver<(String, crate::ui::stats::Stats)>>,
    /// Cancellation flag for the in-flight stats computation (if any).
    /// Setting it true tells the background thread to exit early.
    stats_cancel: Option<Arc<AtomicBool>>,
    was_resizing: bool,
}

impl TabState {
    fn new() -> Self {
        Self {
            state: AppState::new(),
            table: TableView::new(),
            loading: None,
            pending_settings: None,
            sorting_rx: None,
            searching_rx: None,
            stats_rx: None,
            stats_cancel: None,
            was_resizing: false,
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
        let settings = FileSettingsStore::get_for_path(&path);
        let initial_sort = settings.as_ref().and_then(|s| {
            match (s.sort_column_index, s.sort_ascending) {
                (Some(col), Some(asc)) => Some((col, asc)),
                _ => None,
            }
        });
        self.pending_settings = settings;
        self.loading = Some(file_open::open_file_async(path, initial_sort));
    }
}

// ── App config (persisted) ────────────────────────────────────────────────────

/// Per-OS config directory for Colomin.
///
/// On Unix (macOS + Linux) we use `$HOME/.config/colomin/` — preserving the
/// pre-cross-platform path so existing users keep their settings. On Windows
/// we use `%APPDATA%\colomin\` via the standard `dirs` lookup.
fn colomin_config_dir() -> std::path::PathBuf {
    #[cfg(unix)]
    {
        dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".config")
            .join("colomin")
    }
    #[cfg(windows)]
    {
        dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("colomin")
    }
}

fn default_tab_mode() -> bool { true }
fn default_font_size() -> f32 { 12.0 }
fn default_ui_scale() -> f32 { 1.0 }

#[derive(serde::Serialize, serde::Deserialize)]
struct AppConfig {
    #[serde(default)]
    selected_font: Option<String>,
    #[serde(default = "default_tab_mode")]
    tab_mode: bool,
    #[serde(default = "default_font_size")]
    font_size: f32,
    #[serde(default = "default_ui_scale")]
    ui_scale: f32,
    #[serde(default)]
    copy_mode: crate::state::CopyMode,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            selected_font: None,
            tab_mode: default_tab_mode(),
            font_size: default_font_size(),
            ui_scale: default_ui_scale(),
            copy_mode: crate::state::CopyMode::Text,
        }
    }
}

impl AppConfig {
    fn path() -> std::path::PathBuf {
        colomin_config_dir().join("settings.json")
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

// ── Per-file settings (persisted) ────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
struct FileSettings {
    #[serde(default)]
    header_row_enabled: Option<bool>,
    #[serde(default)]
    column_widths: std::collections::HashMap<usize, f32>,
    #[serde(default)]
    row_heights: std::collections::HashMap<usize, f32>,
    #[serde(default)]
    sort_column_index: Option<usize>,
    #[serde(default)]
    sort_ascending: Option<bool>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct FileSettingsStore(std::collections::HashMap<String, FileSettings>);

impl FileSettingsStore {
    fn path() -> std::path::PathBuf {
        colomin_config_dir().join("file_settings.json")
    }

    fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn disk_save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// Load stored settings for a specific file path (one-shot, no ownership kept).
    fn get_for_path(file_path: &str) -> Option<FileSettings> {
        Self::load().0.remove(file_path)
    }

    /// Persist current column widths, row heights, header mode, and sort for the tab's file.
    fn save_for_tab(tab: &TabState) {
        let Some(ref file) = tab.state.file else { return };
        let path = file.file_path.to_string_lossy().to_string();
        let mut store = Self::load();
        store.0.insert(path, FileSettings {
            header_row_enabled: Some(tab.state.header_row_enabled),
            column_widths: tab.state.column_widths.clone(),
            row_heights: tab.state.row_heights.clone(),
            sort_column_index: tab.state.sort_state.as_ref().map(|s| s.column_index),
            sort_ascending: tab.state.sort_state.as_ref()
                .map(|s| matches!(s.direction, SortDirection::Asc)),
        });
        store.disk_save();
    }

    /// Remove stored settings for the tab's file and reset view state to defaults.
    fn reset_for_tab(tab: &mut TabState) {
        let Some(ref file) = tab.state.file else { return };
        let path = file.file_path.to_string_lossy().to_string();
        let mut store = Self::load();
        store.0.remove(&path);
        store.disk_save();

        tab.state.header_row_enabled = true;
        tab.state.column_widths.clear();
        tab.state.row_heights.clear();
        tab.state.sort_state = None;
        if let Some(ref mut f) = tab.state.file {
            f.sort_permutation = None;
        }
        tab.state.clear_cache();
        tab.state.invalidate_row_layout();
        tab.state.invalidate_col_layout();
    }
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
    /// User-chosen zoom multiplier (1.0 = OS default, applied via ctx.set_zoom_factor).
    ui_scale: f32,
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
        cc.egui_ctx.set_zoom_factor(config.ui_scale);

        let mut initial_tab = TabState::new();
        crate::ui::theme::apply_theme(&cc.egui_ctx, &initial_tab.state.current_theme());

        let available_fonts = Self::enumerate_system_fonts();

        if let Some(ref font_name) = config.selected_font {
            Self::apply_font(&cc.egui_ctx, Some(font_name), &available_fonts);
        }
        initial_tab.state.selected_font = config.selected_font;
        initial_tab.state.font_size = config.font_size;
        initial_tab.state.copy_mode = config.copy_mode;

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
            ui_scale: config.ui_scale,
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
                    // Apply stashed non-sort settings (sort was already applied in the loader).
                    if let Some(settings) = tab.pending_settings.take() {
                        if let Some(header) = settings.header_row_enabled {
                            tab.state.header_row_enabled = header;
                            tab.state.clear_cache();
                            tab.state.invalidate_row_layout();
                        }
                        if !settings.column_widths.is_empty() {
                            tab.state.column_widths = settings.column_widths;
                            tab.state.invalidate_col_layout();
                        }
                        if !settings.row_heights.is_empty() {
                            tab.state.row_heights = settings.row_heights;
                            tab.state.invalidate_row_layout();
                        }
                    }
                    tab.loading = None;
                    ctx.request_repaint();
                }
                Ok(Err(e)) => {
                    eprintln!("Load error: {}", e);
                    tab.state.is_loading = false;
                    tab.state.loading_message.clear();
                    tab.loading = None;
                    tab.pending_settings = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    tab.state.is_loading = false;
                    tab.loading = None;
                    tab.pending_settings = None;
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
                        tab.state.invalidate_search_after_data_change();
                    }
                    tab.sorting_rx = None;
                    tab.state.is_sorting = false;
                    FileSettingsStore::save_for_tab(tab);
                    ctx.request_repaint();
                }
                Ok(Err(e)) => { eprintln!("Sort error: {}", e); tab.sorting_rx = None; tab.state.is_sorting = false; }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => { tab.sorting_rx = None; tab.state.is_sorting = false; }
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
                        ctx.request_repaint();
                    } else {
                        crate::dlog!(Debug, "Stats", "async stale (selection moved); discarding");
                    }
                    tab.state.stats_pending_key.clear();
                    tab.state.computing_stats = false;
                    tab.stats_rx = None;
                    tab.stats_cancel = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    tab.state.stats_pending_key.clear();
                    tab.state.computing_stats = false;
                    tab.stats_rx = None;
                    tab.stats_cancel = None;
                }
            }
        }

        // ── Poll background search ──
        if let Some(rx) = &tab.searching_rx {
            match rx.try_recv() {
                Ok(Ok((query, case_sensitive, regex_enabled, cache_version, result))) => {
                    if tab.state.show_search
                        && tab.state.search_query == query
                        && tab.state.search_case_sensitive == case_sensitive
                        && tab.state.search_regex == regex_enabled
                        && tab.state.cache_version == cache_version
                    {
                        tab.state.apply_search_results(result.matches);
                    }
                    tab.searching_rx = None;
                    ctx.request_repaint();
                }
                Ok(Err(e)) => { eprintln!("Search error: {}", e); tab.searching_rx = None; }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint(),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => { tab.searching_rx = None; }
            }
        }

        // ── Kick off stats computation when selection changes ──
        {
            use crate::ui::stats as st;
            const ASYNC_THRESHOLD: usize = 10_000;
            let key = tab.state.selection_stats_key();
            let needs_new = !key.is_empty()
                && key != tab.state.stats_key
                && key != tab.state.stats_pending_key;

            if needs_new {
                // Cancel any in-flight computation for the old selection. The
                // worker thread will exit at its next periodic cancel check;
                // its stale result (if any) is discarded on receive because
                // the selection key won't match.
                if let Some(c) = tab.stats_cancel.take() {
                    c.store(true, Ordering::Relaxed);
                    crate::dlog!(Debug, "Stats", "cancelling in-flight (selection changed)");
                }
                tab.stats_rx = None;
                tab.state.stats_pending_key.clear();

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
                    let cancel = Arc::new(AtomicBool::new(false));
                    let cancel_thread = Arc::clone(&cancel);
                    tab.stats_cancel = Some(cancel);
                    tab.state.stats_pending_key = key;
                    let (tx, rx) = std::sync::mpsc::channel();
                    crate::dlog!(Info, "Stats", "spawn async cells={}", cell_count);
                    std::thread::spawn(move || {
                        let _t = crate::dspan!("Stats", "compute_async");
                        let result = crate::ui::stats::compute_stats_snapshot(&snap, &cancel_thread);
                        let _ = tx.send((key2, result));
                    });
                    tab.stats_rx = Some(rx);
                    ctx.request_repaint();
                }
            } else if key.is_empty() {
                if let Some(c) = tab.stats_cancel.take() {
                    c.store(true, Ordering::Relaxed);
                }
                tab.state.computed_stats = None;
                tab.state.stats_key.clear();
                tab.state.stats_pending_key.clear();
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
                    tab.state.is_sorting = true;
                    ctx.request_repaint();
                }
            }
        }

        // ── Dispatch pending search to background thread ──
        let tab = &mut self.tabs[self.active_tab];
        if tab.searching_rx.is_none() {
            if let Some(query) = tab.state.pending_search.take() {
                if query.is_empty() {
                    tab.state.search_results.clear();
                    tab.state.search_results_set.clear();
                    tab.state.search_cursor = None;
                } else if let Some(ref file) = tab.state.file {
                    let query_for_thread = query.clone();
                    let path = file.file_path.clone();
                    let row_offsets = file.row_offsets.clone();
                    let row_sources = tab.state.search_row_sources_if_needed();
                    let edits = file.edits.clone();
                    let col_count = file.metadata.columns.len();
                    let delimiter = file.delimiter;
                    let case_sensitive = tab.state.search_case_sensitive;
                    let regex_enabled = tab.state.search_regex;
                    let cache_version = tab.state.cache_version;
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let result = crate::csv_engine::query::search_rows(
                            &path,
                            &row_offsets,
                            row_sources.as_deref(),
                            &edits,
                            &query_for_thread,
                            None,
                            col_count,
                            delimiter,
                            case_sensitive,
                            regex_enabled,
                        )
                        .map(|result| (query_for_thread, case_sensitive, regex_enabled, cache_version, result));
                        let _ = tx.send(result);
                    });
                    tab.searching_rx = Some(rx);
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

        // Cmd+T: new empty tab (tab mode only) — Mac convention.
        let new_tab_shortcut = ctx.input(|i| {
            i.key_pressed(egui::Key::T) && i.modifiers.command && !i.modifiers.shift
        });
        if new_tab_shortcut && self.tab_mode {
            self.tabs.push(TabState::new());
            self.active_tab = self.tabs.len() - 1;
        }

        let tab = &mut self.tabs[self.active_tab];

        let save_file = ctx.input(|i| i.key_pressed(egui::Key::S) && i.modifiers.command);
        if save_file { Self::handle_save_tab(tab); }

        let search_shortcut = ctx.input(|i| i.key_pressed(egui::Key::F) && i.modifiers.command);
        if search_shortcut {
            tab.table.commit_active_edit(&mut tab.state);
            tab.state.focus_search_input = true;
            tab.state.show_search = true;
        }

        // Cmd+Shift+T: cycle theme.
        let cycle_theme = ctx.input(|i| {
            i.key_pressed(egui::Key::T) && i.modifiers.command && i.modifiers.shift
        });
        if cycle_theme { tab.state.cycle_theme(); }

        // Pre-read tab config fields so zoom shortcuts don't need to re-borrow self.tabs.
        let zoom_selected_font = tab.state.selected_font.clone();
        let zoom_font_size     = tab.state.font_size;
        let zoom_copy_mode     = tab.state.copy_mode;

        // ── Zoom shortcuts: Cmd+= / Cmd+- / Cmd+0 ──
        let zoom_in    = ctx.input(|i| i.key_pressed(egui::Key::Equals) && i.modifiers.command);
        let zoom_out   = ctx.input(|i| i.key_pressed(egui::Key::Minus)  && i.modifiers.command);
        let zoom_reset = ctx.input(|i| i.key_pressed(egui::Key::Num0)   && i.modifiers.command);
        if zoom_in || zoom_out || zoom_reset {
            let new_scale = if zoom_reset {
                1.0_f32
            } else if zoom_in {
                ((self.ui_scale + 0.05) * 20.0).round() / 20.0
            } else {
                ((self.ui_scale - 0.05) * 20.0).round() / 20.0
            }.clamp(1.0, 2.0);
            self.ui_scale = new_scale;
            ctx.set_zoom_factor(new_scale);
            AppConfig {
                selected_font: zoom_selected_font,
                tab_mode: self.tab_mode,
                font_size: zoom_font_size,
                ui_scale: new_scale,
                copy_mode: zoom_copy_mode,
            }.save();
        }

        // ── Apply theme + UI scale ──
        crate::ui::theme::apply_theme(ctx, &tab.state.current_theme());
        ctx.set_zoom_factor(self.ui_scale);

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
            let text_color = tab.state.current_theme().text_secondary;
            let panel_fill = tab.state.current_theme().bg;
            let mut open_dialog = false;
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE.fill(panel_fill))
                .show(ctx, |ui| {
                    let resp = ui.allocate_response(
                        ui.available_size(),
                        egui::Sense::click(),
                    );
                    if resp.clicked() {
                        open_dialog = true;
                    }
                    if resp.hovered() {
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    ui.painter().text(
                        resp.rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "Open a CSV file with ⌘O, or drag and drop a file here",
                        egui::FontId::proportional(13.0),
                        text_color,
                    );
                });
            if open_dialog {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("CSV Files", &["csv", "tsv", "txt"])
                    .add_filter("All Files", &["*"])
                    .pick_file()
                {
                    self.open_file_in_tab(path.to_string_lossy().into_owned());
                }
            }
            return;
        }

        // ── Status bar ──
        let mut tab_mode_toggle: Option<bool> = None;
        let mut font_choice: Option<Option<String>> = None;
        let mut debug_toggle: Option<bool> = None;
        let mut reveal_log: bool = false;
        let mut open_log: bool = false;
        let mut font_size_change: Option<f32> = None;
        let mut ui_scale_change: Option<f32> = None;
        let mut reset_all = false;
        let mut reset_file_settings = false;
        let mut copy_mode_change: Option<crate::state::CopyMode> = None;
        let header_before = tab.state.header_row_enabled;
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
            &mut font_size_change,
            self.ui_scale,
            &mut ui_scale_change,
            &mut reset_all,
            &mut reset_file_settings,
            &mut copy_mode_change,
            ctx,
        );
        if tab.state.header_row_enabled != header_before {
            FileSettingsStore::save_for_tab(tab);
        }

        if let Some(choice) = font_choice {
            let tab = &mut self.tabs[self.active_tab];
            tab.state.selected_font = choice.clone();
            Self::apply_font(ctx, choice.as_deref(), &self.available_fonts);
            AppConfig {
                selected_font: choice,
                tab_mode: self.tab_mode,
                font_size: tab.state.font_size,
                ui_scale: self.ui_scale,
                copy_mode: tab.state.copy_mode,
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
                font_size: self.tabs[self.active_tab].state.font_size,
                ui_scale: self.ui_scale,
                copy_mode: self.tabs[self.active_tab].state.copy_mode,
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
                let _ = opener::open(&path);
            }
        }

        if let Some(new_size) = font_size_change {
            let tab = &mut self.tabs[self.active_tab];
            tab.state.font_size = new_size;
            AppConfig {
                selected_font: tab.state.selected_font.clone(),
                tab_mode: self.tab_mode,
                font_size: new_size,
                ui_scale: self.ui_scale,
                copy_mode: tab.state.copy_mode,
            }.save();
        }

        if let Some(new_scale) = ui_scale_change {
            self.ui_scale = new_scale;
            ctx.set_zoom_factor(new_scale);
            let tab = &self.tabs[self.active_tab];
            AppConfig {
                selected_font: tab.state.selected_font.clone(),
                tab_mode: self.tab_mode,
                font_size: tab.state.font_size,
                ui_scale: new_scale,
                copy_mode: tab.state.copy_mode,
            }.save();
        }

        if let Some(mode) = copy_mode_change {
            let tab = &mut self.tabs[self.active_tab];
            tab.state.copy_mode = mode;
            AppConfig {
                selected_font: tab.state.selected_font.clone(),
                tab_mode: self.tab_mode,
                font_size: tab.state.font_size,
                ui_scale: self.ui_scale,
                copy_mode: mode,
            }.save();
        }

        if reset_file_settings {
            let tab = &mut self.tabs[self.active_tab];
            FileSettingsStore::reset_for_tab(tab);
            tab.state.settings_menu = false;
        }

        if reset_all {
            let defaults = AppConfig::default();
            self.ui_scale = defaults.ui_scale;
            self.tab_mode = defaults.tab_mode;
            ctx.set_zoom_factor(defaults.ui_scale);
            Self::apply_font(ctx, None, &self.available_fonts);
            let tab = &mut self.tabs[self.active_tab];
            tab.state.selected_font = None;
            tab.state.font_size = defaults.font_size;
            tab.state.copy_mode = defaults.copy_mode;
            tab.state.header_row_enabled = true;
            tab.state.row_highlight_on_hover = true;
            tab.state.set_theme_index(0);
            tab.state.clear_cache();
            tab.state.invalidate_row_layout();
            defaults.save();
        }

        // ── Search bar ──
        let tab = &mut self.tabs[self.active_tab];
        let mut search_close = false;
        let mut search_nav: i32 = 0;
        let mut search_query_changed = false;
        let mut search_case_changed = false;
        let mut search_regex_changed = false;
        let search_field_id = egui::Id::new("search_bar_input");

        if tab.state.show_search {
            let theme = tab.state.current_theme();
            let text_pri = theme.text_primary;
            let text_sec = theme.text_secondary;
            let prev_query = tab.state.search_query.clone();
            egui::TopBottomPanel::bottom("search_bar")
                .exact_height(36.0)
                .frame(egui::Frame::NONE.fill(theme.status_bar_bg))
                .show(ctx, |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.add_space(10.0);

                        let regex_active = tab.state.search_regex;
                        let regex_button = egui::Button::new(
                            egui::RichText::new(".*")
                                .size(12.0)
                                .color(if regex_active { theme.accent } else { text_sec }),
                        )
                        .fill(if regex_active { theme.accent_subtle } else { egui::Color32::TRANSPARENT })
                        .stroke(egui::Stroke::new(1.0, if regex_active { theme.accent } else { theme.border }))
                        .min_size(egui::vec2(30.0, 22.0))
                        .corner_radius(egui::CornerRadius::same(4));
                        if ui.add(regex_button).on_hover_text("Regex").clicked() {
                            tab.state.search_regex = !tab.state.search_regex;
                            search_regex_changed = true;
                        }

                        ui.add_space(4.0);

                        let case_active = tab.state.search_case_sensitive;
                        let case_button = egui::Button::new(
                            egui::RichText::new("Aa")
                                .size(12.0)
                                .color(if case_active { theme.accent } else { text_sec }),
                        )
                        .fill(if case_active { theme.accent_subtle } else { egui::Color32::TRANSPARENT })
                        .stroke(egui::Stroke::new(1.0, if case_active { theme.accent } else { theme.border }))
                        .min_size(egui::vec2(30.0, 22.0))
                        .corner_radius(egui::CornerRadius::same(4));
                        if ui.add(case_button).on_hover_text("Case sensitive").clicked() {
                            tab.state.search_case_sensitive = !tab.state.search_case_sensitive;
                            search_case_changed = true;
                        }

                        ui.add_space(8.0);

                        // Search icon (non-interactive leading glyph)
                        let icon_sz = egui::vec2(15.0, 15.0);
                        let (search_icon_rect, _) = ui.allocate_exact_size(icon_sz, egui::Sense::hover());
                        crate::ui::icons::icon("search", text_sec).paint_at(ui, search_icon_rect);
                        ui.add_space(6.0);

                        let match_text = if tab.state.search_query.is_empty() {
                            String::new()
                        } else if tab.state.search_results.is_empty() {
                            "No matches".to_string()
                        } else if let Some(cursor) = tab.state.search_cursor {
                            format!("{} of {}", cursor + 1, tab.state.search_results.len())
                        } else {
                            let n = tab.state.search_results.len();
                            if n == 1 {
                                "1 match".to_string()
                            } else {
                                format!("{} matches", n)
                            }
                        };
                        let match_text_w = ui.fonts(|f| {
                            f.layout_no_wrap(
                                match_text.clone(),
                                egui::FontId::proportional(12.0),
                                text_sec,
                            ).size().x
                        });

                        // Text field
                        let right_controls_w = match_text_w + 88.0;
                        let field_w = (ui.available_width() - right_controls_w).max(64.0);
                        let te = egui::TextEdit::singleline(&mut tab.state.search_query)
                            .id(search_field_id)
                            .hint_text("Find in table…")
                            .frame(false)
                            .desired_width((field_w - 16.0).max(48.0));
                        let input_frame = egui::Frame::NONE
                            .fill(theme.surface)
                            .stroke(egui::Stroke::new(1.0, theme.border))
                            .corner_radius(egui::CornerRadius::same(4))
                            .inner_margin(egui::Margin::symmetric(8, 1))
                            .outer_margin(egui::Margin { left: 0, right: 0, top: 3, bottom: 3 });
                        let resp = input_frame
                            .show(ui, |ui| {
                                ui.set_min_width((field_w - 16.0).max(48.0));
                                ui.add(te)
                            })
                            .inner;
                        if tab.state.focus_search_input {
                            ctx.memory_mut(|m| m.request_focus(search_field_id));
                            tab.state.focus_search_input = false;
                        }
                        if tab.state.search_query != prev_query {
                            search_query_changed = true;
                        }
                        if resp.has_focus() {
                            if ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift) {
                                search_nav = 1;
                            }
                            if ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.shift) {
                                search_nav = -1;
                            }
                            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                search_close = true;
                            }
                        }

                        ui.add_space(10.0);

                        // Match count
                        ui.label(egui::RichText::new(match_text).color(text_sec).size(12.0));

                        ui.add_space(6.0);

                        // ↑ prev button
                        let (up_rect, up_resp) = ui.allocate_exact_size(icon_sz, egui::Sense::click());
                        let up_color = if up_resp.hovered() { text_pri } else { text_sec };
                        crate::ui::icons::icon("chevron-up", up_color).paint_at(ui, up_rect);
                        if up_resp.clicked() { search_nav = -1; }

                        ui.add_space(4.0);

                        // ↓ next button
                        let (dn_rect, dn_resp) = ui.allocate_exact_size(icon_sz, egui::Sense::click());
                        let dn_color = if dn_resp.hovered() { text_pri } else { text_sec };
                        crate::ui::icons::icon("chevron-down", dn_color).paint_at(ui, dn_rect);
                        if dn_resp.clicked() { search_nav = 1; }

                        ui.add_space(8.0);

                        // ✕ close button
                        let (x_rect, x_resp) = ui.allocate_exact_size(icon_sz, egui::Sense::click());
                        let x_color = if x_resp.hovered() { text_pri } else { text_sec };
                        crate::ui::icons::icon("x", x_color).paint_at(ui, x_rect);
                        if x_resp.clicked() { search_close = true; }

                        ui.add_space(10.0);
                    });
                });
        }

        let tab = &mut self.tabs[self.active_tab];
        if search_close || search_nav != 0 {
            tab.state.suppress_table_keyboard_once = true;
        }
        if search_close {
            tab.state.show_search = false;
            tab.state.search_query.clear();
            tab.state.search_results.clear();
            tab.state.search_results_set.clear();
            tab.state.search_cursor = None;
            tab.state.search_scroll_to = None;
            tab.state.pending_search = None;
            tab.state.focus_search_input = false;
            ctx.memory_mut(|m| m.surrender_focus(search_field_id));
        }
        if search_query_changed || search_case_changed || search_regex_changed {
            tab.state.search_results.clear();
            tab.state.search_results_set.clear();
            tab.state.pending_search = Some(tab.state.search_query.clone());
            tab.state.search_cursor = None;
        }
        if search_nav != 0 && !tab.state.search_results.is_empty() {
            let n = tab.state.search_results.len();
            let current = tab.state.search_cursor;
            let next = if search_nav > 0 {
                current.map_or(0, |cursor| (cursor + 1) % n)
            } else if let Some(cursor) = current {
                if cursor == 0 { n - 1 } else { cursor - 1 }
            } else {
                n - 1
            };
            tab.state.search_cursor = Some(next);
            if let Some(coord) = tab.state.active_search_cell() {
                tab.state.selection_type = Some(crate::state::SelectionType::Cell);
                tab.state.selection_anchor = Some(coord);
                tab.state.selection_focus = Some(coord);
                tab.state.selected_rows.clear();
                tab.state.selected_columns.clear();
                tab.state.search_scroll_to = Some(coord);
            }
        }

        // ── Cell editor sidebar ──
        let tab = &mut self.tabs[self.active_tab];
        let mut sidebar_save: Option<(usize, usize, String)> = None;
        let mut sidebar_close = false;
        let mut sidebar_new_width: Option<f32> = None;
        if tab.state.cell_editor.is_some() {
            let colors      = tab.state.current_theme();
            let sidebar_fill = colors.surface;
            let text_pri    = colors.text_primary;
            let text_sec    = colors.text_secondary;
            let accent      = colors.accent;
            let border      = colors.border;
            let danger      = colors.danger;
            let sidebar_w   = tab.table.cell_editor_width;

            let col_label = if let Some((_, col, _)) = tab.state.cell_editor {
                crate::ui::table::col_name_for_display_pub(&tab.state, col)
            } else {
                String::new()
            };
            let row_label = if let Some((row, _, _)) = tab.state.cell_editor {
                format!("Row {}", row + 1)
            } else {
                String::new()
            };

            // Use resizable(false) + manual drag handle so the width never snaps
            // back due to egui's own panel-open animation conflicting with user drags.
            egui::SidePanel::right("cell_editor_panel")
                .resizable(false)
                .default_width(sidebar_w)
                .min_width(sidebar_w)
                .max_width(sidebar_w)
                .frame(egui::Frame::NONE
                    .fill(sidebar_fill)
                    .inner_margin(egui::Margin::same(0)))
                .show(ctx, |ui| {
                    let panel_rect = ui.max_rect();

                    // ── Custom resize handle (left 5px strip) ──
                    let handle_rect = egui::Rect::from_min_max(
                        panel_rect.left_top(),
                        egui::pos2(panel_rect.left() + 5.0, panel_rect.bottom()),
                    );
                    let handle_resp = ui.interact(
                        handle_rect,
                        egui::Id::new("cell_editor_resize"),
                        egui::Sense::drag(),
                    );
                    if handle_resp.hovered() || handle_resp.dragged() {
                        ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }
                    if handle_resp.dragged() {
                        // Dragging left → dx negative → width increases.
                        let new_w = (sidebar_w - handle_resp.drag_delta().x)
                            .clamp(200.0, 800.0);
                        sidebar_new_width = Some(new_w);
                    }
                    // Paint the handle as a 1px border line (accent when active).
                    let handle_color = if handle_resp.hovered() || handle_resp.dragged() {
                        accent
                    } else {
                        border
                    };
                    ui.painter().rect_filled(
                        egui::Rect::from_min_max(
                            panel_rect.left_top(),
                            egui::pos2(panel_rect.left() + 1.0, panel_rect.bottom()),
                        ),
                        0.0,
                        handle_color,
                    );

                    // ── Header bar ──
                    let header_h = 40.0;
                    let header_rect = egui::Rect::from_min_size(
                        panel_rect.min,
                        egui::vec2(panel_rect.width(), header_h),
                    );
                    ui.painter().rect_filled(header_rect, 0.0, colors.gutter_bg);
                    ui.painter().rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(header_rect.min.x, header_rect.max.y - 1.0),
                            header_rect.max,
                        ),
                        0.0,
                        border,
                    );

                    // Title
                    ui.painter().text(
                        egui::pos2(panel_rect.left() + 14.0, header_rect.center().y - 7.0),
                        egui::Align2::LEFT_CENTER,
                        "Edit Cell",
                        egui::FontId::proportional(13.0),
                        text_pri,
                    );
                    ui.painter().text(
                        egui::pos2(panel_rect.left() + 14.0, header_rect.center().y + 7.0),
                        egui::Align2::LEFT_CENTER,
                        format!("{} · {}", row_label, col_label),
                        egui::FontId::proportional(11.0),
                        text_sec,
                    );

                    // Close (×) button
                    let close_rect = egui::Rect::from_center_size(
                        egui::pos2(panel_rect.right() - 20.0, header_rect.center().y),
                        egui::vec2(24.0, 24.0),
                    );
                    let close_resp = ui.interact(
                        close_rect,
                        egui::Id::new("cell_editor_close_btn"),
                        egui::Sense::click(),
                    );
                    ui.painter().text(
                        close_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "×",
                        egui::FontId::proportional(18.0),
                        if close_resp.hovered() { text_pri } else { text_sec },
                    );
                    if close_resp.clicked() {
                        sidebar_close = true;
                    }

                    // Push layout cursor below the header
                    ui.add_space(header_h);
                    ui.add_space(10.0);

                    let available_w = ui.available_width() - 24.0;
                    let available_h = (ui.available_height() - 60.0).max(80.0);

                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        if let Some((_, _, ref mut buf)) = tab.state.cell_editor {
                            let te = egui::TextEdit::multiline(buf)
                                .font(egui::FontId::proportional(13.0))
                                .desired_width(available_w)
                                .desired_rows(1)
                                .min_size(egui::vec2(available_w, available_h))
                                .frame(true);
                            ui.add(te);
                        }
                    });

                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        let save_btn = egui::Button::new(
                            egui::RichText::new("Save").color(egui::Color32::WHITE).size(12.0),
                        )
                        .fill(accent)
                        .min_size(egui::vec2(70.0, 28.0))
                        .corner_radius(egui::CornerRadius::same(4));
                        if ui.add(save_btn).clicked() {
                            if let Some((r, c, ref buf)) = tab.state.cell_editor {
                                sidebar_save = Some((r, c, buf.clone()));
                            }
                        }

                        ui.add_space(8.0);

                        let cancel_btn = egui::Button::new(
                            egui::RichText::new("Cancel").color(danger).size(12.0),
                        )
                        .fill(egui::Color32::TRANSPARENT)
                        .stroke(egui::Stroke::new(1.0, danger))
                        .min_size(egui::vec2(70.0, 28.0))
                        .corner_radius(egui::CornerRadius::same(4));
                        if ui.add(cancel_btn).clicked() {
                            sidebar_close = true;
                        }
                    });

                    // Cmd+Enter → save
                    if ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command) {
                        if let Some((r, c, ref buf)) = tab.state.cell_editor {
                            sidebar_save = Some((r, c, buf.clone()));
                        }
                    }
                    // Escape → close
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        sidebar_close = true;
                    }
                });
        }

        // Apply sidebar actions
        let tab = &mut self.tabs[self.active_tab];
        if let Some(w) = sidebar_new_width {
            tab.table.cell_editor_width = w;
        }
        if let Some((r, c, new_val)) = sidebar_save {
            crate::ui::table::commit_edit_pub(&mut tab.state, r, c, new_val);
            tab.state.cell_editor = None;
        }
        if sidebar_close {
            tab.state.cell_editor = None;
        }

        // ── Main table ──
        let tab = &mut self.tabs[self.active_tab];
        let panel_fill = tab.state.current_theme().bg;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(panel_fill))
            .show(ctx, |ui| {
                tab.table.show(ui, &mut tab.state, ctx);
            });

        // Save file settings when a column/row resize drag ends or double-click auto-fit fires.
        let tab = &mut self.tabs[self.active_tab];
        let is_resizing = tab.table.is_resizing();
        if (tab.was_resizing && !is_resizing) || tab.table.save_requested {
            FileSettingsStore::save_for_tab(tab);
        }
        tab.was_resizing = is_resizing;
        tab.table.save_requested = false;
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

    /// Show `path` in the system file manager.
    ///
    /// - macOS: Finder, highlighting the file (`open -R`).
    /// - Windows: Explorer, highlighting the file (`explorer /select,`).
    /// - Linux: opens the *containing directory* (xdg-open via `opener`).
    ///   Per-file highlighting in nautilus/dolphin/etc. would require a
    ///   D-Bus call to `org.freedesktop.FileManager1`, deferred for now.
    fn reveal_in_finder(path: &std::path::Path) {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .arg("-R")
                .arg(path)
                .spawn();
        }
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("explorer.exe")
                .arg(format!("/select,{}", path.display()))
                .spawn();
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if let Some(dir) = path.parent() {
                let _ = opener::open(dir);
            }
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
        font_size_change: &mut Option<f32>,
        ui_scale: f32,
        ui_scale_change: &mut Option<f32>,
        reset_all: &mut bool,
        reset_file_settings: &mut bool,
        copy_mode_change: &mut Option<crate::state::CopyMode>,
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
        let stats_key    = tab.state.selection_stats_key();
        let current_stats = if tab.state.stats_key == stats_key { tab.state.computed_stats } else { None };
        let pref         = tab.state.preferred_stat;
        let shape_text   = Self::selection_shape_text_for(&tab.state);

        let theme_name        = tab.state.theme_name();
        let current_theme_idx = tab.state.theme_index;
        let header_on         = tab.state.header_row_enabled;
        let row_hover_on      = tab.state.row_highlight_on_hover;
        let themes            = crate::ui::theme::bundled_themes();
        let theme_submenu     = tab.state.settings_theme_submenu;
        let font_submenu      = tab.state.settings_font_submenu;
        let debug_submenu     = tab.state.settings_debug_submenu;
        let copy_mode_submenu = tab.state.settings_copy_mode_submenu;
        let settings_open     = tab.state.settings_menu;
        let selected_font     = tab.state.selected_font.clone();
        let font_size         = tab.state.font_size;
        let copy_mode         = tab.state.copy_mode;

        let mut toggle_header    = false;
        let mut toggle_row_hover = false;
        let mut new_theme_idx:     Option<usize>                         = None;
        let mut new_pref_stat:     Option<crate::state::PreferredStat>   = None;
        let mut new_theme_sub:     Option<bool>                          = None;
        let mut new_font_sub:      Option<bool>                          = None;
        let mut new_debug_sub:     Option<bool>                          = None;
        let mut new_copy_mode_sub: Option<bool>                          = None;
        let mut new_copy_mode:     Option<crate::state::CopyMode>        = None;
        let mut new_settings_open: Option<bool>                          = None;
        let mut toggle_tab_mode:   Option<bool>                          = None;
        let mut new_debug_toggle:  Option<bool>                          = None;
        let mut new_reveal_log:    bool                                  = false;
        let mut new_open_log:      bool                                  = false;
        let mut new_font_size:     Option<f32>                           = None;
        let mut new_reset_all:          bool                                  = false;
        let mut new_reset_file_settings: bool                                 = false;

        egui::TopBottomPanel::bottom("status_bar")
            .frame(egui::Frame::NONE.fill(fill).inner_margin(egui::Margin::symmetric(8, 4)))
            .show(ctx, |ui| {
                ui.style_mut().override_font_id = Some(egui::FontId::monospace(11.0));
                ui.style_mut().interaction.selectable_labels = false;
                ui.horizontal(|ui| {
                    let bar_rect = ui.max_rect();

                    // ── Left: file info ──
                    if has_file {
                        let row_text = if has_filter {
                            format!("{} / {} rows", st::format_with_commas(rows), st::format_with_commas(unfiltered))
                        } else {
                            format!("{} rows", st::format_with_commas(rows))
                        };
                        ui.colored_label(text_pri, row_text);
                        ui.colored_label(text_sec, "·");
                        ui.colored_label(text_sec, format!("{} cols", cols));
                        ui.colored_label(text_sec, "·");
                        ui.colored_label(text_sec, Self::format_size(bytes));
                        if let Some(ref shape) = shape_text {
                            ui.colored_label(text_sec, "·");
                            ui.colored_label(text_pri, shape);
                        }
                    }
                    let left_edge = ui.min_rect().right();

                    // ── Center: stats badge ──
                    if !stats_key.is_empty() {
                        let badge_display = if let Some(s) = current_stats {
                            let (short, full, _) = st::format_stat_display(s, pref);
                            Some((
                                format!("{}: {}", pref.label(), short),
                                format!("{}: {}", pref.label(), full),
                            ))
                        } else {
                            None
                        };

                        if let Some((ref short_label, ref full_label)) = badge_display {
                            const BADGE_HALF_W: f32 = 55.0;
                            let space = (bar_rect.center().x - BADGE_HALF_W - left_edge).max(8.0);
                            ui.add_space(space);

                            if let Some(s) = current_stats {
                                let badge_id = egui::Id::new("stats_picker_popup");
                                let stat_icon = crate::ui::icons::icon(
                                    crate::ui::icons::stat_icon_name(pref), text_sec,
                                ).fit_to_exact_size(egui::vec2(11.0, 11.0));
                                let mut badge_btn = ui.add(
                                    egui::Button::image_and_text(stat_icon, short_label.as_str()).small(),
                                );
                                if short_label != full_label {
                                    badge_btn = badge_btn.on_hover_text(full_label.as_str());
                                }
                                if badge_btn.clicked() {
                                    if egui::Popup::is_id_open(ui.ctx(), badge_id) {
                                        egui::Popup::close_id(ui.ctx(), badge_id);
                                    } else {
                                        egui::Popup::open_id(ui.ctx(), badge_id);
                                    }
                                }
                                #[allow(deprecated)]
                                egui::popup_above_or_below_widget(
                                    ui, badge_id, &badge_btn,
                                    egui::AboveOrBelow::Above,
                                    egui::PopupCloseBehavior::CloseOnClickOutside,
                                    |ui| {
                                        ui.set_min_width(180.0);
                                        let flash_id = egui::Id::new("stats_copy_flash");
                                        let flash: Option<(crate::state::PreferredStat, std::time::Instant)> =
                                            ui.ctx().data(|d| d.get_temp(flash_id));
                                        for &stat in crate::state::PreferredStat::ALL.iter() {
                                            let (short, full, _) = st::format_stat_display(s, stat);
                                            let is_active = pref == stat;
                                            let icon_color = if is_active { colors.accent } else { text_sec };
                                            let text_color = if is_active { colors.accent } else { text_pri };
                                            let sv_color = if is_active { colors.accent } else { text_sec };
                                            let lbl = egui::RichText::new(stat.label()).size(12.0).color(text_color);

                                            // Custom row (inline rather than `menu_row`) so we
                                            // can layer a hover-revealed copy button on top.
                                            let row_h = 26.0;
                                            let avail_w = ui.available_width();
                                            let (rect, row_resp) = ui.allocate_exact_size(
                                                egui::vec2(avail_w, row_h),
                                                egui::Sense::click(),
                                            );
                                            let row_hovered = ui.rect_contains_pointer(rect);
                                            let bg = if is_active {
                                                egui::Color32::from_rgba_unmultiplied(
                                                    colors.accent.r(), colors.accent.g(),
                                                    colors.accent.b(), 28,
                                                )
                                            } else if row_hovered {
                                                ui.visuals().widgets.hovered.weak_bg_fill
                                            } else {
                                                egui::Color32::TRANSPARENT
                                            };
                                            if bg != egui::Color32::TRANSPARENT {
                                                ui.painter().rect_filled(rect, 4.0, bg);
                                            }

                                            let mut copy_clicked = false;
                                            let inner = rect.shrink2(egui::vec2(8.0, 0.0));
                                            ui.scope_builder(
                                                egui::UiBuilder::new()
                                                    .max_rect(inner)
                                                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                                                |ui| {
                                                    ui.add(crate::ui::icons::icon(
                                                        crate::ui::icons::stat_icon_name(stat),
                                                        icon_color,
                                                    ));
                                                    ui.add_space(8.0);
                                                    ui.add(egui::Label::new(lbl).selectable(false));
                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(egui::Align::Center),
                                                        |ui| {
                                                            // Always lay out the copy button so the
                                                            // value column doesn't jitter on hover.
                                                            // Paint it on row hover, OR while the
                                                            // post-click confirmation flash is active.
                                                            let flashing = flash
                                                                .map(|(s, t)| s == stat
                                                                    && t.elapsed() < std::time::Duration::from_millis(1200))
                                                                .unwrap_or(false);
                                                            let (icon_tint, visible) = if flashing {
                                                                (colors.accent, true)
                                                            } else {
                                                                (text_sec, row_hovered)
                                                            };
                                                            let copy_resp = ui.scope(|ui| {
                                                                if !visible { ui.set_opacity(0.0); }
                                                                let copy_icon = crate::ui::icons::icon(
                                                                    "copy", icon_tint,
                                                                ).fit_to_exact_size(egui::vec2(13.0, 13.0));
                                                                ui.add(
                                                                    egui::Button::image(copy_icon)
                                                                        .small().frame(false),
                                                                )
                                                            }).inner;
                                                            if row_hovered && copy_resp.clicked() {
                                                                ui.ctx().copy_text(full.clone());
                                                                ui.ctx().data_mut(|d| d.insert_temp(
                                                                    flash_id,
                                                                    (stat, std::time::Instant::now()),
                                                                ));
                                                                copy_clicked = true;
                                                            }
                                                            if flashing {
                                                                ui.ctx().request_repaint_after(
                                                                    std::time::Duration::from_millis(120),
                                                                );
                                                            }
                                                            ui.add_space(4.0);
                                                            let val_resp = ui.add(egui::Label::new(
                                                                egui::RichText::new(&short)
                                                                    .size(11.0).color(sv_color),
                                                            ).selectable(false));
                                                            if short != full {
                                                                let _ = val_resp.on_hover_text(full.as_str());
                                                            }
                                                        },
                                                    );
                                                },
                                            );

                                            if row_resp.clicked() && !copy_clicked {
                                                new_pref_stat = Some(stat);
                                                egui::Popup::close_id(ui.ctx(), badge_id);
                                            }
                                        }
                                    },
                                );
                            } else {
                                ui.colored_label(text_sec, short_label);
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
                            if settings_open {
                                new_settings_open = Some(false);
                                new_theme_sub = Some(false);
                                new_font_sub = Some(false);
                                new_debug_sub = Some(false);
                            } else {
                                new_settings_open = Some(true);
                            }
                        }
                        // Custom popover anchored at the gear's right edge with an
                        // 8px window-edge gutter. (Built directly on egui::Area so we
                        // can set our own pivot/offset; the convenience
                        // popup_above_or_below_widget pins LEFT_BOTTOM and clamps to
                        // the screen, leaving the popup flush against the right edge.)
                        if settings_open {
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
                                } else if copy_mode_submenu {
                                    // Copy Mode submenu
                                    if Self::menu_row(
                                        ui, "chevron-left", text_sec,
                                        egui::RichText::new("Back").size(12.0).color(text_sec),
                                        false, colors.accent,
                                        |_ui| {},
                                    ).clicked() { new_copy_mode_sub = Some(false); }
                                    ui.separator();
                                    for &mode in crate::state::CopyMode::ALL.iter() {
                                        let is_active = mode == copy_mode;
                                        let tc = if is_active { colors.accent } else { text_pri };
                                        let ic = if is_active { colors.accent } else { text_sec };
                                        if Self::menu_row(
                                            ui, mode.icon_name(), ic,
                                            egui::RichText::new(mode.label()).size(12.0).color(tc),
                                            is_active, colors.accent,
                                            |ui| {
                                                if is_active {
                                                    ui.add(egui::Label::new(egui::RichText::new("Active").size(11.0).color(colors.accent)).selectable(false));
                                                }
                                            },
                                        ).clicked() { new_copy_mode = Some(mode); }
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

                                    // Font size stepper row
                                    {
                                        let row_h = 26.0;
                                        let avail_w = ui.available_width();
                                        let (row_rect, _) = ui.allocate_exact_size(egui::vec2(avail_w, row_h), egui::Sense::hover());
                                        let inner = row_rect.shrink2(egui::vec2(8.0, 0.0));
                                        ui.scope_builder(
                                            egui::UiBuilder::new()
                                                .max_rect(inner)
                                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                                            |ui| {
                                                ui.add(crate::ui::icons::icon("font", colors.accent));
                                                ui.add_space(8.0);
                                                ui.add(egui::Label::new(egui::RichText::new("Font Size").size(12.0).color(text_pri)).selectable(false));
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    if ui.add(egui::Button::new(egui::RichText::new("+").size(12.0).color(text_pri)).small().frame(false)).clicked() {
                                                        if font_size < 18.0 { new_font_size = Some(font_size + 1.0); }
                                                    }
                                                    ui.add(egui::Label::new(egui::RichText::new(format!("{}", font_size as u32)).size(11.0).color(text_sec)).selectable(false));
                                                    if ui.add(egui::Button::new(egui::RichText::new("−").size(12.0).color(text_pri)).small().frame(false)).clicked() {
                                                        if font_size > 10.0 { new_font_size = Some(font_size - 1.0); }
                                                    }
                                                });
                                            },
                                        );
                                    }

                                    // Zoom stepper row — mirrors Font Size row.
                                    // [−] steps by 5%; [+] steps by 5%.
                                    // Middle field is a DragValue: drag or click-to-type.
                                    // Drag is deferred (commits on release) so the popup
                                    // doesn't shift mid-interaction.
                                    {
                                        let row_h = 26.0;
                                        let avail_w = ui.available_width();
                                        let (row_rect, _) = ui.allocate_exact_size(egui::vec2(avail_w, row_h), egui::Sense::hover());
                                        let inner = row_rect.shrink2(egui::vec2(8.0, 0.0));
                                        ui.scope_builder(
                                            egui::UiBuilder::new()
                                                .max_rect(inner)
                                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                                            |ui| {
                                                ui.add(crate::ui::icons::icon("zoom", colors.accent));
                                                ui.add_space(8.0);
                                                ui.add(egui::Label::new(egui::RichText::new("Zoom").size(12.0).color(text_pri)).selectable(false));
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    if ui.add(egui::Button::new(egui::RichText::new("+").size(12.0).color(text_pri)).small().frame(false)).clicked() {
                                                        let next = (ui_scale + 0.05).min(2.0);
                                                        *ui_scale_change = Some((next * 20.0).round() / 20.0);
                                                    }
                                                    let dv_id = egui::Id::new("zoom_dv");
                                                    let pending: Option<f32> = ui.memory(|m| m.data.get_temp(dv_id));
                                                    let mut pct = (pending.unwrap_or(ui_scale) * 100.0).round();
                                                    let resp = ui.add(
                                                        egui::DragValue::new(&mut pct)
                                                            .range(100.0_f32..=200.0_f32)
                                                            .speed(1.0)
                                                            .suffix("%")
                                                            .fixed_decimals(0),
                                                    );
                                                    if resp.dragged() {
                                                        ui.memory_mut(|m| m.data.insert_temp(dv_id, pct / 100.0));
                                                    } else if resp.drag_stopped() {
                                                        *ui_scale_change = Some((pct / 100.0).clamp(1.0, 2.0));
                                                        ui.memory_mut(|m| m.data.remove::<f32>(dv_id));
                                                    } else if resp.changed() {
                                                        *ui_scale_change = Some((pct / 100.0).clamp(1.0, 2.0));
                                                    }
                                                    if ui.add(egui::Button::new(egui::RichText::new("−").size(12.0).color(text_pri)).small().frame(false)).clicked() {
                                                        let next = (ui_scale - 0.05).max(1.0);
                                                        *ui_scale_change = Some((next * 20.0).round() / 20.0);
                                                    }
                                                });
                                            },
                                        );
                                    }

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

                                    // Row hover highlight toggle
                                    let rh_ic = if row_hover_on { colors.accent } else { text_sec };
                                    if Self::menu_row(
                                        ui, "row-hover", rh_ic,
                                        egui::RichText::new("Row Highlight").size(12.0).color(text_pri),
                                        row_hover_on, colors.accent,
                                        |ui| {
                                            let s = if row_hover_on { "On" } else { "Off" };
                                            let c = if row_hover_on { colors.accent } else { text_sec };
                                            ui.add(egui::Label::new(egui::RichText::new(s).size(11.0).color(c)).selectable(false));
                                        },
                                    ).clicked() { toggle_row_hover = true; }

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

                                    // Copy Mode submenu link
                                    if Self::menu_row(
                                        ui, "copy-mode", colors.accent,
                                        egui::RichText::new(format!("Copy: {}", copy_mode.label())).size(12.0).color(text_pri),
                                        false, colors.accent,
                                        |ui| { ui.add(crate::ui::icons::icon("chevron-right", text_sec)); },
                                    ).clicked() { new_copy_mode_sub = Some(true); }

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

                                    ui.separator();

                                    // Reset file-specific settings (only when a file is open)
                                    if has_file {
                                        if Self::menu_row(
                                            ui, "undo", text_sec,
                                            egui::RichText::new("Reset File Settings").size(12.0).color(text_sec),
                                            false, colors.accent,
                                            |_ui| {},
                                        ).clicked() { new_reset_file_settings = true; }
                                    }

                                    // Reset all settings to defaults
                                    if Self::menu_row(
                                        ui, "undo", text_sec,
                                        egui::RichText::new("Reset All Settings").size(12.0).color(text_sec),
                                        false, colors.accent,
                                        |_ui| {},
                                    ).clicked() { new_reset_all = true; }
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
                                        new_settings_open = Some(false);
                                        new_theme_sub = Some(false);
                                        new_font_sub = Some(false);
                                        new_debug_sub = Some(false);
                                        new_copy_mode_sub = Some(false);
                                    }
                                }
                            }
                        }
                    });
                });
            });

        // Apply mutations.
        if toggle_row_hover {
            tab.state.row_highlight_on_hover = !tab.state.row_highlight_on_hover;
        }
        if toggle_header {
            tab.state.header_row_enabled = !tab.state.header_row_enabled;
            tab.state.clear_cache();
            tab.state.invalidate_row_layout();
            tab.state.computed_stats = None;
            tab.state.stats_key.clear();
        }
        if let Some(open) = new_settings_open {
            tab.state.settings_menu = open;
            if !open {
                tab.state.settings_theme_submenu = false;
                tab.state.settings_font_submenu = false;
                tab.state.settings_debug_submenu = false;
                tab.state.settings_copy_mode_submenu = false;
            }
        }
        if let Some(idx) = new_theme_idx  { tab.state.set_theme_index(idx); }
        if let Some(s)   = new_pref_stat  { tab.state.preferred_stat = s; }
        if let Some(sub) = new_theme_sub  {
            tab.state.settings_theme_submenu = sub;
            if sub {
                tab.state.settings_font_submenu = false;
                tab.state.settings_debug_submenu = false;
                tab.state.settings_copy_mode_submenu = false;
            }
        }
        if let Some(sub) = new_font_sub {
            tab.state.settings_font_submenu = sub;
            if sub {
                tab.state.settings_theme_submenu = false;
                tab.state.settings_debug_submenu = false;
                tab.state.settings_copy_mode_submenu = false;
            }
        }
        if let Some(sub) = new_debug_sub {
            tab.state.settings_debug_submenu = sub;
            if sub {
                tab.state.settings_theme_submenu = false;
                tab.state.settings_font_submenu = false;
                tab.state.settings_copy_mode_submenu = false;
            }
        }
        if let Some(sub) = new_copy_mode_sub {
            tab.state.settings_copy_mode_submenu = sub;
            if sub {
                tab.state.settings_theme_submenu = false;
                tab.state.settings_font_submenu = false;
                tab.state.settings_debug_submenu = false;
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
        if let Some(s) = new_font_size {
            *font_size_change = Some(s);
        }
        if let Some(mode) = new_copy_mode {
            *copy_mode_change = Some(mode);
        }
        if new_reset_all {
            *reset_all = true;
        }
        if new_reset_file_settings {
            *reset_file_settings = true;
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
