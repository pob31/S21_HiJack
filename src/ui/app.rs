use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::console::cue_manager::CueManager;
use crate::console::gang_manager::GangManager;
use crate::console::macro_engine::MacroEngine;
use crate::console::macro_manager::MacroManager;
use crate::console::monitor_manager::MonitorManager;
use crate::console::palette_manager::PaletteManager;
use crate::console::snapshot_engine::SnapshotEngine;
use crate::model::config::ConsoleConfig;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::operating_mode::OperatingMode;
use crate::model::osc_log::OscLog;
use crate::model::pan_link::PanLinkBindings;
use crate::model::recall_progress::ProgressBars;
use crate::model::snapshot::CueList;
use crate::model::state::ConsoleState;
use crate::model::sync_direction::SharedSyncDirection;
use crate::osc::client::OscSender;
use crate::osc::ipad_client::IpadSender;
use crate::persistence::preferences::AppPreferences;

use super::gangs_tab::GangsTabState;
use super::help::{HelpHover, HelpKey, help};
use super::inspector_tab::InspectorTabState;
use super::macros_tab::MacrosTabState;
use super::monitor_tab::MonitorTabState;
use super::osc_log_tab::OscLogTabState;
use super::palettes_ui::PalettesUiState;
use super::pan_link_tab::PanLinkTabState;
use super::setup_tab::SetupTabState;
use super::snapshots_tab::SnapshotsTabState;
use super::status::StatusMessage;
use super::{PendingEngines, Tab, UiEvent};

// ─── Global physical-size (PPI) UI scaling ──────────────────────────────────
// The UI is scaled to a consistent *physical* size derived from the display's
// real PPI (see `crate::platform`); it deliberately does NOT fit-to-window
// scale. Fit-to-window pins the logical layout width to a constant and makes the
// whole UI scale uniformly on resize, which stops tabs from reflowing (gaps can
// never collapse) and shrinks controls to unusable on small windows. Holding a
// stable physical size instead lets each tab reflow against the actual window —
// collapse slack, shrink panels, then scroll.

/// Physical-size reference density. The auto scaler targets `PPI / REF_PPI`
/// effective pixels-per-point, so a 16 pt font is ~16/96″ ≈ 0.17″ tall on any
/// display whose real PPI we can detect. Lower it to make everything physically
/// larger by default.
const REF_PPI: f32 = 96.0;

/// A modest global enlargement applied on top of the computed base size, so the
/// control-dense UI gets a little more breathing room than the OS/PPI baseline
/// alone — viewed at arm's length on a console, the bare OS scale reads slightly
/// small. This is what the manual Display-Scale slider's 100% maps to (i.e. the
/// "reference" size), and the slider multiplies further on top. Tuned on a
/// 15.6" 4K @ 200% panel where the bare OS scale (2.0) was a touch cramped and
/// ~1.15× (≈2.3) made the best use of the screen.
///
/// Folds in a 0.95 factor (1.15 × 0.95 ≈ 1.0925): the Noto Sans Regular body
/// font has slightly larger metrics than the old Ubuntu-Light, so the UI read
/// ~5% bigger — operators were dialing the slider down to 95% to compensate.
/// Baking it in means 100% now maps to that comfortable density out of the box.
const COMFORT_FACTOR: f32 = 1.0925;

/// Below this native scale factor the OS is considered to be doing little or no
/// scaling (Linux / Raspberry Pi report exactly 1.0; small fractional values are
/// noise). At or above it, the OS scale factor is a deliberate user choice and is
/// treated as authoritative — real-PPI physical sizing must NOT override it
/// (Windows 125% / 150% / 200%, macOS Retina). PPI sizing only steps in *below*
/// this threshold, to rescue a high-PPI panel the OS left unscaled. The cutoff
/// sits between 1.0 (unscaled) and Windows' first real step of 1.25; the
/// discontinuity there is intentional and benign given those discrete steps.
const NATIVE_SCALING_THRESHOLD: f32 = 1.10;

// Clamp on the *effective pixels-per-point* (what egui renders at), not on a
// zoom factor. This is the absolute target we hand to `set_pixels_per_point`,
// so the bounds are independent of the display's native scale factor.
const MIN_PPP: f32 = 0.80; // readability floor (body 16pt → ~13px)
const MAX_PPP: f32 = 5.00; // sanity ceiling (15" 4K ≈2.9, Pi @250% slider ≈4.4)

const SCALE_STEP: f32 = 0.02; // snap ppp to 2% steps (kills drag-resize jitter)
const SCALE_EPS: f32 = 0.005; // dead-band; must be < SCALE_STEP
const MIN_PHYS_PX: f32 = 200.0; // reject 0-area minimize frames
const MAX_PHYS_PX: f32 = 20_000.0; // reject egui's 10000×10000 first-frame placeholder

/// Consecutive converged frames `size_window_to_monitor` waits for before the
/// one-shot resize. egui's `monitor_size` lags our zoom change by one pass, so
/// the first settled frame still reports the pre-zoom (stale) monitor size; a
/// small hold lets it catch up so the 95% sizing is computed against a
/// `monitor_size` consistent with the settled ppp.
const SETTLE_HOLD_FRAMES: u8 = 3;

/// Seconds the window geometry must hold steady after a resize/move before it
/// is persisted to preferences (see `HiJackApp::track_window_geometry`). Keeps
/// the prefs file from being rewritten on every drag frame.
const GEOMETRY_SAVE_DEBOUNCE: f64 = 1.0;

/// Pure core of the UI auto-scaler (unit-tested). Produces the *absolute
/// effective pixels-per-point* egui should render at.
///
/// Policy: the OS scale factor (`native_ppp`) is AUTHORITATIVE whenever the OS
/// is actually scaling (`native_ppp > NATIVE_SCALING_THRESHOLD`) — it already
/// encodes the physical size the user chose in their Windows/macOS display
/// settings, so we respect it and never let raw-EDID PPI scale the UI up past
/// it. Only when the OS is doing little/no scaling (`native_ppp ≈ 1.0`, e.g. a
/// Raspberry Pi touchscreen or a high-PPI 4K panel misconfigured at 100%) do we
/// fall back to real-PPI physical sizing (`PPI / REF_PPI`) so the UI isn't
/// microscopic — floored at `native_ppp` so a low-PPI panel is never shrunk
/// below 1×.
///
/// The base is then enlarged by [`COMFORT_FACTOR`] (the "reference" size the
/// slider's 100% maps to) and the user's manual `ui_scale` multiplies on top;
/// clamped for sanity. We target an absolute ppp (applied via
/// `Context::set_pixels_per_point`) rather than an egui *zoom factor*, so egui
/// performs the native division with its own current `native_pixels_per_point`
/// — removing the cross-frame inconsistency that previously doubled the scale
/// during the hidden-window startup transient.
///
/// Deliberately independent of the window size: the UI keeps a stable physical
/// size and the tabs *reflow* against the actual window (collapse slack, then
/// scroll) rather than uniformly shrinking to fit.
fn compute_target_ppp(native_ppp: f32, ppi: Option<f32>, ui_scale: f32) -> f32 {
    let native_ppp = if native_ppp > 0.0 { native_ppp } else { 1.0 };
    let base = if native_ppp > NATIVE_SCALING_THRESHOLD {
        // OS is deliberately scaling — that is the authority. Respect it.
        native_ppp
    } else {
        // OS is not scaling. Use real PPI to reach a sane physical size on a
        // high-PPI panel, but never shrink below what the OS reported.
        match ppi {
            Some(p) if p > 0.0 => (p / REF_PPI).max(native_ppp),
            _ => native_ppp, // PPI unknown → respect the OS scale factor
        }
    };
    (base * COMFORT_FACTOR * ui_scale).clamp(MIN_PPP, MAX_PPP)
}

/// State for the post-capture confirmation popup — a centered, temporary
/// window listing the parameters a capture just recorded. It auto-fades after
/// ~10 s unless the operator clicks or scrolls it (which pins it open), and
/// offers a button to immediately re-recall the snapshot to verify nothing has
/// drifted on the console.
pub struct CaptureConfirm {
    pub snapshot_id: uuid::Uuid,
    pub name: String,
    /// Pre-formatted (label, value) pairs to display.
    pub params: Vec<(String, String)>,
    /// When the popup appeared — drives the 10 s auto-fade.
    pub started_at: std::time::Instant,
    /// Set once the user clicks/scrolls the window; disables the auto-fade.
    pub pinned: bool,
    /// Baseline `ConsoleState::generation()` captured on the first frame. Once
    /// the console sends ≥3 further parameter updates (operator back at the
    /// desk), the popup auto-dismisses. Skipped while `pinned`.
    pub seen_gen_base: Option<u64>,
}

/// Modal shown when a show file fails to load because it is truncated / has a
/// bad header. Lists the recovery candidates (backups + autosaves) for that
/// show so the operator can restore one and optionally repair the original.
pub struct RecoveryDialog {
    /// The corrupt file the operator tried to open.
    pub original_path: String,
    /// Candidates found in `.s21backups/`, newest-first.
    pub candidates: Vec<crate::persistence::backup::RecoveryCandidate>,
    /// Set after a candidate has been loaded — enables the "Save to original
    /// path" repair affordance.
    pub recovered: bool,
}

/// UI-side easing state for one recall progress line. The shared
/// `RecallProgress` handle holds the raw `done`/`total`; this smooths the drawn
/// fraction toward the target and fades the bar out when an op completes.
#[derive(Default)]
struct RecallProgressUi {
    /// Smoothed 0.0..=1.0 fraction actually drawn.
    smoothed: f32,
    /// Fade-out alpha (1.0 while filling; decays to 0.0 after completion).
    fade: f32,
    /// `generation` of the op we're currently showing — when it changes, a new
    /// op has started and we reset `smoothed`/`fade`.
    last_generation: u64,
    /// Sweep phase for the indeterminate (unknown-total) case.
    sweep: f32,
}

/// Main application struct implementing eframe::App.
pub struct HiJackApp {
    // Shared state
    pub state: Arc<RwLock<ConsoleState>>,
    pub cue_manager: Arc<RwLock<CueManager>>,
    pub macro_manager: Arc<RwLock<MacroManager>>,
    pub monitor_manager: Arc<RwLock<MonitorManager>>,
    pub palette_manager: Arc<RwLock<PaletteManager>>,
    pub gang_manager: Arc<RwLock<GangManager>>,
    pub pan_link_bindings: Arc<RwLock<PanLinkBindings>>,
    /// Offline mode — when true, all OSC sends become no-ops AND inbound
    /// messages are dropped. Frozen state mirror, no side effects. Lets
    /// the operator edit show data safely without touching the desk.
    /// Defaults to OFF on every startup; not persisted.
    pub offline_mode: Arc<AtomicBool>,
    /// Auto-save dirty parameters into the previously-recalled snapshot
    /// when firing a new one. Mirrored from `ConnectionSettings`.
    pub auto_update_on_recall: Arc<AtomicBool>,
    /// Direction snapshot recalls flow between app and console (Off /
    /// App→Console / Console→App). Mirrored from `ConnectionSettings`.
    pub snapshot_sync_direction: SharedSyncDirection,
    /// Inter-message pacing (μs) shared between snapshot recall and
    /// macro OSC sends. 0 = no pacing. Owned at the app level so the
    /// Advanced Settings panel and both engines see one consistent
    /// value; persisted in `AppPreferences`.
    pub send_pace_us: Arc<std::sync::atomic::AtomicU64>,
    /// Two shared progress handles for long parameter transfers: inbound
    /// (console dump flood, green) and outbound (snapshot/cue/macro recall
    /// sends, blue). Cloned into the daemon (inbound) and recall engines
    /// (outbound) on connect; the UI polls both each frame to render the two
    /// stacked progress lines under the tabs.
    pub progress: ProgressBars,
    /// UI-only easing/fade state for the inbound (green) progress line.
    progress_ui_in: RecallProgressUi,
    /// UI-only easing/fade state for the outbound (blue) progress line.
    progress_ui_out: RecallProgressUi,
    /// Last-rendered transport cue label `(text, has_cues)`. The top-bar strip
    /// reads `cue_manager` with a non-blocking `try_read` every frame; when a
    /// background writer briefly holds the lock, the cached value is shown for
    /// that frame instead of blocking the UI thread. `RefCell` because the
    /// strip renders behind shared `&self` borrows.
    transport_cue_cache: std::cell::RefCell<(Option<String>, bool)>,
    pub snapshot_engine: Option<Arc<SnapshotEngine>>,
    pub macro_engine: Option<Arc<MacroEngine>>,
    /// Hand-off slot the connect-console async task uses to deliver the OSC
    /// sender and freshly-constructed engines back to the UI thread. Polled
    /// each frame; when populated, the contents are moved into the
    /// `sender` / `snapshot_engine` / `macro_engine` / `ipad_sender` fields.
    pub pending_engines: Arc<std::sync::Mutex<Option<PendingEngines>>>,
    /// Dirty tracker — populated by the OSC dispatcher whenever an inbound
    /// parameter update changes the live state. The scope editor reads it to
    /// power "select modified" / "auto-preselect modified" / "clear changes".
    pub dirty_tracker: Arc<RwLock<DirtyTracker>>,
    /// Most recent inbound parameter address from the connection
    /// daemon. Drives the Macros tab "track latest OSC" affordance — UI
    /// reads this each frame and mirrors it into the Add Step form.
    pub last_received: Arc<RwLock<Option<crate::model::parameter::ParameterAddress>>>,
    /// Stream Deck integration: device selection + per-button macro
    /// sequences. Loaded from / saved to the show file. Mutated by the
    /// Macros tab UI (operator edits) and by `drain_events` when a
    /// physical button press advances the playback cursor.
    pub stream_deck_config: Arc<RwLock<crate::model::streamdeck::StreamDeckConfig>>,
    /// Stream Deck driver engine — owns the device-thread + LCD
    /// rendering. Eagerly constructed at app startup so the UI can
    /// see freshly-plugged devices without an explicit "scan" step;
    /// idle when no device is connected.
    pub stream_deck_engine: Arc<crate::console::streamdeck_engine::StreamDeckEngine>,

