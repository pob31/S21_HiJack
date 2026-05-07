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
    ) -> Self {
        let (ui_tx, ui_rx) = std::sync::mpsc::channel();

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
            snapshot_engine: None,
            macro_engine: None,
            pending_engines: Arc::new(std::sync::Mutex::new(None)),
            dirty_tracker: Arc::new(RwLock::new(DirtyTracker::new())),

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
            setup: SetupTabState::new(
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
            ),
            snapshots: SnapshotsTabState::default(),
            macros: MacrosTabState::default(),
            palettes_ui: PalettesUiState::default(),
            gangs: GangsTabState::default(),
            pan_link: PanLinkTabState::default(),
            monitor: MonitorTabState::default(),
            osc_log_tab: OscLogTabState::default(),
            inspector: InspectorTabState::default(),
        }
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
                        self.setup.send_pace_us = conn.send_pace_us;
                        self.setup.monitor_allow_cidrs = conn.monitor_allow_cidrs;
                        self.setup.trigger_allow_cidrs = conn.trigger_allow_cidrs;
                        self.setup.ui_mode = conn.ui_mode;
                        // Mirror the loaded mode to app preferences so a later
                        // launch (without a show file) starts in this mode.
                        let prefs = AppPreferences {
                            ui_mode: Some(self.setup.ui_mode),
                            show_diagnostics: self.setup.show_diagnostics,
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
            }
        }
    }
}

impl eframe::App for HiJackApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Store context on first frame for async repaint
        let _ = self.egui_ctx.set(ctx.clone());

        // Configure style on first frame
        super::theme::configure_style(ctx);

        // Drain async events
        self.drain_events();

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
                        // group above (right). Hidden in Live music mode.
                        if mode.cue_transport_visible() {
                            const PREV_W: f32 = 80.0;
                            const GO_W: f32 = 80.0;
                            // Preferred cue-label width — keeps Prev / Go
                            // anchored as the current cue changes so
                            // muscle memory still hits the buttons.
                            const LABEL_W: f32 = 320.0;
                            // Floor when the window is tight: still wide
                            // enough to read "Cue 12.3 — A…" before the
                            // ellipsis kicks in.
                            const LABEL_MIN_W: f32 = 120.0;
                            const GAP: f32 = 8.0;
                            // Outer breathing room when the window is
                            // wide; collapses down to MIN_PAD before the
                            // label is allowed to shrink. MIN_PAD is the
                            // floor on *both* sides — the right gap to
                            // the Online toggle stays at least this wide
                            // so the Go button never overlaps it.
                            const PAD_MAX: f32 = 24.0;
                            const MIN_PAD: f32 = 16.0;

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
                            let buttons_w = PREV_W + GO_W + GAP + GAP;
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
                                            format!("Cue {:.1}{}", c.cue_number, name)
                                        });
                                        (label, count > 0)
                                    };
                                    let transport_enabled = is_connected && has_cues;

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
                                    // overflow. Dimmed `—` when no current
                                    // cue is set.
                                    let label_rich = match &current_cue_text {
                                        Some(s) => egui::RichText::new(s)
                                            .color(super::theme::TEXT_PRIMARY)
                                            .strong(),
                                        None => egui::RichText::new("—")
                                            .color(super::theme::TEXT_DISABLED),
                                    };
                                    ui.allocate_ui_with_layout(
                                        egui::Vec2::new(label_w, 26.0),
                                        egui::Layout::centered_and_justified(
                                            egui::Direction::LeftToRight,
                                        ),
                                        |ui| {
                                            ui.add(egui::Label::new(label_rich).truncate());
                                        },
                                    );

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
                        &self.offline_mode,
                        &self.auto_update_on_recall,
                        &self.console_snapshot_follow,
                        &self.snapshots.scope_editor.console_recall,
                        &self.dirty_tracker,
                        &self.pending_engines,
                        &self.connected,
                        &mut self.cancel_token,
                        &self.osc_log,
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
    }
}
