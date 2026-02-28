use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use tracing::{info, error};

use crate::console::connection::ConnectionManager;
use crate::console::cue_manager::CueManager;
use crate::console::snapshot_engine::SnapshotEngine;
use crate::model::snapshot::CueList;
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::trigger_listener::TriggerListener;
use crate::persistence::show_file::ShowFile;
use super::UiEvent;

/// State for the Setup tab.
pub struct SetupTabState {
    pub console_ip: String,
    pub console_port: String,
    pub local_port: String,
    pub trigger_port: String,
    pub show_file_path: String,
    pub status_message: Option<String>,
}

impl SetupTabState {
    pub fn new(console_ip: &str, console_port: u16, local_port: u16, trigger_port: u16) -> Self {
        Self {
            console_ip: console_ip.to_string(),
            console_port: console_port.to_string(),
            local_port: local_port.to_string(),
            trigger_port: trigger_port.to_string(),
            show_file_path: String::new(),
            status_message: None,
        }
    }
}

/// Draw the Setup tab.
pub fn draw_setup_tab(
    ui: &mut egui::Ui,
    setup: &mut SetupTabState,
    state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    snapshot_engine: &mut Option<Arc<SnapshotEngine>>,
    sender: &mut Option<OscSender>,
    connected: &Arc<AtomicBool>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
    egui_ctx: &Arc<std::sync::OnceLock<egui::Context>>,
) {
    let is_connected = connected.load(Ordering::Relaxed);

    ui.heading("Console Connection");
    ui.separator();

    egui::Grid::new("connection_fields")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Console IP:");
            ui.add_enabled(!is_connected, egui::TextEdit::singleline(&mut setup.console_ip).desired_width(200.0));
            ui.end_row();

            ui.label("GP OSC Port:");
            ui.add_enabled(!is_connected, egui::TextEdit::singleline(&mut setup.console_port).desired_width(80.0));
            ui.end_row();

            ui.label("Local Port:");
            ui.add_enabled(!is_connected, egui::TextEdit::singleline(&mut setup.local_port).desired_width(80.0));
            ui.end_row();

            ui.label("Trigger Port:");
            ui.add_enabled(!is_connected, egui::TextEdit::singleline(&mut setup.trigger_port).desired_width(80.0));
            ui.end_row();
        });

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if !is_connected {
            if ui.button("Connect").clicked() {
                start_connection(
                    setup, state, cue_manager, snapshot_engine, sender, connected,
                    runtime, ui_tx, egui_ctx,
                );
            }
        } else {
            ui.add_enabled(false, egui::Button::new("Connected"));
        }
    });

    // Connection status
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let (color, text) = if is_connected {
            (super::theme::COLOR_CONNECTED, "Connected")
        } else {
            (super::theme::COLOR_DISCONNECTED, "Disconnected")
        };
        let circle_size = 12.0;
        let (rect, _) = ui.allocate_exact_size(
            egui::Vec2::splat(circle_size),
            egui::Sense::hover(),
        );
        ui.painter().circle_filled(rect.center(), circle_size / 2.0, color);
        ui.label(text);
    });

    // Status message
    if let Some(msg) = &setup.status_message {
        ui.add_space(4.0);
        ui.colored_label(egui::Color32::YELLOW, msg);
    }

    ui.add_space(8.0);
    ui.separator();

    // Operating mode
    ui.horizontal(|ui| {
        ui.label("Operating Mode:");
        ui.label("Mode 1 (GP OSC)");
        ui.weak("  Modes 2/3: Phase 6");
    });

    ui.add_space(8.0);
    ui.separator();

    // Console info (from state mirror)
    ui.heading("Console Info");
    if let Ok(st) = state.try_read() {
        let cfg = &st.config;
        egui::Grid::new("console_info")
            .num_columns(2)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                if !cfg.console_name.is_empty() {
                    ui.label("Console:");
                    ui.label(&cfg.console_name);
                    ui.end_row();
                }
                if !cfg.console_serial.is_empty() {
                    ui.label("Serial:");
                    ui.label(&cfg.console_serial);
                    ui.end_row();
                }
                if let Some(ref session) = cfg.session_filename {
                    ui.label("Session:");
                    ui.label(session);
                    ui.end_row();
                }
            });

        ui.add_space(8.0);
        ui.heading("Channel Configuration");
        egui::Grid::new("channel_counts")
            .num_columns(4)
            .spacing([20.0, 4.0])
            .show(ui, |ui| {
                ui.label(format!("Inputs: {}", cfg.input_channel_count));
                ui.label(format!("Aux: {}", cfg.aux_output_count));
                ui.label(format!("Groups: {}", cfg.group_output_count));
                ui.label(format!("Matrix: {}", cfg.matrix_output_count));
                ui.end_row();
                ui.label(format!("CGs: {}", cfg.control_group_count));
                ui.label(format!("GEQ: {}", cfg.graphic_eq_count));
                ui.label(format!("Mtx In: {}", cfg.matrix_input_count));
                ui.label(format!("Params: {}", st.parameter_count()));
                ui.end_row();
            });
    } else {
        ui.label("Loading state...");
    }

    ui.add_space(8.0);
    ui.separator();

    // Show file management
    ui.heading("Show File");
    ui.horizontal(|ui| {
        if ui.button("Load Show").clicked() {
            load_show_file(setup, cue_manager, runtime, ui_tx);
        }
        if ui.button("Save Show").clicked() {
            save_show_file(setup, state, cue_manager, runtime, ui_tx);
        }
        if ui.button("New Show").clicked() {
            let cue_mgr = cue_manager.clone();
            runtime.spawn(async move {
                let mut mgr = cue_mgr.write().await;
                mgr.cue_list = CueList::default();
                mgr.snapshots.clear();
                mgr.scope_templates.clear();
            });
            setup.show_file_path.clear();
            setup.status_message = Some("New show created".into());
        }
    });
    if !setup.show_file_path.is_empty() {
        ui.label(format!("File: {}", setup.show_file_path));
    }
}