    // OSC log (shared with network tasks)
    pub osc_log: OscLog,

    // Async bridge
    pub runtime: tokio::runtime::Handle,
    pub egui_ctx: Arc<std::sync::OnceLock<egui::Context>>,
    pub ui_rx: std::sync::mpsc::Receiver<UiEvent>,
    pub ui_tx: std::sync::mpsc::Sender<UiEvent>,

    // Connection state
    pub connected: Arc<AtomicBool>,
    pub sender: Option<OscSender>,
    pub ipad_sender: Option<IpadSender>,
    /// Cancellation token for all connection-related tasks.
    pub cancel_token: Option<CancellationToken>,

    // Tab state
    pub active_tab: Tab,
    pub setup: SetupTabState,
    pub snapshots: SnapshotsTabState,
    pub macros: MacrosTabState,
    pub palettes_ui: PalettesUiState,
    pub gangs: GangsTabState,
    pub pan_link: PanLinkTabState,
    pub monitor: MonitorTabState,
    pub osc_log_tab: OscLogTabState,
    pub inspector: InspectorTabState,

    /// Post-capture confirmation popup, when one is showing.
    pub capture_confirm: Option<CaptureConfirm>,

    /// Whether the top-bar cue-list popup window is open.
    pub show_cue_list_popup: bool,

    /// True while the "Quit while connected?" confirmation modal is showing.
    /// Raised when a window-close is requested while the console link is up so
    /// an accidental close doesn't silently drop a live show. Cleared on either
    /// choice. Runtime-only.
    pub confirm_close: bool,
    /// Set once the operator confirms the quit, so the close request we re-issue
    /// (and any later OS close) passes straight through instead of re-opening
    /// the confirmation modal. Runtime-only.
    pub close_confirmed: bool,

    // ─── Autosave scheduler (see persistence::backup) ────────────────
    /// When the last autosave was taken — throttles to `AUTOSAVE_INTERVAL`.
    pub last_autosave_at: std::time::Instant,
    /// Content fingerprint of the last autosave, so we skip writing when
    /// nothing changed.
    pub last_autosaved_fingerprint: u64,
    /// Last observed `ConsoleState::generation()` and the instant it changed —
    /// drives the "quiet settle" gate.
    pub last_seen_generation: u64,
    pub generation_changed_at: std::time::Instant,
    /// True while an autosave write task is in flight (prevents overlap).
    pub autosave_in_flight: Arc<AtomicBool>,
    /// Active corruption-recovery modal, if any.
    pub recovery_dialog: Option<RecoveryDialog>,

    // ─── DPI-aware UI scaling ────────────────────────────────────────
    /// Cached physical metrics of all monitors. Refreshed by `current_ppi`
    /// only when the window's monitor changes (EDID / Core Graphics reads
    /// are not per-frame).
    monitors: Vec<crate::platform::MonitorMetrics>,
    /// Physical resolution of the monitor the window was last seen on; when it
    /// changes, `monitors` is re-enumerated.
    last_monitor_px: Option<(u32, u32)>,
    /// One-shot: the startup window has been sized to 95% of the monitor and
    /// revealed. Until then the window is hidden (see `main.rs`).
    window_sized: bool,
    /// Frames waited for the monitor size before giving up and revealing at the
    /// fallback size — guards the rare case where `monitor_size` is `None` on
    /// the first frames.
    startup_frames: u8,
    /// True on a frame where `apply_auto_scale` saw the effective ppp within the
    /// dead-band of the target — i.e. the scale has converged this frame.
    scale_settled: bool,
    /// Count of *consecutive* converged frames. egui's reported `monitor_size`
    /// lags our zoom changes by a frame (it is computed from the previous pass's
    /// effective ppp), so `scale_settled` alone is not enough — on the first
    /// settled frame `monitor_size` is still stale. `size_window_to_monitor`
    /// waits for this to reach [`SETTLE_HOLD_FRAMES`] so `monitor_size` has
    /// caught up to the settled ppp before the one-shot resize.
    settled_frames: u8,
    /// Window-geometry persistence (issue: "remember last size/position").
    /// `last_seen_geometry` is the (inner_size, outer_pos) reported last frame;
    /// `last_saved_geometry` is what's currently on disk; `geometry_dirty_at`
    /// is the `ctx.input().time` of the most recent change. We persist once the
    /// geometry has been stable for `GEOMETRY_SAVE_DEBOUNCE` — see
    /// `track_window_geometry`.
    last_seen_geometry: Option<([f32; 2], [f32; 2])>,
    last_saved_geometry: Option<([f32; 2], [f32; 2])>,
    geometry_dirty_at: Option<f64>,
}

