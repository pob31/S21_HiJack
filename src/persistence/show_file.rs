use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::model::config::ConsoleConfig;
use crate::model::gang::GangGroup;
use crate::model::macro_def::MacroDef;
use crate::model::monitor::MonitorClient;
use crate::model::operating_mode::OperatingMode;
use crate::model::palette::ChannelPalette;
use crate::model::pan_link::PanLinkBindings;
use crate::model::recall_scope::ConsoleRecallConfig;
use crate::model::snapshot::{CueList, ScopeTemplate, Snapshot};
use crate::model::ui_mode::UiMode;

/// Connection settings persisted in the show file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionSettings {
    #[serde(default)]
    pub local_ip: String,
    #[serde(default)]
    pub console_ip: String,
    #[serde(default = "default_gp_port")]
    pub console_gp_port: u16,
    #[serde(default = "default_local_port")]
    pub local_gp_port: u16,
    #[serde(default = "default_trigger_port")]
    pub trigger_port: u16,
    #[serde(default)]
    pub operating_mode: OperatingMode,
    #[serde(default)]
    pub ipad_ip: String,
    #[serde(default)]
    pub ipad_send_port: u16,
    #[serde(default)]
    pub ipad_receive_port: u16,
    #[serde(default)]
    pub ipad_listen_port: u16,
    #[serde(default)]
    pub ipad_reply_port: u16,
    #[serde(default = "default_monitor_port")]
    pub monitor_port: u16,
    /// QLab destination IP (for outbound OSC — e.g. building network cues in QLab).
    /// Empty string falls back to localhost in the UI.
    #[serde(default)]
    pub qlab_ip: String,
    /// QLab destination port. Default 53000 (QLab's standard OSC listen port).
    #[serde(default = "default_qlab_port")]
    pub qlab_port: u16,
    /// Inter-message pacing delay in microseconds during snapshot recall.
    /// Prevents flooding the console's ARM chip. 0 = no pacing.
    #[serde(default)]
    pub send_pace_us: u64,
    /// Auto-save dirty parameters into the previously-recalled snapshot
    /// when firing a new one. Filtered by the previous snapshot's scope
    /// template. Per-show workflow toggle.
    #[serde(default)]
    pub auto_update_on_recall: bool,
    /// Follow the console's snapshot list — when the desk fires a memory,
    /// auto-recall the first matching app snapshot in cue-list order.
    /// iPad-protocol-only (Modes 2/3). Per-show workflow toggle.
    #[serde(default)]
    pub console_snapshot_follow: bool,
    /// Source-IP CIDR allowlist for the **monitor** server (audit C2). Empty
    /// = accept all (current behavior). Each entry is a string in CIDR
    /// form, e.g. `"192.168.10.0/24"` or `"10.0.0.5"` (bare IP = `/32`).
    /// Invalid entries are logged and skipped at startup.
    #[serde(default)]
    pub monitor_allow_cidrs: Vec<String>,
    /// Source-IP CIDR allowlist for the **trigger** listener (audit H5).
    /// Same semantics as `monitor_allow_cidrs`.
    #[serde(default)]
    pub trigger_allow_cidrs: Vec<String>,
    /// UI display mode — determines which tabs are visible. Saved per-show
    /// so different shows can prefer different streamlined views. Older
    /// show files (pre-v14) get `UiMode::Full` (all tabs visible).
    #[serde(default)]
    pub ui_mode: UiMode,
}

/// Parse a list of CIDR strings into `Ipv4Cidr`s, logging and discarding
/// any invalid entries. Used at startup to materialize the allowlist from
/// `ConnectionSettings` strings into the typed form the listeners want.
pub fn parse_cidr_allowlist(raw: &[String]) -> Vec<crate::model::cidr::Ipv4Cidr> {
    use std::str::FromStr;
    raw.iter()
        .filter_map(|s| match crate::model::cidr::Ipv4Cidr::from_str(s) {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!(entry = %s, "Skipping invalid CIDR in allowlist: {e}");
                None
            }
        })
        .collect()
}

fn default_gp_port() -> u16 {
    8024
}
fn default_local_port() -> u16 {
    8023
}
fn default_trigger_port() -> u16 {
    53001
}
fn default_qlab_port() -> u16 {
    53000
}
fn default_monitor_port() -> u16 {
    8025
}

impl Default for ConnectionSettings {
    fn default() -> Self {
        Self {
            local_ip: String::new(),
            console_ip: String::new(),
            console_gp_port: default_gp_port(),
            local_gp_port: default_local_port(),
            trigger_port: default_trigger_port(),
            operating_mode: OperatingMode::default(),
            ipad_ip: String::new(),
            ipad_send_port: 0,
            ipad_receive_port: 0,
            ipad_listen_port: 0,
            ipad_reply_port: 0,
            monitor_port: 8025,
            qlab_ip: String::new(),
            qlab_port: default_qlab_port(),
            send_pace_us: 0,
            auto_update_on_recall: false,
            console_snapshot_follow: false,
            monitor_allow_cidrs: Vec::new(),
            trigger_allow_cidrs: Vec::new(),
            ui_mode: UiMode::default(),
        }
    }
}

