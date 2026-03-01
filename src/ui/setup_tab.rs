use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use tracing::{info, error};

use crate::console::connection::ConnectionManager;
use crate::console::cue_manager::CueManager;
use crate::console::eq_palette_manager::EqPaletteManager;
use crate::console::ipad_connection;
use crate::console::macro_engine::MacroEngine;
use crate::console::macro_manager::MacroManager;
use crate::console::monitor_engine::MonitorEngine;
use crate::console::monitor_manager::MonitorManager;
use crate::console::snapshot_engine::SnapshotEngine;
use crate::model::operating_mode::OperatingMode;
use crate::model::snapshot::CueList;
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::monitor_server::MonitorServer;
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
    pub operating_mode: OperatingMode,
    pub ipad_ip: String,
    pub ipad_send_port: String,
    pub ipad_receive_port: String,
    pub ipad_connected: bool,
    pub monitor_port: String,
}

impl SetupTabState {
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
    ) -> Self {
        Self {
            console_ip: console_ip.to_string(),
            console_port: console_port.to_string(),
            local_port: local_port.to_string(),
            trigger_port: trigger_port.to_string(),
            show_file_path: String::new(),
            status_message: None,
            operating_mode,
            ipad_ip: ipad_ip.unwrap_or("").to_string(),
            ipad_send_port: if ipad_send_port > 0 {
                ipad_send_port.to_string()
            } else {
                "8001".to_string()
            },
            ipad_receive_port: if ipad_receive_port > 0 {
                ipad_receive_port.to_string()
            } else {
                "8001".to_string()
            },
            ipad_connected: false,
            monitor_port: if monitor_port > 0 {
                monitor_port.to_string()
            } else {
                String::new()
            },
        }
    }
}

