//! Optional debug logger for developers.
//!
//! Designed to have **zero overhead when disabled**: the macros expand to a
//! single relaxed atomic load and an early return. No allocation, no
//! formatting, no I/O happens unless logging is on.
//!
//! When enabled, lines are appended to a session log file under
//! `~/Library/Logs/Colomin/` (macOS) or `$XDG_STATE_HOME` / temp dir
//! elsewhere. Output is buffered (`BufWriter`) and flushed periodically.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);
static SINK: Mutex<Option<Sink>> = Mutex::new(None);

struct Sink {
    path: PathBuf,
    writer: BufWriter<File>,
}

#[inline(always)]
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Returns the active log file path, if logging is currently on.
pub fn current_log_path() -> Option<PathBuf> {
    SINK.lock().ok().and_then(|g| g.as_ref().map(|s| s.path.clone()))
}

/// Default directory for log files.
pub fn log_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join("Library/Logs/Colomin")
    } else {
        std::env::var("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("colomin")
    }
}

pub fn enable() {
    if is_enabled() {
        return;
    }
    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);
    let stamp = current_local_timestamp_compact();
    let path = dir.join(format!("debug-{}.log", stamp));
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let mut sink = Sink {
        path,
        writer: BufWriter::with_capacity(8 * 1024, file),
    };
    let _ = writeln!(
        sink.writer,
        "── Colomin debug log opened at {} ──",
        current_local_iso_seconds()
    );
    let _ = sink.writer.flush();
    if let Ok(mut guard) = SINK.lock() {
        *guard = Some(sink);
    }
    ENABLED.store(true, Ordering::Release);
}

pub fn disable() {
    ENABLED.store(false, Ordering::Release);
    if let Ok(mut guard) = SINK.lock() {
        if let Some(mut sink) = guard.take() {
            let _ = writeln!(
                sink.writer,
                "── Colomin debug log closed at {} ──",
                current_local_iso_seconds()
            );
            let _ = sink.writer.flush();
        }
    }
}

/// Internal: do not call directly. Use the `dlog!` macro.
pub fn write_line(level: Level, category: &str, message: &str, duration_micros: Option<u64>) {
    let ts = current_local_clock();
    if let Ok(mut guard) = SINK.lock() {
        let Some(sink) = guard.as_mut() else { return };
        let res = if let Some(us) = duration_micros {
            writeln!(
                sink.writer,
                "{} [{:>5}] [{:<10}] {} ({:.2}ms)",
                ts,
                level.as_str(),
                category,
                message,
                us as f64 / 1000.0,
            )
        } else {
            writeln!(
                sink.writer,
                "{} [{:>5}] [{:<10}] {}",
                ts,
                level.as_str(),
                category,
                message,
            )
        };
        if res.is_ok() {
            // Flush every line so `tail -f` shows events live and a session
            // crash never loses the trail. Cost is one syscall per event,
            // negligible at human-interaction rates.
            let _ = sink.writer.flush();
        }
    }
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Trace => "TRACE",
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
}

/// RAII timer. Logs the elapsed duration when dropped.
/// Created via `dspan!(category, "name")`. No-op when logging is disabled
/// (carries `Instant::now()` cost only at construction).
pub struct Span {
    start: Instant,
    category: &'static str,
    name: &'static str,
}

impl Span {
    pub fn new(category: &'static str, name: &'static str) -> Self {
        Span { start: Instant::now(), category, name }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if !is_enabled() {
            return;
        }
        let us = self.start.elapsed().as_micros() as u64;
        write_line(Level::Debug, self.category, self.name, Some(us));
    }
}

// ── Time helpers (no chrono dep — keep the binary lean) ───────────────────────

fn current_local_clock() -> String {
    // HH:MM:SS.mmm in local time.
    let now = std::time::SystemTime::now();
    let dur = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs();
    let millis = dur.subsec_millis();
    let offset = local_offset_seconds();
    let local_secs = (secs as i64 + offset) as u64;
    let h = (local_secs / 3600) % 24;
    let m = (local_secs / 60) % 60;
    let s = local_secs % 60;
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, millis)
}

fn current_local_iso_seconds() -> String {
    let now = std::time::SystemTime::now();
    let dur = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs() as i64 + local_offset_seconds();
    let (y, mo, d, h, mi, se) = ymd_hms(secs);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", y, mo, d, h, mi, se)
}

fn current_local_timestamp_compact() -> String {
    let now = std::time::SystemTime::now();
    let dur = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs() as i64 + local_offset_seconds();
    let (y, mo, d, h, mi, se) = ymd_hms(secs);
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", y, mo, d, h, mi, se)
}

fn local_offset_seconds() -> i64 {
    // Best-effort local offset: ask libc on unix, default to 0 elsewhere.
    #[cfg(unix)]
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let now: libc::time_t = libc::time(std::ptr::null_mut());
        if libc::localtime_r(&now, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff as i64
    }
    #[cfg(not(unix))]
    {
        0
    }
}

fn ymd_hms(epoch: i64) -> (i32, u32, u32, u32, u32, u32) {
    // Convert seconds since epoch (already adjusted for local offset)
    // to civil date. Algorithm: Howard Hinnant's date conversion.
    let z = epoch.div_euclid(86_400) + 719_468;
    let secs_of_day = epoch.rem_euclid(86_400) as u32;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i32 + (era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let h = secs_of_day / 3600;
    let mi = (secs_of_day / 60) % 60;
    let se = secs_of_day % 60;
    (y, m, d, h, mi, se)
}

// ── Macros ────────────────────────────────────────────────────────────────────

/// Log a single event. Zero cost when logging is disabled.
///
/// Usage:
/// ```ignore
/// dlog!(Info, "FileIO", "opened {} ({} bytes)", path, size);
/// ```
#[macro_export]
macro_rules! dlog {
    ($level:ident, $category:expr, $($arg:tt)+) => {{
        if $crate::debug_log::is_enabled() {
            let msg = format!($($arg)+);
            $crate::debug_log::write_line(
                $crate::debug_log::Level::$level,
                $category,
                &msg,
                None,
            );
        }
    }};
}

/// Begin a timed scope. The returned `Span` logs its duration on drop.
///
/// Usage:
/// ```ignore
/// let _t = dspan!("Sort", "apply_sort");
/// ```
#[macro_export]
macro_rules! dspan {
    ($category:expr, $name:expr) => {
        $crate::debug_log::Span::new($category, $name)
    };
}
