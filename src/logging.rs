//! Application logging + crash capture.
//!
//! Initializes `tracing` to write both to stdout (unchanged console
//! behaviour) and to a rotating daily file under the per-user config
//! directory, and installs a panic hook so crashes — in the egui UI thread,
//! a tokio worker, or any spawned task — leave a durable, shareable trail for
//! bug reports.
//!
//! The on-disk layout (mirrors [`crate::persistence::preferences`]'s use of
//! `dirs::config_dir()`):
//! ```text
//! <config_dir>/s21_hijack/logs/s21_hijack.<YYYY-MM-DD>.log   (rolling, keep 14)
//! <config_dir>/s21_hijack/logs/crashes.log                   (append-only panics)
//! ```
//!
//! Size is bounded too (audit M13): release builds log the app at `info`, the
//! warnings incoming traffic can trigger go through a [`LogThrottle`], one
//! day's file stops at [`DAILY_LOG_MAX`], and at startup and each new day the
//! folder is pruned to [`LOG_DIR_BUDGET`] and `crashes.log` is rotated past
//! [`CRASH_LOG_MAX`].

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

/// App config subfolder, matching [`crate::persistence::preferences`].
const APP_DIR: &str = "s21_hijack";
/// Number of daily log files to retain.
const KEEP_DAILY_LOGS: usize = 14;
/// Default filter when `RUST_LOG` is unset. Debug builds log the app at
/// `debug`; release builds at `info`, since `debug` logs every outbound OSC
/// message and filled small disks (audit M13). `RUST_LOG` still overrides.
const DEFAULT_FILTER: &str = if cfg!(debug_assertions) {
    "info,s21_hijack=debug,sctk_adwaita=error"
} else {
    "info,sctk_adwaita=error"
};
/// Most the daily logs may take together; the oldest go first at startup.
const LOG_DIR_BUDGET: u64 = 200 * 1024 * 1024;
/// Size past which `crashes.log` is moved to `crashes.old.log` at startup.
const CRASH_LOG_MAX: u64 = 5 * 1024 * 1024;
/// Most one day's log may take; later messages that day are dropped. Pruning
/// only ran at startup and never touches the newest file, so a daemon left
/// running had no bound on it.
const DAILY_LOG_MAX: u64 = 100 * 1024 * 1024;

/// Directory that holds the rotating log files + `crashes.log`.
/// `None` only if the platform config dir cannot be resolved.
pub fn log_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(APP_DIR).join("logs"))
}

fn env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER))
}

/// Initialize logging. Returns a [`WorkerGuard`] that **must be kept alive**
/// for the program's lifetime so the non-blocking file writer is flushed on
/// exit; `None` means we fell back to stdout-only logging.
///
/// Installs the panic hook regardless of whether file logging succeeded.
pub fn init() -> Option<WorkerGuard> {
    let dir = match log_dir() {
        Some(d) => d,
        None => {
            init_stdout_only();
            install_panic_hook(None);
            tracing::warn!("Could not resolve config dir — logging to stdout only");
            return None;
        }
    };

    if let Err(e) = std::fs::create_dir_all(&dir) {
        init_stdout_only();
        install_panic_hook(None);
        tracing::warn!(error = %e, dir = %dir.display(), "Could not create log dir — stdout only");
        return None;
    }
    prune_logs(&dir, LOG_DIR_BUDGET, CRASH_LOG_MAX);

    let appender = match RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(APP_DIR)
        .filename_suffix("log")
        .max_log_files(KEEP_DAILY_LOGS)
        .build(&dir)
    {
        Ok(a) => a,
        Err(e) => {
            init_stdout_only();
            install_panic_hook(None);
            tracing::warn!(error = %e, "Could not build file appender — stdout only");
            return None;
        }
    };

    let (file_writer, guard) =
        tracing_appender::non_blocking(DailyCap::new(appender, dir.clone(), DAILY_LOG_MAX));

    tracing_subscriber::registry()
        .with(env_filter())
        .with(tracing_subscriber::fmt::layer())
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(file_writer),
        )
        .init();

    install_panic_hook(Some(dir));
    Some(guard)
}

