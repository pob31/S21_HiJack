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
use crate::model::snapshot::CueList;
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::ipad_client::IpadSender;
use crate::persistence::preferences::AppPreferences;

use super::gangs_tab::GangsTabState;
use super::inspector_tab::InspectorTabState;
use super::macros_tab::MacrosTabState;
use super::monitor_tab::MonitorTabState;
use super::osc_log_tab::OscLogTabState;
use super::palettes_ui::PalettesUiState;
use super::pan_link_tab::PanLinkTabState;
use super::setup_tab::SetupTabState;
use super::snapshots_tab::SnapshotsTabState;
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

// Clamp on the *zoom factor* (effective ppp = zoom × native_ppp).
const MIN_ZOOM: f32 = 0.70; // readability floor (body 16pt → ~11px at native 1.0)
const MAX_ZOOM: f32 = 3.00; // sanity ceiling (also bounds the manual ui_scale)

const SCALE_STEP: f32 = 0.02; // snap zoom to 2% steps (kills drag-resize jitter)
const SCALE_EPS: f32 = 0.005; // dead-band; must be < SCALE_STEP
const MIN_PHYS_PX: f32 = 200.0; // reject 0-area minimize frames
const MAX_PHYS_PX: f32 = 20_000.0; // reject egui's 10000×10000 first-frame placeholder

