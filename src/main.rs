// Many of the daemon's public APIs (engine getters, manager helpers,
// config builders) are deliberately exposed but not yet called from
// internal code — the UI / CLI surface drives them on demand. Removing
// this global allow surfaces ~34 warnings across 23 modules; per-item
// triage is tracked under audit finding M3 in
// `Documentation/AUDIT_2026-04-27.md` and `Documentation/todo.md` §7.
#![allow(dead_code)]

mod console;
mod model;
mod osc;
mod persistence;
mod ui;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use console::connection::ConnectionManager;
use console::cue_manager::CueManager;
use console::gang_engine::GangEngine;
use console::gang_manager::GangManager;
use console::ipad_connection;
use console::macro_engine::MacroEngine;
use console::macro_manager::MacroManager;
use console::monitor_engine::MonitorEngine;
use console::monitor_manager::MonitorManager;
use console::palette_manager::PaletteManager;
use console::pan_link_engine::PanLinkEngine;
use console::snapshot_engine::SnapshotEngine;
use model::operating_mode::OperatingMode;
use model::snapshot::CueList;
use osc::monitor_server::MonitorServer;
use osc::trigger_listener::TriggerListener;
use persistence::preferences::AppPreferences;

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

    /// Monitor server port (0 = disabled, default 8025 for autodiscovery)
    #[arg(long, default_value_t = 8025)]
    monitor_port: u16,

    /// Repeatable: source-IP CIDR allowed to talk to the monitor server
    /// (e.g. `192.168.10.0/24`). Empty = accept all (current behavior).
    /// See audit C2 for the closed-LAN deployment rationale.
    #[arg(long = "monitor-allow-cidr")]
    monitor_allow_cidrs: Vec<String>,

    /// Repeatable: source-IP CIDR allowed to talk to the trigger listener.
    /// Empty = accept all. See audit H5.
    #[arg(long = "trigger-allow-cidr")]
    trigger_allow_cidrs: Vec<String>,

    /// Run in headless mode (no UI, daemon only)
    #[arg(long)]
    headless: bool,

    /// Optional show-file path to load on startup. The OS file
    /// associations (Linux .desktop / Windows .reg / macOS Info.plist)
    /// invoke the binary with this positional argument so
    /// double-clicking a `.s21show` file in a file manager opens it
    /// in the app.
    #[arg(value_name = "SHOW_FILE")]
    show_file: Option<PathBuf>,
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
                .unwrap_or_else(|_| EnvFilter::new("info,s21_hijack=debug,sctk_adwaita=error")),
        )
        .init();

    let args = Args::parse();

    let mode = OperatingMode::from_cli(&args.mode).unwrap_or_default();

    info!(
        "S21 HiJack starting — console {}:{}, local port {}, trigger port {}, mode={}, ipad_send={}, ipad_recv={}, ipad_ip={:?}, monitor_port={}, headless={}",
        args.console_ip,
        args.console_port,
        args.local_port,
        args.trigger_port,
        mode,
        args.effective_ipad_send_port(),
        args.effective_ipad_receive_port(),
        args.ipad_ip,
        args.monitor_port,
        args.headless
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
    // Phase C: dirty tracker for "select modified" / "auto-preselect modified".
    let dirty_tracker = Arc::new(RwLock::new(model::dirty_tracker::DirtyTracker::new()));

    // Create state + OscClient so we can build GangEngine before spawning the loop
    let state = {
        let config = model::config::ConsoleConfig::default();
        Arc::new(RwLock::new(model::state::ConsoleState::new(config)))
    };
    let client = match osc::client::OscClient::new(local_addr, console_addr, None).await {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to create OSC client: {e}");
            std::process::exit(1);
        }
    };
    let (sender, rx) = client.into_parts();

    let gang_engine = Arc::new(RwLock::new(GangEngine::new(state.clone(), sender.clone())));

    let pan_link_bindings = Arc::new(RwLock::new(model::pan_link::PanLinkBindings::default()));
    let pan_link_engine = Arc::new(RwLock::new(PanLinkEngine::new(
        state.clone(),
        sender.clone(),
        pan_link_bindings.clone(),
        dirty_tracker.clone(),
        gang_manager.clone(),
    )));

    let cancel_token = tokio_util::sync::CancellationToken::new();
    let offline_mode = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let daemon = console::connection::DaemonState {
        state,
        macro_manager: macro_manager.clone(),
        gang_engine: gang_engine.clone(),
        gang_manager: gang_manager.clone(),
        pan_link_engine: pan_link_engine.clone(),
        dirty_tracker: dirty_tracker.clone(),
        offline_mode: offline_mode.clone(),
    };
    let manager =
        ConnectionManager::connect_from_parts(sender.clone(), rx, daemon, cancel_token.clone());
    info!("Connected successfully");

    // Parse operating mode
    let mode = OperatingMode::from_cli(&args.mode).unwrap_or_default();

    // Set up snapshot, macro, and palette systems
    let cue_manager = Arc::new(RwLock::new(CueManager::new(CueList::default())));
    let palette_manager = Arc::new(RwLock::new(PaletteManager::new()));
    let mut snapshot_engine = SnapshotEngine::new(manager.state(), manager.sender());
    snapshot_engine.set_dirty_tracker(dirty_tracker.clone());
    let mut macro_engine = MacroEngine::new(manager.state(), manager.sender());
    macro_engine.set_dirty_tracker(dirty_tracker.clone());
    let macro_engine = Arc::new(macro_engine);

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
                match ipad_connection::connect_mode2(
                    console_ipad_addr,
                    ipad_local,
                    manager.state(),
                    dirty_tracker.clone(),
                    offline_mode.clone(),
                    None,
                    None,
                )
                .await
                {
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
                let local_console_addr: SocketAddr = format!("0.0.0.0:{}", recv_port)
                    .parse()
                    .expect("Invalid iPad local port (--ipad-receive-port required for Mode 3)");
                // In headless mode, use recv_port+1000 for iPad listener by default
                let ipad_listen_port = recv_port + 1000;
                let ipad_reply_port = recv_port + 1000 - 1;
                let ipad_listen_addr: SocketAddr = format!("0.0.0.0:{}", ipad_listen_port)
                    .parse()
                    .expect("Invalid iPad listen address");
                let ipad_target = args.ipad_ip.as_ref().map(|ip| {
                    let addr: SocketAddr = format!("{ip}:{ipad_reply_port}")
                        .parse()
                        .expect("Invalid iPad IP");
                    addr
                });
                match ipad_connection::connect_mode3_proxy(
                    console_ipad_addr,
                    local_console_addr,
                    ipad_listen_addr,
                    ipad_target,
                    ipad_reply_port,
                    manager.state(),
                    dirty_tracker.clone(),
                    offline_mode.clone(),
                    None,
                    cancel_token.clone(),
                    None,
                )
                .await
                {
                    Ok(ipad_sender) => {
                        info!(
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

    // Wire iPad sender into gang & pan link engines (for iPad-only parameters)
    if let Some(ref ipad) = ipad_sender_for_monitor {
        gang_engine
            .write()
            .await
            .set_ipad_sender(Some(ipad.clone()));
        pan_link_engine
            .write()
            .await
            .set_ipad_sender(Some(ipad.clone()));
    }

    // Start monitor server (if enabled)
    if args.monitor_port > 0 {
        let monitor_addr: SocketAddr = format!("0.0.0.0:{}", args.monitor_port)
            .parse()
            .expect("Invalid monitor address");
        let monitor_allowlist =
            persistence::show_file::parse_cidr_allowlist(&args.monitor_allow_cidrs);
        match MonitorServer::start_with_cancel(
            monitor_addr,
            cancel_token.clone(),
            None,
            monitor_allowlist,
        )
        .await
        {
            Ok((monitor_sender, mut monitor_rx)) => {
                info!(port = args.monitor_port, "Monitor server started");
                let mut monitor_engine = MonitorEngine::new(manager.state(), manager.sender());
                monitor_engine.set_ipad_sender(ipad_sender_for_monitor);
                let mon_mgr = monitor_manager.clone();
                tokio::spawn(async move {
                    let mut last_send_state = std::collections::HashMap::new();
                    let mut last_aux_state = std::collections::HashMap::new();
                    let mut last_generation: u64 = 0;
                    let mut last_push_times = std::collections::HashMap::new();
                    let mut last_aux_push_times = std::collections::HashMap::new();
                    let mut poll_interval =
                        tokio::time::interval(std::time::Duration::from_millis(10));
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
                                    &mut last_aux_state,
                                    &mut last_generation,
                                    &mut last_push_times,
                                    &mut last_aux_push_times,
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

    let trigger_allowlist = persistence::show_file::parse_cidr_allowlist(&args.trigger_allow_cidrs);
    let mut trigger_rx = match TriggerListener::start_with_cancel(
        trigger_addr,
        cancel_token.clone(),
        None,
        trigger_allowlist,
    )
    .await
    {
        Ok(rx) => rx,
        Err(e) => {
            error!("Failed to start trigger listener: {e}");
            std::process::exit(1);
        }
    };

    // Spawn trigger processing task
    let trigger_cue_mgr = cue_manager.clone();
    let trigger_macro_mgr = macro_manager.clone();
    let trigger_palette_mgr = palette_manager.clone();
    let trigger_macro_eng = macro_engine.clone();
    let trigger_engine = Arc::new(snapshot_engine);
    let reply_socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await.ok();

    tokio::spawn(async move {
        while let Some(event) = trigger_rx.recv().await {
            console::trigger_dispatch::handle_trigger_event(
                event,
                &trigger_cue_mgr,
                &trigger_palette_mgr,
                &trigger_macro_mgr,
                &trigger_macro_eng,
                &trigger_engine,
                reply_socket.as_ref(),
            )
            .await;
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
    info!(macros = mmgr.macros.len(), "Final macro system state");

    let pmgr = palette_manager.read().await;
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
    info!(gangs = gmgr.groups.len(), "Final gang system state");

    info!("Daemon stopped.");
}

/// Run with the egui UI.
fn run_ui(args: Args) {
    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    let mode = OperatingMode::from_cli(&args.mode).unwrap_or_default();
    let prefs = AppPreferences::load();

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
        prefs,
        runtime.handle().clone(),
        args.show_file.clone(),
    );

    // Load the app icon. Embedded at compile time so the binary is
    // self-contained — no runtime dependency on assets/icon.png being
    // present next to the executable.
    let icon = load_app_icon();

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1400.0, 950.0])
        .with_title("S21 HiJack")
        .with_app_id("s21_hijack");
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        viewport,
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

/// Load the app icon for the window title bar / dock / taskbar.
///
/// The 256x256 PNG at `assets/icon.png` is embedded into the binary at
/// compile time via `include_bytes!` and decoded into raw RGBA on first
/// call. Returns None and logs a warning if decoding fails so a broken
/// icon doesn't prevent the app from starting.
fn load_app_icon() -> Option<eframe::egui::IconData> {
    const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");
    match image::load_from_memory(ICON_PNG) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();
            Some(eframe::egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            })
        }
        Err(e) => {
            warn!("Failed to decode app icon: {e}");
            None
        }
    }
}