/// The daily file appender with a cap on what one day writes (the file's size
/// at startup counts), and the folder pruned again whenever the day changes,
/// so a daemon left running for weeks stays within bounds too.
struct DailyCap<W> {
    inner: W,
    dir: PathBuf,
    day: chrono::NaiveDate,
    written: u64,
    cap: u64,
    full: bool,
}

impl<W: Write> DailyCap<W> {
    fn new(inner: W, dir: PathBuf, cap: u64) -> Self {
        let day = chrono::Utc::now().date_naive();
        // The appender names files by UTC date and appends to today's.
        let written =
            std::fs::metadata(dir.join(format!("{APP_DIR}.{day}.log"))).map_or(0, |m| m.len());
        Self {
            inner,
            dir,
            day,
            written,
            cap,
            full: false,
        }
    }

    fn write_on(&mut self, today: chrono::NaiveDate, buf: &[u8]) -> std::io::Result<usize> {
        if today != self.day {
            self.day = today;
            self.written = 0;
            self.full = false;
            prune_logs(&self.dir, LOG_DIR_BUDGET, CRASH_LOG_MAX);
        }
        if self.full {
            return Ok(buf.len());
        }
        if self.written + buf.len() as u64 > self.cap {
            self.full = true;
            let _ = self
                .inner
                .write_all(b"[log limit for today reached: later messages are dropped]\n");
            return Ok(buf.len());
        }
        let n = self.inner.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }
}

impl<W: Write> Write for DailyCap<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.write_on(chrono::Utc::now().date_naive(), buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Fallback: the original stdout-only subscriber, used when the log dir is
/// unavailable. Logging must never block app startup.
fn init_stdout_only() {
    tracing_subscriber::fmt()
        .with_env_filter(env_filter())
        .init();
}

/// Install a global panic hook that records the panic (message, location,
/// backtrace) to the tracing log and — crash-safely — appends it directly to
/// `<log_dir>/crashes.log`. Chains to the previously-installed hook so the
/// normal stderr message is preserved.
fn install_panic_hook(log_dir: Option<PathBuf>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let thread = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string();
        let backtrace = std::backtrace::Backtrace::force_capture();

        // Goes to the rotating file + stdout via the subscriber.
        tracing::error!(
            target: "panic",
            thread = %thread,
            location = %location,
            "PANIC: {payload}\n{backtrace}"
        );

        // Crash-safe direct append — guaranteed even if the async writer never
        // flushes (e.g. an abort right after the panic).
        if let Some(dir) = &log_dir {
            let ts = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f UTC");
            let record = format!(
                "\n===== PANIC {ts} =====\nthread: {thread}\nlocation: {location}\nmessage: {payload}\n{backtrace}\n"
            );
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("crashes.log"))
            {
                let _ = f.write_all(record.as_bytes());
                let _ = f.flush();
            }
        }

        // Preserve default behaviour (stderr message, abort-on-panic, etc.).
        previous(info);
    }));
}

/// Keep the log folder within bounds: move `crashes.log` aside once it passes
/// `crash_max`, and delete the oldest daily logs while they take more than
/// `budget` together. The newest daily log is always kept. Best-effort.
fn prune_logs(dir: &Path, budget: u64, crash_max: u64) {
    let crashes = dir.join("crashes.log");
    if std::fs::metadata(&crashes).is_ok_and(|m| m.len() > crash_max) {
        let _ = std::fs::rename(&crashes, dir.join("crashes.old.log"));
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("{APP_DIR}.");
    let mut logs: Vec<(PathBuf, u64)> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with(&prefix) && name.ends_with(".log")
        })
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.len())))
        .collect();
    // The date in the name sorts oldest first.
    logs.sort();
    let mut total: u64 = logs.iter().map(|(_, len)| len).sum();
    for (path, len) in logs.iter().take(logs.len().saturating_sub(1)) {
        if total <= budget {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total -= len;
        }
    }
}

