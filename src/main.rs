mod csv_engine;
mod file_open;
mod state;
mod ui;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow;
use gpui::*;
use state::AppState;
use state::PreferredStat;
use ui::status_bar::StatusBar;
use ui::table::{self, TableView};

actions!(colomin, [OpenFile, Quit, SaveFile, CycleTheme]);

/// Resolve a bundled asset path at runtime.
/// In a .app bundle: <exe>/../Resources/<name>
/// In dev (cargo run): <workspace>/assets/<name>
pub fn asset_path(name: &str) -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let bundle = exe.parent() // MacOS/
            .and_then(|p| p.parent()) // Contents/
            .map(|p| p.join("Resources").join("assets").join(name));
        if let Some(p) = bundle {
            if p.exists() { return p; }
        }
    }
    // fallback: dev path relative to workspace
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets").join(name)
}

/// AssetSource that reads from the bundle Resources/assets directory
/// (or the workspace assets/ directory in dev mode).
struct ColominAssets;

impl AssetSource for ColominAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        // `path` is e.g. "assets/spinner.svg" as passed to svg().path(...)
        // Resolve relative to bundle Resources/ or workspace root.
        let full = if let Ok(exe) = std::env::current_exe() {
            let bundle = exe.parent()
                .and_then(|p| p.parent())
                .map(|p| p.join("Resources").join(path));
            if let Some(p) = bundle {
                if p.exists() { p } else { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path) }
            } else {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
            }
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
        };
        let bytes = std::fs::read(&full).map_err(|e| {
            anyhow::anyhow!("AssetSource: failed to read {}: {}", full.display(), e)
        })?;
        Ok(Some(std::borrow::Cow::Owned(bytes)))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

struct Colomin {
    state: Entity<AppState>,
    table_view: Entity<TableView>,
    status_bar: Entity<StatusBar>,
}

impl Colomin {
    /// Open file asynchronously — ALL I/O on background thread
    fn open_file_async(&mut self, path: String, cx: &mut Context<Self>) {
        file_open::open_file_async(self.state.clone(), path, cx);
    }

    fn on_open_file(&mut self, _: &OpenFile, _window: &mut Window, cx: &mut Context<Self>) {
        // rfd must run on the main thread on macOS
        let path = rfd::FileDialog::new()
            .add_filter("CSV Files", &["csv", "tsv", "txt"])
            .add_filter("All Files", &["*"])
            .pick_file();

        if let Some(path) = path {
            let path_str = path.to_string_lossy().into_owned();
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
        let save_result = csv_engine::writer::save_file(file, &target_path);
        let _ = state;

        match save_result {
            Ok(()) => {
                let path_str = target_path.to_string_lossy().to_string();
                self.open_file_async(path_str, cx);
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
    fn render_settings_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let state = self.state.read(cx);
        if !state.settings_menu {
            return None;
        }
        let colors = state.current_theme();
        let header_on = state.header_row_enabled;
        let theme_name = state.theme_name();
        let theme_index = state.theme_index;
        let theme_submenu = state.settings_theme_submenu;
        let themes = ui::theme::bundled_themes();
        let _ = state;

        let se = self.state.clone();

        let menu_item = |id: &str, icon_path: &str, label: String, icon_color: Hsla, se: Entity<AppState>, action: Box<dyn Fn(&mut AppState)>| {
            let se2 = se.clone();
            div()
                .id(SharedString::from(id.to_string()))
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(12.0))
                .py(px(6.0))
                .text_size(px(12.0))
                .text_color(colors.text_primary)
                .cursor_pointer()
                .rounded(px(4.0))
                .hover(|s| s.bg(colors.accent_subtle))
                .child(
                    svg()
                        .path(SharedString::from(icon_path.to_string()))
                        .size(px(15.0))
                        .flex_shrink_0()
                        .text_color(icon_color),
                )
                .child(label)
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    se2.update(cx, |s, _| {
                        action(s);
                        s.settings_menu = false;
                    });
                })
        };

        let header_icon = if header_on {
            "assets/icons/columns-on.svg"
        } else {
            "assets/icons/columns-off.svg"
        };

        // Popover anchored bottom-right, above the status bar
        let mut menu = div()
            .id("settings-menu-popover")
            .occlude()
            .absolute()
            .bottom(px(28.0))
            .right(px(8.0))
            .w(px(220.0))
            .py(px(4.0))
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border)
            .rounded(px(8.0))
            .shadow_lg()
            .text_color(colors.text_primary);

