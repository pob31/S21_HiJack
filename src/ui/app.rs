use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;

use crate::console::cue_manager::CueManager;
use crate::console::macro_engine::MacroEngine;
use crate::console::macro_manager::MacroManager;
use crate::console::snapshot_engine::SnapshotEngine;
use crate::model::config::ConsoleConfig;
use crate::model::snapshot::CueList;
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;

use super::{Tab, UiEvent};
use super::live_tab::LiveTabState;
use super::macros_tab::MacrosTabState;
use super::setup_tab::SetupTabState;
use super::snapshots_tab::SnapshotsTabState;

/// Main application struct implementing eframe::App.
pub struct HiJackApp {
    // Shared state
    pub state: Arc<RwLock<ConsoleState>>,
    pub cue_manager: Arc<RwLock<CueManager>>,
    pub macro_manager: Arc<RwLock<MacroManager>>,
    pub snapshot_engine: Option<Arc<SnapshotEngine>>,
    pub macro_engine: Option<Arc<MacroEngine>>,

    // Async bridge
    pub runtime: tokio::runtime::Handle,
    pub egui_ctx: Arc<std::sync::OnceLock<egui::Context>>,
    pub ui_rx: std::sync::mpsc::Receiver<UiEvent>,
    pub ui_tx: std::sync::mpsc::Sender<UiEvent>,

    // Connection state
    pub connected: Arc<AtomicBool>,
    pub sender: Option<OscSender>,

    // Tab state
    pub active_tab: Tab,
    pub setup: SetupTabState,
    pub snapshots: SnapshotsTabState,
    pub macros: MacrosTabState,
    pub live: LiveTabState,
}

impl HiJackApp {
    pub fn new(
        console_ip: &str,
        console_port: u16,
        local_port: u16,
        trigger_port: u16,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        let (ui_tx, ui_rx) = std::sync::mpsc::channel();

        Self {
            state: Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default()))),
            cue_manager: Arc::new(RwLock::new(CueManager::new(CueList::default()))),
            macro_manager: Arc::new(RwLock::new(MacroManager::new())),
            snapshot_engine: None,
            macro_engine: None,

            runtime,
            egui_ctx: Arc::new(std::sync::OnceLock::new()),
            ui_rx,
            ui_tx,

            connected: Arc::new(AtomicBool::new(false)),
            sender: None,

            active_tab: Tab::Setup,
            setup: SetupTabState::new(console_ip, console_port, local_port, trigger_port),
            snapshots: SnapshotsTabState::default(),
            macros: MacrosTabState::default(),
            live: LiveTabState::default(),
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
                UiEvent::ShowFileLoaded(path) => {
                    self.setup.status_message = Some(format!("Loaded: {path}"));
                }
                UiEvent::ShowFileSaved(path) => {
                    self.setup.status_message = Some(format!("Saved: {path}"));
                }
                UiEvent::ShowFileError(msg) => {
                    self.setup.status_message = Some(msg);
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
        egui::TopBottomPanel::top("tab_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.active_tab, Tab::Setup, "Setup");
                ui.selectable_value(&mut self.active_tab, Tab::Snapshots, "Snapshots");
                ui.selectable_value(&mut self.active_tab, Tab::Macros, "Macros");
                ui.selectable_value(&mut self.active_tab, Tab::Live, "Live");
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
                        &mut self.snapshot_engine,
                        &mut self.sender,
                        &self.connected,
                        &self.runtime,
                        &self.ui_tx,
                        &self.egui_ctx,
                    );
                }
                Tab::Snapshots => {
                    super::snapshots_tab::draw_snapshots_tab(
                        ui,
                        &mut self.snapshots,
                        &self.state,
                        &self.cue_manager,
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
                        &self.snapshot_engine,
                        &self.macro_engine,
                        &self.connected,
                        &self.runtime,
                        &self.ui_tx,
                    );
                }
            }
        });
    }
}
