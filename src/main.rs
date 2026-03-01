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
use console::ipad_connection;
use console::macro_engine::MacroEngine;
use console::macro_manager::MacroManager;
use console::snapshot_engine::SnapshotEngine;
use model::operating_mode::OperatingMode;
use model::snapshot::CueList;
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
        "S21 HiJack starting — console {}:{}, local port {}, trigger port {}, mode={}, ipad_send={}, ipad_recv={}, ipad_ip={:?}, headless={}",
        args.console_ip, args.console_port, args.local_port, args.trigger_port,
        mode, args.effective_ipad_send_port(), args.effective_ipad_receive_port(),
        args.ipad_ip, args.headless
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

    // Set up macro system
    let macro_manager = Arc::new(RwLock::new(MacroManager::new()));

    // Connect to console
    let manager = match ConnectionManager::connect(local_addr, console_addr, macro_manager.clone()).await {
        Ok(m) => {
            info!("Connected successfully");
            m
        }
        Err(e) => {
            error!("Failed to connect: {e}");
            std::process::exit(1);
        }
    };

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
