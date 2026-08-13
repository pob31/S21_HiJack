pub mod advanced_settings;
pub mod app;
pub mod channel_tile;
pub mod cue_list_popup;
pub mod cue_transport;
pub mod fonts;
pub mod gang_member_picker;
pub mod gangs_tab;
pub mod help;
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
pub mod sidecar_tab;
pub mod snapshots_tab;
pub mod status;
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
    Sidecar,
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
    /// This connection's shared sent-value log (echo screening) — attached
    /// to the sidecar service's `ConsoleTx` so its writes are screened too.
    pub sent_log: crate::console::console_tx::SentLog,
    /// The connected console's profile, so the sidecar's own write path
    /// encodes with the same Pad wire quirks as the engines.
    pub profile: std::sync::Arc<crate::model::family::ConsoleProfile>,
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
    /// A capture / re-capture finished — drives the post-capture
    /// confirmation popup (lists the recorded params, offers an immediate
    /// re-recall). Separate from `SnapshotCaptured` (still used by undo /
    /// QLab export as a generic status channel).
    SnapshotCaptureConfirm {
        snapshot_id: uuid::Uuid,
        name: String,
        /// Pre-formatted (label, value) pairs, sorted for stable display.
        params: Vec<(String, String)>,
    },
    /// A snapshot recall finished. Distinct from `SnapshotCaptured` so the
    /// status line reads "Recalled …" rather than "Captured …".
    SnapshotRecalled {
        name: String,
        params_sent: usize,
    },
    /// Result of writing a scope into a snapshot (the scope editor's "Apply
    /// Scope to Snapshot"). `ok == false` means no snapshot with that id was
    /// found — it was deleted while the editor was open. Drives an honest
    /// status message after the async write completes.
    SnapshotScopeApplied {
        name: String,
        ok: bool,
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
    /// A bulk assign/clear from an assign-overlay header / snapshot-name click.
    PaletteBulkAssigned {
        palette_name: String,
        linked: bool,
        count: usize,
    },
    PaletteUpdated {
        name: String,
        affected_count: usize,
    },
    /// Operator hit "Revert changes" on a palette: drop its in-session working
    /// overlay and reload the last-captured (stored) values onto the channel.
    PaletteRevert {
        palette_id: uuid::Uuid,
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
    /// An autosave write task finished. `wrote` is false when the content
    /// fingerprint was unchanged (nothing written). Carries the new
    /// fingerprint so the UI updates its dedup baseline and clears the
    /// in-flight guard.
    AutosaveCompleted {
        fingerprint: u64,
        wrote: bool,
    },
    /// A load failed because the file looks truncated or has a bad header.
    /// Carries the original path and the recovery candidates (backups +
    /// autosaves) found for that show, so the UI can offer recovery.
    ShowFileCorrupt {
        path: String,
        candidates: Vec<crate::persistence::backup::RecoveryCandidate>,
    },
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
    WebServerStarted,
    WebServerFailed(String),

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

    // ─── Fader sidecar ───────────────────────────────────────────────
    //
    // The sidecar MIDI engine runs on a dedicated background thread
    // (midir handles are !Send). Connection-state changes arrive here;
    // the app thread updates the Sidecar tab status and triggers a
    // console-wins surface sync on (re)connect. Hardware *events*
    // don't pass through here — they flow straight to the sidecar
    // service over its own channel.
    //
    /// Sidecar MIDI input (+ optional feedback output) connected.
    SidecarMidiConnected {
        input: String,
        output: Option<String>,
    },
    /// Sidecar MIDI device dropped (unplugged or operator disconnect).
    SidecarMidiDisconnected,
    /// Driver-level error, surfaced as a status string for the UI.
    SidecarError {
        message: String,
    },

    // ─── App update check ────────────────────────────────────────────
    /// Result of a GitHub "latest release" lookup (auto on launch or the
    /// Setup-tab "Check for updates" button). Drained on the UI thread into
    /// `setup.update_status` for the version footer.
    UpdateCheckResult(crate::version::UpdateStatus),
}
