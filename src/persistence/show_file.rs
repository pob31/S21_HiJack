use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::model::config::ConsoleConfig;
use crate::model::eq_palette::EqPalette;
use crate::model::macro_def::MacroDef;
use crate::model::snapshot::{CueList, ScopeTemplate, Snapshot};

/// Top-level show file — the persistent state of the daemon.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShowFile {
    /// File format version for future compatibility.
    pub version: u32,
    /// Console configuration from discovery.
    pub console_config: ConsoleConfig,
    /// Saved scope templates.
    #[serde(default)]
    pub scope_templates: Vec<ScopeTemplate>,
    /// All snapshots.
    #[serde(default)]
    pub snapshots: Vec<Snapshot>,
    /// The cue list.
    #[serde(default)]
    pub cue_list: CueList,
    /// All macros (Phase 4).
    #[serde(default)]
    pub macros: Vec<MacroDef>,
    /// UUIDs of macros pinned to the Live tab quick-trigger bar.
    #[serde(default)]
    pub macro_quick_trigger_ids: Vec<uuid::Uuid>,
    /// EQ palettes (Phase 5).
    #[serde(default)]
    pub eq_palettes: Vec<EqPalette>,
}

impl ShowFile {
    pub fn new(config: ConsoleConfig) -> Self {
        Self {
            version: 3,
            console_config: config,
            scope_templates: Vec::new(),
            snapshots: Vec::new(),
            cue_list: CueList::default(),
            macros: Vec::new(),
            macro_quick_trigger_ids: Vec::new(),
            eq_palettes: Vec::new(),
        }
    }

    /// Save the show file to disk as JSON.
    pub async fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("Serialize error: {e}"))
        })?;
        tokio::fs::write(path, json).await
    }

    /// Load a show file from disk.
    pub async fn load(path: &Path) -> std::io::Result<Self> {
        let json = tokio::fs::read_to_string(path).await?;
        serde_json::from_str(&json).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("Deserialize error: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn save_load_round_trip() {
        let config = ConsoleConfig::default();
        let show = ShowFile::new(config);

        let dir = std::env::temp_dir().join("s21_hijack_test");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let path = dir.join("test_show_v2.json");

        show.save(&path).await.unwrap();
        let loaded = ShowFile::load(&path).await.unwrap();

        assert_eq!(loaded.version, 3);
        assert_eq!(loaded.console_config.input_channel_count, 48);
        assert_eq!(loaded.console_config.control_group_count, 10);
        assert!(loaded.scope_templates.is_empty());
        assert!(loaded.snapshots.is_empty());
        assert!(loaded.cue_list.cues.is_empty());

        // Cleanup
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn v1_file_loads_with_defaults() {
        // Simulate a v1 file (no scope_templates, snapshots, cue_list fields)
        let v1_json = r#"{
            "version": 1,
            "console_config": {
                "console_name": "",
                "console_serial": "",
                "session_filename": null,
                "input_channel_count": 48,
                "aux_output_count": 8,
                "group_output_count": 16,
                "matrix_output_count": 8,
                "matrix_input_count": 10,
                "control_group_count": 10,
                "graphic_eq_count": 16,
                "talkback_output_count": 0,
                "mix_output_types": [],
                "mix_output_modes": [],
                "input_modes": [],
                "group_modes": []
            }
        }"#;

        let dir = std::env::temp_dir().join("s21_hijack_test");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let path = dir.join("test_v1_compat.json");
        tokio::fs::write(&path, v1_json).await.unwrap();

        let loaded = ShowFile::load(&path).await.unwrap();
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.console_config.input_channel_count, 48);
        // New fields should have defaults
        assert!(loaded.scope_templates.is_empty());
        assert!(loaded.snapshots.is_empty());
        assert!(loaded.cue_list.cues.is_empty());

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn v2_file_loads_with_macro_defaults() {
        // A v2 file has no macros or macro_quick_trigger_ids fields
        let v2_json = r#"{
            "version": 2,
            "console_config": {
                "console_name": "",
                "console_serial": "",
                "session_filename": null,
                "input_channel_count": 48,
                "aux_output_count": 8,
                "group_output_count": 16,
                "matrix_output_count": 8,
                "matrix_input_count": 10,
                "control_group_count": 10,
                "graphic_eq_count": 16,
                "talkback_output_count": 0,
                "mix_output_types": [],
                "mix_output_modes": [],
                "input_modes": [],
                "group_modes": []
            },
            "scope_templates": [],
            "snapshots": [],
            "cue_list": { "id": "00000000-0000-0000-0000-000000000000", "name": "Main", "cues": [] }
        }"#;

        let dir = std::env::temp_dir().join("s21_hijack_test");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let path = dir.join("test_v2_macro_compat.json");
        tokio::fs::write(&path, v2_json).await.unwrap();

        let loaded = ShowFile::load(&path).await.unwrap();
        assert_eq!(loaded.version, 2);
        assert!(loaded.macros.is_empty());
        assert!(loaded.macro_quick_trigger_ids.is_empty());
        assert!(loaded.eq_palettes.is_empty());

        let _ = tokio::fs::remove_file(&path).await;
    }
}