/// Top-level show file — the persistent state of the daemon.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShowFile {
    /// File format version for future compatibility.
    pub version: u32,
    /// Console configuration from discovery.
    pub console_config: ConsoleConfig,
    /// Connection settings (IP, ports, mode).
    #[serde(default)]
    pub connection: ConnectionSettings,
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
    /// Channel palettes (EQ / Compressor / Gate). New in v9; the
    /// `eq_palettes` alias lets v8 show files load — every legacy palette
    /// deserializes as a `ChannelPalette { kind: PaletteKind::Eq, .. }`.
    #[serde(default, alias = "eq_palettes")]
    pub palettes: Vec<ChannelPalette>,
    /// Monitor client profiles (Phase 7).
    #[serde(default)]
    pub monitor_clients: Vec<MonitorClient>,
    /// Gang groups for smart ganging.
    #[serde(default)]
    pub gang_groups: Vec<GangGroup>,
    /// Console recall scope & per-channel recall safe (visual reference).
    #[serde(default)]
    pub console_recall: ConsoleRecallConfig,
    /// Pan link bindings: aux-send pans that follow an input's main pan.
    #[serde(default)]
    pub pan_link: PanLinkBindings,
}

impl ShowFile {
    pub fn new(config: ConsoleConfig) -> Self {
        Self {
            version: 14,
            console_config: config,
            connection: ConnectionSettings::default(),
            scope_templates: Vec::new(),
            snapshots: Vec::new(),
            cue_list: CueList::default(),
            macros: Vec::new(),
            macro_quick_trigger_ids: Vec::new(),
            palettes: Vec::new(),
            monitor_clients: Vec::new(),
            gang_groups: Vec::new(),
            console_recall: ConsoleRecallConfig::default(),
            pan_link: PanLinkBindings::default(),
        }
    }

    /// Save the show file to disk as JSON.
    ///
    /// Atomic-replace: writes to `<path>.tmp`, fsyncs, then renames over the
    /// destination. A crash mid-save leaves the previous file intact; the
    /// orphan `.tmp` is best-effort cleaned up on error.
    pub async fn save(&self, path: &Path) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;

