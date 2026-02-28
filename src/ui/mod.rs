pub mod app;
pub mod live_tab;
pub mod scope_editor;
pub mod setup_tab;
pub mod snapshots_tab;
pub mod theme;

/// Active UI tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Setup,
    Snapshots,
    Live,
}

/// Events sent from async tasks back to the UI thread.
#[derive(Debug)]
pub enum UiEvent {
    ConnectionEstablished,
    ConnectionFailed(String),
    SnapshotCaptured {
        name: String,
        param_count: usize,
    },
    CueRecalled {
        cue_number: f32,
        params_sent: usize,
    },
    ShowFileLoaded(String),
    ShowFileSaved(String),
    ShowFileError(String),
}
