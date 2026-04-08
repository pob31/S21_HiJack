use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio::time;
use tokio_util::sync::CancellationToken;
use tracing::{info, debug};

use crate::console::gang_engine::GangEngine;
use crate::console::gang_manager::GangManager;
use crate::console::macro_manager::MacroManager;
use crate::model::state::ConsoleState;
use crate::osc::client::{OscSender, ReceivedOscMessage};
use crate::osc::parse::{self, ParsedOscMessage};

/// Connection manager handles the lifecycle of the console connection.
pub struct ConnectionManager {
    state: Arc<RwLock<ConsoleState>>,
    sender: OscSender,
    macro_manager: Arc<RwLock<MacroManager>>,
    gang_engine: Arc<RwLock<GangEngine>>,
    gang_manager: Arc<RwLock<GangManager>>,
}

impl ConnectionManager {
    /// Get a reference to the shared console state.
    pub fn state(&self) -> Arc<RwLock<ConsoleState>> {
        self.state.clone()
    }

    /// Get a clone of the sender for sending commands.
    pub fn sender(&self) -> OscSender {
        self.sender.clone()
    }

    /// Get a reference to the macro manager.
    pub fn macro_manager(&self) -> Arc<RwLock<MacroManager>> {
        self.macro_manager.clone()
    }

    /// Build a ConnectionManager from pre-created parts (when OscClient was created externally).
    /// Spawns the state mirror loop with cancellation support.
    ///
    /// GP OSC has no discovery or state dump mechanism — the state mirror is populated
    /// only from live parameter changes sent by the console. For initial state, use the
    /// iPad protocol (Mode 2 or 3).
    pub fn connect_from_parts(
        sender: OscSender,
        rx: tokio::sync::mpsc::Receiver<ReceivedOscMessage>,
        state: Arc<RwLock<ConsoleState>>,
        macro_manager: Arc<RwLock<MacroManager>>,
        gang_engine: Arc<RwLock<GangEngine>>,
        gang_manager: Arc<RwLock<GangManager>>,
        cancel: CancellationToken,
    ) -> Self {
        info!("ConnectionManager created from parts");

        tokio::spawn(run_loop(
            sender.clone(), rx, state.clone(), macro_manager.clone(),
            gang_engine.clone(), gang_manager.clone(), cancel,
        ));

        Self {
            state,
            sender,
            macro_manager,
            gang_engine,
            gang_manager,
        }
    }

    /// Get a reference to the gang engine.
    pub fn gang_engine(&self) -> Arc<RwLock<GangEngine>> {
        self.gang_engine.clone()
    }

    /// Get a reference to the gang manager.
    pub fn gang_manager(&self) -> Arc<RwLock<GangManager>> {
        self.gang_manager.clone()
    }
}

/// Main state mirror loop.
///
/// GP OSC on the S21 is purely reactive — the console sends `/channel/{ch}/{param}`
/// messages whenever a parameter changes on the surface. There is no discovery,
/// no state dump, and no keepalive in the GP protocol.
async fn run_loop(
    _sender: OscSender,
    mut rx: tokio::sync::mpsc::Receiver<ReceivedOscMessage>,
    state: Arc<RwLock<ConsoleState>>,
    macro_manager: Arc<RwLock<MacroManager>>,
    gang_engine: Arc<RwLock<GangEngine>>,
    gang_manager: Arc<RwLock<GangManager>>,
    cancel: CancellationToken,
) {
    info!("GP OSC state mirror started — waiting for console parameter updates");

    let mut param_count_log_interval = time::interval(Duration::from_secs(10));

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("Connection cancelled — shutting down state mirror");
                return;
            }

            // Process incoming OSC messages
            Some(msg) = rx.recv() => {
                let parsed = parse::parse_gp_osc(&msg.path, &msg.args);
                process_message(&parsed, &state, &macro_manager, &gang_engine, &gang_manager).await;
            }

            // Periodically log state mirror size
            _ = param_count_log_interval.tick() => {
                let count = state.read().await.parameter_count();
                debug!(count, "State mirror parameter count");
            }

            else => {
                info!("Message channel closed, shutting down");
                break;
            }
        }
    }
}

/// Process a parsed GP OSC message — update state mirror, propagate gangs, record macros.
async fn process_message(
    parsed: &ParsedOscMessage,
    state: &Arc<RwLock<ConsoleState>>,
    macro_manager: &Arc<RwLock<MacroManager>>,
    gang_engine: &Arc<RwLock<GangEngine>>,
    gang_manager: &Arc<RwLock<GangManager>>,
) {
    match parsed {
        ParsedOscMessage::ParameterUpdate(addr, value) => {
            debug!(%addr, %value, "Parameter update");
            let old_value = state.write().await.update(addr.clone(), value.clone());

            // Gang propagation — before macro recording so the engineer's
            // original change is what gets recorded, not ganged echoes.
            {
                let mut engine = gang_engine.write().await;
                let manager = gang_manager.read().await;
                engine
                    .process_gang_update(addr, value, old_value.as_ref(), &manager)
                    .await;
            }

            // Feed into macro learn mode if recording
            let mut mgr = macro_manager.write().await;
            if mgr.is_recording() {
                mgr.record_change(addr.clone(), value.clone());
            }
        }
        // GP protocol doesn't use these, but log them if they appear
        ParsedOscMessage::Ping | ParsedOscMessage::Pong => {
            debug!("Unexpected ping/pong on GP OSC");
        }
        ParsedOscMessage::DiscoveryCount { channel_type, count } => {
            debug!(channel_type, count, "Unexpected discovery response on GP OSC");
        }
        ParsedOscMessage::Unknown(path) => {
            tracing::trace!(path, "Unknown OSC message");
        }
    }
}
