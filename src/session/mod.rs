//! Console session files: DiGiCo S21/S31 `.session` (SQLite) and SD-range /
//! Quantum `.ses` (binary), read into one console-agnostic [`SessionDoc`].
//!
//! Read-only. The file formats and the evidence behind every decoded field
//! are documented in `Documentation/SESSION_FILES_FIELD_NOTES.md`.

pub mod cp1252;
pub mod doc;
pub mod report;
pub mod s21;
pub mod ses;
#[cfg(test)]
mod tests;

use std::fmt;
use std::path::Path;

pub use doc::SessionDoc;

#[derive(Debug)]
pub enum SessionError {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
    /// The file is not a session file, or its layout is not understood.
    Format(String),
}

impl SessionError {
    pub fn format(msg: impl Into<String>) -> Self {
        Self::Format(msg.into())
    }
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "could not read the session file: {e}"),
            Self::Sqlite(e) => write!(f, "could not read the S21 session database: {e}"),
            Self::Format(m) => write!(f, "unrecognised session file: {m}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<std::io::Error> for SessionError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<rusqlite::Error> for SessionError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

/// Options for [`load_with`].
#[derive(Debug, Clone, Copy, Default)]
pub struct LoadOptions {
    /// Also keep undecoded SD/Quantum fields as raw `unconfirmed` params.
    pub include_raw: bool,
}

/// `<session stem>_report.html` next to the session file.
pub fn default_report_path(session: &Path) -> std::path::PathBuf {
    let stem = session
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "session".to_string());
    session.with_file_name(format!("{stem}_report.html"))
}

/// `file:///…` URL for a local path, for opening a report in the browser.
pub fn file_url(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    let mut url = String::from("file://");
    if !p.starts_with('/') {
        url.push('/');
    }
    for b in p.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'-' | b'_' | b'.' | b'~' => {
                url.push(b as char)
            }
            _ => url.push_str(&format!("%{b:02X}")),
        }
    }
    url
}

/// Reads `session` and writes its HTML report to `out`.
pub fn write_report(session: &Path, out: &Path) -> Result<(), SessionError> {
    let doc = load(session)?;
    let generated = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    std::fs::write(out, report::render(&doc, &generated))?;
    Ok(())
}

/// Loads a session file, detecting its format from its first bytes.
pub fn load(path: &Path) -> Result<SessionDoc, SessionError> {
    load_with(path, LoadOptions::default())
}

pub fn load_with(path: &Path, opts: LoadOptions) -> Result<SessionDoc, SessionError> {
    let data = std::fs::read(path)?;
    if data.starts_with(b"SQLite format 3\0") {
        return s21::read(path);
    }
    if data.starts_with(b"DiGiCo ") {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        return ses::parse(&data, &name, opts.include_raw);
    }
    Err(SessionError::format(
        "expected a DiGiCo S21/S31 .session or SD/Quantum .ses file",
    ))
}
