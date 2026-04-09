use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::console::cue_manager::CueManager;
use crate::console::eq_palette_manager::EqPaletteManager;
use crate::console::gang_manager::GangManager;
use crate::console::macro_engine::MacroEngine;
use crate::console::macro_manager::MacroManager;
use crate::console::monitor_manager::MonitorManager;
use crate::console::snapshot_engine::SnapshotEngine;
use crate::model::config::ConsoleConfig;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::osc_log::OscLog;
use crate::model::snapshot::CueList;
use crate::model::operating_mode::OperatingMode;
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::ipad_client::IpadSender;

use super::{Tab, UiEvent};
use super::eq_palettes_ui::EqPalettesUiState;
use super::gangs_tab::GangsTabState;
use super::inspector_tab::InspectorTabState;
use super::live_tab::LiveTabState;
use super::macros_tab::MacrosTabState;
use super::monitor_tab::MonitorTabState;
use super::osc_log_tab::OscLogTabState;
use super::setup_tab::SetupTabState;
use super::snapshots_tab::SnapshotsTabState;

/// Main application struct implementing eframe::App.
pub struct HiJackApp {
    // Shared state
    pub state: Arc<RwLock<ConsoleState>>,
    pub cue_manager: Arc<RwLock<CueManager>>,
    pub macro_manager: Arc<RwLock<MacroManager>>,
    pub monitor_manager: Arc<RwLock<MonitorManager>>,
    pub eq_palette_manager: Arc<RwLock<EqPaletteManager>>,
    pub gang_manager: Arc<RwLock<GangManager>>,
    pub snapshot_engine: Option<Arc<SnapshotEngine>>,
    pub macro_engine: Option<Arc<MacroEngine>>,
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
    pub live: LiveTabState,
    pub eq_palettes_ui: EqPalettesUiState,
    pub gangs: GangsTabState,
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
        runtime: tokio::runtime::Handle,
    ) -> Self {
        let (ui_tx, ui_rx) = std::sync::mpsc::channel();

        Self {
            state: Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default()))),
            cue_manager: Arc::new(RwLock::new(CueManager::new(CueList::default()))),
            macro_manager: Arc::new(RwLock::new(MacroManager::new())),
            monitor_manager: Arc::new(RwLock::new(MonitorManager::new())),
            eq_palette_manager: Arc::new(RwLock::new(EqPaletteManager::new())),
            gang_manager: Arc::new(RwLock::new(GangManager::new())),
            snapshot_engine: None,
            macro_engine: None,
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
                console_ip, console_port, local_port, trigger_port,
                operating_mode, ipad_ip, ipad_send_port, ipad_receive_port,
                monitor_port,
            ),
            snapshots: SnapshotsTabState::default(),
            macros: MacrosTabState::default(),
            live: LiveTabState::default(),
            eq_palettes_ui: EqPalettesUiState::default(),
            gangs: GangsTabState::default(),
            monitor: MonitorTabState::default(),
            osc_log_tab: OscLogTabState::default(),
            inspector: InspectorTabState::default(),
        }
    }

    /// Process UI events from async tasks.
    fn drain_events(&mut self) {
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
                    self.setup.ipad_connected = false;
                    self.setup.status_message = Some("Disconnected".into());
                    self.monitor.monitor_server_running = false;
                }
                UiEvent::SnapshotCaptured { name, param_count } => {
                    self.snapshots.status_message = Some(
                        format!("Captured '{name}' ({param_count} params)"),
                    );
                }
                UiEvent::CueRecalled { cue_number, params_sent } => {
                    self.live.last_recall_info = Some(
                        format!("Cue {cue_number:.1} recalled ({params_sent} params sent)"),
                    );
                }
                UiEvent::MacroExecuted { name, steps_executed } => {
                    self.macros.last_execution_info = Some(
                        format!("Executed '{name}' ({steps_executed} steps sent)"),
                    );
                    self.live.last_recall_info = Some(
                        format!("Macro '{name}' ({steps_executed} steps)"),
                    );
                }
                UiEvent::MacroRecordingStopped { step_count } => {
                    self.macros.status_message = Some(
                        format!("Recording stopped: {step_count} steps captured"),
                    );
                }
                UiEvent::PaletteCaptured { name, param_count } => {
                    self.eq_palettes_ui.status_message = Some(
                        format!("Captured palette '{name}' ({param_count} EQ params)"),
                    );
                }
                UiEvent::PaletteLinked { palette_name, snapshot_name } => {
                    self.eq_palettes_ui.status_message = Some(
                        format!("Linked '{palette_name}' to '{snapshot_name}'"),
                    );
                }
                UiEvent::PaletteUpdated { name, affected_count } => {
                    self.eq_palettes_ui.status_message = Some(
                        format!("Updated '{name}' — {affected_count} snapshots affected"),
                    );
                }
                UiEvent::ShowFileLoaded(path, conn) => {
                    self.setup.status_message = Some(format!("Loaded: {path}"));
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
                        self.setup.channel_option = conn.channel_option;
                        self.setup.aux_count = conn.aux_count.to_string();
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
                UiEvent::FadeProgress { cue_number, progress, done } => {
                    if done {
                        self.live.fade_progress = None;
                    } else {
                        self.live.fade_progress = Some((cue_number, progress));
                    }
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
            .frame(egui::Frame::new()
                .fill(super::theme::BG_DARK)
                .inner_margin(egui::Margin::symmetric(8, 4)))
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

                    // Tab buttons
                    let tabs = [
                        (Tab::Setup, "Setup"),
                        (Tab::Snapshots, "Snapshots"),
                        (Tab::Macros, "Macros"),
                        (Tab::Live, "Live"),
                        (Tab::Gangs, "Gangs"),
                        (Tab::Monitor, "Monitor"),
                        (Tab::OscLog, "OSC Log"),
                        (Tab::Inspector, "Inspector"),
                    ];
                    for (tab, label) in tabs {
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

                    // Connection status (right-aligned)
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let is_connected = self.connected.load(std::sync::atomic::Ordering::Relaxed);
                        let (color, text) = if is_connected {
                            (super::theme::COLOR_CONNECTED, "Connected")
                        } else {
                            (super::theme::COLOR_DISCONNECTED, "Disconnected")
                        };
                        ui.colored_label(color, text);
                        super::theme::status_dot(ui, color);
                    });
                });
            });

        // Main content
        egui::CentralPanel::default().show(ctx, |ui| {
            match self.active_tab {
                Tab::Setup => {
                    super::setup_tab::draw_setup_tab(
                        ui,
                        &mut self.setup,
                        &self.state,
                        &self.cue_manager,
                        &self.macro_manager,
                        &self.monitor_manager,
                        &self.eq_palette_manager,
                        &self.gang_manager,
                        &self.dirty_tracker,
                        &mut self.snapshot_engine,
                        &mut self.sender,
                        &self.connected,
                        &mut self.cancel_token,
                        &self.osc_log,
                        &self.runtime,
                        &self.ui_tx,
                        &self.egui_ctx,
                    );
                }
                Tab::Snapshots => {
                    super::snapshots_tab::draw_snapshots_tab(
                        ui,
                        &mut self.snapshots,
                        &mut self.eq_palettes_ui,
                        &self.state,
                        &self.cue_manager,
                        &self.eq_palette_manager,
                        &self.snapshot_engine,
                        &self.dirty_tracker,
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
                Tab::Live => {
                    super::live_tab::draw_live_tab(
                        ui,
                        &mut self.live,
                        &self.cue_manager,
                        &self.macro_manager,
                        &self.eq_palette_manager,
                        &self.snapshot_engine,
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
                        &self.connected,
                        &self.runtime,
                    );
                }
                Tab::Monitor => {
                    super::monitor_tab::draw_monitor_tab(
                        ui,
                        &mut self.monitor,
                        &self.monitor_manager,
                        &self.connected,
                        &self.runtime,
                    );
                }
                Tab::OscLog => {
                    super::osc_log_tab::draw_osc_log_tab(
                        ui,
                        &mut self.osc_log_tab,
                        &self.osc_log,
                    );
                }
                Tab::Inspector => {
                    super::inspector_tab::draw_inspector_tab(
                        ui,
                        &mut self.inspector,
                        &self.state,
                    );
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
