pub mod advanced_settings;
pub mod app;
pub mod cue_transport;
pub mod fonts;
pub mod gangs_tab;
pub mod inspector_tab;
pub mod macros_tab;
pub mod monitor_channel_picker;
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

/// Active UI tab. The previous `Live` variant was retired when the GO /
/// PREV / current-cue transport moved into the top-bar strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Setup,
    Snapshots,
    Macros,
    Gangs,
    PanLink,
    Monitor,
    OscLog,
    Inspector,
}

/// Engine handles produced by the connect-console async task.
///
/// The task that establishes the console connection constructs the OSC sender
/// and the snapshot/macro engines, then drops them into a shared
/// `Arc<Mutex<Option<PendingEngines>>>`. The egui `update()` loop polls that
/// slot each frame and moves the handles into `HiJackApp` so UI buttons (Run
/// macro, Recall snapshot) can use them. Without this hand-off, the App
/// fields stay `None` and UI-driven actions silently no-op.
pub struct PendingEngines {
    pub sender: crate::osc::client::OscSender,
    pub snapshot_engine: std::sync::Arc<crate::console::snapshot_engine::SnapshotEngine>,
    pub macro_engine: std::sync::Arc<crate::console::macro_engine::MacroEngine>,
    pub ipad_sender: Option<crate::osc::ipad_client::IpadSender>,
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
        steps_skipped: usize,
    },
    /// Macro could not be executed (engine missing, macro not found, etc.).
    /// Surfaces the reason in the Macros tab status area so the operator
    /// gets feedback instead of a silent no-op when the Run button is clicked.
    MacroExecutionFailed(String),
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

    // ─── App-internal commands triggered by macro steps ──────────────
    //
    // The macro engine emits these events instead of calling app-side
    // helpers directly — keeps the engine ↔ UI coupling thin (the
    // engine doesn't need handles to CueManager, SnapshotEngine, the
    // setup-tab connection helpers, etc.). The UI thread drains them
    // in `HiJackApp::drain_events` and dispatches to the appropriate
    // existing handler (cue_transport::fire_go etc.).
    //
    /// Macro step requested a Go (advance to next cue).
    MacroFireGo,
    /// Macro step requested a Prev (step back to previous cue).
    MacroFirePrev,
    /// Macro step requested a console connect using current Setup
    /// settings.
    MacroConnect,
    /// Macro step requested a disconnect. **Note:** this tears down
    /// the connection running the macro — subsequent steps in the
    /// same macro will not execute.
    MacroDisconnect,
    /// Macro step requested a direct snapshot recall (bypasses cue
    /// list).
    MacroRecallSnapshot {
        snapshot_id: uuid::Uuid,
    },
    /// Macro step requested applying a palette to a specific channel.
    MacroRecallPalette {
        palette_id: uuid::Uuid,
        channel: crate::model::channel::ChannelId,
    },
    /// Macro step requested a QLab OSC action — send `addr` (with the
    /// optional `string_arg`) to the configured QLab destination.
    /// `label` is the human-readable action name shown in failure
    /// messages. The macro engine constructs the address; the app
    /// thread just routes it to QLab using the current Setup-tab
    /// QLab IP / port.
    MacroQLabSend {
        addr: String,
        string_arg: Option<String>,
        label: String,
    },

    // ─── Stream Deck integration ─────────────────────────────────────
    //
    // The Stream Deck engine runs on a dedicated background thread
    // (HID I/O is sync). Button presses and connection-state changes
    // arrive here and are drained on the UI thread, which then fires
    // the next-to-fire macro for the pressed button and/or refreshes
    // the LCD via the engine's command channel.
    //
    /// One physical button on the connected Stream Deck was pressed
    /// (transition to "down"). `button_idx` is the device's 0-based
    /// row-major index.
    StreamDeckButtonPressed {
        button_idx: usize,
    },
    /// Connection succeeded; UI should resize its config to match
    /// `button_count` and push initial LCD labels.
    StreamDeckConnected {
        device_name: String,
        button_count: u8,
    },
    /// Device dropped (unplugged, read error, or operator disabled).
    StreamDeckDisconnected,
    /// Driver-level error, surfaced as a status string for the UI.
    StreamDeckError {
        message: String,
    },
}
