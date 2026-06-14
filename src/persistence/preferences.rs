use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::model::ui_mode::{ColorTheme, UiMode};

const APP_DIR: &str = "s21_hijack";
const PREFS_FILE: &str = "preferences.json";

/// MIDI output settings. **Machine-bound** (a port name / virtual-port choice
/// is a property of this computer, not of any show), so it lives in app
/// preferences rather than the show file. The MIDI engine auto-connects to the
/// configured port on launch.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiSettings {
    /// Master enable for MIDI output.
    #[serde(default)]
    pub enabled: bool,
    /// Existing output port to connect to (when `use_virtual_port` is false).
    #[serde(default)]
    pub output_port_name: Option<String>,
    /// When true (macOS / Linux only), create a virtual `S21_HiJack` output
    /// port instead of connecting to an existing one.
    #[serde(default)]
    pub use_virtual_port: bool,
}

/// Application-wide UI preferences. Lives outside any show file: tracks
/// the operator's last-used display mode and whether the diagnostic tabs
/// should be shown. Loaded once at startup and rewritten whenever the
/// user changes these settings.
///
/// `ui_mode = None` means "first run" — the app shows a welcome popup
/// asking the operator to pick a mode, then saves the choice here.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppPreferences {
    #[serde(default)]
    pub ui_mode: Option<UiMode>,
    #[serde(default)]
    pub show_diagnostics: bool,
    /// Inter-message pacing (μs) shared between snapshot recall and
    /// macro OSC sends. 0 = no pacing. Migrated from the per-show
    /// `ConnectionSettings::send_pace_us` field on first load if
    /// the prefs file doesn't carry a value yet.
    #[serde(default)]
    pub send_pace_us: u64,
    /// User-facing global UI scale multiplier, folded on top of the automatic
    /// physical-size (PPI) scaling. 1.0 = pure auto-fit. Persisted so a chosen
    /// scale (e.g. enlarging the UI for a large-TV demo viewed from a distance)
    /// survives restarts. The scaler clamps the final result to a sane range.
    #[serde(default = "default_ui_scale")]
    pub ui_scale: f32,
    /// Operator's chosen colour theme (Advanced Settings → Appearance).
    /// Defaults to `Dark` for preference files saved before this field
    /// existed, preserving the original look.
    #[serde(default)]
    pub color_theme: ColorTheme,
    /// Operator's chosen help-bubble language code (Advanced Settings →
    /// Appearance). Empty or `"en"` = the English reference; any other value
    /// names a `locales/<code>.json` translation file. Defaults to English for
    /// preference files saved before this field existed.
    #[serde(default)]
    pub help_language: String,
    /// Last window inner (content) size in logical points, restored on the
    /// next launch. `None` on first run → the app picks an on-screen fit.
    #[serde(default)]
    pub window_size: Option<[f32; 2]>,
    /// Last window outer (decorated) top-left position in logical points,
    /// restored on the next launch. `None` on first run.
    #[serde(default)]
    pub window_pos: Option<[f32; 2]>,
    /// Last directory a show file was opened from or saved to. Used to seed the
    /// starting folder of the Open / Save dialogs so the operator resumes where
    /// they last were instead of the OS default each session. `None` until the
    /// first file is picked. Cross-platform: serde (de)serializes `PathBuf` as
    /// the native path string, so a folder saved on one OS is simply ignored
    /// (falls back to the default) if the file is later opened on another.
    #[serde(default)]
    pub last_open_dir: Option<PathBuf>,
    /// MIDI output settings (machine-bound; see [`MidiSettings`]). New in
    /// v0.1.2; older preference files load with MIDI disabled.
    #[serde(default)]
    pub midi: MidiSettings,
}

/// Default UI scale multiplier — `1.0` means "use the automatic scaling
/// as-is". A free function (not `Default::default`'s `0.0` for `f32`) so both
/// the serde fallback and the struct `Default` impl agree on a non-zero value;
/// a stray `0.0` would collapse the entire UI.
fn default_ui_scale() -> f32 {
    1.0
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            ui_mode: None,
            show_diagnostics: false,
            send_pace_us: 0,
            ui_scale: default_ui_scale(),
            color_theme: ColorTheme::default(),
            help_language: String::new(),
            window_size: None,
            window_pos: None,
            last_open_dir: None,
            midi: MidiSettings::default(),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_prefs_without_send_pace_us_loads_as_zero() {
        // Before the Advanced Settings work, preferences.json had no
        // send_pace_us field. Verify that file still loads and pacing
        // defaults to 0 (i.e. "no pacing", same as before).
        let json = r#"{"ui_mode":null,"show_diagnostics":true}"#;
        let prefs: AppPreferences = serde_json::from_str(json).unwrap();
        assert_eq!(prefs.send_pace_us, 0);
        assert!(prefs.show_diagnostics);
        // Missing ui_scale must default to 1.0, never 0.0 (which would collapse
        // the UI). Guards the serde + Default footgun.
        assert_eq!(prefs.ui_scale, 1.0);
        // Missing color_theme defaults to Dark — older files keep the old look.
        assert_eq!(prefs.color_theme, ColorTheme::Dark);
        // Missing help_language defaults to empty = the English reference.
        assert!(prefs.help_language.is_empty());
        // Missing window geometry defaults to None — first-run on-screen fit.
        assert!(prefs.window_size.is_none());
        assert!(prefs.window_pos.is_none());
        // Missing last-open dir defaults to None — dialogs use the OS default.
        assert!(prefs.last_open_dir.is_none());
    }

    #[test]
    fn empty_prefs_loads_as_default() {
        let prefs: AppPreferences = serde_json::from_str("{}").unwrap();
        assert!(prefs.ui_mode.is_none());
        assert!(!prefs.show_diagnostics);
        assert_eq!(prefs.send_pace_us, 0);
        assert_eq!(prefs.ui_scale, 1.0);
        assert_eq!(prefs.color_theme, ColorTheme::Dark);
        assert!(prefs.help_language.is_empty());
        assert!(prefs.window_size.is_none());
        assert!(prefs.window_pos.is_none());
        assert!(prefs.last_open_dir.is_none());
    }

    #[test]
    fn prefs_round_trip_through_json() {
        let prefs = AppPreferences {
            ui_mode: None,
            show_diagnostics: true,
            send_pace_us: 1500,
            ui_scale: 1.25,
            color_theme: ColorTheme::Light,
            help_language: "fr".to_string(),
            window_size: Some([1600.0, 900.0]),
            window_pos: Some([40.0, 30.0]),
            last_open_dir: Some(PathBuf::from("/shows/tour-2026")),
            midi: MidiSettings {
                enabled: true,
                output_port_name: Some("IAC Driver Bus 1".into()),
                use_virtual_port: false,
            },
        };
        let json = serde_json::to_string(&prefs).unwrap();
        let back: AppPreferences = serde_json::from_str(&json).unwrap();
        assert_eq!(prefs.midi, back.midi);
        assert_eq!(prefs.send_pace_us, back.send_pace_us);
        assert_eq!(prefs.show_diagnostics, back.show_diagnostics);
        assert_eq!(prefs.ui_scale, back.ui_scale);
        assert_eq!(prefs.color_theme, back.color_theme);
        assert_eq!(prefs.help_language, back.help_language);
        assert_eq!(prefs.window_size, back.window_size);
        assert_eq!(prefs.window_pos, back.window_pos);
        assert_eq!(prefs.last_open_dir, back.last_open_dir);
    }
}
