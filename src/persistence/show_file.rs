use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::model::config::ConsoleConfig;
use crate::model::cue_trigger::{OscTarget, TriggerTemplate};
use crate::model::gang::GangGroup;
use crate::model::macro_def::MacroDef;
use crate::model::monitor::MonitorClient;
use crate::model::operating_mode::OperatingMode;
use crate::model::palette::ChannelPalette;
use crate::model::pan_link::PanLinkBindings;
use crate::model::recall_scope::ConsoleRecallConfig;
use crate::model::sidecar::SidecarConfig;
use crate::model::snapshot::{CueList, ScopeTemplate, Snapshot};
use crate::model::streamdeck::StreamDeckConfig;
use crate::model::sync_direction::SnapshotSyncDirection;
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
    /// QLab network patch for cues that target **this app** — the trigger
    /// cues whose customString is `/snapshot/recall`. Should point at the
    /// app's trigger listener (`local_ip:trigger_port`). Default 1.
    #[serde(default = "default_qlab_patch_app")]
    pub qlab_patch_app: i32,
    /// QLab network patch for cues that target the **S21 console** — the
    /// per-parameter GP OSC cues. Should point at the console
    /// (`console_ip:console_gp_port`). Default 2.
    #[serde(default = "default_qlab_patch_console")]
    pub qlab_patch_console: i32,
    /// Inter-message pacing delay in microseconds during snapshot recall.
    /// Prevents flooding the console's ARM chip. 0 = no pacing.
    #[serde(default)]
    pub send_pace_us: u64,
    /// Auto-save dirty parameters into the previously-recalled snapshot
    /// when firing a new one. Filtered by the previous snapshot's scope
    /// template. Per-show workflow toggle.
    #[serde(default)]
    pub auto_update_on_recall: bool,
    /// Direction snapshot recalls flow between the app and the desk
    /// (Off / App→Console / Console→App). Unified on GP OSC, works in all
    /// modes. Per-show workflow setting. See [`effective_sync_direction`].
    ///
    /// [`effective_sync_direction`]: ConnectionSettings::effective_sync_direction
    #[serde(default)]
    pub snapshot_sync_direction: SnapshotSyncDirection,
    /// Legacy pre-direction follow toggle. Read-only for backward compat with
    /// older show files; reconciled into `snapshot_sync_direction` via
    /// [`effective_sync_direction`](ConnectionSettings::effective_sync_direction)
    /// and never written back out (newer files only carry the direction).
    #[serde(default, rename = "console_snapshot_follow", skip_serializing)]
    pub console_snapshot_follow_legacy: Option<bool>,
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
    /// Web monitor server port (0 = disabled, default 8080). Serves the
    /// browser-based personal-monitoring surface over HTTP/WebSocket.
    #[serde(default = "default_web_port")]
    pub web_port: u16,
    /// Source-IP CIDR allowlist for the **web** monitor server. Same
    /// semantics as `monitor_allow_cidrs`. LAN use only.
    #[serde(default)]
    pub web_allow_cidrs: Vec<String>,
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
fn default_qlab_patch_app() -> i32 {
    1
}
fn default_qlab_patch_console() -> i32 {
    2
}
fn default_monitor_port() -> u16 {
    8025
}
fn default_web_port() -> u16 {
    8080
}

impl ConnectionSettings {
    /// Resolve the effective sync direction, honoring the legacy
    /// `console_snapshot_follow` bool from pre-direction show files. A legacy
    /// `true` (which only ever meant follow-the-desk) maps to
    /// [`ConsoleToApp`](SnapshotSyncDirection::ConsoleToApp); a legacy `false`
    /// (or no legacy field at all) defers to `snapshot_sync_direction`.
    pub fn effective_sync_direction(&self) -> SnapshotSyncDirection {
        if self.console_snapshot_follow_legacy == Some(true) {
            return SnapshotSyncDirection::ConsoleToApp;
        }
        self.snapshot_sync_direction
    }
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
            qlab_patch_app: default_qlab_patch_app(),
            qlab_patch_console: default_qlab_patch_console(),
            send_pace_us: 0,
            auto_update_on_recall: false,
            snapshot_sync_direction: SnapshotSyncDirection::Off,
            console_snapshot_follow_legacy: None,
            monitor_allow_cidrs: Vec::new(),
            trigger_allow_cidrs: Vec::new(),
            web_port: 8080,
            web_allow_cidrs: Vec::new(),
            ui_mode: UiMode::default(),
        }
    }
}

