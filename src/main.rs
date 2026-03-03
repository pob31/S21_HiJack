// Public APIs are being built up across phases — suppress dead_code until all wired in.
#![allow(dead_code)]

mod console;
mod model;
mod osc;
mod persistence;
mod ui;

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use rosc::OscType;
use tokio::sync::RwLock;
use tracing::{info, warn, error};
use tracing_subscriber::EnvFilter;

use console::connection::ConnectionManager;
use console::cue_manager::CueManager;
use console::eq_palette_manager::EqPaletteManager;
use console::gang_engine::GangEngine;
use console::gang_manager::GangManager;
use console::ipad_connection;
use console::macro_engine::MacroEngine;
use console::macro_manager::MacroManager;
use console::monitor_engine::MonitorEngine;
use console::monitor_manager::MonitorManager;
use console::snapshot_engine::SnapshotEngine;
use model::operating_mode::OperatingMode;
use model::snapshot::CueList;
use osc::monitor_server::MonitorServer;
use osc::trigger_listener::{TriggerEvent, TriggerListener};

/// DiGiCo S21/S31 Snapshot Manager Daemon
#[derive(Parser, Debug)]
#[command(name = "s21_hijack", version, about)]
struct Args {
    /// Console IP address
    #[arg(long, default_value = "192.168.1.1")]
    console_ip: String,

    /// Console GP OSC port
    #[arg(long, default_value_t = 8000)]
    console_port: u16,

    /// Local UDP port to bind
    #[arg(long, default_value_t = 8001)]
    local_port: u16,

    /// QLab trigger listener port
    #[arg(long, default_value_t = 53001)]
    trigger_port: u16,

    /// Console iPad protocol port (send target). Overrides --ipad-port.
    #[arg(long)]
    ipad_send_port: Option<u16>,

    /// Local iPad receive port (listen on). Overrides --ipad-port.
    #[arg(long)]
    ipad_receive_port: Option<u16>,

    /// Legacy: single iPad port (used for both send/receive if split args not set)
    #[arg(long, default_value_t = 0)]
    ipad_port: u16,

    /// iPad device IP address (for Mode 3 proxy; optional for Mode 2)
    #[arg(long)]
    ipad_ip: Option<String>,

    /// Operating mode: mode1, mode2, mode3
    #[arg(long, default_value = "mode1")]
    mode: String,

    /// Monitor server port (0 = disabled)
    #[arg(long, default_value_t = 0)]
    monitor_port: u16,

    /// Run in headless mode (no UI, daemon only)
    #[arg(long)]
    headless: bool,
}

impl Args {
    /// Resolve effective iPad send port (console's iPad listening port).
    fn effective_ipad_send_port(&self) -> u16 {
        self.ipad_send_port.unwrap_or(self.ipad_port)
    }

    /// Resolve effective iPad receive port (our local listen port).
    fn effective_ipad_receive_port(&self) -> u16 {
        self.ipad_receive_port.unwrap_or(self.ipad_port)
    }
}

fn main() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,s21_hijack=debug")),
        )
        .init();

    let args = Args::parse();

    let mode = OperatingMode::from_cli(&args.mode).unwrap_or_default();

    info!(
        "S21 HiJack starting — console {}:{}, local port {}, trigger port {}, mode={}, ipad_send={}, ipad_recv={}, ipad_ip={:?}, monitor_port={}, headless={}",
        args.console_ip, args.console_port, args.local_port, args.trigger_port,
        mode, args.effective_ipad_send_port(), args.effective_ipad_receive_port(),
        args.ipad_ip, args.monitor_port, args.headless
    );

    if args.headless {
        let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        runtime.block_on(run_headless(args));
    } else {
        run_ui(args);
    }
}

