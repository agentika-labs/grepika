//! Compile-time gated profiling for MCP tool calls.
//!
//! When a `--log-file` path is provided at startup, [`init`] activates profiling
//! and all subsequent [`log`] / [`log_tool_call`] calls write timestamped entries
//! to that file. When no path is given, the [`is_active`] flag stays false and
//! every call short-circuits with negligible overhead.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static PROFILING_ACTIVE: AtomicBool = AtomicBool::new(false);
static LOG_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();

pub fn is_active() -> bool {
    PROFILING_ACTIVE.load(Ordering::Relaxed)
}

/// Returns an ISO 8601 UTC timestamp string, e.g. "2026-02-07T15:04:05Z".
fn timestamp() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    // Break epoch seconds into date/time components
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;
    // Civil date from days since 1970-01-01 (algorithm from Howard Hinnant)
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Initialize profiling with optional log file path.
/// When path is Some, profiling is activated and logs are appended to the file.
/// When path is None, profiling remains inactive (no overhead).
pub fn init(path: Option<&Path>) {
    LOG_FILE.get_or_init(|| {
        path.and_then(|p| {
            // Create parent directories if needed
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            match OpenOptions::new().create(true).append(true).open(p) {
                Ok(f) => Some(Mutex::new(f)),
                Err(e) => {
                    eprintln!("warning: cannot open profiling log {}: {}", p.display(), e);
                    None
                }
            }
        })
    });
    // Only activate profiling if the log file was successfully opened.
    // Log the first entry *before* setting the flag so no other thread
    // can observe is_active()==true before the log file is ready.
    if path.is_some() {
        if let Some(Some(_)) = LOG_FILE.get() {
            log("profiling started");
            PROFILING_ACTIVE.store(true, Ordering::Relaxed);
        }
    }
}

/// Log a profiling message to the log file.
pub fn log(msg: &str) {
    let ts = timestamp();
    if let Some(Some(file)) = LOG_FILE.get() {
        if let Ok(mut f) = file.lock() {
            let _ = writeln!(f, "{ts} {msg}");
            let _ = f.flush();
        }
    }
}

/// Gets current memory usage in MB.
pub fn get_memory_mb() -> f64 {
    #[cfg(unix)]
    {
        use std::process::Command;
        Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(|kb| kb as f64 / 1024.0)
            .unwrap_or(0.0)
    }
    #[cfg(not(unix))]
    {
        0.0
    }
}

/// Metrics captured around a single tool invocation.
pub struct ToolMetrics {
    pub name: String,
    pub elapsed: std::time::Duration,
    pub response_bytes: usize,
    pub mem_before_mb: f64,
    pub is_error: bool,
}

/// Formats and logs a tool call's metrics. No-op when profiling is inactive.
pub fn log_tool_call(m: &ToolMetrics) {
    if !is_active() {
        return;
    }
    let mem_after = get_memory_mb();
    let mem_delta = mem_after - m.mem_before_mb;
    let tokens = (m.response_bytes + 2) / 4;
    let status = if m.is_error { " | ERROR" } else { "" };
    log(&format!(
        "[{}] {:?} | mem: {:.1}MB ({:+.1}MB) | ~{} tokens ({:.1}KB){status}",
        m.name,
        m.elapsed,
        mem_after,
        mem_delta,
        tokens,
        m.response_bytes as f64 / 1024.0
    ));
}