/// Pure core of the UI auto-scaler (unit-tested). Produces the egui *zoom
/// factor* for a consistent physical size: `dpi_target = (PPI / REF_PPI) /
/// native_ppp`, or `1.0` (respect the OS scale factor) when the real PPI is
/// unknown, times the user's manual `ui_scale`, clamped for sanity.
///
/// Deliberately independent of the window size: the UI keeps a stable physical
/// size and the tabs *reflow* against the actual window (collapse slack, then
/// scroll) rather than uniformly shrinking to fit.
fn compute_zoom(native_ppp: f32, ppi: Option<f32>, ui_scale: f32) -> f32 {
    let native_ppp = if native_ppp > 0.0 { native_ppp } else { 1.0 };
    let dpi_target = match ppi {
        Some(p) if p > 0.0 => (p / REF_PPI) / native_ppp,
        _ => 1.0,
    };
    (dpi_target * ui_scale).clamp(MIN_ZOOM, MAX_ZOOM)
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
    /// Follow console snapshot recalls. Mirrored from `ConnectionSettings`.
    pub console_snapshot_follow: Arc<AtomicBool>,
    /// Inter-message pacing (μs) shared between snapshot recall and
    /// macro OSC sends. 0 = no pacing. Owned at the app level so the
    /// Advanced Settings panel and both engines see one consistent
    /// value; persisted in `AppPreferences`.
    pub send_pace_us: Arc<std::sync::atomic::AtomicU64>,
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
            console_snapshot_follow: Arc::new(AtomicBool::new(false)),
            send_pace_us: Arc::new(std::sync::atomic::AtomicU64::new(prefs.send_pace_us)),
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
        }
    }

    /// Apply global fit-to-window + physical-size UI scaling. Called at the top
    /// of `update`, before any panel is built. The new zoom takes effect on the
    /// next pass; egui keeps physical px constant across the change, so this is
    /// a stable fixed point (no oscillation).
    fn apply_auto_scale(&mut self, ctx: &egui::Context) {
        let logical = ctx.content_rect();
        let ppp = ctx.pixels_per_point(); // effective ppp = zoom × native
        let native_ppp = ctx.native_pixels_per_point().unwrap_or(1.0);
        // Real window size in physical px — invariant under our own zoom changes.
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
        // recovers physical px (stable across our own zoom changes).
        let monitor_px = ctx
            .input(|i| i.viewport().monitor_size)
            .map(|m| ((m.x * ppp).round() as u32, (m.y * ppp).round() as u32));
        let ppi = self.current_ppi(monitor_px);

        let mut target = compute_zoom(native_ppp, ppi, self.setup.ui_scale);
        target = (target / SCALE_STEP).round() * SCALE_STEP; // snap to step
        if (target - ctx.zoom_factor()).abs() > SCALE_EPS {
            ctx.set_zoom_factor(target);
        }
    }

    /// One-shot startup sizing: resize to 95% of the monitor and center, then
    /// reveal the (initially hidden) window. Windowed, not maximized — the user
    /// avoids macOS full-screen-space behavior and desktop battles with QLab
    /// video out. Falls back to revealing at the builder's fallback size if the
    /// monitor size never arrives.
    fn size_window_to_monitor(&mut self, ctx: &egui::Context) {
        if self.window_sized {
            return;
        }
        self.startup_frames = self.startup_frames.saturating_add(1);
        match ctx.input(|i| i.viewport().monitor_size) {
            Some(mon) => {
                let inner = (mon * 0.95).max(egui::vec2(900.0, 520.0));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(inner));
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                    ((mon - inner) * 0.5).to_pos2(),
                ));
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
                            if ui.button("×").on_hover_text("Dismiss").clicked() {
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
                        .on_hover_text("Re-recall this snapshot now to confirm nothing drifted")
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
                    self.setup.status_message = Some(format!("Connection failed: {msg}"));
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
                }
                UiEvent::SnapshotCaptured { name, param_count } => {
                    self.snapshots.status_message =
                        Some(format!("Captured '{name}' ({param_count} params)"));
                }
                UiEvent::SnapshotCaptureConfirm {
                    snapshot_id,
                    name,
                    params,
                } => {
                    self.snapshots.status_message =
                        Some(format!("Captured '{name}' ({} params)", params.len()));
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
                        Some(format!("Recalled '{name}' ({params_sent} params sent)"));
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
                    self.macros.status_message = Some(format!("Run failed: {msg}"));
                }
                UiEvent::MacroRecordingStopped { step_count } => {
                    self.macros.status_message =
                        Some(format!("Recording stopped: {step_count} steps captured"));
                }
                UiEvent::PaletteCaptured { name, param_count } => {
                    self.palettes_ui.status_message = Some(format!(
                        "Captured palette '{name}' ({param_count} EQ params)"
                    ));
                }
                UiEvent::PaletteLinked {
                    palette_name,
                    snapshot_name,
                } => {
                    self.palettes_ui.status_message =
                        Some(format!("Linked '{palette_name}' to '{snapshot_name}'"));
                }
                UiEvent::PaletteUpdated {
                    name,
                    affected_count,
                } => {
                    self.palettes_ui.status_message = Some(format!(
                        "Updated '{name}' — {affected_count} snapshots affected"
                    ));
                }
                UiEvent::ShowFileLoaded(path, conn, recall) => {
                    self.setup.status_message = Some(format!("Loaded: {path}"));
                    // If this load resolved a recovery, flag it so the dialog
                    // can offer to repair the original path.
                    if let Some(rd) = &mut self.recovery_dialog {
                        rd.recovered = true;
                    }
                    self.snapshots.scope_editor.console_recall = recall;
                    if let Some(c) = &conn {
                        self.auto_update_on_recall
                            .store(c.auto_update_on_recall, Ordering::Relaxed);
                        self.console_snapshot_follow
                            .store(c.console_snapshot_follow, Ordering::Relaxed);
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
                        self.setup.ui_mode = conn.ui_mode;
                        // Mirror the loaded mode to app preferences so a later
                        // launch (without a show file) starts in this mode.
                        let prefs = AppPreferences {
                            ui_mode: Some(self.setup.ui_mode),
                            show_diagnostics: self.setup.show_diagnostics,
                            send_pace_us: pace_to_save,
                            ui_scale: self.setup.ui_scale,
                        };
                        if let Err(e) = prefs.save() {
                            tracing::warn!(error = %e, "Failed to save app preferences after show load");
                        }
                    }
                }
                UiEvent::ShowFileSaved(path) => {
                    self.setup.status_message = Some(format!("Saved: {path}"));
                }
                UiEvent::ShowFileError(msg) => {
                    self.setup.status_message = Some(msg);
                }
                UiEvent::AutosaveCompleted { fingerprint, wrote } => {
                    self.last_autosaved_fingerprint = fingerprint;
                    self.autosave_in_flight.store(false, Ordering::Relaxed);
                    if wrote {
                        self.setup.status_message = Some("Autosaved".into());
                    }
                }
                UiEvent::ShowFileCorrupt { path, candidates } => {
                    self.setup.status_message =
                        Some("Show file appears corrupt — choose a recovery candidate".into());
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
                    self.setup.status_message = Some(format!("iPad connection failed: {msg}"));
                }
                UiEvent::FadeProgress { .. } => {
                    // Fade-progress display lived on the old Live tab.
                    // Drop it for now — could be re-surfaced as a thin
                    // overlay under the top-bar transport later if
                    // operators want it back.
                }
                UiEvent::MonitorClientConnected { name } => {
                    self.monitor.status_message = Some(format!("Client '{name}' connected"));
                }
                UiEvent::MonitorClientDisconnected { name } => {
                    self.monitor.status_message = Some(format!("Client '{name}' disconnected"));
                }
                UiEvent::MonitorServerStarted => {
                    self.monitor.monitor_server_running = true;
                    self.setup.status_message = Some("Monitor server started".into());
                }
                UiEvent::MonitorServerFailed(msg) => {
                    self.monitor.monitor_server_running = false;
                    self.setup.status_message = Some(format!("Monitor server failed: {msg}"));
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
                        &self.console_snapshot_follow,
                        &self.dirty_tracker,
                        &self.last_received,
                        &self.pending_engines,
                        &self.connected,
                        &mut self.cancel_token,
                        &self.osc_log,
                        &self.send_pace_us,
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
                        let pmgr = pmgr.read().await;
                        let _ = engine
                            .recall_cue(&cue, Some(&snapshot), &pmgr.palettes, false)
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
                    self.macros.status_message = Some(format!("Stream Deck: {message}"));
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
            self.console_snapshot_follow.load(Ordering::Relaxed),
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
                    );
                    if rd.recovered {
                        ui.colored_label(
                            super::theme::ACCENT_GREEN,
                            "Recovered. You can re-save it to the original path below.",
                        );
                    }
                    ui.separator();

                    if rd.candidates.is_empty() {
                        ui.label("No backups or autosaves were found for this show.");
                    } else {
                        ui.label("Choose a copy to restore (newest first):");
                        egui::ScrollArea::vertical()
                            .max_height(260.0)
                            .show(ui, |ui| {
                                for cand in &rd.candidates {
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add_enabled(cand.valid, egui::Button::new("Load this"))
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
                        if rd.recovered && ui.button("Save recovered to original path").clicked() {
                            action = Action::SaveToOriginal;
                        }
                        if ui.button("Cancel").clicked() {
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
                    self.console_snapshot_follow.load(Ordering::Relaxed),
                    self.snapshots.scope_editor.console_recall.clone(),
                    &self.runtime,
                    &self.ui_tx,
                );
                self.recovery_dialog = None;
            }
        }
    }
}

