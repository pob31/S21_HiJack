use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::model::ui_mode::UiMode;

const APP_DIR: &str = "s21_hijack";
const PREFS_FILE: &str = "preferences.json";

/// Application-wide UI preferences. Lives outside any show file: tracks
/// the operator's last-used display mode and whether the diagnostic tabs
/// should be shown. Loaded once at startup and rewritten whenever the
/// user changes these settings.
///
/// `ui_mode = None` means "first run" — the app shows a welcome popup
/// asking the operator to pick a mode, then saves the choice here.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppPreferences {
    #[serde(default)]
    pub ui_mode: Option<UiMode>,
    #[serde(default)]
    pub show_diagnostics: bool,
}

impl AppPreferences {
    /// Resolve the platform-specific config path:
    /// `%APPDATA%\s21_hijack\preferences.json` on Windows,
    /// `~/.config/s21_hijack/preferences.json` on Linux,
    /// `~/Library/Application Support/s21_hijack/preferences.json` on macOS.
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join(APP_DIR).join(PREFS_FILE))
    }

    /// Load preferences from disk. Returns `Default::default()` (no
    /// preferences yet — first run) on missing file or any I/O / parse
    /// error. Failure to read is non-fatal.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
                warn!(?path, error = %e, "Failed to parse app preferences — using defaults");
                Self::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                warn!(?path, error = %e, "Failed to read app preferences — using defaults");
                Self::default()
            }
        }
    }

    /// Save preferences to disk. Atomic-replace pattern (mirrors
    /// `ShowFile::save`): write to `<path>.tmp`, then rename. Creates
    /// the parent directory on first save.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not resolve config directory",
            )
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Serialize error: {e}"),
            )
        })?;
        let mut tmp_os = path.as_os_str().to_owned();
        tmp_os.push(".tmp");
        let tmp_path = PathBuf::from(tmp_os);

        if let Err(e) = std::fs::write(&tmp_path, json.as_bytes()) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&tmp_path, &path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }
        Ok(())
    }
}