/// Run in headless mode — the original daemon behavior.
async fn run_headless(args: Args) {
    let console_addr: SocketAddr = format!("{}:{}", args.console_ip, args.console_port)
        .parse()
        .unwrap_or_else(|e| {
            error!("Invalid console address: {e}");
            std::process::exit(1);
        });

    let local_addr: SocketAddr = format!("0.0.0.0:{}", args.local_port)
        .parse()
        .expect("Invalid local address");

    // Set up macro, monitor, and gang systems
    let macro_manager = Arc::new(RwLock::new(MacroManager::new()));
    let monitor_manager = Arc::new(RwLock::new(MonitorManager::new()));
    let gang_manager = Arc::new(RwLock::new(GangManager::new()));

    // Create state + OscClient so we can build GangEngine before spawning the loop
    let state = {
        let config = model::config::ConsoleConfig::default();
        Arc::new(RwLock::new(model::state::ConsoleState::new(config)))
    };
    let client = match osc::client::OscClient::new(local_addr, console_addr).await {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to create OSC client: {e}");
            std::process::exit(1);
        }
    };
    let (sender, rx) = client.into_parts();

    let gang_engine = Arc::new(RwLock::new(GangEngine::new(state.clone(), sender.clone())));

    let manager = ConnectionManager::connect_from_parts(
        sender.clone(), rx, state, macro_manager.clone(),
        gang_engine.clone(), gang_manager.clone(),
    );
    info!("Connected successfully");

    // Parse operating mode
    let mode = OperatingMode::from_cli(&args.mode).unwrap_or_default();

    // Set up snapshot, macro, and palette systems
    let cue_manager = Arc::new(RwLock::new(CueManager::new(CueList::default())));
    let eq_palette_manager = Arc::new(RwLock::new(EqPaletteManager::new()));
    let mut snapshot_engine = SnapshotEngine::new(manager.state(), manager.sender());
    let macro_engine = Arc::new(MacroEngine::new(manager.state(), manager.sender()));

    // iPad protocol connection (Mode 2 or 3)
    let send_port = args.effective_ipad_send_port();
    let recv_port = args.effective_ipad_receive_port();
    let mut ipad_sender_for_monitor = None;

    if mode.uses_ipad_protocol() && send_port > 0 {
        let console_ipad_addr: SocketAddr = format!("{}:{}", args.console_ip, send_port)
            .parse()
            .expect("Invalid console iPad address");

        match mode {
            OperatingMode::Mode2 => {
                let ipad_local: SocketAddr = if recv_port > 0 {
                    format!("0.0.0.0:{}", recv_port).parse().unwrap()
                } else {
                    "0.0.0.0:0".parse().unwrap()
                };
                match ipad_connection::connect_mode2(console_ipad_addr, ipad_local, manager.state()).await {
                    Ok((ipad_sender, result, _handle)) => {
                        info!(
                            name = %result.config.console_name,
                            "Mode 2: iPad protocol connected"
                        );
                        ipad_sender_for_monitor = Some(ipad_sender.clone());
                        snapshot_engine.set_ipad_sender(Some(ipad_sender));
                    }
                    Err(e) => {
                        error!("Mode 2: iPad connection failed: {e}");
                    }
                }
            }
            OperatingMode::Mode3 => {
                let listen_addr: SocketAddr = format!("0.0.0.0:{}", recv_port)
                    .parse()
                    .expect("Invalid iPad listen address (--ipad-receive-port required for Mode 3)");
                let outbound_addr: SocketAddr = "0.0.0.0:0".parse().unwrap();
                match ipad_connection::connect_mode3(console_ipad_addr, listen_addr, outbound_addr, manager.state()).await {
                    Ok((ipad_sender, _forwarder, result, _handle)) => {
                        info!(
                            name = %result.config.console_name,
                            ipad_ip = ?args.ipad_ip,
                            "Mode 3: iPad proxy started"
                        );
                        ipad_sender_for_monitor = Some(ipad_sender.clone());
                        snapshot_engine.set_ipad_sender(Some(ipad_sender));
                    }
                    Err(e) => {
                        error!("Mode 3: iPad proxy setup failed: {e}");
                    }
                }
            }
            OperatingMode::Mode1 => {}
        }
    }

    // Wire iPad sender into gang engine (for iPad-only parameters)
    if let Some(ref ipad) = ipad_sender_for_monitor {
        gang_engine.write().await.set_ipad_sender(Some(ipad.clone()));
    }

    // Start monitor server (if enabled)
    if args.monitor_port > 0 {
        let monitor_addr: SocketAddr = format!("0.0.0.0:{}", args.monitor_port)
            .parse()
            .expect("Invalid monitor address");
        match MonitorServer::start(monitor_addr).await {
            Ok((monitor_sender, mut monitor_rx)) => {
                info!(port = args.monitor_port, "Monitor server started");
                let mut monitor_engine = MonitorEngine::new(manager.state(), manager.sender());
                monitor_engine.set_ipad_sender(ipad_sender_for_monitor);
                let mon_mgr = monitor_manager.clone();
                tokio::spawn(async move {
                    let mut last_send_state = std::collections::HashMap::new();
                    let mut poll_interval = tokio::time::interval(std::time::Duration::from_millis(500));
                    loop {
                        tokio::select! {
                            Some(cmd) = monitor_rx.recv() => {
                                let mut mgr = mon_mgr.write().await;
                                monitor_engine.handle_command(cmd, &mut mgr, &monitor_sender, true).await;
                            }
                            _ = poll_interval.tick() => {
                                let mgr = mon_mgr.read().await;
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
            }
        }
    }

    // Start QLab trigger listener
    let trigger_addr: SocketAddr = format!("0.0.0.0:{}", args.trigger_port)
        .parse()
        .expect("Invalid trigger address");

    let mut trigger_rx = match TriggerListener::start(trigger_addr).await {
        Ok(rx) => rx,
        Err(e) => {
            error!("Failed to start trigger listener: {e}");
            std::process::exit(1);
        }
    };

    // Spawn trigger processing task
    let trigger_cue_mgr = cue_manager.clone();
    let trigger_macro_mgr = macro_manager.clone();
    let trigger_eq_mgr = eq_palette_manager.clone();
    let trigger_macro_eng = macro_engine.clone();
    let trigger_engine = Arc::new(snapshot_engine);
    let reply_socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await.ok();

    tokio::spawn(async move {
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
                            info!(sent = result.parameters_sent, "Cue GO recall complete");
                        } else {
                            warn!(snapshot_id = %cue.snapshot_id, "Snapshot not found for cue");
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
                            info!(sent = result.parameters_sent, "Cue PREVIOUS recall complete");
                        } else {
                            warn!(snapshot_id = %cue.snapshot_id, "Snapshot not found for cue");
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
                            info!(number, sent = result.parameters_sent, "Cue FIRE recall complete");
                        } else {
                            warn!(snapshot_id = %cue.snapshot_id, "Snapshot not found for cue");
                        }
                    }
                }
                TriggerEvent::QueryCurrent { reply_addr } => {
                    let mgr = trigger_cue_mgr.read().await;
                    let current = mgr.current_cue_number().unwrap_or(-1.0);
                    info!(current, %reply_addr, "Responding to /cue/current query");
                    if let Some(ref sock) = reply_socket {
                        let _ = osc::trigger_listener::send_reply(
                            sock,
                            reply_addr,
                            "/cue/current",
                            vec![OscType::Float(current)],
                        ).await;
                    }
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
                            "MacroFire trigger complete"
                        );
                    } else {
                        warn!(name, "MacroFire: macro not found");
                    }
                }
            }
        }
    });

    // Wait for shutdown signal
    info!("Daemon running (headless). Press Ctrl+C to stop.");
    match tokio::signal::ctrl_c().await {
        Ok(()) => {
            info!("Shutdown signal received");
        }
        Err(e) => {
            error!("Failed to listen for shutdown signal: {e}");
        }
    }

    // Log final state
    let state = manager.state();
    let count = state.read().await.parameter_count();
    info!(count, "Final state mirror parameter count");

    let mgr = cue_manager.read().await;
    info!(
        snapshots = mgr.snapshots.len(),
        cues = mgr.cue_list.cues.len(),
        scope_templates = mgr.scope_templates.len(),
        "Final snapshot system state"
    );

    let mmgr = macro_manager.read().await;
    info!(
        macros = mmgr.macros.len(),
        quick_triggers = mmgr.quick_trigger_ids.len(),
        "Final macro system state"
    );

    let pmgr = eq_palette_manager.read().await;
    info!(
        palettes = pmgr.palettes.len(),
        "Final EQ palette system state"
    );

    let mon_mgr = monitor_manager.read().await;
    info!(
        clients = mon_mgr.clients.len(),
        connected = mon_mgr.connected_count(),
        "Final monitor system state"
    );

    let gmgr = gang_manager.read().await;
    info!(
        gangs = gmgr.groups.len(),
        "Final gang system state"
    );

    info!("Daemon stopped.");
}

/// Run with the egui UI.
fn run_ui(args: Args) {
    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    let mode = OperatingMode::from_cli(&args.mode).unwrap_or_default();

    let app = ui::app::HiJackApp::new(
        &args.console_ip,
        args.console_port,
        args.local_port,
        args.trigger_port,
        mode,
        args.ipad_ip.as_deref(),
        args.effective_ipad_send_port(),
        args.effective_ipad_receive_port(),
        args.monitor_port,
        runtime.handle().clone(),
    );

    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1024.0, 600.0])
            .with_title("S21 HiJack"),
        ..Default::default()
    };

    // Keep runtime alive — it's dropped after run_native returns (window closed)
    let _runtime_guard = runtime;

    eframe::run_native(
        "S21 HiJack",
        native_options,
        Box::new(|_cc| Ok(Box::new(app))),
    )
    .unwrap_or_else(|e| {
        error!("eframe error: {e}");
    });

    info!("UI closed.");
}