/// Rate limit for a warning that incoming traffic can trigger once per packet
/// (audit M13): at most one message per interval, reporting how many were
/// held back since the last.
///
/// ```ignore
/// static DECODE: LogThrottle = LogThrottle::new(Duration::from_secs(10));
/// if let Some(held_back) = DECODE.allow() {
///     warn!(held_back, "Failed to decode OSC packet");
/// }
/// ```
///
/// [`warn_throttled!`] does the same in one line, 10 s per call site.
pub struct LogThrottle {
    every_ms: u64,
    /// Process-relative ms of the last message; 0 means none yet.
    last_ms: AtomicU64,
    held_back: AtomicU64,
}

impl LogThrottle {
    pub const fn new(every: Duration) -> Self {
        Self {
            every_ms: every.as_millis() as u64,
            last_ms: AtomicU64::new(0),
            held_back: AtomicU64::new(0),
        }
    }

    /// `Some(n)` if this message may go out, `n` being how many were held
    /// back since the last one; `None` to skip it.
    pub fn allow(&self) -> Option<u64> {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        // +1 keeps "now" clear of the "never" sentinel.
        let now = START
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_millis() as u64
            + 1;
        let last = self.last_ms.load(Ordering::Relaxed);
        if (last != 0 && now.saturating_sub(last) < self.every_ms)
            || self
                .last_ms
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            self.held_back.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        Some(self.held_back.swap(0, Ordering::Relaxed))
    }
}

/// `warn!` through a [`LogThrottle`] of 10 s per call site, with the count of
/// messages held back: for warnings a client's packets can trigger one per
/// packet (audit M13).
macro_rules! warn_throttled {
    ($($arg:tt)+) => {{
        static THROTTLE: $crate::logging::LogThrottle =
            $crate::logging::LogThrottle::new(::std::time::Duration::from_secs(10));
        if let Some(held_back) = THROTTLE.allow() {
            ::tracing::warn!(held_back, $($arg)+);
        }
    }};
}
pub(crate) use warn_throttled;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_lets_one_through_per_interval_and_counts_the_rest() {
        let t = LogThrottle::new(Duration::from_millis(50));
        assert_eq!(t.allow(), Some(0));
        assert_eq!(t.allow(), None);
        assert_eq!(t.allow(), None);
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(t.allow(), Some(2));
    }

    /// Audit M13: one day's log stops at its cap with a note, and starts
    /// again the next day.
    #[test]
    fn a_day_of_logging_is_capped() {
        let dir = std::env::temp_dir().join(format!(
            "s21_logs_cap_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let day = |d: u32| chrono::NaiveDate::from_ymd_opt(2026, 9, d).unwrap();
        let mut out = DailyCap {
            inner: Vec::new(),
            dir: dir.clone(),
            day: day(28),
            written: 0,
            cap: 20,
            full: false,
        };
        out.write_on(day(28), b"0123456789\n").unwrap();
        out.write_on(day(28), b"0123456789\n").unwrap(); // over the cap
        out.write_on(day(28), b"dropped\n").unwrap();
        out.write_on(day(29), b"next day\n").unwrap();

        let text = String::from_utf8(out.inner).unwrap();
        assert_eq!(
            text,
            "0123456789\n[log limit for today reached: later messages are dropped]\nnext day\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_keeps_the_folder_within_budget() {
        let dir = std::env::temp_dir().join(format!(
            "s21_logs_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for day in ["2026-09-25", "2026-09-26", "2026-09-27", "2026-09-28"] {
            std::fs::write(dir.join(format!("s21_hijack.{day}.log")), vec![b'x'; 100]).unwrap();
        }
        std::fs::write(dir.join("crashes.log"), vec![b'x'; 50]).unwrap();
        std::fs::write(dir.join("notes.txt"), vec![b'x'; 1000]).unwrap();

        prune_logs(&dir, 250, 40);

        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "crashes.old.log",
                "notes.txt",
                "s21_hijack.2026-09-27.log",
                "s21_hijack.2026-09-28.log",
            ]
        );

        // The newest log stays even when it alone is over budget.
        prune_logs(&dir, 10, 40);
        assert!(dir.join("s21_hijack.2026-09-28.log").exists());
        assert!(!dir.join("s21_hijack.2026-09-27.log").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