        let json = serde_json::to_string_pretty(self).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Serialize error: {e}"),
            )
        })?;

        let mut tmp_os = path.as_os_str().to_owned();
        tmp_os.push(".tmp");
        let tmp_path = std::path::PathBuf::from(tmp_os);

        let write_result: std::io::Result<()> = async {
            let mut file = tokio::fs::File::create(&tmp_path).await?;
            file.write_all(json.as_bytes()).await?;
            file.sync_all().await?;
            Ok(())
        }
        .await;

        if let Err(e) = write_result {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(e);
        }

        if let Err(e) = tokio::fs::rename(&tmp_path, path).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(e);
        }
        Ok(())
    }

    /// Load a show file from disk.
    pub async fn load(path: &Path) -> std::io::Result<Self> {
        let json = tokio::fs::read_to_string(path).await?;
        serde_json::from_str(&json).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Deserialize error: {e}"),
            )
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

        assert_eq!(loaded.version, 14);
        assert_eq!(loaded.console_config.input_channel_count, 48);
        assert_eq!(loaded.console_config.control_group_count, 10);
        assert!(loaded.scope_templates.is_empty());
        assert!(loaded.snapshots.is_empty());
        assert!(loaded.cue_list.cues.is_empty());

        // Cleanup
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn save_is_atomic_and_leaves_no_tmp_file() {
        let dir = std::env::temp_dir().join("s21_hijack_test");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let path = dir.join("test_atomic_save.json");
        let tmp_path = {
            let mut s = path.as_os_str().to_owned();
            s.push(".tmp");
            std::path::PathBuf::from(s)
        };

        // Clean slate
        let _ = tokio::fs::remove_file(&path).await;
        let _ = tokio::fs::remove_file(&tmp_path).await;

        // First save: with default config (48 inputs)
        let mut show1 = ShowFile::new(ConsoleConfig::default());
        show1.connection.console_ip = "10.0.0.1".to_string();
        show1.save(&path).await.unwrap();
        assert!(path.exists(), "destination should exist after save");
        assert!(
            !tmp_path.exists(),
            "no .tmp file should remain after successful save"
        );

        // Second save: replace with different content
        let mut show2 = ShowFile::new(ConsoleConfig::default());
        show2.connection.console_ip = "192.168.1.42".to_string();
        show2.save(&path).await.unwrap();
        assert!(
            !tmp_path.exists(),
            "no .tmp file should remain after replace"
        );

        // Verify the file actually contains the second version's content
        let loaded = ShowFile::load(&path).await.unwrap();
        assert_eq!(loaded.connection.console_ip, "192.168.1.42");

        // Cleanup
        let _ = tokio::fs::remove_file(&path).await;
        let _ = tokio::fs::remove_file(&tmp_path).await;
    }

    #[tokio::test]
    async fn v7_file_loads_with_legacy_section_scopes() {
        // A v7 ChannelScope has only the `sections` field, no `paths`. The
        // new `paths` field must default to empty AND legacy scopes must
        // still be honoured by ScopeTemplate::contains via the additive
        // read path.
        let v7_json = r#"{
            "version": 7,
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
            "scope_templates": [
                {
                    "id": "00000000-0000-0000-0000-000000000001",
                    "name": "Legacy Eq Scope",
                    "channel_scopes": [
                        {
                            "channel": {"Input": 1},
                            "sections": ["Eq"]
                        }
                    ]
                }
            ],
            "snapshots": [],
            "cue_list": { "id": "00000000-0000-0000-0000-000000000000", "name": "Main", "cues": [] },
            "macros": [],
            "macro_quick_trigger_ids": [],
            "eq_palettes": [],
            "monitor_clients": [],
            "gang_groups": []
        }"#;

        let dir = std::env::temp_dir().join("s21_hijack_test");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let path = dir.join("test_v7_legacy_scope_compat.json");
        tokio::fs::write(&path, v7_json).await.unwrap();

        let loaded = ShowFile::load(&path).await.unwrap();
        assert_eq!(loaded.version, 7);
        assert_eq!(loaded.scope_templates.len(), 1);
        let scope = &loaded.scope_templates[0];
        // Legacy section is preserved on the loaded scope.
        assert_eq!(scope.channel_scopes.len(), 1);
        assert!(scope.channel_scopes[0].paths.is_empty());
        assert!(!scope.channel_scopes[0].sections.is_empty());

        // Crucially: ScopeTemplate::contains still returns true for an EQ
        // parameter on Input(1), via the legacy `sections` read path.
        use crate::model::channel::ChannelId;
        use crate::model::parameter::{ParameterAddress, ParameterPath};
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::EqBandFrequency(2),
        };
        assert!(scope.contains(&addr));

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn v8_file_loads_with_legacy_palettes_and_palette_refs() {
        // V8 had `eq_palettes: Vec<EqPalette>` (now aliased to `palettes`)
        // and per-snapshot `eq_palette_refs: HashMap<ChannelId, Uuid>` (now
        // migrated into `palette_refs` keyed by `(channel, PaletteKind::Eq)`).
        // This test verifies both legacy fields load correctly.
        let v8_json = r#"{
            "version": 8,
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
            "snapshots": [
                {
                    "id": "11111111-1111-1111-1111-111111111111",
                    "name": "Verse 1",
                    "scope": {"id": "00000000-0000-0000-0000-000000000001", "name": "S", "channel_scopes": []},
                    "data": {"values": []},
                    "eq_palette_refs": [
                        {"channel": {"Input": 1}, "palette_id": "22222222-2222-2222-2222-222222222222"}
                    ],
                    "created_at": "2025-01-01T00:00:00Z",
                    "modified_at": "2025-01-01T00:00:00Z"
                }
            ],
            "cue_list": { "id": "00000000-0000-0000-0000-000000000000", "name": "Main", "cues": [] },
            "macros": [],
            "macro_quick_trigger_ids": [],
            "eq_palettes": [
                {
                    "id": "22222222-2222-2222-2222-222222222222",
                    "name": "Legacy Vocal EQ",
                    "channel": {"Input": 1},
                    "eq_values": [
                        {"path": "EqEnabled", "value": {"Bool": true}},
                        {"path": {"EqBandFrequency": 1}, "value": {"Float": 1200.0}}
                    ],
                    "referencing_snapshots": ["11111111-1111-1111-1111-111111111111"],
                    "created_at": "2025-01-01T00:00:00Z",
                    "modified_at": "2025-01-01T00:00:00Z"
                }
            ],
            "monitor_clients": [],
            "gang_groups": []
        }"#;

        let dir = std::env::temp_dir().join("s21_hijack_test");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let path = dir.join("test_v8_legacy_palettes_compat.json");
        tokio::fs::write(&path, v8_json).await.unwrap();

        let loaded = ShowFile::load(&path).await.unwrap();
        assert_eq!(loaded.version, 8);

        // Legacy `eq_palettes` field should have loaded into `palettes`
        // via the serde alias. The palette's `kind` should default to Eq.
        use crate::model::parameter::PaletteKind;
        assert_eq!(loaded.palettes.len(), 1);
        let palette = &loaded.palettes[0];
        assert_eq!(palette.name, "Legacy Vocal EQ");
        assert_eq!(palette.kind, PaletteKind::Eq);
        assert_eq!(palette.parameter_count(), 2);

        // The snapshot's legacy `eq_palette_refs` should have migrated into
        // the unified `palette_refs` map keyed by (channel, Eq).
        use crate::model::channel::ChannelId;
        let snap = &loaded.snapshots[0];
        assert_eq!(snap.palette_refs.len(), 1);
        assert!(
            snap.palette_refs
                .contains_key(&(ChannelId::Input(1), PaletteKind::Eq))
        );

        let _ = tokio::fs::remove_file(&path).await;
    }
}