/// Top-level show file — the persistent state of the daemon.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShowFile {
    /// File format version for future compatibility.
    pub version: u32,
    /// App version that wrote this file (e.g. `"0.1.0"`), for support and
    /// diagnostics — distinct from `version`, which is the file *format*
    /// revision used for migrations. New in v16; files written by older builds
    /// load with an empty string ("unknown").
    #[serde(default)]
    pub app_version: String,
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
    /// Stream Deck integration: device selection + per-button macro
    /// sequences. New in v15. Older show files (v14 and earlier) load
    /// with `StreamDeckConfig::default()` (disabled, no device, empty
    /// button list).
    #[serde(default)]
    pub stream_deck: StreamDeckConfig,
    /// Reusable OSC trigger destinations (QLab / LiveProfessor / custom).
    /// New in v17; older show files load with an empty list.
    #[serde(default)]
    pub osc_targets: Vec<OscTarget>,
    /// User-created external-trigger templates. Built-in templates live in
    /// code, so only user templates persist here. New in v17.
    #[serde(default)]
    pub trigger_templates: Vec<TriggerTemplate>,
    /// Fader sidecar: hardware control → parameter binding table. The
    /// MIDI port choice is machine-bound and lives in app preferences,
    /// not here. New in v18; older show files load with the sidecar
    /// disabled and no bindings.
    #[serde(default)]
    pub sidecar: SidecarConfig,
}