impl eframe::App for HiJackApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // First-frame init: install embedded fonts (NotoSans fallback so
        // Unicode arrows / symbols don't tofu) before the style pass.
        // `OnceLock` keeps it cheap on subsequent frames.
        static FONTS_INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        FONTS_INSTALLED.get_or_init(|| super::fonts::install_fonts(ctx));

        // Global fit-to-window + physical-size UI scaling. Must run before any
        // panel is built; the new zoom applies on the next pass.
        self.apply_auto_scale(ctx);

        // First-run window sizing: open at 95% of the monitor, centered, then
        // reveal the initially-hidden window (no fallback-size flash).
        self.size_window_to_monitor(ctx);

        // Store context on first frame for async repaint
        let _ = self.egui_ctx.set(ctx.clone());

        // Configure style on first frame
        super::theme::configure_style(ctx);

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

        // Periodic autosave during quiet periods (no-op most frames).
        self.maybe_autosave();

        // Tab bar
        egui::TopBottomPanel::top("tab_bar")
            .frame(
                egui::Frame::new()
                    .fill(super::theme::BG_DARK)
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
                            super::theme::BG_ELEVATED
                        };
                        let text_color = if is_active {
                            super::theme::TEXT_PRIMARY
                        } else {
                            super::theme::TEXT_SECONDARY
                        };
                        let btn = egui::Button::new(
                            egui::RichText::new(label).color(text_color).strong(),
                        )
                        .fill(fill)
                        .corner_radius(4.0);
                        if ui.add(btn).clicked() {
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
                        let (color, text) = if is_connected {
                            (super::theme::COLOR_CONNECTED, "Connected")
                        } else {
                            (super::theme::COLOR_DISCONNECTED, "Disconnected")
                        };
                        ui.colored_label(color, text);
                        super::theme::status_dot(ui, color);

                        ui.add_space(12.0);
                        // Offline mode toggle — freezes all OSC traffic both ways.
                        let mut is_offline = self.offline_mode.load(Ordering::Relaxed);
                        let label = if is_offline { "OFFLINE" } else { "Online" };
                        let fill = if is_offline {
                            super::theme::ACCENT_AMBER
                        } else {
                            super::theme::BG_ELEVATED
                        };
                        let text_color = if is_offline {
                            super::theme::BG_DARK
                        } else {
                            super::theme::TEXT_SECONDARY
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
                            .on_hover_text(
                                "Offline mode: drops every inbound and outbound OSC message. \
                             Lets you edit show data without affecting the desk. \
                             Toggle back to Online to resume — the state mirror will be \
                             stale until you click Refresh on the Setup tab.",
                            )
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
                                    ui.add_space(pad);

                                    let (current_cue_text, has_cues) = {
                                        let mgr = self.runtime.block_on(self.cue_manager.read());
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
                                        (label, count > 0)
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
                                        .on_hover_text("Undo the last cue / snapshot recall.")
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
                                        .on_hover_text("Recall the previous cue.")
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
                                            .color(super::theme::TEXT_PRIMARY)
                                            .strong(),
                                        None => egui::RichText::new("—")
                                            .color(super::theme::TEXT_DISABLED),
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
                                                .on_hover_text("Open the cue list.")
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
                                        .on_hover_text("Recall the next cue.")
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

                                    // SKIP (advance without firing). Built from
                                    // `▶` (U+25B6, Geometric Shapes — same font
                                    // family as the working ◀/▶) plus an ASCII
                                    // bar, since the arrows-block `⇥` rendered as
                                    // tofu. Reads as skip-to-next (▶|).
                                    let skip_btn = egui::Button::new(
                                        egui::RichText::new("▶|")
                                            .color(super::theme::TEXT_PRIMARY)
                                            .strong(),
                                    )
                                    .fill(super::theme::BG_ELEVATED)
                                    .corner_radius(4.0)
                                    .min_size(egui::Vec2::new(SKIP_W, 26.0));
                                    if ui
                                        .add_enabled(has_cues, skip_btn)
                                        .on_hover_text(
                                            "Skip — advance to the next cue without recalling it.",
                                        )
                                        .clicked()
                                    {
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
                                .color(super::theme::BG_DARK),
                        );
                        ui.label(
                            egui::RichText::new(
                                "— no OSC in/out. State mirror is frozen. \
                                 Edits will not affect the console.",
                            )
                            .color(super::theme::BG_DARK),
                        );
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
                                .color(super::theme::BG_DARK),
                        );
                        ui.label(
                            egui::RichText::new(
                                "— connect to console for gang propagation \
                                 to take effect.",
                            )
                            .color(super::theme::BG_DARK),
                        );
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
                                .color(super::theme::BG_DARK),
                        );
                        ui.label(
                            egui::RichText::new("— connect to console to capture snapshots.")
                                .color(super::theme::BG_DARK),
                        );
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
                                .color(super::theme::BG_DARK),
                        );
                        ui.label(
                            egui::RichText::new(
                                "— console IP is not on the selected NIC's network. \
                                 Pick a different NIC or change the console IP to match. \
                                 Click here to dismiss.",
                            )
                            .color(super::theme::BG_DARK),
                        );
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
        let overlap_count = self
            .runtime
            .block_on(self.gang_manager.read())
            .count_overlap_conflict_channels();
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
                                .color(super::theme::BG_DARK),
                        );
                        let plural = if overlap_count == 1 { "" } else { "s" };
                        ui.label(
                            egui::RichText::new(format!(
                                "— {overlap_count} channel{plural} appear in multiple active \
                                 gangs sharing parameters. Propagation will fight; remove the \
                                 overlap in the Gangs tab.",
                            ))
                            .color(super::theme::BG_DARK),
                        );
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
                        &self.console_snapshot_follow,
                        &self.snapshots.scope_editor.console_recall,
                        &self.dirty_tracker,
                        &self.last_received,
                        &self.pending_engines,
                        &self.connected,
                        &mut self.cancel_token,
                        &self.osc_log,
                        &self.send_pace_us,
                        &self.runtime,
                        &self.ui_tx,
                        &self.egui_ctx,
                    );
                }
                Tab::Snapshots => {
                    let qlab_port: u16 = self.setup.qlab_port.parse().unwrap_or(53000);
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
                        &self.console_snapshot_follow,
                        self.setup.operating_mode,
                        qlab_ip,
                        qlab_port,
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

        // ── Scope editor window (floats above any tab) ──
        // Drawn outside the CentralPanel so it can overlay anything. Borrows
        // ConsoleState (and the dirty tracker) for one frame to compute
        // availability, channel names, and the dirty earmark map. try_read()
        // on both so we don't deadlock if a write is in flight; the next
        // frame will redraw.
        if self.snapshots.scope_editor.window_open {
            if let Ok(state_guard) = self.state.try_read() {
                let dirty_guard = self.dirty_tracker.try_read().ok();
                let outcome = super::scope_editor::draw_scope_window(
                    ctx,
                    &mut self.snapshots.scope_editor,
                    &state_guard,
                    dirty_guard.as_deref(),
                    &self.cue_manager,
                    &self.runtime,
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
            }
        }

        // Post-capture confirmation popup floats above everything.
        self.draw_capture_confirm(ctx);

        // Corruption-recovery modal (when a load failed on a bad file).
        self.draw_recovery_dialog(ctx);

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

    fn approx(z: f32, expected: f32) {
        assert!((z - expected).abs() <= 0.03, "zoom {z} not ≈ {expected}");
    }

    #[test]
    fn large_low_ppi_tv_is_gentle_not_cartoonish() {
        // 40" 4K @100%: ~110 PPI → gentle physical-size zoom, not cartoonish.
        approx(compute_zoom(1.0, Some(110.0), 1.0), 110.0 / 96.0); // ≈ 1.15
    }

    #[test]
    fn small_high_ppi_panel_scales_up() {
        // 15" 4K @100%: ~282 PPI → dpi_target ≈ 2.94 (under the MAX_ZOOM ceiling).
        // The UI reflows / scrolls to fit rather than shrinking to the window.
        approx(compute_zoom(1.0, Some(282.0), 1.0), 282.0 / 96.0); // ≈ 2.94
    }

    #[test]
    fn standard_1080p_is_about_unity() {
        approx(compute_zoom(1.0, Some(92.0), 1.0), 0.96);
    }

    #[test]
    fn unknown_ppi_respects_os_scale() {
        // No detected PPI → respect the OS scale factor (zoom 1.0), independent of
        // window size (tabs reflow / scroll instead of fit-down-scaling).
        approx(compute_zoom(1.0, None, 1.0), 1.0);
        approx(compute_zoom(2.0, None, 1.0), 1.0);
    }

    #[test]
    fn retina_targets_reference_physical_size() {
        // 15" Retina: native 2.0, ~220 PPI → effective ppp ≈ 220/96, i.e. a zoom
        // factor of (220/96)/2.0 ≈ 1.15 on top of the OS's 2× scaling.
        approx(compute_zoom(2.0, Some(220.0), 1.0), (220.0 / 96.0) / 2.0);
    }

    #[test]
    fn manual_ui_scale_and_clamps_apply() {
        let base = compute_zoom(1.0, Some(110.0), 1.0);
        // ui_scale > 1 enlarges (distance viewing) ...
        assert!(compute_zoom(1.0, Some(110.0), 1.5) > base);
        // ... but the final result is clamped both ways.
        approx(compute_zoom(1.0, Some(110.0), 100.0), MAX_ZOOM);
        approx(compute_zoom(1.0, Some(110.0), 0.001), MIN_ZOOM);
    }
}