fn start_connection(
    setup: &mut SetupTabState,
    state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    _snapshot_engine: &mut Option<Arc<SnapshotEngine>>,
    _sender: &mut Option<OscSender>,
    connected: &Arc<AtomicBool>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
    egui_ctx: &Arc<std::sync::OnceLock<egui::Context>>,
) {
    let console_port: u16 = match setup.console_port.parse() {
        Ok(p) => p,
        Err(_) => {
            setup.status_message = Some("Invalid console port".into());
            return;
        }
    };
    let local_port: u16 = match setup.local_port.parse() {
        Ok(p) => p,
        Err(_) => {
            setup.status_message = Some("Invalid local port".into());
            return;
        }
    };
    let trigger_port: u16 = match setup.trigger_port.parse() {
        Ok(p) => p,
        Err(_) => {
            setup.status_message = Some("Invalid trigger port".into());
            return;
        }
    };

    let console_addr_str = format!("{}:{}", setup.console_ip, console_port);
    let console_addr: SocketAddr = match console_addr_str.parse() {
        Ok(a) => a,
        Err(_) => {
            setup.status_message = Some("Invalid console address".into());
            return;
        }
    };
    let local_addr: SocketAddr = format!("0.0.0.0:{}", local_port)
        .parse()
        .expect("Invalid local address");
    let trigger_addr: SocketAddr = format!("0.0.0.0:{}", trigger_port)
        .parse()
        .expect("Invalid trigger address");

    setup.status_message = Some("Connecting...".into());

    let st = state.clone();
    let cue_mgr = cue_manager.clone();
    let conn_flag = connected.clone();
    let tx = ui_tx.clone();
    let ctx = egui_ctx.clone();

    runtime.spawn(async move {
        match ConnectionManager::connect_with_state(local_addr, console_addr, st.clone()).await {
            Ok(manager) => {
                info!("Connected to console via UI");
                conn_flag.store(true, Ordering::Relaxed);

                // Start trigger listener
                match TriggerListener::start(trigger_addr).await {
                    Ok(mut trigger_rx) => {
                        let engine = Arc::new(SnapshotEngine::new(st.clone(), manager.sender()));
                        let trigger_cue_mgr = cue_mgr.clone();
                        let trigger_engine = engine.clone();

                        // Spawn trigger processing
                        tokio::spawn(async move {
                            use crate::osc::trigger_listener::TriggerEvent;
                            while let Some(event) = trigger_rx.recv().await {
                                match event {
                                    TriggerEvent::GoNext => {
                                        let mut mgr = trigger_cue_mgr.write().await;
                                        if let Some(cue) = mgr.go_next() {
                                            let cue = cue.clone();
                                            if let Some(snapshot) = mgr.get_snapshot(&cue.snapshot_id).cloned() {
                                                drop(mgr);
                                                let result = trigger_engine.recall_cue(&cue, &snapshot).await;
                                                info!(sent = result.parameters_sent, "Trigger GO recall complete");
                                            }
                                        }
                                    }
                                    TriggerEvent::GoPrevious => {
                                        let mut mgr = trigger_cue_mgr.write().await;
                                        if let Some(cue) = mgr.go_previous() {
                                            let cue = cue.clone();
                                            if let Some(snapshot) = mgr.get_snapshot(&cue.snapshot_id).cloned() {
                                                drop(mgr);
                                                let result = trigger_engine.recall_cue(&cue, &snapshot).await;
                                                info!(sent = result.parameters_sent, "Trigger PREV recall complete");
                                            }
                                        }
                                    }
                                    TriggerEvent::FireCue(number) => {
                                        let mut mgr = trigger_cue_mgr.write().await;
                                        if let Some(cue) = mgr.fire_cue_number(number) {
                                            let cue = cue.clone();
                                            if let Some(snapshot) = mgr.get_snapshot(&cue.snapshot_id).cloned() {
                                                drop(mgr);
                                                let result = trigger_engine.recall_cue(&cue, &snapshot).await;
                                                info!(number, sent = result.parameters_sent, "Trigger FIRE recall complete");
                                            }
                                        }
                                    }
                                    TriggerEvent::QueryCurrent { reply_addr } => {
                                        let mgr = trigger_cue_mgr.read().await;
                                        let current = mgr.current_cue_number().unwrap_or(-1.0);
                                        info!(current, %reply_addr, "Trigger /cue/current query");
                                    }
                                    TriggerEvent::MacroFire(name) => {
                                        tracing::warn!(name, "Macro triggers not yet implemented (Phase 4)");
                                    }
                                }
                            }
                        });
                    }
                    Err(e) => {
                        error!("Failed to start trigger listener: {e}");
                    }
                }

                let _ = tx.send(UiEvent::ConnectionEstablished);
                if let Some(ctx) = ctx.get() {
                    ctx.request_repaint();
                }
            }
            Err(e) => {
                error!("Connection failed: {e}");
                let _ = tx.send(UiEvent::ConnectionFailed(e.to_string()));
                if let Some(ctx) = ctx.get() {
                    ctx.request_repaint();
                }
            }
        }
    });
}