        if theme_submenu {
            let se_back = se.clone();
            menu = menu
                .child(
                    div()
                        .id("sm-theme-back")
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(12.0))
                        .py(px(6.0))
                        .text_size(px(12.0))
                        .text_color(colors.text_secondary)
                        .cursor_pointer()
                        .rounded(px(4.0))
                        .hover(|s| s.bg(colors.accent_subtle))
                        .child("← Back")
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            se_back.update(cx, |s, _| {
                                s.settings_theme_submenu = false;
                            });
                        }),
                )
                .child(div().h(px(1.0)).my(px(4.0)).bg(colors.border));

            for (idx, theme) in themes.iter().enumerate() {
                let is_active = idx == theme_index;
                let se_theme = se.clone();
                let theme_name = theme.name.clone();
                let item = div()
                    .id(SharedString::from(format!("sm-theme-{}", idx)))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .px(px(12.0))
                    .py(px(6.0))
                    .text_size(px(12.0))
                    .text_color(if is_active { colors.accent } else { colors.text_primary })
                    .cursor_pointer()
                    .rounded(px(4.0))
                    .hover(|s| s.bg(colors.accent_subtle))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                svg()
                                    .path("assets/icons/theme.svg")
                                    .size(px(15.0))
                                    .flex_shrink_0()
                                    .text_color(if is_active { colors.accent } else { colors.text_secondary }),
                            )
                            .child(theme_name),
                    )
                    .child(if is_active {
                        div()
                            .text_size(px(11.0))
                            .text_color(colors.accent)
                            .child("Active")
                    } else {
                        div()
                    })
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        se_theme.update(cx, |s, _| {
                            s.set_theme_index(idx);
                            s.settings_theme_submenu = false;
                            s.settings_menu = false;
                        });
                    });
                menu = menu.child(item);
            }
        } else {
            let se_theme_nav = se.clone();
            menu = menu
                .child(
                    div()
                        .id("sm-theme-root")
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .px(px(12.0))
                        .py(px(6.0))
                        .text_size(px(12.0))
                        .text_color(colors.text_primary)
                        .cursor_pointer()
                        .rounded(px(4.0))
                        .hover(|s| s.bg(colors.accent_subtle))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    svg()
                                        .path("assets/icons/theme.svg")
                                        .size(px(15.0))
                                        .flex_shrink_0()
                                        .text_color(colors.accent),
                                )
                                .child(format!("Theme: {}", theme_name)),
                        )
                        .child(
                            div()
                                .text_color(colors.text_tertiary)
                                .child("›"),
                        )
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            se_theme_nav.update(cx, |s, _| {
                                s.settings_theme_submenu = true;
                            });
                        }),
                )
                .child(div().h(px(1.0)).my(px(4.0)).bg(colors.border))
                .child(menu_item(
                    "sm-header-toggle",
                    header_icon,
                    "Header Row".into(),
                    if header_on { colors.accent } else { colors.text_secondary },
                    se,
                    Box::new(|s| {
                        s.header_row_enabled = !s.header_row_enabled;
                        s.clear_cache();
                        s.invalidate_row_layout();
                    }),
                ));
        }

        Some(menu)
    }
    fn render_stats_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let state = self.state.read(cx);
        if !state.stats_menu {
            return None;
        }
        let colors = state.current_theme();
        let current = state.preferred_stat;
        let _ = state;

        let se = self.state.clone();

        let mut inner = div()
            .w(px(160.0))
            .py(px(4.0))
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border)
            .rounded(px(8.0))
            .shadow_lg()
            .text_color(colors.text_primary);

        for stat in PreferredStat::ALL {
            let is_active = stat == current;
            let se2 = se.clone();
            let item = div()
                .id(SharedString::from(format!("stat-{}", stat.label())))
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(12.0))
                .py(px(6.0))
                .text_size(px(12.0))
                .text_color(if is_active { colors.accent } else { colors.text_primary })
                .cursor_pointer()
                .rounded(px(4.0))
                .hover(|s| s.bg(colors.accent_subtle))
                .child(
                    svg()
                        .path(SharedString::from(stat.icon_path()))
                        .size(px(14.0))
                        .flex_shrink_0()
                        .text_color(if is_active { colors.accent } else { colors.text_tertiary }),
                )
                .child(stat.label().to_string())
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    se2.update(cx, |s, _| {
                        s.preferred_stat = stat;
                        s.stats_menu = false;
                    });
                });
            inner = inner.child(item);
        }

        Some(inner)
    }

    fn render_context_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let state = self.state.read(cx);
        let (mx, my, _row, col) = state.context_menu?;
        let colors = state.current_theme();
        let col_name = state.file.as_ref()
            .and_then(|f| f.metadata.columns.get(col))
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let _ = state;

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
                s.pending_sort = Some((col, true));
            })))
            .child(menu_item("cm-sort-desc", format!("Sort {} \u{2193}", col_name), se.clone(), Box::new(move |s| {
                s.pending_sort = Some((col, false));
            })));

        Some(menu)
    }
}

