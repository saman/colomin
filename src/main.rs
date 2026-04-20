mod app;
#[cfg(target_os = "macos")]
mod apple_events;
mod csv_engine;
mod debug_log;
mod file_open;
mod state;
mod ui;

fn main() {
    // Capture panics to a log file so post-mortem inspection works even when
    // the binary is launched via the .app bundle (no stderr).
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("{}\n{:?}\n", info, std::backtrace::Backtrace::force_capture());
        let _ = std::fs::write("/tmp/colomin_panic.log", &msg);
        eprintln!("{}", msg);
    }));

    let tab_mode = app::is_tab_mode();

    // ── Single-instance check ─────────────────────────────────────────────────
    // When tab mode is enabled: if another Colomin is running and we were
    // launched with a file, forward the path to it via the Unix socket and exit.
    // When tab mode is disabled: skip forwarding so each launch is its own process.
    #[cfg(unix)]
    if tab_mode {
        if let Some(path) = std::env::args().nth(1).filter(|a| !a.starts_with('-')) {
            use std::io::Write;
            if let Ok(mut stream) =
                std::os::unix::net::UnixStream::connect(unix_socket_path())
            {
                if stream.write_all(path.as_bytes()).is_ok() {
                    return;
                }
            }
        }
    }

    // ── Unified file-open channel ─────────────────────────────────────────────
    // Apple Events MUST be installed in both modes — without it, macOS shows
    // "Cannot open files in CSV format" when the user uses Finder "Open With"
    // on a running instance. The infinite-spawn loop in instance mode is
    // prevented by deduplication inside ColominApp (see `cli_arg_dedup`):
    // a re-delivered Apple Event matching the CLI arg of a freshly-spawned
    // child is silently dropped during a short startup window.
    let (tx, rx) = std::sync::mpsc::channel::<String>();

    #[cfg(target_os = "macos")]
    {
        apple_events::set_sender(tx.clone());
        apple_events::install_bootstrap();
    }

    // Unix socket: only used in tab mode for second-launch path forwarding.
    // In instance mode each process is standalone — no socket needed.
    #[cfg(unix)]
    if tab_mode {
        let socket_path = unix_socket_path();
        let _ = std::fs::remove_file(&socket_path);
        if let Ok(listener) = std::os::unix::net::UnixListener::bind(&socket_path) {
            let tx2 = tx.clone();
            std::thread::spawn(move || {
                use std::io::Read;
                for stream in listener.incoming() {
                    match stream {
                        Ok(mut s) => {
                            let mut buf = String::new();
                            s.read_to_string(&mut buf).ok();
                            let path = buf.trim().to_string();
                            if !path.is_empty() && tx2.send(path).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }
    }
    let _ = tx; // tx is moved into the Apple Events handler / socket thread

    let ipc_rx: Option<std::sync::mpsc::Receiver<String>> = Some(rx);

    // ── eframe window ─────────────────────────────────────────────────────────
    // On macOS, the .app bundle's CFBundleIconFile (.icns) provides the icon
    // for the Dock and window manager — including multi-resolution variants
    // and the modern macOS shading. eframe's runtime IconData would override
    // this by calling NSApp.setApplicationIconImage with a single-bitmap
    // NSImage, which loses the multi-res treatment and looks worse.
    //
    // Passing IconData::default() is the documented way to opt out: eframe's
    // AppTitleIconSetter detects the default and skips setApplicationIconImage
    // on macOS, leaving the bundle icon untouched. Other platforms still get
    // the embedded PNG.
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("Colomin")
        .with_inner_size([1200.0, 800.0])
        .with_min_inner_size([400.0, 300.0]);

    #[cfg(target_os = "macos")]
    {
        viewport = viewport.with_icon(std::sync::Arc::new(eframe::egui::IconData::default()));
    }

    #[cfg(not(target_os = "macos"))]
    {
        let icon: Option<std::sync::Arc<eframe::egui::IconData>> = {
            let bytes = include_bytes!("../assets/app_icon_256.png");
            image::load_from_memory(bytes).ok().map(|img| {
                let img = img.into_rgba8();
                let (w, h) = img.dimensions();
                std::sync::Arc::new(eframe::egui::IconData {
                    rgba: img.into_raw(),
                    width: w,
                    height: h,
                })
            })
        };
        if let Some(ic) = icon {
            viewport = viewport.with_icon(ic);
        }
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Colomin",
        native_options,
        Box::new(move |cc| Ok(Box::new(app::ColominApp::new(cc, ipc_rx)))),
    )
    .expect("Failed to start Colomin");
}

#[cfg(unix)]
fn unix_socket_path() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "colomin".to_string());
    format!("/tmp/colomin-{}.sock", user)
}