/// Draw the Setup tab.
pub fn draw_setup_tab(
    ui: &mut egui::Ui,
    setup: &mut SetupTabState,
    state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    macro_manager: &Arc<RwLock<MacroManager>>,
    monitor_manager: &Arc<RwLock<MonitorManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
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
                    setup, state, cue_manager, macro_manager, monitor_manager,
                    eq_palette_manager, snapshot_engine, sender, connected,
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
        egui::ComboBox::from_id_salt("operating_mode")
            .selected_text(setup.operating_mode.label())
            .width(200.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut setup.operating_mode, OperatingMode::Mode1, OperatingMode::Mode1.label());
                ui.selectable_value(&mut setup.operating_mode, OperatingMode::Mode2, OperatingMode::Mode2.label());
                ui.selectable_value(&mut setup.operating_mode, OperatingMode::Mode3, OperatingMode::Mode3.label());
            });
    });

    // iPad protocol settings (visible when mode 2 or 3)
    if setup.operating_mode.uses_ipad_protocol() {
        ui.add_space(4.0);

        egui::Grid::new("ipad_fields")
            .num_columns(2)
            .spacing([10.0, 6.0])
            .show(ui, |ui| {
                ui.label("Console iPad Port (send to):");
                ui.add_enabled(
                    !is_connected,
                    egui::TextEdit::singleline(&mut setup.ipad_send_port).desired_width(80.0),
                );
                ui.end_row();

                ui.label("Local Receive Port (listen on):");
                ui.add_enabled(
                    !is_connected,
                    egui::TextEdit::singleline(&mut setup.ipad_receive_port).desired_width(80.0),
                );
                ui.end_row();

                if setup.operating_mode == OperatingMode::Mode3 {
                    ui.label("iPad IP:");
                    ui.add_enabled(
                        !is_connected,
                        egui::TextEdit::singleline(&mut setup.ipad_ip)
                            .desired_width(200.0)
                            .hint_text("auto-detected from first packet"),
                    );
                    ui.end_row();
                }
            });

        // iPad connection status
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let (color, text) = if setup.ipad_connected {
                (super::theme::COLOR_CONNECTED, "iPad Connected")
            } else {
                (super::theme::COLOR_DISCONNECTED, "iPad Not Connected")
            };
            let circle_size = 10.0;
            let (rect, _) = ui.allocate_exact_size(
                egui::Vec2::splat(circle_size),
                egui::Sense::hover(),
            );
            ui.painter().circle_filled(rect.center(), circle_size / 2.0, color);
            ui.label(text);
        });
    }

    // Monitor server port
    ui.add_space(4.0);
    egui::Grid::new("monitor_fields")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Monitor Port:");
            ui.add_enabled(
                !is_connected,
                egui::TextEdit::singleline(&mut setup.monitor_port)
                    .desired_width(80.0)
                    .hint_text("disabled"),
            );
            ui.end_row();
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
            load_show_file(setup, cue_manager, macro_manager, monitor_manager, eq_palette_manager, runtime, ui_tx);
        }
        if ui.button("Save Show").clicked() {
            save_show_file(setup, state, cue_manager, macro_manager, monitor_manager, eq_palette_manager, runtime, ui_tx);
        }
        if ui.button("New Show").clicked() {
            let cue_mgr = cue_manager.clone();
            let macro_mgr = macro_manager.clone();
            let eq_mgr = eq_palette_manager.clone();
            runtime.spawn(async move {
                let mut mgr = cue_mgr.write().await;
                mgr.cue_list = CueList::default();
                mgr.snapshots.clear();
                mgr.scope_templates.clear();
                drop(mgr);
                let mut mmgr = macro_mgr.write().await;
                mmgr.macros.clear();
                mmgr.quick_trigger_ids.clear();
                drop(mmgr);
                let mut pmgr = eq_mgr.write().await;
                pmgr.palettes.clear();
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
    macro_manager: &Arc<RwLock<MacroManager>>,
    monitor_manager: &Arc<RwLock<MonitorManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
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

    // Parse iPad fields
    let operating_mode = setup.operating_mode;
    let ipad_send_port: u16 = if operating_mode.uses_ipad_protocol() {
        match setup.ipad_send_port.parse() {
            Ok(p) if p > 0 => p,
            _ => {
                setup.status_message = Some("Invalid iPad send port".into());
                return;
            }
        }
    } else {
        0
    };
    let ipad_receive_port: u16 = if operating_mode.uses_ipad_protocol() {
        match setup.ipad_receive_port.parse() {
            Ok(p) => p,
            Err(_) => {
                setup.status_message = Some("Invalid iPad receive port".into());
                return;
            }
        }
    } else {
        0
    };

    let monitor_port: u16 = setup.monitor_port.parse().unwrap_or(0);

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
    let macro_mgr = macro_manager.clone();
    let mon_mgr = monitor_manager.clone();
    let eq_mgr = eq_palette_manager.clone();
    let conn_flag = connected.clone();
    let tx = ui_tx.clone();
    let ctx = egui_ctx.clone();
    let console_ip = setup.console_ip.clone();

    runtime.spawn(async move {
        match ConnectionManager::connect_with_state(local_addr, console_addr, st.clone(), macro_mgr).await {
            Ok(manager) => {
                info!("Connected to console via UI");
                conn_flag.store(true, Ordering::Relaxed);

                // Create SnapshotEngine (mut so we can set iPad sender before wrapping in Arc)
                let mut engine = SnapshotEngine::new(st.clone(), manager.sender());

                // iPad connection (Mode 2 or 3)
                if operating_mode.uses_ipad_protocol() && ipad_send_port > 0 {
                    let console_ipad_addr: SocketAddr = format!("{}:{}", console_ip, ipad_send_port)
                        .parse()
                        .expect("Invalid console iPad address");

                    match operating_mode {
                        OperatingMode::Mode2 => {
                            let ipad_local: SocketAddr = if ipad_receive_port > 0 {
                                format!("0.0.0.0:{}", ipad_receive_port).parse().unwrap()
                            } else {
                                "0.0.0.0:0".parse().unwrap()
                            };
                            match ipad_connection::connect_mode2(console_ipad_addr, ipad_local, st.clone()).await {
                                Ok((ipad_sender, result, _handle)) => {
                                    info!(
                                        name = %result.config.console_name,
                                        "UI Mode 2: iPad protocol connected"
                                    );
                                    engine.set_ipad_sender(Some(ipad_sender));
                                    let _ = tx.send(UiEvent::IpadConnected);
                                }
                                Err(e) => {
                                    error!("UI Mode 2: iPad connection failed: {e}");
                                    let _ = tx.send(UiEvent::IpadConnectionFailed(e.to_string()));
                                }
                            }
                        }
                        OperatingMode::Mode3 => {
                            let listen_addr: SocketAddr = format!("0.0.0.0:{}", ipad_receive_port)
                                .parse()
                                .expect("Invalid iPad listen address");
                            let outbound_addr: SocketAddr = "0.0.0.0:0".parse().unwrap();
                            match ipad_connection::connect_mode3(console_ipad_addr, listen_addr, outbound_addr, st.clone()).await {
                                Ok((ipad_sender, _forwarder, result, _handle)) => {
                                    info!(
                                        name = %result.config.console_name,
                                        "UI Mode 3: iPad proxy started"
                                    );
                                    engine.set_ipad_sender(Some(ipad_sender));
                                    let _ = tx.send(UiEvent::IpadConnected);
                                }
                                Err(e) => {
                                    error!("UI Mode 3: iPad proxy setup failed: {e}");
                                    let _ = tx.send(UiEvent::IpadConnectionFailed(e.to_string()));
                                }
                            }
                        }
                        OperatingMode::Mode1 => {}
                    }
                }

                let engine = Arc::new(engine);

                // Start trigger listener
                match TriggerListener::start(trigger_addr).await {
                    Ok(mut trigger_rx) => {
                        let macro_eng = Arc::new(MacroEngine::new(st.clone(), manager.sender()));
                        let trigger_cue_mgr = cue_mgr.clone();
                        let trigger_macro_mgr = manager.macro_manager();
                        let trigger_eq_mgr = eq_mgr.clone();
                        let trigger_engine = engine.clone();
                        let trigger_macro_eng = macro_eng.clone();

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
                                                let pmgr = trigger_eq_mgr.read().await;
                                                let result = trigger_engine.recall_cue(&cue, &snapshot, &pmgr.palettes).await;
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
                                                let pmgr = trigger_eq_mgr.read().await;
                                                let result = trigger_engine.recall_cue(&cue, &snapshot, &pmgr.palettes).await;
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
                                                let pmgr = trigger_eq_mgr.read().await;
                                                let result = trigger_engine.recall_cue(&cue, &snapshot, &pmgr.palettes).await;
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
                                        let mgr = trigger_macro_mgr.read().await;
                                        if let Some(macro_def) = mgr.find_by_name_or_id(&name).cloned() {
                                            drop(mgr);
                                            let result = trigger_macro_eng.execute(&macro_def).await;
                                            info!(
                                                name = %result.macro_name,
                                                executed = result.steps_executed,
                                                skipped = result.steps_skipped,
                                                "Trigger MacroFire complete"
                                            );
                                        } else {
                                            tracing::warn!(name, "MacroFire: macro not found");
                                        }
                                    }
                                }
                            }
                        });
                    }
                    Err(e) => {
                        error!("Failed to start trigger listener: {e}");
                    }
                }

                // Start monitor server (if port configured)
                if monitor_port > 0 {
                    let monitor_addr: SocketAddr = format!("0.0.0.0:{}", monitor_port)
                        .parse()
                        .expect("Invalid monitor address");
                    match MonitorServer::start(monitor_addr).await {
                        Ok((monitor_sender, mut monitor_rx)) => {
                            info!(port = monitor_port, "Monitor server started via UI");
                            let monitor_engine = MonitorEngine::new(st.clone(), manager.sender());
                            let mon_mgr_loop = mon_mgr.clone();
                            let tx_monitor = tx.clone();
                            let _ = tx_monitor.send(UiEvent::MonitorServerStarted);
                            tokio::spawn(async move {
                                let mut last_send_state = std::collections::HashMap::new();
                                let mut poll_interval = tokio::time::interval(
                                    std::time::Duration::from_millis(500),
                                );
                                loop {
                                    tokio::select! {
                                        Some(cmd) = monitor_rx.recv() => {
                                            let mut mgr = mon_mgr_loop.write().await;
                                            monitor_engine.handle_command(
                                                cmd, &mut mgr, &monitor_sender, true,
                                            ).await;
                                        }
                                        _ = poll_interval.tick() => {
                                            let mgr = mon_mgr_loop.read().await;
                                            monitor_engine.poll_and_push_state_changes(
                                                &mut last_send_state,
                                                &mgr,
                                                &monitor_sender,
                                            ).await;
                                        }
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            error!("Failed to start monitor server: {e}");
                            let _ = tx.send(UiEvent::MonitorServerFailed(e.to_string()));
                        }
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
    macro_manager: &Arc<RwLock<MacroManager>>,
    monitor_manager: &Arc<RwLock<MonitorManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    if setup.show_file_path.is_empty() {
        setup.status_message = Some("Enter a file path first".into());
        return;
    }

    let path = std::path::PathBuf::from(&setup.show_file_path);
    let cue_mgr = cue_manager.clone();
    let macro_mgr = macro_manager.clone();
    let mon_mgr = monitor_manager.clone();
    let eq_mgr = eq_palette_manager.clone();
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
                drop(mgr);

                // Restore macros
                let mut mmgr = macro_mgr.write().await;
                mmgr.macros.clear();
                for macro_def in show.macros {
                    mmgr.macros.insert(macro_def.id, macro_def);
                }
                mmgr.quick_trigger_ids = show.macro_quick_trigger_ids;
                drop(mmgr);

                // Restore EQ palettes
                let mut pmgr = eq_mgr.write().await;
                pmgr.palettes.clear();
                for palette in show.eq_palettes {
                    pmgr.palettes.insert(palette.id, palette);
                }
                drop(pmgr);

                // Restore monitor clients
                let mut monmgr = mon_mgr.write().await;
                monmgr.clients.clear();
                for client in show.monitor_clients {
                    monmgr.clients.insert(client.id, client);
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
    macro_manager: &Arc<RwLock<MacroManager>>,
    monitor_manager: &Arc<RwLock<MonitorManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
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
    let macro_mgr = macro_manager.clone();
    let mon_mgr = monitor_manager.clone();
    let eq_mgr = eq_palette_manager.clone();
    let tx = ui_tx.clone();
    let path_str = setup.show_file_path.clone();

    runtime.spawn(async move {
        let state_guard = st.read().await;
        let mgr = cue_mgr.read().await;
        let mmgr = macro_mgr.read().await;
        let monmgr = mon_mgr.read().await;
        let pmgr = eq_mgr.read().await;

        let show = ShowFile {
            version: 4,
            console_config: state_guard.config.clone(),
            scope_templates: mgr.scope_templates.values().cloned().collect(),
            snapshots: mgr.snapshots.values().cloned().collect(),
            cue_list: mgr.cue_list.clone(),
            macros: mmgr.macros.values().cloned().collect(),
            macro_quick_trigger_ids: mmgr.quick_trigger_ids.clone(),
            eq_palettes: pmgr.palettes.values().cloned().collect(),
            monitor_clients: monmgr.clients.values().cloned().collect(),
        };

        drop(state_guard);
        drop(mgr);
        drop(mmgr);
        drop(monmgr);
        drop(pmgr);

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