impl Render for Colomin {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        cx.observe(&self.state, |_this, _state, cx| cx.notify()).detach();

        let state = self.state.read(cx);
        let has_file = state.file.is_some();
        let is_loading = state.is_loading;

        // Update window title with loading filename or current file + unsaved edit count.
        let title = if state.is_loading && !state.loading_message.is_empty() {
            state.loading_message.clone()
        } else if let Some(ref f) = state.file {
            let name = f.file_path.file_name()
                .and_then(|n| n.to_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "Colomin".into());
            let changes = state.total_changes();
            if changes > 0 {
                format!("{} ({})", name, changes)
            } else {
                name
            }
        } else {
            "Colomin".into()
        };
        let _ = state;

        window.set_window_title(&title);
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
            );

        // Only show status bar when a file is loaded (not during loading or empty state)
        if has_file && !is_loading {
            root = root.child(self.status_bar.clone());
        }

        // Context menu overlay
        if let Some(menu) = self.render_context_menu(cx) {
            root = root.child(
                // Full-screen click-away backdrop
                div()
                    .id("ctx-backdrop")
                    .occlude()
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

        // Settings popover (anchored bottom-right, no full-screen backdrop)
        if let Some(menu) = self.render_settings_menu(cx) {
            // Invisible backdrop to close on click-away, but doesn't block interaction feel
            root = root.child(
                div()
                    .id("settings-backdrop")
                    .occlude()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_mouse_down(MouseButton::Left, {
                        let se = se.clone();
                        move |_, _, cx| {
                            se.update(cx, |s, _| {
                                s.settings_menu = false;
                                s.settings_theme_submenu = false;
                            });
                        }
                    })
                    .on_mouse_down(MouseButton::Right, {
                        let se = se.clone();
                        move |_, _, cx| {
                            se.update(cx, |s, _| {
                                s.settings_menu = false;
                                s.settings_theme_submenu = false;
                            });
                        }
                    })
                    .child(menu),
            );
        }

        // Stats picker popover (anchored bottom-center)
        if let Some(menu) = self.render_stats_menu(cx) {
            let badge_cx = self.state.read(cx).stat_badge_center_x;
            let menu_left = (badge_cx - 80.0).max(0.0);
            root = root.child(
                div()
                    .id("stats-backdrop")
                    .occlude()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_mouse_down(MouseButton::Left, {
                        let se = se.clone();
                        move |_, _, cx| {
                            se.update(cx, |s, _| {
                                s.stats_menu = false;
                            });
                        }
                    })
                    .on_mouse_down(MouseButton::Right, {
                        let se = se.clone();
                        move |_, _, cx| {
                            se.update(cx, |s, _| {
                                s.stats_menu = false;
                            });
                        }
                    })
                    .child(
                        div()
                            .absolute()
                            .bottom(px(28.0))
                            .left(px(menu_left))
                            .child(menu),
                    ),
            );
        }

        root
    }
}

fn open_colomin_window(
    cx: &mut App,
    file_to_open: Option<String>,
) -> Result<WindowHandle<Colomin>> {
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
        move |window, cx| {
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

            cx.new(|cx| {
                let mut c = Colomin {
                    state: app_state,
                    table_view,
                    status_bar,
                };
                if let Some(ref path) = file_to_open {
                    c.open_file_async(path.clone(), cx);
                }
                c
            })
        },
    )
}