impl ShowFile {
    pub fn new(config: ConsoleConfig) -> Self {
        Self {
            version: 19,
            app_version: crate::version::APP_VERSION.to_string(),
            console_config: config,
            connection: ConnectionSettings::default(),
            scope_templates: Vec::new(),
            snapshots: Vec::new(),
            cue_list: CueList::default(),
            macros: Vec::new(),
            palettes: Vec::new(),
            monitor_clients: Vec::new(),
            gang_groups: Vec::new(),
            console_recall: ConsoleRecallConfig::default(),
            pan_link: PanLinkBindings::default(),
            stream_deck: StreamDeckConfig::default(),
            osc_targets: Vec::new(),
            trigger_templates: Vec::new(),
            sidecar: SidecarConfig::default(),
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
        let mut show: ShowFile = serde_json::from_str(&json).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Deserialize error: {e}"),
            )
        })?;
        // In-memory cleanup of values that older versions should never have
        // persisted. Non-destructive: the cleaned show is written out the next
        // time the operator saves.
        show.strip_total_gain();
        Ok(show)
    }

    /// Remove every persisted `TotalGain` reference from a loaded show. TotalGain
    /// (GP OSC `total/gain`) is a console-derived, read-only monitor value
    /// (post-fader + CG sum) that older versions captured into snapshots,
    /// selected in scopes, or recorded in macros. It can't be written back, so
    /// strip it on load. Idempotent: a show with no TotalGain is left unchanged.
    fn strip_total_gain(&mut self) {
        use crate::model::parameter::ParameterPath;

        // Snapshot stored values + each snapshot's embedded scope paths.
        for snap in &mut self.snapshots {
            snap.data
                .values
                .retain(|addr, _| addr.parameter != ParameterPath::TotalGain);
            for cs in &mut snap.scope.channel_scopes {
                cs.paths.remove(&ParameterPath::TotalGain);
            }
        }

        // Standalone scope templates.
        for tmpl in &mut self.scope_templates {
            for cs in &mut tmpl.channel_scopes {
                cs.paths.remove(&ParameterPath::TotalGain);
            }
        }

        // Macro parameter-write steps (keep all non-Parameter steps).
        for mac in &mut self.macros {
            mac.steps.retain(|step| {
                step.parameter_address()
                    .map(|a| a.parameter != ParameterPath::TotalGain)
                    .unwrap_or(true)
            });
        }

        // Sidecar bindings targeting TotalGain (hand-edited files only —
        // the learn flow refuses to create them).
        self.sidecar.bindings.retain(|b| {
            b.target
                .console_address()
                .map(|a| a.parameter != ParameterPath::TotalGain)
                .unwrap_or(true)
        });
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

        assert_eq!(loaded.version, 19);
        // A freshly-written show stamps the running app version.
        assert_eq!(loaded.app_version, crate::version::APP_VERSION);
        assert_eq!(loaded.console_config.input_channel_count, 48);
        assert_eq!(loaded.console_config.control_group_count, 10);
        assert!(loaded.scope_templates.is_empty());
        assert!(loaded.snapshots.is_empty());
        assert!(loaded.cue_list.cues.is_empty());
        assert_eq!(
            loaded.stream_deck,
            crate::model::streamdeck::StreamDeckConfig::default()
        );

        // Cleanup
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[test]
    fn legacy_follow_true_maps_to_console_to_app() {
        // A pre-direction show file carried `console_snapshot_follow: true`,
        // which only ever meant follow-the-desk → Console→App.
        let cs: ConnectionSettings =
            serde_json::from_str(r#"{ "console_snapshot_follow": true }"#).unwrap();
        assert_eq!(cs.console_snapshot_follow_legacy, Some(true));
        assert_eq!(
            cs.effective_sync_direction(),
            SnapshotSyncDirection::ConsoleToApp
        );
    }

    #[test]
    fn legacy_follow_false_is_off() {
        let cs: ConnectionSettings =
            serde_json::from_str(r#"{ "console_snapshot_follow": false }"#).unwrap();
        assert_eq!(cs.effective_sync_direction(), SnapshotSyncDirection::Off);
    }

    #[test]
    fn new_direction_round_trips_and_skips_legacy() {
        let cs = ConnectionSettings {
            snapshot_sync_direction: SnapshotSyncDirection::AppToConsole,
            ..Default::default()
        };
        let json = serde_json::to_string(&cs).unwrap();
        // Newer files only carry the direction; the legacy key is never written.
        assert!(!json.contains("console_snapshot_follow"));
        assert!(json.contains("snapshot_sync_direction"));
        let back: ConnectionSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.console_snapshot_follow_legacy, None);
        assert_eq!(
            back.effective_sync_direction(),
            SnapshotSyncDirection::AppToConsole
        );
    }

    #[tokio::test]
    async fn stream_deck_config_round_trips() {
        use crate::model::streamdeck::{StreamDeckButton, StreamDeckConfig, StreamDeckStep};

        let mut show = ShowFile::new(ConsoleConfig::default());
        show.stream_deck = StreamDeckConfig {
            enabled: true,
            device_serial: Some("AL12K1A12345".into()),
            buttons: vec![
                StreamDeckButton::default(),
                StreamDeckButton {
                    steps: vec![
                        StreamDeckStep {
                            macro_id: uuid::Uuid::from_bytes([1; 16]),
                            ..Default::default()
                        },
                        StreamDeckStep {
                            macro_id: uuid::Uuid::from_bytes([2; 16]),
                            ..Default::default()
                        },
                    ],
                    current_step: 1,
                },
            ],
            user_swatches: vec![],
        };

        let dir = std::env::temp_dir().join("s21_hijack_test");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let path = dir.join("test_show_streamdeck.json");
        show.save(&path).await.unwrap();
        let loaded = ShowFile::load(&path).await.unwrap();
        assert_eq!(loaded.version, 19);
        assert_eq!(loaded.stream_deck, show.stream_deck);
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn legacy_v14_show_loads_with_default_streamdeck() {
        // Older show files (without `stream_deck`) should round-trip
        // through serde with the field defaulted. Synthesise one by
        // hand-crafting a minimal v14 JSON blob.
        let json = r#"{
            "version": 14,
            "console_config": {
                "console_name": "",
                "console_serial": "",
                "session_filename": null,
                "input_channel_count": 48,
                "aux_output_count": 8,
                "group_output_count": 8,
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
        let parsed: ShowFile = serde_json::from_str(json).expect("v14 JSON parses");
        assert_eq!(parsed.version, 14);
        assert_eq!(
            parsed.stream_deck,
            crate::model::streamdeck::StreamDeckConfig::default()
        );
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
        // via the serde alias. The palette covers only EQ values, so its
        // derived `kinds()` should report exactly `[Eq]`.
        use crate::model::parameter::PaletteKind;
        assert_eq!(loaded.palettes.len(), 1);
        let palette = &loaded.palettes[0];
        assert_eq!(palette.name, "Legacy Vocal EQ");
        assert_eq!(palette.kinds(), vec![PaletteKind::Eq]);
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

    #[test]
    fn strip_total_gain_removes_all_references() {
        use crate::model::channel::ChannelId;
        use crate::model::macro_def::{MacroDef, MacroStep, MacroStepKind, MacroStepMode};
        use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
        use crate::model::snapshot::{
            ChannelScope, ScopeTemplate, Snapshot, SnapshotData, SnapshotKind,
        };
        use std::collections::HashSet;

        let tg = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::TotalGain,
        };
        let fader = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        let again = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::AnalogGain,
        };

        // Snapshot: TotalGain + Fader in data; TotalGain + AnalogGain in scope paths.
        let mut data = SnapshotData::new();
        data.values.insert(tg.clone(), ParameterValue::Float(-10.0));
        data.values
            .insert(fader.clone(), ParameterValue::Float(-5.0));
        let snap_scope = ScopeTemplate::new(
            "snap".into(),
            vec![ChannelScope::new(
                ChannelId::Input(1),
                HashSet::from([ParameterPath::TotalGain, ParameterPath::AnalogGain]),
            )],
        );
        let snapshot = Snapshot::new("S".into(), snap_scope, data, SnapshotKind::ApplyOnSave);

        // Standalone scope template with TotalGain + Fader.
        let tmpl = ScopeTemplate::new(
            "tmpl".into(),
            vec![ChannelScope::new(
                ChannelId::Input(1),
                HashSet::from([ParameterPath::TotalGain, ParameterPath::Fader]),
            )],
        );

        // Macro: a TotalGain step, an AnalogGain step, and a non-Parameter step.
        let mac = MacroDef::new(
            "M".into(),
            vec![
                MacroStep::parameter(
                    tg.clone(),
                    MacroStepMode::Fixed(ParameterValue::Float(-10.0)),
                    0,
                ),
                MacroStep::parameter(
                    again.clone(),
                    MacroStepMode::Fixed(ParameterValue::Float(20.0)),
                    0,
                ),
                MacroStep {
                    kind: MacroStepKind::GoNextCue,
                    delay_ms: 0,
                },
            ],
        );

        let mut show = ShowFile::new(ConsoleConfig::default());
        show.snapshots.push(snapshot);
        show.scope_templates.push(tmpl);
        show.macros.push(mac);

        show.strip_total_gain();

        // Snapshot data: TotalGain gone, Fader kept.
        assert!(!show.snapshots[0].data.values.contains_key(&tg));
        assert!(show.snapshots[0].data.values.contains_key(&fader));
        // Snapshot scope paths: TotalGain gone, AnalogGain kept.
        let snap_paths = &show.snapshots[0].scope.channel_scopes[0].paths;
        assert!(!snap_paths.contains(&ParameterPath::TotalGain));
        assert!(snap_paths.contains(&ParameterPath::AnalogGain));
        // Standalone template: TotalGain gone, Fader kept.
        let tmpl_paths = &show.scope_templates[0].channel_scopes[0].paths;
        assert!(!tmpl_paths.contains(&ParameterPath::TotalGain));
        assert!(tmpl_paths.contains(&ParameterPath::Fader));
        // Macro: TotalGain step gone; AnalogGain + GoNextCue kept.
        assert_eq!(show.macros[0].steps.len(), 2);
        assert!(
            show.macros[0]
                .steps
                .iter()
                .any(|s| matches!(&s.kind, MacroStepKind::GoNextCue))
        );
        assert!(
            show.macros[0]
                .steps
                .iter()
                .any(|s| s.parameter_address() == Some(&again))
        );

        // Idempotent: a second pass changes nothing.
        let before = show.macros[0].steps.len();
        show.strip_total_gain();
        assert_eq!(show.macros[0].steps.len(), before);
    }

    #[tokio::test]
    async fn cue_triggers_and_targets_round_trip() {
        use crate::model::cue_trigger::{
            CueTrigger, MidiMessage, OscArg, OscTarget, TriggerAction, TriggerTemplate,
        };
        use crate::model::snapshot::Cue;

        let mut show = ShowFile::new(ConsoleConfig::default());

        // OSC target + a user template persisted on the show.
        let target = OscTarget::new("QLab", "10.0.0.9", 53000);
        let target_id = target.id;
        show.osc_targets.push(target);
        show.trigger_templates.push(TriggerTemplate::user(
            "My LiveProfessor",
            TriggerAction::Osc {
                target_id: None,
                host: Some("10.0.0.5".into()),
                port: Some(8000),
                path: "/GlobalSnapshots/Recall".into(),
                args: vec![OscArg::Int(3)],
            },
        ));

        // A cue carrying mixed OSC (named + inline) + MIDI triggers.
        let mut cue = Cue::new(1.0, "Opening".into());
        cue.console_snapshot = Some(1);
        cue.triggers = vec![
            CueTrigger::new(TriggerAction::Osc {
                target_id: Some(target_id),
                host: None,
                port: None,
                path: "/go".into(),
                args: vec![OscArg::Str("Q3".into())],
            }),
            CueTrigger::new(TriggerAction::Midi {
                message: MidiMessage::ProgramChange {
                    channel: 1,
                    program: 7,
                },
            }),
        ];
        show.cue_list.cues.push(cue);

        let dir = std::env::temp_dir().join("s21_hijack_test");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let path = dir.join("test_show_triggers.json");
        show.save(&path).await.unwrap();
        let loaded = ShowFile::load(&path).await.unwrap();

        assert_eq!(loaded.version, 19);
        assert_eq!(loaded.osc_targets, show.osc_targets);
        assert_eq!(loaded.trigger_templates, show.trigger_templates);
        assert_eq!(
            loaded.cue_list.cues[0].triggers,
            show.cue_list.cues[0].triggers
        );

        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn legacy_v16_show_loads_with_empty_triggers() {
        // A v16 show (no triggers / osc_targets / trigger_templates) loads with
        // those fields defaulted empty — zero-migration backward compatibility.
        let json = r#"{
            "version": 16,
            "console_config": {
                "console_name": "",
                "console_serial": "",
                "session_filename": null,
                "input_channel_count": 48,
                "aux_output_count": 8,
                "group_output_count": 8,
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
            "cue_list": {
                "id": "00000000-0000-0000-0000-000000000000",
                "name": "Main",
                "cues": [
                    {
                        "id": "00000000-0000-0000-0000-000000000001",
                        "cue_number": 1.0,
                        "name": "Old cue",
                        "console_snapshot": 1,
                        "snapshot_id": null,
                        "scope_override": null,
                        "qlab_cue_id": null,
                        "notes": ""
                    }
                ]
            }
        }"#;
        let parsed: ShowFile = serde_json::from_str(json).expect("v16 JSON parses");
        assert_eq!(parsed.version, 16);
        assert!(parsed.osc_targets.is_empty());
        assert!(parsed.trigger_templates.is_empty());
        assert_eq!(parsed.cue_list.cues.len(), 1);
        assert!(parsed.cue_list.cues[0].triggers.is_empty());
    }

    #[tokio::test]
    async fn sidecar_config_round_trips() {
        use crate::model::channel::ChannelId;
        use crate::model::parameter::{ParameterAddress, ParameterPath};
        use crate::model::sidecar::{
            BindingTarget, ControlMode, ControlSelector, SidecarBinding, SidecarConfig, Taper,
            mcu_default_touch_note,
        };
        use uuid::Uuid;

        let mut show = ShowFile::new(ConsoleConfig::default());
        show.sidecar = SidecarConfig {
            enabled: true,
            bindings: vec![SidecarBinding {
                id: Uuid::from_bytes([9; 16]),
                label: "CH 12 Fader".into(),
                control: ControlSelector::PitchBend { channel: 1 },
                mode: ControlMode::PitchBend14,
                target: BindingTarget::ConsoleParameter {
                    address: ParameterAddress {
                        channel: ChannelId::Input(12),
                        parameter: ParameterPath::Fader,
                    },
                },
                taper: Taper::FaderDb { max_db: 10.0 },
                motor_feedback: true,
                touch: mcu_default_touch_note(1),
                relative_step: 1.0 / 300.0,
                enabled: true,
            }],
        };

        let dir = std::env::temp_dir().join("s21_hijack_test");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let path = dir.join("test_show_sidecar.json");
        show.save(&path).await.unwrap();
        let loaded = ShowFile::load(&path).await.unwrap();
        assert_eq!(loaded.version, 19);
        assert_eq!(loaded.sidecar, show.sidecar);
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[test]
    fn legacy_v18_show_loads_as_s_series_family() {
        // A v18 show (no `family` / `pad_quirk_overrides` on the console
        // config) loads as an S-series desk with the hardware-verified S21
        // wire quirks — zero-migration backward compat.
        let json = r#"{
            "version": 18,
            "console_config": {
                "console_name": "",
                "console_serial": "",
                "session_filename": null,
                "input_channel_count": 48,
                "aux_output_count": 8,
                "group_output_count": 8,
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
        let parsed: ShowFile = serde_json::from_str(json).expect("v18 JSON parses");
        assert_eq!(parsed.version, 18);
        assert_eq!(
            parsed.console_config.family,
            crate::model::family::ConsoleFamily::SSeries
        );
        assert!(parsed.console_config.pad_quirk_overrides.is_none());
        assert_eq!(
            parsed.console_config.profile().pad_quirks,
            crate::model::family::PadQuirks::S21
        );
    }

    #[test]
    fn show_naming_a_pad_only_family_loads_and_gates_s_series_features() {
        // End-to-end: a show that names a Pad-only family must come back with
        // that family, the family's wire quirks, and the S-series-only recall
        // scope UI gated off.
        use crate::model::family::{AppFeature, ConsoleFamily, PadQuirks};
        let json = r#"{
            "version": 19,
            "console_config": {
                "console_name": "Quantum 338",
                "console_serial": "",
                "session_filename": null,
                "input_channel_count": 128,
                "aux_output_count": 16,
                "group_output_count": 16,
                "matrix_output_count": 16,
                "matrix_input_count": 16,
                "control_group_count": 16,
                "graphic_eq_count": 16,
                "talkback_output_count": 0,
                "mix_output_types": [],
                "mix_output_modes": [],
                "input_modes": [],
                "group_modes": [],
                "family": "Quantum"
            }
        }"#;
        let parsed: ShowFile = serde_json::from_str(json).expect("v19 Quantum show parses");
        assert_eq!(parsed.console_config.family, ConsoleFamily::Quantum);
        let profile = parsed.console_config.profile();
        assert_eq!(profile.pad_quirks, PadQuirks::SD_HYPOTHESIS);
        assert!(!profile.has_gp_osc);
        assert!(!profile.supports(AppFeature::RecallScopeUi));
        // Channel counts beyond the S-series range survive the u16 widening.
        assert_eq!(parsed.console_config.input_channel_count, 128);
    }

    #[test]
    fn legacy_v17_show_loads_with_default_sidecar() {
        // A v17 show (no `sidecar` field) loads with the sidecar disabled
        // and an empty binding table — zero-migration backward compat.
        let json = r#"{
            "version": 17,
            "console_config": {
                "console_name": "",
                "console_serial": "",
                "session_filename": null,
                "input_channel_count": 48,
                "aux_output_count": 8,
                "group_output_count": 8,
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
        let parsed: ShowFile = serde_json::from_str(json).expect("v17 JSON parses");
        assert_eq!(parsed.version, 17);
        assert_eq!(
            parsed.sidecar,
            crate::model::sidecar::SidecarConfig::default()
        );
        assert!(!parsed.sidecar.enabled);
        assert!(parsed.sidecar.bindings.is_empty());
    }

    #[test]
    fn strip_total_gain_removes_sidecar_bindings() {
        use crate::model::channel::ChannelId;
        use crate::model::parameter::{ParameterAddress, ParameterPath};
        use crate::model::sidecar::{
            BindingTarget, ControlMode, ControlSelector, SidecarBinding, Taper,
        };
        use uuid::Uuid;

        let mk = |n: u8, parameter: ParameterPath| SidecarBinding {
            id: Uuid::from_bytes([n; 16]),
            label: String::new(),
            control: ControlSelector::Cc { channel: 1, cc: n },
            mode: ControlMode::Absolute7,
            target: BindingTarget::ConsoleParameter {
                address: ParameterAddress {
                    channel: ChannelId::Input(1),
                    parameter,
                },
            },
            taper: Taper::Linear { min: 0.0, max: 1.0 },
            motor_feedback: false,
            touch: None,
            relative_step: 1.0 / 300.0,
            enabled: true,
        };
        let mut show = ShowFile::new(ConsoleConfig::default());
        show.sidecar.bindings = vec![mk(1, ParameterPath::TotalGain), mk(2, ParameterPath::Fader)];
        show.strip_total_gain();
        assert_eq!(show.sidecar.bindings.len(), 1);
        assert_eq!(
            show.sidecar.bindings[0]
                .target
                .console_address()
                .unwrap()
                .parameter,
            ParameterPath::Fader
        );
    }
}
