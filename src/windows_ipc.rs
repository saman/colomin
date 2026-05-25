//! Windows named-pipe single-instance IPC.
//!
//! Mirrors the Unix domain socket flow in `main.rs`. When tab-mode is enabled,
//! a second launch of Colomin connects to the named pipe of the already-running
//! instance, sends the CLI path, and exits — so file-association double-clicks
//! reuse the existing window instead of spawning a new process.
//!
//! Uses the `interprocess` crate's local-socket abstraction, which maps to
//! Windows named pipes under the `GenericNamespaced` namespace.

use std::io::{Read, Write};
use std::sync::mpsc::Sender;
use std::thread;

use interprocess::local_socket::{
    prelude::*, GenericNamespaced, ListenerOptions, Stream,
};

/// Per-user pipe name. `interprocess` prepends `\\.\pipe\` on Windows.
fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "colomin".to_string());
    format!("colomin-{}.sock", user)
}

/// Try to forward `path` to an already-running instance. Returns true on
/// success (i.e. another Colomin was listening and accepted the bytes).
pub fn forward_to_existing_instance(path: &str) -> bool {
    let Ok(name) = pipe_name().to_ns_name::<GenericNamespaced>() else { return false };
    let Ok(mut stream) = Stream::connect(name) else { return false };
    stream.write_all(path.as_bytes()).is_ok()
}

/// Install the IPC listener that forwards received paths into `tx`.
/// Spawns a background thread; returns immediately. Silently no-ops if the
/// pipe is already claimed (another Colomin owns this user's pipe).
pub fn install(tx: Sender<String>) {
    let Ok(name) = pipe_name().to_ns_name::<GenericNamespaced>() else { return };
    let Ok(listener) = ListenerOptions::new().name(name).create_sync() else { return };
    thread::spawn(move || {
        for conn in listener.incoming().filter_map(|c| c.ok()) {
            let mut buf = String::new();
            let mut s = conn;
            if s.read_to_string(&mut buf).is_ok() {
                let path = buf.trim().to_string();
                if !path.is_empty() && tx.send(path).is_err() {
                    break;
                }
            }
        }
    });
}