fn main() {
    let file_to_open = std::env::args().nth(1).and_then(|arg| {
        if !arg.starts_with('-') {
            let path = PathBuf::from(&arg);
            if path.exists() { Some(arg) } else { None }
        } else { None }
    });

    // Queue for files sent from Finder (via Apple Events / application:openURLs:).
    // The on_open_urls callback fires on a non-GPUI thread, so we use a Mutex queue
    // and drain it on the main loop via cx.on_reopen or a periodic check.
    let finder_queue: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let finder_queue_cb = finder_queue.clone();

    let app = gpui_platform::application()
        .with_assets(ColominAssets)
        .with_quit_mode(QuitMode::Explicit);

    app.on_open_urls(move |urls| {
        let paths: Vec<String> = urls
            .into_iter()
            .filter_map(|url| {
                url.strip_prefix("file://").map(|p| {
                    // Percent-decode (e.g. spaces encoded as %20)
                    let decoded: String = p
                        .split('%')
                        .enumerate()
                        .map(|(i, s)| {
                            if i == 0 { s.to_string() }
                            else if s.len() >= 2 {
                                let hex = &s[..2];
                                let rest = &s[2..];
                                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                                    format!("{}{}", byte as char, rest)
                                } else {
                                    format!("%{}", s)
                                }
                            } else {
                                format!("%{}", s)
                            }
                        })
                        .collect();
                    decoded
                })
            })
            .collect();
        if let Ok(mut q) = finder_queue_cb.lock() {
            q.extend(paths);
        }
    });

    app.on_reopen({
        let finder_queue = finder_queue.clone();
        move |cx| {
            let finder_queue = finder_queue.clone();
            cx.spawn(async move |cx| {
                // Give open-urls delivery a brief chance to enqueue paths first.
                cx.background_executor().timer(std::time::Duration::from_millis(180)).await;
                let has_queued_paths = finder_queue.lock().map(|q| !q.is_empty()).unwrap_or(false);
                if has_queued_paths {
                    return;
                }
                let _ = cx.update(|cx| {
                    if cx.windows().is_empty() || cx.active_window().is_none() {
                        let _ = open_colomin_window(cx, None);
                    }
                });
            }).detach();
        }
    });

    app.run(move |cx: &mut App| {
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
            KeyBinding::new("cmd-z", table::Undo, Some("TableView")),
            KeyBinding::new("cmd-shift-z", table::Redo, Some("TableView")),
            KeyBinding::new("cmd-o", table::TOpenFile, Some("TableView")),
            KeyBinding::new("cmd-s", table::TSaveFile, Some("TableView")),
            KeyBinding::new("cmd-t", table::TCycleTheme, Some("TableView")),
            KeyBinding::new("cmd-q", table::TQuit, Some("TableView")),
        ]);

        if let Some(path) = file_to_open.clone() {
            let _ = open_colomin_window(cx, Some(path));
        } else {
            // Delay initial empty-window creation briefly so Finder-open URLs
            // can be queued first. This avoids opening an extra blank window.
            cx.spawn({
                let finder_queue = finder_queue.clone();
                async move |cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(220)).await;
                    let has_queued_paths = finder_queue.lock().map(|q| !q.is_empty()).unwrap_or(false);
                    if !has_queued_paths {
                        let _ = cx.update(|cx| {
                            if cx.windows().is_empty() {
                                let _ = open_colomin_window(cx, None);
                            }
                        });
                    }
                }
            }).detach();
        }

        // Poll the finder queue and open any queued files.
        // First iteration fires immediately (no delay) to catch files already
        // queued when the app was launched from Finder.
        cx.spawn({
            let finder_queue = finder_queue.clone();
            async move |cx| {
                let mut first = true;
                loop {
                    if !first {
                        cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                    }
                    first = false;
                    let paths = {
                        let mut q = finder_queue.lock().unwrap();
                        std::mem::take(&mut *q)
                    };
                    for path in paths {
                        let _ = cx.update(|cx| {
                            let _ = open_colomin_window(cx, Some(path));
                        });
                    }
                }
            }
        }).detach();

        cx.activate(true);
    });
}
