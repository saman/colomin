mod app;
#[cfg(target_os = "macos")]
mod apple_events;
mod csv_engine;
mod file_open;
mod state;
mod ui;

fn main() {
    // ── Single-instance check ─────────────────────────────────────────────────
    // If another Colomin is running and we were launched with a file, forward
    // the path to it via the Unix socket and exit immediately.
    #[cfg(unix)]
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

    // ── Unified file-open channel ─────────────────────────────────────────────
    // Both the Apple Events handler and the Unix socket listener send paths
    // through this channel. ColominApp drains it every frame.
    let (tx, rx) = std::sync::mpsc::channel::<String>();

    // Apple Events: Finder "Open With" when the app is NOT already running.
    // The event is queued by macOS and delivered once the CFRunLoop starts
    // (i.e. inside eframe::run_native). We only stash the sender here — the
    // handler itself must be installed AFTER NSApplication is up, which
    // happens inside ColominApp::new().
    #[cfg(target_os = "macos")]
    {
        apple_events::set_sender(tx.clone());
        // Register a notification observer so our kAEOpenDocuments handlers
        // get attached to winit's NSApplicationDelegate class at
        // applicationWillFinishLaunching — before AppKit dispatches the
        // queued Apple Event.
        apple_events::install_bootstrap();
    }

    // Unix socket: handles paths forwarded by a second process (see above) and
    // any future IPC. Removes the stale socket file first, then binds.
    #[cfg(unix)]
    {
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
                                break; // receiver dropped — app is closing
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }
    }

    // ── eframe window ─────────────────────────────────────────────────────────
    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Colomin")
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([400.0, 300.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Colomin",
        native_options,
        Box::new(move |cc| Ok(Box::new(app::ColominApp::new(cc, Some(rx))))),
    )
    .expect("Failed to start Colomin");
}

#[cfg(unix)]
fn unix_socket_path() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "colomin".to_string());
    format!("/tmp/colomin-{}.sock", user)
}