impl HiJackApp {
    pub fn new(
        console_ip: &str,
        console_port: u16,
        local_port: u16,
        trigger_port: u16,
        operating_mode: OperatingMode,
        ipad_ip: Option<&str>,
        ipad_send_port: u16,
        ipad_receive_port: u16,
        monitor_port: u16,
        prefs: AppPreferences,
        runtime: tokio::runtime::Handle,
        show_file: Option<std::path::PathBuf>,
    ) -> Self {
        let (ui_tx, ui_rx) = std::sync::mpsc::channel();

        let mut setup = SetupTabState::new(
            console_ip,
            console_port,
            local_port,
            trigger_port,
            operating_mode,
            ipad_ip,
            ipad_send_port,
            ipad_receive_port,
            monitor_port,
            &prefs,
        );
        if let Some(path) = show_file {
            // Pre-populate the path field so the UI shows where we
            // came from, and queue an auto-load on the first frame
            // the Setup tab draws.
            setup.show_file_path = path.display().to_string();
            setup.pending_initial_load = Some(path);
        }

        Self {
            state: Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default()))),
            cue_manager: Arc::new(RwLock::new(CueManager::new(CueList::default()))),
            macro_manager: Arc::new(RwLock::new(MacroManager::new())),
            monitor_manager: Arc::new(RwLock::new(MonitorManager::new())),
            palette_manager: Arc::new(RwLock::new(PaletteManager::new())),
            gang_manager: Arc::new(RwLock::new(GangManager::new())),
            pan_link_bindings: Arc::new(RwLock::new(PanLinkBindings::default())),
            offline_mode: Arc::new(AtomicBool::new(false)),
            auto_update_on_recall: Arc::new(AtomicBool::new(false)),
            snapshot_sync_direction: SharedSyncDirection::default(),
            send_pace_us: Arc::new(std::sync::atomic::AtomicU64::new(prefs.send_pace_us)),
            progress: ProgressBars::new(),
            progress_ui_in: RecallProgressUi::default(),
            progress_ui_out: RecallProgressUi::default(),
            transport_cue_cache: std::cell::RefCell::new((None, false)),
            snapshot_engine: None,
            macro_engine: None,
            pending_engines: Arc::new(std::sync::Mutex::new(None)),
            dirty_tracker: Arc::new(RwLock::new(DirtyTracker::new())),
            last_received: Arc::new(RwLock::new(None)),
            stream_deck_config: Arc::new(RwLock::new(
                crate::model::streamdeck::StreamDeckConfig::default(),
            )),
            stream_deck_engine: crate::console::streamdeck_engine::StreamDeckEngine::new(
                ui_tx.clone(),
            ),

            osc_log: OscLog::new(),

            runtime,
            egui_ctx: Arc::new(std::sync::OnceLock::new()),
            ui_rx,
            ui_tx,

            connected: Arc::new(AtomicBool::new(false)),
            sender: None,
            ipad_sender: None,
            cancel_token: None,

            active_tab: Tab::Setup,
            setup,
            snapshots: SnapshotsTabState::default(),
            macros: MacrosTabState::default(),
            palettes_ui: PalettesUiState::default(),
            gangs: GangsTabState::default(),
            pan_link: PanLinkTabState::default(),
            monitor: MonitorTabState::default(),
            osc_log_tab: OscLogTabState::default(),
            inspector: InspectorTabState::default(),
            capture_confirm: None,
            show_cue_list_popup: false,
            confirm_close: false,
            close_confirmed: false,

            last_autosave_at: std::time::Instant::now(),
            last_autosaved_fingerprint: 0,
            last_seen_generation: 0,
            generation_changed_at: std::time::Instant::now(),
            autosave_in_flight: Arc::new(AtomicBool::new(false)),
            recovery_dialog: None,
            monitors: crate::platform::enumerate(),
            last_monitor_px: None,
            window_sized: false,
            startup_frames: 0,
            scale_settled: false,
            settled_frames: 0,
            last_seen_geometry: None,
            last_saved_geometry: None,
            geometry_dirty_at: None,
        }
    }

    /// Apply global physical-size UI scaling. Called at the top of `update`,
    /// before any panel is built. The new ppp takes effect on the next pass;
    /// egui keeps physical px constant across the change, so this is a stable
    /// fixed point (no oscillation).
    fn apply_auto_scale(&mut self, ctx: &egui::Context) {
        let logical = ctx.content_rect();
        // Effective ppp egui currently renders at = zoom_factor × native_ppp.
        let ppp = ctx.pixels_per_point();
        // OS scale factor. egui-winit always reports this as the window's
        // scale_factor (2.0 on a 4K@200% panel), seeded before the first frame,
        // so it is reliable here. Used only as the fallback when the real PPI is
        // unknown — we target an absolute ppp and let `set_pixels_per_point` do
        // the native division itself, so there is no cross-frame native mismatch.
        let native_ppp = ctx.native_pixels_per_point().unwrap_or(1.0);
        // Real window size in physical px — invariant under our own scale changes.
        let phys_w = logical.width() * ppp;
        let phys_h = logical.height() * ppp;
        if !(phys_w.is_finite() && phys_h.is_finite())
            || phys_w < MIN_PHYS_PX
            || phys_h < MIN_PHYS_PX
            || phys_w > MAX_PHYS_PX
            || phys_h > MAX_PHYS_PX
        {
            return;
        }

        // Physical resolution of the monitor the window is on. egui-winit divides
        // monitor_size by the *effective* ppp, so multiplying back by `ppp`
        // recovers physical px. This cancels exactly in steady state; during the
        // startup zoom transient `monitor_size` lags our zoom by one pass, so the
        // product is briefly off (self-corrects within a couple frames, which is
        // why window sizing waits for `SETTLE_HOLD_FRAMES`).
        let monitor_px = ctx
            .input(|i| i.viewport().monitor_size)
            .map(|m| ((m.x * ppp).round() as u32, (m.y * ppp).round() as u32));
        let ppi = self.current_ppi(monitor_px);

        let mut target = compute_target_ppp(native_ppp, ppi, self.setup.ui_scale);
        target = (target / SCALE_STEP).round() * SCALE_STEP; // snap to step

        // Converged once the rendered ppp matches the target within the dead-band.
        // Track *consecutive* converged frames so `size_window_to_monitor` can
        // wait for egui's `monitor_size` (which lags our zoom by a pass) to catch
        // up to the settled ppp before the one-shot resize.
        self.scale_settled = (target - ppp).abs() <= SCALE_EPS;
        self.settled_frames = if self.scale_settled {
            self.settled_frames.saturating_add(1)
        } else {
            0
        };
        if !self.scale_settled {
            // Target an absolute ppp; egui converts to a zoom factor using its own
            // native value (no cross-frame native mismatch / doubling). Not
            // persisted — eframe's `persistence` feature is off in this build, so
            // each launch starts at zoom 1.0 and re-derives from ui_scale + PPI.
            ctx.set_pixels_per_point(target);
        }
    }

    /// One-shot startup sizing: restore the operator's last saved window
    /// geometry (clamped to the current monitor), or — on first run — fit to
    /// ~90% of the monitor centred with headroom for the title bar and desktop
    /// panels, then reveal the (initially hidden) window. Windowed, not
    /// maximized — the user avoids macOS full-screen-space behavior and desktop
    /// battles with QLab video out. Falls back to revealing at the builder's
    /// fallback size if the monitor size never arrives.
    fn size_window_to_monitor(&mut self, ctx: &egui::Context) {
        if self.window_sized {
            return;
        }
        self.startup_frames = self.startup_frames.saturating_add(1);

        // Don't size the window until the scale has converged AND held for a few
        // frames: monitor_size is reported in logical points (physical ÷ effective
        // ppp), and InnerSize is re-multiplied by the effective ppp when applied.
        // egui's monitor_size lags our zoom by one pass, so on the *first* settled
        // frame it is still the pre-zoom value while ppp has already moved — sizing
        // then yields a window at the wrong fraction of the screen (e.g. ~71%
        // instead of 95%, off-center). Waiting `SETTLE_HOLD_FRAMES` lets
        // monitor_size catch up to the settled ppp so the round-trip cancels. The
        // 10-frame escape hatch still guarantees we never stay hidden forever.
        if self.settled_frames < SETTLE_HOLD_FRAMES && self.startup_frames < 10 {
            return;
        }

        match ctx.input(|i| i.viewport().monitor_size) {
            Some(mon) => {
                let min = egui::vec2(900.0, 520.0);
                match (self.setup.window_size, self.setup.window_pos) {
                    // Restore the operator's last geometry, clamped to the
                    // current monitor so a changed display can't reopen the
                    // window off-screen.
                    (Some(size), Some(pos)) => {
                        let size = egui::vec2(size[0], size[1]).max(min).min(mon);
                        let pos = egui::pos2(
                            pos[0].clamp(0.0, (mon.x - size.x).max(0.0)),
                            pos[1].clamp(0.0, (mon.y - size.y).max(0.0)),
                        );
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
                    }
                    // First run: fit on-screen with headroom for the title bar
                    // and desktop panels. Centring by content size alone
                    // overflows the bottom-right; 0.90 leaves an even margin.
                    // Later runs restore the saved geometry above.
                    _ => {
                        let inner = (mon * 0.90).max(min);
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(inner));
                        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                            ((mon - inner) * 0.5).to_pos2(),
                        ));
                    }
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                self.window_sized = true;
            }
            // Monitor size not reported yet — wait a few frames, then reveal at
            // the fallback size rather than staying hidden forever.
            None if self.startup_frames >= 10 => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                self.window_sized = true;
            }
            None => {}
        }
    }

    /// Capture the live window geometry each frame and persist it (debounced)
    /// so the window reopens where the operator left it. Writes ~1 s after the
    /// last resize/move, only when the geometry actually differs from what's on
    /// disk — minimal file churn, no dependence on an eframe save/exit hook.
    fn track_window_geometry(&mut self, ctx: &egui::Context) {
        // Don't capture until the one-shot startup sizing has applied — the
        // first frames report the hidden fallback geometry.
        if !self.window_sized {
            return;
        }
        let (inner, outer) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().outer_rect));
        let (Some(inner), Some(outer)) = (inner, outer) else {
            return;
        };
        let size = inner.size();
        let pos = outer.min;
        // Skip degenerate / minimised frames.
        if !(size.x.is_finite() && size.y.is_finite() && pos.x.is_finite() && pos.y.is_finite())
            || size.x < 200.0
            || size.y < 200.0
        {
            return;
        }
        let geom = ([size.x, size.y], [pos.x, pos.y]);

        // Mirror into setup so `save_app_preferences` serializes it.
        self.setup.window_size = Some(geom.0);
        self.setup.window_pos = Some(geom.1);

        let now = ctx.input(|i| i.time);
        let geom_eq = |a: &([f32; 2], [f32; 2]), b: &([f32; 2], [f32; 2])| {
            let near = |x: f32, y: f32| (x - y).abs() < 0.5;
            near(a.0[0], b.0[0])
                && near(a.0[1], b.0[1])
                && near(a.1[0], b.1[0])
                && near(a.1[1], b.1[1])
        };

        // Still moving/resizing this frame → reset the debounce timer.
        if self.last_seen_geometry.is_none_or(|g| !geom_eq(&g, &geom)) {
            self.last_seen_geometry = Some(geom);
            self.geometry_dirty_at = Some(now);
            return;
        }
        // Stable this frame: persist once it has held for the debounce window
        // and differs from what's on disk.
        if let Some(t) = self.geometry_dirty_at {
            if now - t >= GEOMETRY_SAVE_DEBOUNCE {
                if self.last_saved_geometry.is_none_or(|g| !geom_eq(&g, &geom)) {
                    super::setup_tab::save_app_preferences(&self.setup);
                    self.last_saved_geometry = Some(geom);
                }
                self.geometry_dirty_at = None;
            }
        }
    }

    /// Real horizontal PPI of the monitor whose physical resolution is
    /// `monitor_px`, from a cache of [`crate::platform::enumerate`] refreshed
    /// only when the monitor changes. Returns `None` when physical metrics are
    /// unavailable, so the scaler falls back to respecting the OS scale factor.
    fn current_ppi(&mut self, monitor_px: Option<(u32, u32)>) -> Option<f32> {
        let monitor_px = monitor_px?;
        if self.last_monitor_px != Some(monitor_px) {
            self.monitors = crate::platform::enumerate();
            self.last_monitor_px = Some(monitor_px);
        }
        match self.monitors.as_slice() {
            [] => None,
            // Single display: use it regardless of an exact resolution match —
            // robust for laptops / Pi touchscreens and backends that report the
            // panel's native rather than its current mode.
            [only] => Some(only.ppi).filter(|p| *p > 0.0),
            // Multiple displays: match by physical resolution (±2 px rounding).
            many => {
                let close = |a: u32, b: u32| a.abs_diff(b) <= 2;
                many.iter()
                    .find(|m| close(m.px_w, monitor_px.0) && close(m.px_h, monitor_px.1))
                    .map(|m| m.ppi)
                    .filter(|p| *p > 0.0)
            }
        }
    }

    /// Draw the post-capture confirmation popup, if one is active. Centered,
    /// auto-fades after ~10 s, or dismisses early once the console sends ≥3
    /// parameter updates (operator back at the desk) — both skipped once the
    /// operator clicks or scrolls it (which pins it open). The × dismisses it;
    /// "Reload snapshot to verify" re-recalls the snapshot (force-send) so the
    /// operator can watch the surface confirm.
    fn draw_capture_confirm(&mut self, ctx: &egui::Context) {
        use std::time::Duration;
        const VISIBLE: Duration = Duration::from_secs(8);
        const FADE: Duration = Duration::from_secs(2);
        /// Inbound console parameter updates after which the popup self-dismisses.
        const DISMISS_AFTER_UPDATES: u64 = 3;

        // Take ownership for the frame so we can render without borrowing self;
        // we put it back at the end unless it should close.
        let Some(mut cc) = self.capture_confirm.take() else {
            return;
        };

        // Auto-dismiss once the console reports activity (≥3 parameter updates
        // since the popup appeared), unless the operator has pinned it. Uses
        // the live mirror's monotonic generation counter as the update count.
        if !cc.pinned {
            if let Ok(state) = self.state.try_read() {
                let gen_now = state.generation();
                match cc.seen_gen_base {
                    None => cc.seen_gen_base = Some(gen_now),
                    Some(base) => {
                        if gen_now.saturating_sub(base) >= DISMISS_AFTER_UPDATES {
                            return; // drop `cc` → closed
                        }
                    }
                }
            }
        }

        let alpha = if cc.pinned {
            1.0
        } else {
            let e = cc.started_at.elapsed();
            if e < VISIBLE {
                1.0
            } else if e < VISIBLE + FADE {
                1.0 - (e - VISIBLE).as_secs_f32() / FADE.as_secs_f32()
            } else {
                // Fully faded — drop `cc` (leaves capture_confirm None) and close.
                return;
            }
        };

        let mut dismiss = false;
        let mut reload = false;

        let area = egui::Area::new(egui::Id::new("capture_confirm_popup"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(440.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "Captured ‘{}’ ({} params)",
                                cc.name,
                                cc.params.len()
                            ))
                            .strong(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .button("×")
                                .on_hover_text(help(HelpKey::Dismiss))
                                .clicked()
                            {
                                dismiss = true;
                            }
                        });
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .max_height(360.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            egui::Grid::new("capture_confirm_params")
                                .num_columns(2)
                                .striped(true)
                                .show(ui, |ui| {
                                    for (label, value) in &cc.params {
                                        ui.label(label);
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| ui.monospace(value),
                                        );
                                        ui.end_row();
                                    }
                                });
                        });
                    ui.separator();
                    if ui
                        .add_sized(
                            [ui.available_width(), 28.0],
                            egui::Button::new("Reload snapshot to verify"),
                        )
                        .on_hover_text(help(HelpKey::SnapshotReRecall))
                        .clicked()
                    {
                        reload = true;
                    }
                });
            });

        // A click or scroll inside the popup pins it open (cancels the fade).
        // Mere hover does not.
        let hovered = area.response.contains_pointer();
        let activity = ctx.input(|i| i.pointer.any_pressed() || i.smooth_scroll_delta.y != 0.0);
        if hovered && activity {
            cc.pinned = true;
        }

        if dismiss {
            return; // drop `cc` → closed
        }
        if reload {
            cc.pinned = true;
            super::snapshots_tab::recall_snapshot_by_id(
                cc.snapshot_id,
                &self.cue_manager,
                &self.palette_manager,
                &self.snapshot_engine,
                &self.runtime,
                &self.ui_tx,
                false,
            );
        }
        // Keep showing.
        self.capture_confirm = Some(cc);
    }

    /// Move any engine handles produced by the connect-console task into the
    /// per-tab fields. Called each frame before draining UI events so that a
    /// `ConnectionEstablished` event arriving in the same frame finds the
    /// engines already in place.
    fn pickup_pending_engines(&mut self) {
        let pending = self
            .pending_engines
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(p) = pending {
            self.sender = Some(p.sender);
            self.snapshot_engine = Some(p.snapshot_engine);
            self.macro_engine = Some(p.macro_engine);
            if p.ipad_sender.is_some() {
                self.ipad_sender = p.ipad_sender;
            }
        }
    }

    /// Draw the two thin progress lines under the tab bar — inbound (the
    /// console dump flood) in green on top, outbound (snapshot/cue/macro recall
    /// sends) in blue below — matching the OSC log's In/Out color convention.
    /// Each bar eases its drawn fraction and fades out on completion so it reads
    /// smoothly rather than snapping; either can animate independently.
    fn draw_recall_progress(&mut self, ctx: &egui::Context, area: egui::Rect) {
        let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.1);
        let in_view = self.progress.inbound.snapshot();
        let out_view = self.progress.outbound.snapshot();

        // Paint on the BACKGROUND layer (where panels render), not a foreground
        // layer: that keeps the bars on top of the tab content but BELOW
        // floating windows (`Order::Middle`) — otherwise they cover popups like
        // the scope editor. This is called AFTER the CentralPanel renders, so
        // the bar shapes are appended on top of its content within the same
        // background layer. `area` is the content rect captured before the
        // CentralPanel consumed it. Reserves no layout space → no reflow.
        let painter = ctx.layer_painter(egui::LayerId::background());

        // Inbound (green) sits at the very top edge, outbound (blue) 3px below.
        let in_anim = Self::draw_one_progress_bar(
            &painter,
            area,
            in_view,
            &mut self.progress_ui_in,
            dt,
            super::theme::ACCENT_GREEN,
            0.0,
        );
        let out_anim = Self::draw_one_progress_bar(
            &painter,
            area,
            out_view,
            &mut self.progress_ui_out,
            dt,
            super::theme::ACCENT_BLUE,
            3.0,
        );

        // Keep repainting while either bar is animating so the ease/fade runs
        // even when nothing else requests frames.
        if in_anim || out_anim {
            ctx.request_repaint();
        }
    }

    /// Advance one progress bar's easing/fade and draw it (in `color`) if it is
    /// active or still fading. Returns whether the bar needs further animation
    /// frames. Associated fn (no `&self`) so the two bars can borrow their own
    /// easing state mutably without aliasing.
    ///
    /// Painted via the supplied background-layer `painter` (no `TopBottomPanel`),
    /// so it consumes no layout space: whether 0, 1, or 2 bars are visible, the
    /// CentralPanel never reflows. `y_offset` stacks the bars (0 for the top
    /// one, 3 for the one below) relative to `area.top()`.
    fn draw_one_progress_bar(
        painter: &egui::Painter,
        area: egui::Rect,
        view: crate::model::recall_progress::RecallProgressView,
        st: &mut RecallProgressUi,
        dt: f32,
        color: egui::Color32,
        y_offset: f32,
    ) -> bool {
        // A new op (generation bumped) resets the easing/fade.
        if view.generation != st.last_generation {
            st.last_generation = view.generation;
            st.smoothed = 0.0;
            st.fade = 1.0;
            st.sweep = 0.0;
        }

        // Target fraction. Time mode (a timed cue recall) fills over its
        // max(pre_wait+fade) window — recomputed live so an operator override
        // that shrinks the window advances the bar toward the nearer finish.
        // Otherwise: determinate when a total is known, else a looping sweep.
        use crate::model::recall_progress::ProgressMode;
        let target = if view.mode == ProgressMode::Time {
            let raw = view.time_fraction();
            if view.active { raw.min(0.99) } else { 1.0 }
        } else if view.total > 0 {
            let raw = view.done as f32 / view.total as f32;
            // Hold just shy of full while active so the plateau-snap to 100%
            // reads as completion; allow full once the op has finished.
            if view.active { raw.min(0.99) } else { 1.0 }
        } else if view.active {
            st.sweep = (st.sweep + dt * 0.6) % 1.0;
            st.sweep
        } else {
            1.0
        };

        if !view.active && target >= 1.0 {
            st.smoothed = 1.0; // snap to full on completion
        } else {
            st.smoothed += (target - st.smoothed) * 0.25; // ease
        }

        // Fade out once the op is done.
        if !view.active {
            st.fade -= dt / 0.4;
        }

        // Don't draw during a determinate op's pre-stream handshake (active,
        // known total, nothing received yet) — that's the multi-second gap
        // before the console starts flooding on connect — nor when fully idle.
        let pre_stream = view.active && view.total > 0 && view.done == 0;
        if pre_stream || (!view.active && st.fade <= 0.0) {
            return false;
        }

        let frac = st.smoothed.clamp(0.0, 1.0);
        let alpha = st.fade.clamp(0.0, 1.0);

        // Paint at the top of the content area. No layout allocation → no
        // reflow. The bar spans the full content width at `area.top() + offset`.
        let top = area.top() + y_offset;
        let track = egui::Rect::from_min_max(
            egui::pos2(area.left(), top),
            egui::pos2(area.right(), top + 3.0),
        );
        // Subtle track, then the accent fill — both alpha-modulated so the whole
        // bar fades out together on completion.
        painter.rect_filled(
            track,
            0.0,
            super::theme::bg_panel().gamma_multiply(alpha * 0.5),
        );
        let mut fill = track;
        fill.set_width(track.width() * frac);
        painter.rect_filled(fill, 0.0, color.gamma_multiply(alpha));

        true
    }

    /// Process UI events from async tasks.
    fn drain_events(&mut self) {
        self.pickup_pending_engines();
        while let Ok(event) = self.ui_rx.try_recv() {
            match event {
                UiEvent::ConnectionEstablished => {
                    self.connected.store(true, Ordering::Relaxed);
                    self.setup.status_message = Some("Connected to console".into());
                }
                UiEvent::ConnectionFailed(msg) => {
                    self.connected.store(false, Ordering::Relaxed);
                    self.setup.status_message = Some(StatusMessage::with_help(
                        format!("Connection failed: {msg}"),
                        HelpKey::SetupWarnConnectionFailed,
                    ));
                }
                UiEvent::Disconnected => {
                    self.connected.store(false, Ordering::Relaxed);
                    self.sender = None;
                    self.ipad_sender = None;
                    self.snapshot_engine = None;
                    self.macro_engine = None;
                    self.cancel_token = None;
                    if let Ok(mut slot) = self.pending_engines.lock() {
                        slot.take();
                    }
                    self.setup.ipad_connected = false;
                    self.setup.status_message = Some("Disconnected".into());
                    self.monitor.monitor_server_running = false;
                    self.monitor.web_server_running = false;
                }
                UiEvent::SnapshotCaptured { name, param_count } => {
                    self.snapshots.status_message =
                        Some(format!("Captured '{name}' ({param_count} params)").into());
                }
                UiEvent::SnapshotCaptureConfirm {
                    snapshot_id,
                    name,
                    params,
                } => {
                    self.snapshots.status_message =
                        Some(format!("Captured '{name}' ({} params)", params.len()).into());
                    self.capture_confirm = Some(CaptureConfirm {
                        snapshot_id,
                        name,
                        params,
                        started_at: std::time::Instant::now(),
                        pinned: false,
                        seen_gen_base: None,
                    });
                }
                UiEvent::SnapshotRecalled { name, params_sent } => {
                    self.snapshots.status_message =
                        Some(format!("Recalled '{name}' ({params_sent} params sent)").into());
                }
                UiEvent::CueRecalled { .. } => {
                    // The Live tab used to surface a "Cue X.Y recalled
                    // (N params)" line here; now that the transport
                    // lives in the top bar there's no place to print
                    // that, so the event is just consumed for any
                    // downstream listeners (logs, etc.).
                }
                UiEvent::MacroExecuted {
                    name,
                    steps_executed,
                    steps_skipped,
                } => {
                    let suffix = if steps_skipped > 0 {
                        format!(", {steps_skipped} skipped")
                    } else {
                        String::new()
                    };
                    self.macros.last_execution_info =
                        Some(format!("Executed '{name}' ({steps_executed} sent{suffix})"));
                }
                UiEvent::MacroExecutionFailed(msg) => {
                    self.macros.status_message = Some(format!("Run failed: {msg}").into());
                }
                UiEvent::MacroRecordingStopped { step_count } => {
                    self.macros.status_message =
                        Some(format!("Recording stopped: {step_count} steps captured").into());
                }
                UiEvent::PaletteCaptured { name, param_count } => {
                    self.palettes_ui.status_message =
                        Some(format!("Captured palette '{name}' ({param_count} EQ params)").into());
                }
                UiEvent::PaletteLinked {
                    palette_name,
                    snapshot_name,
                } => {
                    self.palettes_ui.status_message =
                        Some(format!("Linked '{palette_name}' to '{snapshot_name}'").into());
                }
                UiEvent::PaletteUpdated {
                    name,
                    affected_count,
                } => {
                    self.palettes_ui.status_message = Some(
                        format!("Updated '{name}' — {affected_count} snapshots affected").into(),
                    );
                }
                UiEvent::PaletteRevert { palette_id } => {
                    // Drop the palette's in-session overlay, stop the absorb loop
                    // re-capturing those params, and reload the last-captured
                    // values onto the desk.
                    let pmgr = self.palette_manager.clone();
                    let dirty = self.dirty_tracker.clone();
                    let engine = self.snapshot_engine.clone();
                    self.runtime.spawn(async move {
                        let palette = {
                            let mut mgr = pmgr.write().await;
                            let Some(p) = mgr.get_palette_mut(&palette_id) else {
                                return;
                            };
                            p.discard_working();
                            p.clone()
                        };
                        {
                            let mut t = dirty.write().await;
                            for kind in palette.kinds() {
                                t.clear_channel_section(&palette.channel, kind.section());
                            }
                        }
                        if let Some(engine) = engine {
                            engine.reload_palette(&palette).await;
                        }
                    });
                }
                UiEvent::ShowFileLoaded(path, conn, recall) => {
                    self.setup.status_message = Some(format!("Loaded: {path}").into());
                    // If this load resolved a recovery, flag it so the dialog
                    // can offer to repair the original path.
                    if let Some(rd) = &mut self.recovery_dialog {
                        rd.recovered = true;
                    }
                    self.snapshots.scope_editor.console_recall = recall;
                    if let Some(c) = &conn {
                        self.auto_update_on_recall
                            .store(c.auto_update_on_recall, Ordering::Relaxed);
                        self.snapshot_sync_direction
                            .set(c.effective_sync_direction());
                    }
                    if let Some(conn) = conn {
                        if !conn.local_ip.is_empty() {
                            self.setup.local_ip = conn.local_ip;
                        }
                        if !conn.console_ip.is_empty() {
                            self.setup.console_ip = conn.console_ip;
                        }
                        if conn.console_gp_port > 0 {
                            self.setup.console_port = conn.console_gp_port.to_string();
                        }
                        if conn.local_gp_port > 0 {
                            self.setup.local_port = conn.local_gp_port.to_string();
                        }
                        if conn.trigger_port > 0 {
                            self.setup.trigger_port = conn.trigger_port.to_string();
                        }
                        self.setup.operating_mode = conn.operating_mode;
                        if !conn.ipad_ip.is_empty() {
                            self.setup.ipad_ip = conn.ipad_ip;
                        }
                        if conn.ipad_send_port > 0 {
                            self.setup.ipad_console_port = conn.ipad_send_port.to_string();
                        }
                        if conn.ipad_receive_port > 0 {
                            self.setup.ipad_local_port = conn.ipad_receive_port.to_string();
                        }
                        if conn.ipad_listen_port > 0 {
                            self.setup.ipad_listen_port = conn.ipad_listen_port.to_string();
                        }
                        if conn.ipad_reply_port > 0 {
                            self.setup.ipad_reply_port = conn.ipad_reply_port.to_string();
                        }
                        if conn.monitor_port > 0 {
                            self.setup.monitor_port = conn.monitor_port.to_string();
                        }
                        if !conn.qlab_ip.is_empty() {
                            self.setup.qlab_ip = conn.qlab_ip;
                        }
                        if conn.qlab_port > 0 {
                            self.setup.qlab_port = conn.qlab_port.to_string();
                        }
                        if conn.qlab_patch_app > 0 {
                            self.setup.qlab_patch_app = conn.qlab_patch_app.to_string();
                        }
                        if conn.qlab_patch_console > 0 {
                            self.setup.qlab_patch_console = conn.qlab_patch_console.to_string();
                        }
                        // One-time migration: pacing used to live on
                        // the show file. If the user opens an old show
                        // and prefs has no pacing yet, adopt the show's
                        // value as the new app-wide default. After this
                        // first migration, prefs is authoritative and
                        // `conn.send_pace_us` is ignored on load.
                        let current_pace =
                            self.send_pace_us.load(std::sync::atomic::Ordering::Relaxed);
                        if current_pace == 0 && conn.send_pace_us != 0 {
                            self.send_pace_us
                                .store(conn.send_pace_us, std::sync::atomic::Ordering::Relaxed);
                        }
                        let pace_to_save =
                            self.send_pace_us.load(std::sync::atomic::Ordering::Relaxed);
                        self.setup.send_pace_us = pace_to_save;
                        self.setup.monitor_allow_cidrs = conn.monitor_allow_cidrs;
                        self.setup.trigger_allow_cidrs = conn.trigger_allow_cidrs;
                        self.setup.web_port = if conn.web_port > 0 {
                            conn.web_port.to_string()
                        } else {
                            String::new()
                        };
                        self.setup.web_allow_cidrs = conn.web_allow_cidrs;
                        self.setup.ui_mode = conn.ui_mode;
                        // Mirror the loaded mode to app preferences so a later
                        // launch (without a show file) starts in this mode.
                        let prefs = AppPreferences {
                            ui_mode: Some(self.setup.ui_mode),
                            show_diagnostics: self.setup.show_diagnostics,
                            send_pace_us: pace_to_save,
                            ui_scale: self.setup.ui_scale,
                            color_theme: self.setup.color_theme,
                            help_language: self.setup.help_language.clone(),
                            window_size: self.setup.window_size,
                            window_pos: self.setup.window_pos,
                            last_open_dir: self.setup.last_open_dir.clone(),
                        };
                        if let Err(e) = prefs.save() {
                            tracing::warn!(error = %e, "Failed to save app preferences after show load");
                        }
                    }
                }
                UiEvent::ShowFileSaved(path) => {
                    self.setup.status_message = Some(format!("Saved: {path}").into());
                }
                UiEvent::ShowFileError(msg) => {
                    self.setup.status_message = Some(StatusMessage::with_help(
                        msg,
                        HelpKey::SetupWarnShowFileError,
                    ));
                }
                UiEvent::AutosaveCompleted { fingerprint, wrote } => {
                    self.last_autosaved_fingerprint = fingerprint;
                    self.autosave_in_flight.store(false, Ordering::Relaxed);
                    if wrote {
                        self.setup.status_message = Some("Autosaved".into());
                    }
                }
                UiEvent::ShowFileCorrupt { path, candidates } => {
                    self.setup.status_message = Some(StatusMessage::with_help(
                        "Show file appears corrupt — choose a recovery candidate",
                        HelpKey::SetupWarnShowCorrupt,
                    ));
                    self.recovery_dialog = Some(RecoveryDialog {
                        original_path: path,
                        candidates,
                        recovered: false,
                    });
                }
                UiEvent::IpadConnected => {
                    self.setup.ipad_connected = true;
                    self.setup.status_message = Some("iPad protocol connected".into());
                }
                UiEvent::IpadConnectionFailed(msg) => {
                    self.setup.ipad_connected = false;
                    self.setup.status_message = Some(StatusMessage::with_help(
                        format!("iPad connection failed: {msg}"),
                        HelpKey::SetupWarnIpadConnFailed,
                    ));
                }
                UiEvent::FadeProgress { .. } => {
                    // Fade-progress display lived on the old Live tab.
                    // Drop it for now — could be re-surfaced as a thin
                    // overlay under the top-bar transport later if
                    // operators want it back.
                }
                UiEvent::MonitorClientConnected { name } => {
                    self.monitor.status_message = Some(format!("Client '{name}' connected").into());
                }
                UiEvent::MonitorClientDisconnected { name } => {
                    self.monitor.status_message =
                        Some(format!("Client '{name}' disconnected").into());
                }
                UiEvent::MonitorServerStarted => {
                    self.monitor.monitor_server_running = true;
                    self.setup.status_message = Some("Monitor server started".into());
                }
                UiEvent::MonitorServerFailed(msg) => {
                    self.monitor.monitor_server_running = false;
                    self.setup.status_message = Some(StatusMessage::with_help(
                        format!("Monitor server failed: {msg}"),
                        HelpKey::SetupWarnMonitorServerFailed,
                    ));
                }
                UiEvent::WebServerStarted => {
                    self.monitor.web_server_running = true;
                    self.setup.status_message = Some("Web monitor server started".into());
                }
                UiEvent::WebServerFailed(msg) => {
                    self.monitor.web_server_running = false;
                    self.setup.status_message = Some(StatusMessage::with_help(
                        format!("Web server failed: {msg}"),
                        HelpKey::SetupWarnWebServerFailed,
                    ));
                }
                // ── Macro-emitted app-internal commands ─────────────
                UiEvent::MacroFireGo => {
                    super::cue_transport::fire_go(
                        &self.cue_manager,
                        &self.palette_manager,
                        &self.snapshot_engine,
                        &self.runtime,
                        &self.ui_tx,
                    );
                }
                UiEvent::MacroFirePrev => {
                    super::cue_transport::fire_prev(
                        &self.cue_manager,
                        &self.palette_manager,
                        &self.snapshot_engine,
                        &self.runtime,
                        &self.ui_tx,
                    );
                }
                UiEvent::MacroConnect if self.connected.load(Ordering::Relaxed) => {
                    // Already connected — ignore the macro-driven Connect so
                    // automation can't churn a live session (the on-screen
                    // button is likewise hidden while connected).
                    tracing::warn!("MacroConnect ignored — already connected");
                }
                UiEvent::MacroConnect => {
                    // Macro-driven Connect routes to the same
                    // start_connection helper the operator's Connect
                    // button uses, with the current Setup-tab state.
                    super::setup_tab::start_connection(
                        &mut self.setup,
                        &self.state,
                        &self.cue_manager,
                        &self.macro_manager,
                        &self.monitor_manager,
                        &self.palette_manager,
                        &self.gang_manager,
                        &self.pan_link_bindings,
                        &self.offline_mode,
                        &self.auto_update_on_recall,
                        &self.snapshot_sync_direction,
                        &self.dirty_tracker,
                        &self.last_received,
                        &self.pending_engines,
                        &self.connected,
                        &mut self.cancel_token,
                        &self.osc_log,
                        &self.send_pace_us,
                        &self.progress,
                        &self.runtime,
                        &self.ui_tx,
                        &self.egui_ctx,
                    );
                }
                UiEvent::MacroDisconnect => {
                    super::setup_tab::do_disconnect(
                        &self.connected,
                        &mut self.cancel_token,
                        &self.ui_tx,
                    );
                }
                UiEvent::MacroRecallSnapshot { snapshot_id } => {
                    let cue_mgr = self.cue_manager.clone();
                    let pmgr = self.palette_manager.clone();
                    let engine = self.snapshot_engine.clone();
                    let tx = self.ui_tx.clone();
                    self.runtime.spawn(async move {
                        let Some(engine) = engine else {
                            let _ = tx.send(UiEvent::MacroExecutionFailed(
                                "Macro: snapshot recall — engine not available".into(),
                            ));
                            return;
                        };
                        let mgr = cue_mgr.read().await;
                        let snapshot = mgr.snapshots.get(&snapshot_id).cloned();
                        drop(mgr);
                        let Some(snapshot) = snapshot else {
                            let _ = tx.send(UiEvent::MacroExecutionFailed(format!(
                                "Macro: snapshot recall — id {snapshot_id} not found"
                            )));
                            return;
                        };
                        // Build a synthetic Cue wrapping this
                        // snapshot — `recall_cue` is the path that
                        // already handles fades + dirty tracking +
                        // recall scope. Cue number `0.0` signals "no
                        // cue context" since we're bypassing the
                        // list.
                        let cue = crate::model::snapshot::Cue::new(
                            0.0,
                            format!("(macro) {}", snapshot.name),
                        )
                        .with_snapshot_id(snapshot_id);
                        // Clone palettes and drop the read guard before the
                        // recall await (avoids the palette<->state ABBA deadlock
                        // with the absorb loop; see palette_tracker.rs).
                        let palettes = pmgr.read().await.palettes.clone();
                        let _ = engine
                            .recall_cue(&cue, Some(&snapshot), &palettes, false)
                            .await;
                    });
                }
                UiEvent::MacroRecallPalette {
                    palette_id,
                    channel,
                } => {
                    // Palette apply path — surface a status message
                    // and defer the actual write to a runtime task
                    // that locates the palette and applies its
                    // values to the channel via the snapshot engine.
                    let pmgr = self.palette_manager.clone();
                    let engine = self.snapshot_engine.clone();
                    let tx = self.ui_tx.clone();
                    self.runtime.spawn(async move {
                        let Some(_engine) = engine else {
                            let _ = tx.send(UiEvent::MacroExecutionFailed(
                                "Macro: palette recall — engine not available".into(),
                            ));
                            return;
                        };
                        let mgr = pmgr.read().await;
                        let palette = mgr.palettes.get(&palette_id).cloned();
                        drop(mgr);
                        match palette {
                            Some(p) => {
                                let _ = tx.send(UiEvent::PaletteUpdated {
                                    name: p.name.clone(),
                                    affected_count: 1,
                                });
                                tracing::info!(
                                    palette = %p.name,
                                    %channel,
                                    "Macro: palette recall queued (apply not yet implemented)"
                                );
                            }
                            None => {
                                let _ = tx.send(UiEvent::MacroExecutionFailed(format!(
                                    "Macro: palette recall — id {palette_id} not found"
                                )));
                            }
                        }
                    });
                }
                UiEvent::MacroQLabSend {
                    addr,
                    string_arg,
                    label,
                } => {
                    let args = match string_arg {
                        Some(s) => vec![rosc::OscType::String(s)],
                        None => vec![],
                    };
                    self.spawn_qlab_send(addr, args, &label);
                }
                UiEvent::StreamDeckButtonPressed { button_idx } => {
                    self.handle_streamdeck_button(button_idx);
                }
                UiEvent::StreamDeckConnected {
                    device_name,
                    button_count,
                } => {
                    tracing::info!(
                        device = %device_name,
                        button_count,
                        "Stream Deck connected"
                    );
                    // No status_message — the green dot in the SD card
                    // is enough; a yellow banner duplicates the signal
                    // and clutters the column.
                    let labels = self.streamdeck_resize_and_collect_labels(button_count as usize);
                    self.stream_deck_engine.refresh_all(labels);
                }
                UiEvent::StreamDeckDisconnected => {
                    tracing::info!("Stream Deck disconnected");
                }
                UiEvent::StreamDeckError { message } => {
                    tracing::warn!("Stream Deck error: {message}");
                    self.macros.status_message = Some(format!("Stream Deck: {message}").into());
                }
            }
        }
    }

    /// Fire-and-forget OSC message to QLab using the current Setup-tab
    /// IP / port (falls back to 127.0.0.1:53000 if empty/unset). `label`
    /// is the human-readable action name used in failure messages.
    fn spawn_qlab_send(&self, addr: String, args: Vec<rosc::OscType>, label: &str) {
        let qlab_port: u16 = self.setup.qlab_port.parse().unwrap_or(53000);
        let qlab_ip = if self.setup.qlab_ip.is_empty() {
            "127.0.0.1".to_string()
        } else {
            self.setup.qlab_ip.clone()
        };
        let tx = self.ui_tx.clone();
        let label = label.to_string();
        self.runtime.spawn(async move {
            match crate::osc::qlab_client::QLabClient::new(&qlab_ip, qlab_port).await {
                Ok(client) => {
                    let msg = rosc::OscMessage { addr, args };
                    if let Err(e) = client.send_message(msg).await {
                        let _ = tx.send(UiEvent::MacroExecutionFailed(format!(
                            "{label} failed: {e}"
                        )));
                    }
                }
                Err(e) => {
                    let _ = tx.send(UiEvent::MacroExecutionFailed(format!(
                        "{label}: connect failed: {e}"
                    )));
                }
            }
        });
    }

    /// Handle a Stream Deck button press: fire the next-to-fire macro
    /// for that button, advance the cursor (with wrap-around), then
    /// refresh the LCD to show the now-next-to-fire macro's name.
    fn handle_streamdeck_button(&self, button_idx: usize) {
        let cfg = self.stream_deck_config.clone();
        let macro_mgr = self.macro_manager.clone();
        let macro_engine = self.macro_engine.clone();
        let sd_engine = self.stream_deck_engine.clone();
        let tx = self.ui_tx.clone();
        self.runtime.spawn(async move {
            // Take a snapshot of the macro_id to fire and advance the
            // cursor under a single write-lock.
            let macro_id_to_fire: Option<uuid::Uuid> = {
                let mut cfg_w = cfg.write().await;
                let Some(button) = cfg_w.buttons.get_mut(button_idx) else {
                    return;
                };
                if button.steps.is_empty() {
                    return;
                }
                let idx = (button.current_step as usize).min(button.steps.len() - 1);
                let macro_id = button.steps[idx].macro_id;
                button.advance();
                Some(macro_id)
            };
            let Some(macro_id) = macro_id_to_fire else {
                return;
            };
            // Fire the macro.
            if let Some(engine) = macro_engine {
                let mgr = macro_mgr.read().await;
                let macro_def = mgr.get_macro(&macro_id).cloned();
                drop(mgr);
                match macro_def {
                    Some(def) => {
                        engine.execute(&def).await;
                    }
                    None => {
                        let _ = tx.send(UiEvent::MacroExecutionFailed(format!(
                            "Stream Deck: macro {macro_id} no longer exists"
                        )));
                    }
                }
            } else {
                let _ = tx.send(UiEvent::MacroExecutionFailed(
                    "Stream Deck: macro engine not initialised — connect to console first".into(),
                ));
            }
            // Refresh the LCD with the new next-to-fire label + color.
            let (label, bg) = {
                let cfg_r = cfg.read().await;
                let mgr = macro_mgr.read().await;
                let next = cfg_r.buttons.get(button_idx).and_then(|b| b.next_step());
                let label = next
                    .map(|s| {
                        mgr.get_macro(&s.macro_id)
                            .map(|m| m.name.clone())
                            .unwrap_or_else(|| "(deleted)".into())
                    })
                    .unwrap_or_default();
                let bg = next
                    .map(|s| s.color)
                    .unwrap_or(crate::model::streamdeck::StepColor::BLACK);
                (label, bg)
            };
            sd_engine.refresh_button(button_idx as u8, label, bg);
        });
    }

    /// Grow the StreamDeck per-button vector up to `count` (never
    /// truncating — assignments past the connected device's button
    /// count persist in the show file so switching to a smaller device
    /// doesn't destroy them), then build the per-slot label + bg color
    /// list for the device's visible buttons.
    fn streamdeck_resize_and_collect_labels(
        &self,
        count: usize,
    ) -> Vec<(String, crate::model::streamdeck::StepColor)> {
        // Synchronously block on the runtime to read+write — this
        // runs on the UI thread but operations are quick (memory only).
        let cfg = self.stream_deck_config.clone();
        let macro_mgr = self.macro_manager.clone();
        self.runtime.block_on(async move {
            let mut cfg_w = cfg.write().await;
            if cfg_w.buttons.len() < count {
                cfg_w.buttons.resize_with(count, Default::default);
            }
            let mgr = macro_mgr.read().await;
            (0..count)
                .map(|i| {
                    let next = cfg_w.buttons.get(i).and_then(|b| b.next_step());
                    let label = next
                        .map(|s| {
                            mgr.get_macro(&s.macro_id)
                                .map(|m| m.name.clone())
                                .unwrap_or_else(|| "(deleted)".into())
                        })
                        .unwrap_or_default();
                    let bg = next
                        .map(|s| s.color)
                        .unwrap_or(crate::model::streamdeck::StepColor::BLACK);
                    (label, bg)
                })
                .collect()
        })
    }

    /// Take a periodic autosave when the session is quiet. Called once per
    /// frame. No-ops unless the show has a path, no recovery is pending, the
    /// console state has settled, no recall/cue burst is suppressing, and the
    /// autosave interval has elapsed. The actual build + write runs off-thread.
    fn maybe_autosave(&mut self) {
        use crate::persistence::backup;

        // Skip unnamed shows and while a recovery dialog is open (don't
        // autosave over an unrecovered corrupt session).
        if self.setup.show_file_path.is_empty() || self.recovery_dialog.is_some() {
            return;
        }
        let now = std::time::Instant::now();

        // Sample generation; a held lock means a write is in flight → not quiet.
        let generation = match self.state.try_read() {
            Ok(s) => s.generation(),
            Err(_) => return,
        };
        if generation != self.last_seen_generation {
            self.last_seen_generation = generation;
            self.generation_changed_at = now;
            return;
        }

        // Quiet gate: settled long enough AND nothing suppressing marks.
        if now.duration_since(self.generation_changed_at) < backup::QUIET_SETTLE {
            return;
        }
        let suppressed = self
            .dirty_tracker
            .try_read()
            .map(|d| d.is_suppressed())
            .unwrap_or(true);
        if suppressed {
            return;
        }

        // Interval + overlap gates.
        if now.duration_since(self.last_autosave_at) < backup::AUTOSAVE_INTERVAL {
            return;
        }
        if self.autosave_in_flight.load(Ordering::Relaxed) {
            return;
        }

        // All gates passed. Build connection settings on the UI thread, then
        // spawn the gather + write.
        let conn = super::setup_tab::connection_settings_from_setup(
            &self.setup,
            self.auto_update_on_recall.load(Ordering::Relaxed),
            self.snapshot_sync_direction.get(),
        );
        let console_recall = self.snapshots.scope_editor.console_recall.clone();
        let path = std::path::PathBuf::from(&self.setup.show_file_path);

        let st = self.state.clone();
        let cue_mgr = self.cue_manager.clone();
        let macro_mgr = self.macro_manager.clone();
        let mon_mgr = self.monitor_manager.clone();
        let pmgr = self.palette_manager.clone();
        let gang_mgr = self.gang_manager.clone();
        let pl = self.pan_link_bindings.clone();
        let sd = self.stream_deck_config.clone();
        let dirty = self.dirty_tracker.clone();
        let prev_fp = self.last_autosaved_fingerprint;
        let tx = self.ui_tx.clone();
        let in_flight = self.autosave_in_flight.clone();

        self.autosave_in_flight.store(true, Ordering::Relaxed);
        self.last_autosave_at = now;

        self.runtime.spawn(async move {
            // Re-check suppression now that we're scheduled — a recall may
            // have started in the gap. If so, abort without writing.
            if dirty.read().await.is_suppressed() {
                let _ = tx.send(UiEvent::AutosaveCompleted {
                    fingerprint: prev_fp,
                    wrote: false,
                });
                in_flight.store(false, Ordering::Relaxed);
                return;
            }

            let show = super::setup_tab::build_show_file(
                &st,
                &cue_mgr,
                &macro_mgr,
                &mon_mgr,
                &pmgr,
                &gang_mgr,
                &pl,
                &sd,
                conn,
                console_recall,
            )
            .await;

            let json = match serde_json::to_vec_pretty(&show) {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!(error = %e, "Autosave serialize failed");
                    let _ = tx.send(UiEvent::AutosaveCompleted {
                        fingerprint: prev_fp,
                        wrote: false,
                    });
                    in_flight.store(false, Ordering::Relaxed);
                    return;
                }
            };

            let fingerprint = {
                use std::hash::Hasher;
                let mut h = std::collections::hash_map::DefaultHasher::new();
                h.write(&json);
                h.finish()
            };

            let mut wrote = false;
            if fingerprint != prev_fp {
                match backup::write_and_rotate(&path, backup::BackupKind::Autosave, &json).await {
                    Ok(p) => {
                        tracing::info!(path = %p.display(), "Autosaved");
                        wrote = true;
                    }
                    Err(e) => tracing::warn!(error = %e, "Autosave write failed"),
                }
            }

            let _ = tx.send(UiEvent::AutosaveCompleted { fingerprint, wrote });
            in_flight.store(false, Ordering::Relaxed);
        });
    }

    /// Render the corruption-recovery modal, if open, and act on the
    /// operator's choice.
    fn draw_recovery_dialog(&mut self, ctx: &egui::Context) {
        if self.recovery_dialog.is_none() {
            return;
        }

        enum Action {
            None,
            Cancel,
            Load(std::path::PathBuf),
            SaveToOriginal,
        }
        let mut action = Action::None;

        {
            let rd = self.recovery_dialog.as_ref().unwrap();
            egui::Window::new("Recover show file")
                .collapsible(false)
                .resizable(true)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "“{}” could not be loaded (truncated or bad header).",
                            rd.original_path
                        ))
                        .strong(),
                    )
                    .on_hover_help_inline(HelpKey::RecoveryInfoCorrupt);
                    if rd.recovered {
                        ui.colored_label(
                            super::theme::ACCENT_GREEN,
                            "Recovered. You can re-save it to the original path below.",
                        );
                    }
                    ui.separator();

                    if rd.candidates.is_empty() {
                        ui.label("No backups or autosaves were found for this show.")
                            .on_hover_help_inline(HelpKey::RecoveryInfoNoBackups);
                    } else {
                        ui.label("Choose a copy to restore (newest first):")
                            .on_hover_help_inline(HelpKey::RecoveryInfoChoose);
                        egui::ScrollArea::vertical()
                            .max_height(260.0)
                            .show(ui, |ui| {
                                for cand in &rd.candidates {
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add_enabled(cand.valid, egui::Button::new("Load this"))
                                            .on_hover_text(help(HelpKey::RecoveryLoad))
                                            .on_disabled_hover_text(help(
                                                HelpKey::RecoveryLoadInvalid,
                                            ))
                                            .clicked()
                                        {
                                            action = Action::Load(cand.path.clone());
                                        }
                                        ui.label(cand.describe());
                                    });
                                }
                            });
                    }

                    ui.separator();
                    ui.horizontal(|ui| {
                        if rd.recovered
                            && ui
                                .button("Save recovered to original path")
                                .on_hover_text(help(HelpKey::RecoverySaveOriginal))
                                .clicked()
                        {
                            action = Action::SaveToOriginal;
                        }
                        if ui
                            .button("Cancel")
                            .on_hover_text(help(HelpKey::RecoveryCancel))
                            .clicked()
                        {
                            action = Action::Cancel;
                        }
                    });
                });
        }

        match action {
            Action::None => {}
            Action::Cancel => {
                self.recovery_dialog = None;
            }
            Action::Load(path) => {
                self.setup.show_file_path = path.display().to_string();
                super::setup_tab::load_show_file(
                    &mut self.setup,
                    &self.state,
                    &self.cue_manager,
                    &self.macro_manager,
                    &self.monitor_manager,
                    &self.palette_manager,
                    &self.gang_manager,
                    &self.pan_link_bindings,
                    &self.stream_deck_config,
                    &self.connected,
                    &self.runtime,
                    &self.ui_tx,
                );
            }
            Action::SaveToOriginal => {
                let orig = self
                    .recovery_dialog
                    .as_ref()
                    .map(|rd| rd.original_path.clone())
                    .unwrap_or_default();
                self.setup.show_file_path = orig;
                super::setup_tab::save_show_file(
                    &mut self.setup,
                    &self.state,
                    &self.cue_manager,
                    &self.macro_manager,
                    &self.monitor_manager,
                    &self.palette_manager,
                    &self.gang_manager,
                    &self.pan_link_bindings,
                    &self.stream_deck_config,
                    self.auto_update_on_recall.load(Ordering::Relaxed),
                    self.snapshot_sync_direction.get(),
                    self.snapshots.scope_editor.console_recall.clone(),
                    &self.runtime,
                    &self.ui_tx,
                );
                self.recovery_dialog = None;
            }
        }
    }

    /// Confirmation modal for closing the app while the console link is live.
    ///
    /// The close itself is intercepted in `update` (vetoed with
    /// `ViewportCommand::CancelClose`, `confirm_close` raised) only while
    /// `connected`, so a disconnected app still quits instantly. "Quit"
    /// re-issues the close — now allowed through by `close_confirmed` — and
    /// "Stay connected" simply dismisses the modal. The modal's backdrop blocks
    /// the rest of the UI until the operator chooses.
    fn draw_close_confirm(&mut self, ctx: &egui::Context) {
        if !self.confirm_close {
            return;
        }

        enum Action {
            None,
            Stay,
            Quit,
        }
        let mut action = Action::None;

        egui::Modal::new(egui::Id::new("close_confirm_modal")).show(ctx, |ui| {
            ui.set_min_width(380.0);
            ui.label(
                egui::RichText::new("Quit while connected?")
                    .strong()
                    .size(super::theme::FONT_SIZE_SECTION)
                    .color(super::theme::ACCENT_BLUE),
            );
            ui.add_space(6.0);
            ui.label(
                "The app is still connected to the console. Quitting now drops \
                 the link, so it will stop responding to triggers, macros and \
                 incoming OSC until you reconnect.",
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .add(super::theme::action_button(
                        "Quit",
                        super::theme::ACCENT_RED,
                        egui::Vec2::new(120.0, 28.0),
                    ))
                    .clicked()
                {
                    action = Action::Quit;
                }
                if ui
                    .add(super::theme::action_button(
                        "Stay connected",
                        super::theme::ACCENT_GREEN,
                        egui::Vec2::new(160.0, 28.0),
                    ))
                    .clicked()
                {
                    action = Action::Stay;
                }
            });
        });

        match action {
            Action::None => {}
            Action::Stay => {
                self.confirm_close = false;
            }
            Action::Quit => {
                self.confirm_close = false;
                self.close_confirmed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

impl eframe::App for HiJackApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Install embedded fonts (Noto Sans Regular primary + NotoSans symbol
        // fallbacks so Unicode arrows / symbols don't tofu) before the style
        // pass, and swap the CJK fallback to match the help-bubble language so
        // zh/ja/ko render with their region's glyph shapes. Self-gating: only
        // rebuilds when the CJK choice changes, so it's cheap every frame.
        super::fonts::install_fonts_for(ctx, &self.setup.help_language);

        // First-frame init: discover help-bubble translation files and publish
        // the saved language. Later changes are pushed by the Advanced Settings
        // dropdown via `help::set_active_language`.
        static LOCALES_LOADED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if LOCALES_LOADED.get().is_none() {
            super::help::load_locales();
            super::help::set_active_language(&self.setup.help_language);
            let _ = LOCALES_LOADED.set(());
        }

        // Global fit-to-window + physical-size UI scaling. Must run before any
        // panel is built; the new zoom applies on the next pass.
        self.apply_auto_scale(ctx);

        // First-run window sizing: restore the last saved geometry (or fit
        // on-screen on first run), then reveal the initially-hidden window.
        self.size_window_to_monitor(ctx);

        // Persist size/position (debounced) so the window reopens where the
        // operator left it.
        self.track_window_geometry(ctx);

        // Store context on first frame for async repaint
        let _ = self.egui_ctx.set(ctx.clone());

        // Configure style every frame so changing the colour theme in
        // Advanced Settings hot-switches the UI without a restart.
        super::theme::configure_style(ctx, self.setup.color_theme);

        // egui only repaints on UI events by default — so OSC-driven
        // state changes (faders moving on the desk, parameters arriving
        // from the daemon, "track latest OSC" in the Macros tab) would
        // only redraw when the operator nudges the mouse. Request a
        // 20 ms tick (~50 Hz) so every tab reflects live state without
        // each tab having to opt in. Cheap when nothing changed —
        // egui's diff is layout-only.
        ctx.request_repaint_after(std::time::Duration::from_millis(20));

        // Drain async events
        self.drain_events();

        // Intercept a window-close request while the console link is live so an
        // accidental close doesn't silently drop a running show. The first close
        // is vetoed and a confirmation modal is raised (see `draw_close_confirm`);
        // once the operator confirms (`close_confirmed`) — or when we're not
        // connected — the close proceeds normally.
        if ctx.input(|i| i.viewport().close_requested()) {
            let connected = self.connected.load(Ordering::Relaxed);
            if connected && !self.close_confirmed {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.confirm_close = true;
            }
        }

        // Auto-preselect: keep the working scope tracking changed parameters
        // even while the scope editor window is CLOSED, so a capture uses the
        // current changed-since-last-snapshot set instead of a value frozen the
        // last time the editor happened to be open. (When the window is open,
        // draw_scope_window already does this every frame the dirty generation
        // changes.)
        if !self.snapshots.scope_editor.window_open
            && self.snapshots.scope_editor.auto_preselect_modified
            && let Ok(dirty) = self.dirty_tracker.try_read()
        {
            let generation = dirty.generation();
            if generation != self.snapshots.scope_editor.last_dirty_generation {
                self.snapshots
                    .scope_editor
                    .apply_dirty_set(dirty.dirty_set());
                self.snapshots.scope_editor.last_dirty_generation = generation;
            }
        }

        // Periodic autosave during quiet periods (no-op most frames).
        self.maybe_autosave();

        // Tab bar
        egui::TopBottomPanel::top("tab_bar")
            .frame(
                egui::Frame::new()
                    .fill(super::theme::bg_dark())
                    .inner_margin(egui::Margin::symmetric(8, 4)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // App title
                    ui.label(
                        egui::RichText::new("S21 HiJack")
                            .strong()
                            .size(super::theme::FONT_SIZE_SECTION)
                            .color(super::theme::ACCENT_BLUE),
                    );
                    ui.add_space(16.0);

                    // Tab buttons — filtered by current display mode.
                    // Order: stable tabs first (always visible), then the
                    // two mode-toggleable tabs (Snapshots, Monitor), then
                    // the diagnostic tabs. Keeps Macros..Pan Link in
                    // fixed positions when modes change.
                    let all_tabs = [
                        (Tab::Setup, "Setup"),
                        (Tab::Macros, "Macros"),
                        (Tab::Gangs, "Gangs"),
                        (Tab::PanLink, "Pan Link"),
                        (Tab::Snapshots, "Snapshots"),
                        (Tab::Monitor, "Monitor"),
                        (Tab::OscLog, "OSC Log"),
                        (Tab::Inspector, "Inspector"),
                    ];
                    let mode = self.setup.ui_mode;
                    let diag = self.setup.show_diagnostics;
                    // If the active tab just got hidden by a mode change,
                    // fall back to Setup so the central panel stays valid.
                    if !mode.tab_visible(self.active_tab, diag) {
                        self.active_tab = Tab::Setup;
                    }
                    for (tab, label) in all_tabs
                        .into_iter()
                        .filter(|(t, _)| mode.tab_visible(*t, diag))
                    {
                        let is_active = self.active_tab == tab;
                        let fill = if is_active {
                            super::theme::ACCENT_BLUE
                        } else {
                            super::theme::btn_neutral()
                        };
                        let text_color = if is_active {
                            super::theme::TEXT_PRIMARY
                        } else {
                            super::theme::neutral_inactive_text()
                        };
                        let btn = egui::Button::new(
                            egui::RichText::new(label).color(text_color).strong(),
                        )
                        .fill(fill)
                        .corner_radius(4.0);
                        let tab_help = match tab {
                            Tab::Setup => HelpKey::TabSetup,
                            Tab::Macros => HelpKey::TabMacros,
                            Tab::Gangs => HelpKey::TabGangs,
                            Tab::PanLink => HelpKey::TabPanLink,
                            Tab::Snapshots => HelpKey::TabSnapshots,
                            Tab::Monitor => HelpKey::TabMonitor,
                            Tab::OscLog => HelpKey::TabOscLog,
                            Tab::Inspector => HelpKey::TabInspector,
                        };
                        if ui.add(btn).on_hover_text(help(tab_help)).clicked() {
                            self.active_tab = tab;
                        }
                    }

                    // Right-side status group + centred cue transport.
                    //
                    // Layout strategy: the outer right-to-left scope reserves
                    // all the space remaining after the tab buttons. The
                    // status group (Connected dot + Online toggle) is rendered
                    // first so it anchors to the right edge. The cue transport
                    // is then placed in the leftover horizontal slack via a
                    // nested left-to-right sub-region with symmetric padding,
                    // so the strip stays centred between the tabs and the
                    // Online toggle as the window resizes.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let is_connected =
                            self.connected.load(std::sync::atomic::Ordering::Relaxed);
                        // Reflect link *health*: green healthy, yellow
                        // "Connecting…" when a session is up but the console
                        // isn't currently reachable, red when no session.
                        // Non-blocking: never freeze the UI on the state lock
                        // (heavily written during the inbound dump). A rare
                        // miss falls back to "Connecting" for that frame.
                        let health = self
                            .state
                            .try_read()
                            .map(|s| s.health)
                            .unwrap_or(crate::model::state::ConnectionHealth::Connecting);
                        let (color, text) = super::theme::console_status(is_connected, health);
                        // Dot only (no text label) to reclaim top-bar width; the
                        // status wording is available on hover.
                        super::theme::status_dot(ui, color).on_hover_text(text);

                        ui.add_space(12.0);
                        // Offline mode toggle — freezes all OSC traffic both ways.
                        let mut is_offline = self.offline_mode.load(Ordering::Relaxed);
                        let label = if is_offline { "OFFLINE" } else { "Online" };
                        let fill = if is_offline {
                            super::theme::ACCENT_AMBER
                        } else {
                            super::theme::btn_neutral()
                        };
                        let text_color = if is_offline {
                            super::theme::TEXT_ON_BRIGHT
                        } else {
                            super::theme::neutral_inactive_text()
                        };
                        // Fixed min-size so the button doesn't change
                        // width between "Online" (6 chars) and "OFFLINE"
                        // (7 chars) — the rest of the top bar would
                        // otherwise jiggle every time it's toggled.
                        let btn = egui::Button::new(
                            egui::RichText::new(label).color(text_color).strong(),
                        )
                        .fill(fill)
                        .corner_radius(4.0)
                        .min_size(egui::Vec2::new(80.0, 26.0));
                        if ui
                            .add(btn)
                            .on_hover_text(help(HelpKey::OfflineToggle))
                            .clicked()
                        {
                            is_offline = !is_offline;
                            self.offline_mode.store(is_offline, Ordering::Relaxed);
                        }

                        // ── Cue transport strip — centred in the slack
                        // between the tab buttons (left) and the status
                        // group above (right). Hidden in Live music mode
                        // OR when the tab bar's right side doesn't have
                        // enough horizontal room (e.g. diagnostic tabs
                        // visible AND a narrow window) — without this
                        // floor the GO button overflows the allocated
                        // rect and lands on top of the Online toggle.
                        const PREV_W: f32 = 80.0;
                        const GO_W: f32 = 80.0;
                        // Skip — advance without firing — (right of Go) and
                        // Undo (leftmost, where the Cues button used to be; the
                        // cue label now opens the cue-list popup).
                        const SKIP_W: f32 = 44.0;
                        const UNDO_W: f32 = 70.0;
                        // Preferred cue-label width — keeps Prev / Go
                        // anchored as the current cue changes so muscle
                        // memory still hits the buttons.
                        const LABEL_W: f32 = 320.0;
                        // Floor when the window is tight: enough to
                        // read "Cue 12.3" before the ellipsis kicks in.
                        const LABEL_MIN_W: f32 = 60.0;
                        const GAP: f32 = 8.0;
                        // Outer breathing room when the window is wide;
                        // collapses down to MIN_PAD before the label is
                        // allowed to shrink.
                        const PAD_MAX: f32 = 24.0;
                        const MIN_PAD: f32 = 16.0;

                        // Strip's absolute minimum: MIN_PAD on the
                        // left, both buttons + their gaps, label at
                        // its floor, MIN_PAD on the right.
                        const MIN_STRIP_W: f32 = MIN_PAD * 2.0
                            + UNDO_W
                            + PREV_W
                            + GO_W
                            + SKIP_W
                            + GAP * 4.0
                            + LABEL_MIN_W;

                        if mode.cue_transport_visible() && ui.available_width() >= MIN_STRIP_W {
                            // Reserve the right-side gap explicitly: in
                            // the right-to-left parent, add_space here
                            // pushes the cursor leftward by MIN_PAD,
                            // carving an unconditional gap between the
                            // Online toggle and the transport strip.
                            ui.add_space(MIN_PAD);

                            let leftover_w = ui.available_width();
                            let leftover_h = ui.available_height();

                            // Two-stage responsive shrink (operating on
                            // the inner sub-region, with MIN_PAD already
                            // reserved on the right):
                            //   1. Trim left padding from PAD_MAX → MIN_PAD.
                            //   2. Once left padding is at MIN_PAD,
                            //      shrink the cue label toward LABEL_MIN_W.
                            let buttons_w = UNDO_W + PREV_W + GO_W + SKIP_W + GAP * 4.0;
                            let pref_w = buttons_w + LABEL_W;
                            let (pad, label_w) = if leftover_w >= pref_w + PAD_MAX + MIN_PAD {
                                (PAD_MAX, LABEL_W)
                            } else if leftover_w >= pref_w + MIN_PAD * 2.0 {
                                (leftover_w - pref_w - MIN_PAD, LABEL_W)
                            } else {
                                let shrunk = (leftover_w - buttons_w - MIN_PAD).max(LABEL_MIN_W);
                                (MIN_PAD, shrunk)
                            };

                            ui.allocate_ui_with_layout(
                                egui::Vec2::new(leftover_w, leftover_h),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    // Explicit `pad` / `GAP` spaces are the only
                                    // spacing here; zero egui's per-item spacing so
                                    // the width budget above (buttons_w / pref_w /
                                    // label_w) is exact and Skip can't overflow onto
                                    // the Online toggle.
                                    ui.spacing_mut().item_spacing.x = 0.0;
                                    ui.add_space(pad);

                                    // Non-blocking read: never block the UI
                                    // thread on the async lock (a background
                                    // recall/follow writer can hold it). On a
                                    // miss, reuse last frame's cached label.
                                    let (current_cue_text, has_cues) = match self
                                        .cue_manager
                                        .try_read()
                                    {
                                        Ok(mgr) => {
                                            let count = mgr.cue_list.cues.len();
                                            let label = mgr.current_cue().map(|c| {
                                                let name = if c.name.is_empty() {
                                                    String::new()
                                                } else {
                                                    format!(" — {}", c.name)
                                                };
                                                let row = c
                                                    .console_snapshot
                                                    .map(|n| format!(" · row {n}"))
                                                    .unwrap_or_default();
                                                format!("Cue {:.1}{}{}", c.cue_number, name, row)
                                            });
                                            let v = (label, count > 0);
                                            *self.transport_cue_cache.borrow_mut() = v.clone();
                                            v
                                        }
                                        Err(_) => self.transport_cue_cache.borrow().clone(),
                                    };
                                    let transport_enabled = is_connected && has_cues;

                                    // UNDO the last recall (cue or direct
                                    // snapshot), leftmost in the transport.
                                    let has_undo = self
                                        .snapshot_engine
                                        .as_ref()
                                        .map(|e| e.has_undo())
                                        .unwrap_or(false);
                                    let undo_btn = egui::Button::new(
                                        egui::RichText::new("Undo")
                                            .color(super::theme::TEXT_PRIMARY)
                                            .strong(),
                                    )
                                    .fill(super::theme::ACCENT_AMBER)
                                    .corner_radius(4.0)
                                    .min_size(egui::Vec2::new(UNDO_W, 26.0));
                                    if ui
                                        .add_enabled(has_undo, undo_btn)
                                        .on_hover_text(help(HelpKey::CueUndo))
                                        .clicked()
                                    {
                                        super::cue_transport::fire_undo(
                                            &self.snapshot_engine,
                                            &self.runtime,
                                            &self.ui_tx,
                                        );
                                    }

                                    ui.add_space(GAP);

                                    // PREV (red)
                                    let prev_btn = egui::Button::new(
                                        egui::RichText::new("◀ Prev")
                                            .color(super::theme::TEXT_PRIMARY)
                                            .strong(),
                                    )
                                    .fill(super::theme::ACCENT_RED)
                                    .corner_radius(4.0)
                                    .min_size(egui::Vec2::new(PREV_W, 26.0));
                                    if ui
                                        .add_enabled(transport_enabled, prev_btn)
                                        .on_hover_text(help(HelpKey::CuePrev))
                                        .clicked()
                                    {
                                        super::cue_transport::fire_prev(
                                            &self.cue_manager,
                                            &self.palette_manager,
                                            &self.snapshot_engine,
                                            &self.runtime,
                                            &self.ui_tx,
                                        );
                                    }

                                    ui.add_space(GAP);

                                    // Cue label — fixed-width, centred,
                                    // truncated with ellipsis if it would
                                    // overflow. Dimmed `—` when no current cue
                                    // is set. Clicking it opens the cue-list
                                    // popup (replaces the old "Cues" button).
                                    let label_rich = match &current_cue_text {
                                        Some(s) => egui::RichText::new(s)
                                            .color(super::theme::label_color())
                                            .strong(),
                                        None => egui::RichText::new("—")
                                            .color(super::theme::label_disabled()),
                                    };
                                    let label_clicked = ui
                                        .allocate_ui_with_layout(
                                            egui::Vec2::new(label_w, 26.0),
                                            egui::Layout::centered_and_justified(
                                                egui::Direction::LeftToRight,
                                            ),
                                            |ui| {
                                                ui.add(
                                                    egui::Label::new(label_rich)
                                                        .truncate()
                                                        .sense(egui::Sense::click()),
                                                )
                                                .on_hover_text(help(HelpKey::CueListOpen))
                                                .clicked()
                                            },
                                        )
                                        .inner;
                                    if label_clicked {
                                        self.show_cue_list_popup = !self.show_cue_list_popup;
                                    }

                                    ui.add_space(GAP);

                                    // GO (blue)
                                    let go_btn = egui::Button::new(
                                        egui::RichText::new("Go ▶")
                                            .color(super::theme::TEXT_PRIMARY)
                                            .strong(),
                                    )
                                    .fill(super::theme::ACCENT_BLUE)
                                    .corner_radius(4.0)
                                    .min_size(egui::Vec2::new(GO_W, 26.0));
                                    if ui
                                        .add_enabled(transport_enabled, go_btn)
                                        .on_hover_text(help(HelpKey::CueGo))
                                        .clicked()
                                    {
                                        super::cue_transport::fire_go(
                                            &self.cue_manager,
                                            &self.palette_manager,
                                            &self.snapshot_engine,
                                            &self.runtime,
                                            &self.ui_tx,
                                        );
                                    }

                                    ui.add_space(GAP);

                                    // SKIP (advance without firing). The icon —
                                    // a right triangle + bar — is hand-painted
                                    // over an empty button so it always reads as
                                    // one cohesive "skip to next" mark; the old
                                    // `▶|` font composite looked disjointed
                                    // (symbol-vs-text baselines), worse with the
                                    // light theme's heavier face.
                                    let skip_btn = egui::Button::new("")
                                        .fill(super::theme::btn_neutral())
                                        .corner_radius(4.0)
                                        .min_size(egui::Vec2::new(SKIP_W, 26.0));
                                    let skip_resp = ui
                                        .add_enabled(has_cues, skip_btn)
                                        .on_hover_text(help(HelpKey::CueSkip));
                                    super::theme::paint_skip_glyph(
                                        ui.painter(),
                                        skip_resp.rect,
                                        if has_cues {
                                            super::theme::on_fill_text(super::theme::btn_neutral())
                                        } else {
                                            super::theme::TEXT_DISABLED
                                        },
                                    );
                                    if skip_resp.clicked() {
                                        super::cue_transport::fire_skip(
                                            &self.cue_manager,
                                            &self.runtime,
                                        );
                                    }
                                },
                            );
                        }
                    });
                });
            });

        // Capture the content rect (below the tab bar) NOW, before the
        // CentralPanel consumes it. The thin recall-progress lines are painted
        // from here, but only AFTER the CentralPanel renders (further below) so
        // they sit on top of its content yet below any popup window.
        let progress_area = ctx.available_rect();

        // Offline-mode banner — anchored to the bottom of the window so
        // it doesn't push the rest of the UI down when toggled. Drawn
        // before the central panel so the central panel sees the
        // remaining vertical space.
        if self.offline_mode.load(Ordering::Relaxed) {
            egui::TopBottomPanel::bottom("offline_banner")
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(super::theme::ACCENT_AMBER)
                        .inner_margin(egui::Margin::symmetric(10, 6)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("⚠ OFFLINE MODE")
                                .strong()
                                .color(super::theme::TEXT_ON_BRIGHT),
                        );
                        ui.label(
                            egui::RichText::new(
                                "— no OSC in/out. State mirror is frozen. \
                                 Edits will not affect the console.",
                            )
                            .color(super::theme::TEXT_ON_BRIGHT),
                        )
                        .on_hover_help_inline(HelpKey::BannerOffline);
                    });
                });
        }

        // Gangs-tab disconnected hint — same bottom-anchored amber
        // banner as the offline notice, so the Gangs UI doesn't shift
        // up/down when the connection state flips. Only shown on the
        // Gangs tab where this hint is relevant.
        if self.active_tab == Tab::Gangs
            && !self.connected.load(Ordering::Relaxed)
            && !self.offline_mode.load(Ordering::Relaxed)
        {
            egui::TopBottomPanel::bottom("gangs_disconnected_banner")
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(super::theme::ACCENT_AMBER)
                        .inner_margin(egui::Margin::symmetric(10, 6)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("⚠ DISCONNECTED")
                                .strong()
                                .color(super::theme::TEXT_ON_BRIGHT),
                        );
                        ui.label(
                            egui::RichText::new(
                                "— connect to console for gang propagation \
                                 to take effect.",
                            )
                            .color(super::theme::TEXT_ON_BRIGHT),
                        )
                        .on_hover_help_inline(HelpKey::BannerGangsDisconnected);
                    });
                });
        }

        // Snapshots-tab disconnected hint — same bottom-anchored amber
        // banner pattern. Suppressed when the offline banner is already
        // showing so they don't stack.
        if self.active_tab == Tab::Snapshots
            && !self.connected.load(Ordering::Relaxed)
            && !self.offline_mode.load(Ordering::Relaxed)
        {
            egui::TopBottomPanel::bottom("snapshots_disconnected_banner")
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(super::theme::ACCENT_AMBER)
                        .inner_margin(egui::Margin::symmetric(10, 6)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("⚠ DISCONNECTED")
                                .strong()
                                .color(super::theme::TEXT_ON_BRIGHT),
                        );
                        ui.label(
                            egui::RichText::new("— connect to console to capture snapshots.")
                                .color(super::theme::TEXT_ON_BRIGHT),
                        )
                        .on_hover_help_inline(HelpKey::BannerSnapshotsDisconnected);
                    });
                });
        }

        // Setup-tab console-IP / NIC mismatch warning — fires when a
        // specific NIC is selected and the console IP isn't reachable
        // from it under a /16 mask (first two octets differ). Click
        // anywhere on the strip to dismiss; same as clicking the ⚠
        // icon next to the IP edit. Re-evaluated whenever the IP or
        // NIC changes.
        if self.active_tab == Tab::Setup
            && super::setup_tab::console_ip_mismatch(&self.setup)
            && !self.setup.console_ip_warning_dismissed
        {
            egui::TopBottomPanel::bottom("setup_console_ip_warning")
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(super::theme::ACCENT_AMBER)
                        .inner_margin(egui::Margin::symmetric(10, 6)),
                )
                .show(ctx, |ui| {
                    let inner = ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("⚠ NETWORK")
                                .strong()
                                .color(super::theme::TEXT_ON_BRIGHT),
                        );
                        ui.label(
                            egui::RichText::new(
                                "— console IP is not on the selected NIC's network. \
                                 Pick a different NIC or change the console IP to match. \
                                 Click here to dismiss.",
                            )
                            .color(super::theme::TEXT_ON_BRIGHT),
                        )
                        .on_hover_help_inline(HelpKey::BannerNetworkMismatch);
                    });
                    // Make the whole strip clickable so the operator
                    // can dismiss without aiming at the small ⚠ icon.
                    let dismiss_resp = inner.response.interact(egui::Sense::click());
                    if dismiss_resp.clicked() {
                        self.setup.console_ip_warning_dismissed = true;
                    }
                });
        }

        // Gang overlap warning — fires (regardless of active tab)
        // whenever two or more *active* gangs share a channel AND
        // share at least one linked section. That's a configuration
        // bug: a parameter change on the shared channel propagates
        // through both gangs in the same dispatch cycle, and any
        // difference between the two paths becomes a race. Always
        // visible until the operator untangles the gangs in the
        // Gangs tab.
        // Non-blocking: a rare lock miss simply hides the warning for one
        // frame rather than stalling the UI thread.
        let overlap_count = self
            .gang_manager
            .try_read()
            .map(|m| m.count_overlap_conflict_channels())
            .unwrap_or(0);
        if overlap_count > 0 {
            egui::TopBottomPanel::bottom("gangs_overlap_warning")
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(super::theme::ACCENT_AMBER)
                        .inner_margin(egui::Margin::symmetric(10, 6)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("⚠ GANG OVERLAP")
                                .strong()
                                .color(super::theme::TEXT_ON_BRIGHT),
                        );
                        let plural = if overlap_count == 1 { "" } else { "s" };
                        ui.label(
                            egui::RichText::new(format!(
                                "— {overlap_count} channel{plural} appear in multiple active \
                                 gangs sharing parameters. Propagation will fight; remove the \
                                 overlap in the Gangs tab.",
                            ))
                            .color(super::theme::TEXT_ON_BRIGHT),
                        )
                        .on_hover_help_inline(HelpKey::BannerGangOverlap);
                    });
                });
        }

        // Web monitor configured but not running — its URLs/QR codes won't be
        // reachable until the operator Connects. Shown on the Monitor tab,
        // where those URLs/QR live.
        if self.active_tab == Tab::Monitor
            && self.setup.web_port.parse::<u16>().unwrap_or(0) > 0
            && !self.monitor.web_server_running
        {
            egui::TopBottomPanel::bottom("web_server_offline_warning")
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(super::theme::ACCENT_AMBER)
                        .inner_margin(egui::Margin::symmetric(10, 6)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("⚠ WEB MONITOR")
                                .strong()
                                .color(super::theme::TEXT_ON_BRIGHT),
                        );
                        ui.label(
                            egui::RichText::new(
                                "— not running. Connect to start it; the URLs and QR codes won't \
                                 be reachable until then.",
                            )
                            .color(super::theme::TEXT_ON_BRIGHT),
                        )
                        .on_hover_help_inline(HelpKey::BannerWebMonitorOff);
                    });
                });
        }

        // Main content
        egui::CentralPanel::default().show(ctx, |ui| {
            // Reset transient per-tab state when the user navigates away.
            // Pan Link's stage / apply buffer should drop unapplied work
            // and clear the input selection on tab switch — re-entry
            // always lands on a clean slate. Idempotent each frame.
            if self.active_tab != Tab::PanLink {
                self.pan_link.mark_needs_sync();
            }

            match self.active_tab {
                Tab::Setup => {
                    super::setup_tab::draw_setup_tab(
                        ui,
                        &mut self.setup,
                        &self.state,
                        &self.cue_manager,
                        &self.macro_manager,
                        &self.monitor_manager,
                        &self.palette_manager,
                        &self.gang_manager,
                        &self.pan_link_bindings,
                        &self.stream_deck_config,
                        &self.offline_mode,
                        &self.auto_update_on_recall,
                        &self.snapshot_sync_direction,
                        &self.snapshots.scope_editor.console_recall,
                        &self.dirty_tracker,
                        &self.last_received,
                        &self.pending_engines,
                        &self.connected,
                        &mut self.cancel_token,
                        &self.osc_log,
                        &self.send_pace_us,
                        &self.progress,
                        &self.runtime,
                        &self.ui_tx,
                        &self.egui_ctx,
                    );
                }
                Tab::Snapshots => {
                    let qlab_port: u16 = self.setup.qlab_port.parse().unwrap_or(53000);
                    let qlab_patch_app: i32 = self.setup.qlab_patch_app.parse().unwrap_or(1);
                    let qlab_patch_console: i32 =
                        self.setup.qlab_patch_console.parse().unwrap_or(2);
                    let qlab_ip = if self.setup.qlab_ip.is_empty() {
                        "127.0.0.1"
                    } else {
                        self.setup.qlab_ip.as_str()
                    };
                    super::snapshots_tab::draw_snapshots_tab(
                        ui,
                        &mut self.snapshots,
                        &mut self.palettes_ui,
                        &self.state,
                        &self.cue_manager,
                        &self.palette_manager,
                        &self.snapshot_engine,
                        &self.dirty_tracker,
                        &self.auto_update_on_recall,
                        &self.snapshot_sync_direction,
                        self.setup.operating_mode,
                        qlab_ip,
                        qlab_port,
                        qlab_patch_app,
                        qlab_patch_console,
                        &self.connected,
                        &self.runtime,
                        &self.ui_tx,
                    );
                }
                Tab::Macros => {
                    super::macros_tab::draw_macros_tab(
                        ui,
                        &mut self.macros,
                        &self.macro_manager,
                        &self.macro_engine,
                        &self.connected,
                        &self.state,
                        &self.cue_manager,
                        &self.palette_manager,
                        &self.last_received,
                        &self.stream_deck_engine,
                        &self.stream_deck_config,
                        &self.runtime,
                        &self.ui_tx,
                    );
                }
                Tab::Gangs => {
                    super::gangs_tab::draw_gangs_tab(
                        ui,
                        &mut self.gangs,
                        &self.gang_manager,
                        &self.state,
                        &self.runtime,
                    );
                }
                Tab::PanLink => {
                    super::pan_link_tab::draw_pan_link_tab(
                        ui,
                        &mut self.pan_link,
                        &self.pan_link_bindings,
                        &self.state,
                        &self.sender,
                        &self.connected,
                        &self.runtime,
                        self.setup.operating_mode.uses_ipad_protocol(),
                    );
                }
                Tab::Monitor => {
                    super::monitor_tab::draw_monitor_tab(
                        ui,
                        &mut self.monitor,
                        &self.monitor_manager,
                        &self.state,
                        &self.connected,
                        &self.runtime,
                        self.setup.web_port.parse().unwrap_or(0),
                    );
                }
                Tab::OscLog => {
                    super::osc_log_tab::draw_osc_log_tab(ui, &mut self.osc_log_tab, &self.osc_log);
                }
                Tab::Inspector => {
                    super::inspector_tab::draw_inspector_tab(ui, &mut self.inspector, &self.state);
                }
            }
        });

        // Thin recall-progress lines (green inbound / blue outbound), painted on
        // the background layer AFTER the CentralPanel so they sit on top of its
        // content but BELOW the floating windows below. No layout reflow.
        self.draw_recall_progress(ctx, progress_area);

        // ── Scope editor window (floats above any tab) ──
        // Drawn outside the CentralPanel so it can overlay anything. Borrows
        // ConsoleState (and the dirty tracker) for one frame to compute
        // availability, channel names, and the dirty earmark map. try_read()
        // on both so we don't deadlock if a write is in flight; the next
        // frame will redraw.
        if self.snapshots.scope_editor.window_open {
            // The editor always edits the WORKING scope (used by the next
            // capture). OK never writes to a snapshot. Apply / Recapture
            // deliberately target the currently-SELECTED snapshot, so the title
            // shows that target.
            let selected_name = self.snapshots.selected_snapshot_id.and_then(|id| {
                self.cue_manager
                    .try_read()
                    .ok()
                    .and_then(|m| m.get_snapshot(&id).map(|s| s.name.clone()))
            });
            let window_title = match &selected_name {
                Some(name) => format!("Edit Scope  ·  selected: {name}"),
                None => "Edit Scope".to_string(),
            };
            if let Ok(state_guard) = self.state.try_read() {
                let dirty_guard = self.dirty_tracker.try_read().ok();
                let outcome = super::scope_editor::draw_scope_window(
                    ctx,
                    &mut self.snapshots.scope_editor,
                    &state_guard,
                    dirty_guard.as_deref(),
                    &self.cue_manager,
                    &self.runtime,
                    &window_title,
                    selected_name.as_deref(),
                );
                drop(dirty_guard);
                drop(state_guard);

                // Phase C: the toolbar can request a dirty-tracker clear.
                // We can only fulfil it after dropping the read borrow, so
                // do it here outside the render closure.
                if outcome.clear_dirty_requested {
                    let dirty_arc = self.dirty_tracker.clone();
                    self.runtime.spawn(async move {
                        dirty_arc.write().await.clear();
                    });
                }

                // "Apply to selected": write the current (working) scope into
                // the selected snapshot's scope — data untouched. For editing a
                // snapshot's scope offline, with no live desk to recapture from.
                if outcome.apply_to_selected_requested
                    && let Some(id) = self.snapshots.selected_snapshot_id
                {
                    let name = selected_name.clone().unwrap_or_default();
                    let scope = self.snapshots.scope_editor.to_scope_template(name.clone());
                    let cue_mgr = self.cue_manager.clone();
                    self.runtime.spawn(async move {
                        cue_mgr.write().await.update_snapshot_scope(id, scope);
                    });
                    self.snapshots.status_message =
                        Some(format!("Applied scope to '{name}'").into());
                }
            }
        }

        // Post-capture confirmation popup floats above everything.
        self.draw_capture_confirm(ctx);

        // Corruption-recovery modal (when a load failed on a bad file).
        self.draw_recovery_dialog(ctx);

        // Confirmation modal for quitting while connected to the console.
        self.draw_close_confirm(ctx);

        // Cue-list popup (opened from the top-bar "Cues" button).
        super::cue_list_popup::draw_cue_list_popup(
            ctx,
            &mut self.show_cue_list_popup,
            &self.cue_manager,
            &self.palette_manager,
            &self.snapshot_engine,
            self.connected.load(Ordering::Relaxed),
            &self.runtime,
            &self.ui_tx,
        );
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;

    // `compute_target_ppp` returns an *absolute effective pixels-per-point*, so
    // expectations below are ppp values (independent of the display's native
    // scale) — egui itself divides by native inside `set_pixels_per_point`. Every
    // expectation includes the global `COMFORT_FACTOR` the slider's 100% maps to.
    fn approx(p: f32, expected: f32) {
        assert!((p - expected).abs() <= 0.03, "ppp {p} not ≈ {expected}");
    }

    #[test]
    fn large_low_ppi_tv_is_gentle_not_cartoonish() {
        // 40" 4K @100%: ~110 PPI → base ≈ 110/96, native irrelevant.
        approx(
            compute_target_ppp(1.0, Some(110.0), 1.0),
            110.0 / 96.0 * COMFORT_FACTOR,
        );
    }

    #[test]
    fn small_high_ppi_panel_scales_up() {
        // 15" 4K: ~282 PPI → base ≈ 2.94, still under the MAX_PPP ceiling after
        // the comfort factor. The UI reflows / scrolls to fit rather than shrinking.
        approx(
            compute_target_ppp(1.0, Some(282.0), 1.0),
            282.0 / 96.0 * COMFORT_FACTOR,
        );
    }

    #[test]
    fn standard_1080p_not_shrunk_below_unity() {
        // 1080p @100%: native 1.0, ~92 PPI. The PPI fallback is floored at native,
        // so the base is 1.0 (not 92/96 ≈ 0.96) before the comfort factor.
        approx(compute_target_ppp(1.0, Some(92.0), 1.0), COMFORT_FACTOR);
    }

    #[test]
    fn unknown_ppi_respects_os_scale() {
        // No detected PPI → base is the OS scale factor itself (independent of
        // window size; tabs reflow / scroll instead of fit-down-scaling).
        approx(compute_target_ppp(1.0, None, 1.0), COMFORT_FACTOR);
        approx(compute_target_ppp(2.0, None, 1.0), 2.0 * COMFORT_FACTOR);
    }

    #[test]
    fn four_k_at_200_percent_respects_os_scale_not_ppi() {
        // The user's real machine: 15.6" 4K at Windows 200% (native 2.0) with a
        // detected raw-EDID PPI of 283. The OS scale is authoritative, so the base
        // is 2.0 — NOT 283/96 ≈ 2.94, which would override the user's deliberate
        // 200% choice and render the UI ~1.5× too large.
        approx(
            compute_target_ppp(2.0, Some(283.0), 1.0),
            2.0 * COMFORT_FACTOR,
        );
    }

    #[test]
    fn retina_respects_os_scale() {
        // 15" Retina ~220 PPI at native 2.0: respect the OS like every mac app —
        // base 2.0, not 220/96.
        approx(
            compute_target_ppp(2.0, Some(220.0), 1.0),
            2.0 * COMFORT_FACTOR,
        );
    }

    #[test]
    fn windows_125_and_150_respected_not_overridden_by_ppi() {
        // Fractional Windows scales are deliberate OS choices: PPI must not win.
        approx(
            compute_target_ppp(1.25, Some(283.0), 1.0),
            1.25 * COMFORT_FACTOR,
        );
        approx(
            compute_target_ppp(1.5, Some(200.0), 1.0),
            1.5 * COMFORT_FACTOR,
        );
    }

    #[test]
    fn pi_touchscreen_unscaled_gets_ppi_bump() {
        // Raspberry Pi touchscreen: OS does no scaling (native 1.0) but the panel
        // is high-PPI, so the PPI fallback enlarges the UI to a usable size.
        approx(
            compute_target_ppp(1.0, Some(150.0), 1.0),
            150.0 / 96.0 * COMFORT_FACTOR,
        );
        approx(
            compute_target_ppp(1.0, Some(200.0), 1.0),
            200.0 / 96.0 * COMFORT_FACTOR,
        );
    }

    #[test]
    fn high_ppi_4k_at_100_percent_gets_bump() {
        // Misconfigured 4K external left at Windows 100% (native 1.0, ~160 PPI):
        // PPI fallback rescues it from being microscopic.
        approx(
            compute_target_ppp(1.0, Some(160.0), 1.0),
            160.0 / 96.0 * COMFORT_FACTOR,
        );
    }

    #[test]
    fn threshold_boundary_below_uses_ppi_fallback() {
        // Just under the threshold (an unusual fractional scale) still uses the
        // PPI fallback; at/above it the OS scale wins (see the 1.25 test above).
        approx(
            compute_target_ppp(1.05, Some(200.0), 1.0),
            200.0 / 96.0 * COMFORT_FACTOR,
        );
    }

    #[test]
    fn manual_ui_scale_and_clamps_apply() {
        let base = compute_target_ppp(1.0, Some(110.0), 1.0);
        // ui_scale > 1 enlarges (distance viewing) ...
        assert!(compute_target_ppp(1.0, Some(110.0), 1.5) > base);
        // ... but the final result is clamped both ways.
        approx(compute_target_ppp(1.0, Some(110.0), 100.0), MAX_PPP);
        approx(compute_target_ppp(1.0, Some(110.0), 0.001), MIN_PPP);
    }
}
