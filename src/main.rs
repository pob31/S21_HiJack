// Public APIs are being built up across phases — suppress dead_code until UI (Phase 3) wires them in.
#![allow(dead_code)]

mod console;
mod model;
mod osc;
mod persistence;

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use rosc::OscType;
use tokio::sync::RwLock;
use tracing::{info, warn, error};
use tracing_subscriber::EnvFilter;

use console::connection::ConnectionManager;
use console::cue_manager::CueManager;
use console::snapshot_engine::SnapshotEngine;
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
}

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,s21_hijack=debug")),
        )
        .init();

    let args = Args::parse();

    info!(
        "S21 HiJack daemon starting — console {}:{}, local port {}, trigger port {}",
        args.console_ip, args.console_port, args.local_port, args.trigger_port
    );

    let console_addr: SocketAddr = format!("{}:{}", args.console_ip, args.console_port)
        .parse()
        .unwrap_or_else(|e| {
            error!("Invalid console address: {e}");
            std::process::exit(1);
        });

    let local_addr: SocketAddr = format!("0.0.0.0:{}", args.local_port)
        .parse()
        .expect("Invalid local address");

    // Connect to console
    let manager = match ConnectionManager::connect(local_addr, console_addr).await {
        Ok(m) => {
            info!("Connected successfully");
            m
        }
        Err(e) => {
            error!("Failed to connect: {e}");
            std::process::exit(1);
        }
    };

    // Set up snapshot system
    let cue_manager = Arc::new(RwLock::new(CueManager::new(CueList::default())));
    let snapshot_engine = SnapshotEngine::new(manager.state(), manager.sender());

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
                            let result = trigger_engine.recall_cue(&cue, &snapshot).await;
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
                            let result = trigger_engine.recall_cue(&cue, &snapshot).await;
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
                            let result = trigger_engine.recall_cue(&cue, &snapshot).await;
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
                    warn!(name, "Macro triggers not yet implemented (Phase 4)");
                }
            }
        }
    });

    // Wait for shutdown signal
    info!("Daemon running. Press Ctrl+C to stop.");
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

    info!("Daemon stopped.");
}
