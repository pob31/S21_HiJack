pub mod app;
pub mod gangs_tab;
pub mod inspector_tab;
pub mod live_tab;
pub mod macros_tab;
pub mod monitor_tab;
pub mod net_interfaces;
pub mod osc_log_tab;
pub mod palettes_ui;
pub mod pan_link_tab;
pub mod recall_scope_popup;
pub mod scope_editor;
pub mod setup_tab;
pub mod snapshots_tab;
pub mod theme;

/// Active UI tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Setup,
    Snapshots,
    Macros,
    Live,
    Gangs,
    PanLink,
    Monitor,
    OscLog,
    Inspector,
}

/// Events sent from async tasks back to the UI thread.
#[derive(Debug)]
pub enum UiEvent {
    ConnectionEstablished,
    ConnectionFailed(String),
    Disconnected,
    SnapshotCaptured {
        name: String,
        param_count: usize,
    },
    CueRecalled {
        cue_number: f32,
        params_sent: usize,
    },
    MacroExecuted {
        name: String,
        steps_executed: usize,
    },
    MacroRecordingStopped {
        step_count: usize,
    },
    PaletteCaptured {
        name: String,
        param_count: usize,
    },
    PaletteLinked {
        palette_name: String,
        snapshot_name: String,
    },
    PaletteUpdated {
        name: String,
        affected_count: usize,
    },
    /// `ConnectionSettings` is boxed to keep the enum's largest variant
    /// small enough not to trip `clippy::large_enum_variant` — the struct
    /// has many `String`/`Vec<String>` fields and grows over time.
    ShowFileLoaded(
        String,
        Option<Box<crate::persistence::show_file::ConnectionSettings>>,
        crate::model::recall_scope::ConsoleRecallConfig,
    ),
    ShowFileSaved(String),
    ShowFileError(String),
    IpadConnected,
    IpadConnectionFailed(String),
    FadeProgress {
        cue_number: f32,
        progress: f32,
        done: bool,
    },
    MonitorClientConnected {
        name: String,
    },
    MonitorClientDisconnected {
        name: String,
    },
    MonitorServerStarted,
    MonitorServerFailed(String),
}