fn load_show_file(
    setup: &mut SetupTabState,
    cue_manager: &Arc<RwLock<CueManager>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    if setup.show_file_path.is_empty() {
        setup.status_message = Some("Enter a file path first".into());
        return;
    }

    let path = std::path::PathBuf::from(&setup.show_file_path);
    let cue_mgr = cue_manager.clone();
    let tx = ui_tx.clone();
    let path_str = setup.show_file_path.clone();

    runtime.spawn(async move {
        match ShowFile::load(&path).await {
            Ok(show) => {
                let mut mgr = cue_mgr.write().await;
                mgr.cue_list = show.cue_list;
                mgr.snapshots.clear();
                for snap in show.snapshots {
                    mgr.snapshots.insert(snap.id, snap);
                }
                mgr.scope_templates.clear();
                for tmpl in show.scope_templates {
                    mgr.scope_templates.insert(tmpl.id, tmpl);
                }
                info!("Show file loaded: {path_str}");
                let _ = tx.send(UiEvent::ShowFileLoaded(path_str));
            }
            Err(e) => {
                let _ = tx.send(UiEvent::ShowFileError(format!("Load failed: {e}")));
            }
        }
    });
}

fn save_show_file(
    setup: &mut SetupTabState,
    state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    if setup.show_file_path.is_empty() {
        setup.status_message = Some("Enter a file path first".into());
        return;
    }

    let path = std::path::PathBuf::from(&setup.show_file_path);
    let st = state.clone();
    let cue_mgr = cue_manager.clone();
    let tx = ui_tx.clone();
    let path_str = setup.show_file_path.clone();

    runtime.spawn(async move {
        let state_guard = st.read().await;
        let mgr = cue_mgr.read().await;

        let show = ShowFile {
            version: 2,
            console_config: state_guard.config.clone(),
            scope_templates: mgr.scope_templates.values().cloned().collect(),
            snapshots: mgr.snapshots.values().cloned().collect(),
            cue_list: mgr.cue_list.clone(),
        };

        drop(state_guard);
        drop(mgr);

        match show.save(&path).await {
            Ok(()) => {
                info!("Show file saved: {path_str}");
                let _ = tx.send(UiEvent::ShowFileSaved(path_str));
            }
            Err(e) => {
                let _ = tx.send(UiEvent::ShowFileError(format!("Save failed: {e}")));
            }
        }
    });
}
