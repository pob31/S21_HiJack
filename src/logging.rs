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

use std::io::Write;
use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

/// App config subfolder, matching [`crate::persistence::preferences`].
const APP_DIR: &str = "s21_hijack";
/// Number of daily log files to retain.
const KEEP_DAILY_LOGS: usize = 14;
/// Default filter when `RUST_LOG` is unset (unchanged from the original init).
const DEFAULT_FILTER: &str = "info,s21_hijack=debug,sctk_adwaita=error";

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

    let (file_writer, guard) = tracing_appender::non_blocking(appender);

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
