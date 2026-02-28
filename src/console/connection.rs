use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio::time;
use tracing::{info, warn, debug, error};

use crate::console::discovery;
use crate::console::macro_manager::MacroManager;
use crate::model::config::ConsoleConfig;
use crate::model::state::ConsoleState;
use crate::osc::client::{OscClient, OscSender, ReceivedOscMessage};
use crate::osc::encode::SystemCommand;
use crate::osc::parse::{self, ParsedOscMessage};

/// Keepalive interval in seconds.
const PING_INTERVAL_SECS: u64 = 5;

/// Time to wait for discovery responses before moving on.
const DISCOVERY_WAIT_SECS: u64 = 3;

/// Connection manager handles the lifecycle of the console connection.
pub struct ConnectionManager {
    state: Arc<RwLock<ConsoleState>>,
    sender: OscSender,
    macro_manager: Arc<RwLock<MacroManager>>,
}

impl ConnectionManager {
    /// Connect to the console and begin the discovery + state mirror loop.
    pub async fn connect(
        local_addr: SocketAddr,
        console_addr: SocketAddr,
        macro_manager: Arc<RwLock<MacroManager>>,
    ) -> std::io::Result<Self> {
        let config = ConsoleConfig::default();
        let state = Arc::new(RwLock::new(ConsoleState::new(config)));
        Self::connect_with_state(local_addr, console_addr, state, macro_manager).await
    }

    /// Connect using a pre-existing shared state (for UI mode where state is created before connection).
    pub async fn connect_with_state(
        local_addr: SocketAddr,
        console_addr: SocketAddr,
        state: Arc<RwLock<ConsoleState>>,
        macro_manager: Arc<RwLock<MacroManager>>,
    ) -> std::io::Result<Self> {
        info!(
            "Connecting to console at {console_addr}, local port {}",
            local_addr.port()
        );

        let client = OscClient::new(local_addr, console_addr).await?;
        let (sender, rx) = client.into_parts();

        let manager = Self {
            state: state.clone(),
            sender: sender.clone(),
            macro_manager: macro_manager.clone(),
        };

        // Spawn the main processing loop
        tokio::spawn(run_loop(sender, rx, state, macro_manager));

        Ok(manager)
    }

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
}

/// Main processing loop: discovery, then state mirror + keepalive.
async fn run_loop(
    sender: OscSender,
    mut rx: tokio::sync::mpsc::Receiver<ReceivedOscMessage>,
    state: Arc<RwLock<ConsoleState>>,
    macro_manager: Arc<RwLock<MacroManager>>,
) {
    // Phase 1: Discovery — send request and collect responses
    info!("Starting console discovery...");
    if let Err(e) = sender.send(
        SystemCommand::ChannelCounts.path(),
        SystemCommand::ChannelCounts.args(),
    ).await {
        error!("Failed to send channel counts request: {e}");
        return;
    }
    info!("Sent channel counts request");

    // Wait for discovery responses with a timeout
    let discovery_deadline = time::Instant::now() + Duration::from_secs(DISCOVERY_WAIT_SECS);
    let mut discovery_count = 0u32;

    loop {
        let timeout = time::sleep_until(discovery_deadline);
        tokio::select! {
            Some(msg) = rx.recv() => {
                let parsed = parse::parse_gp_osc(&msg.path, &msg.args);
                match parsed {
                    ParsedOscMessage::DiscoveryCount { channel_type, count } => {
                        let mut st = state.write().await;
                        if discovery::apply_channel_count(&mut st.config, &channel_type, count) {
                            info!(channel_type, count, "Discovery: channel count");
                            discovery_count += 1;
                        } else {
                            warn!(channel_type, "Discovery: unknown channel type");
                        }
                    }
                    // During discovery, still process other messages
                    _ => {
                        process_message_inner(&parsed, &state, &sender, &macro_manager).await;
                    }
                }
            }
            _ = timeout => {
                info!(discovery_count, "Discovery phase complete (timeout)");
                break;
            }
        }
    }

    // Log discovered configuration
    {
        let st = state.read().await;
        info!(
            "Console config: {} inputs, {} aux, {} group, {} matrix, {} CGs",
            st.config.input_channel_count,
            st.config.aux_output_count,
            st.config.group_output_count,
            st.config.matrix_output_count,
            st.config.control_group_count,
        );
    }

    // Phase 2: Request full state dump
    info!("Requesting full state dump...");
    if let Err(e) = sender.send(
        SystemCommand::Resend.path(),
        SystemCommand::Resend.args(),
    ).await {
        error!("Failed to send resend command: {e}");
        return;
    }

    // Phase 3: Process incoming messages + keepalive
    info!("Entering state mirror loop...");
    let mut ping_interval = time::interval(Duration::from_secs(PING_INTERVAL_SECS));
    let mut param_count_log_interval = time::interval(Duration::from_secs(10));

    loop {
        tokio::select! {
            // Process incoming OSC messages
            Some(msg) = rx.recv() => {
                let parsed = parse::parse_gp_osc(&msg.path, &msg.args);
                process_message_inner(&parsed, &state, &sender, &macro_manager).await;
            }

            // Send keepalive ping
            _ = ping_interval.tick() => {
                if let Err(e) = sender.send(
                    SystemCommand::Ping.path(),
                    SystemCommand::Ping.args(),
                ).await {
                    warn!("Failed to send ping: {e}");
                }
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

/// Process a parsed OSC message — update state, respond to pings, etc.
async fn process_message_inner(
    parsed: &ParsedOscMessage,
    state: &Arc<RwLock<ConsoleState>>,
    sender: &OscSender,
    macro_manager: &Arc<RwLock<MacroManager>>,
) {
    match parsed {
        ParsedOscMessage::ParameterUpdate(addr, value) => {
            debug!(%addr, %value, "Parameter update");
            state.write().await.update(addr.clone(), value.clone());

            // Feed into macro learn mode if recording
            let mut mgr = macro_manager.write().await;
            if mgr.is_recording() {
                mgr.record_change(addr.clone(), value.clone());
            }
        }
        ParsedOscMessage::Ping => {
            debug!("Received ping from console — sending pong");
            if let Err(e) = sender.send(
                SystemCommand::Pong.path(),
                SystemCommand::Pong.args(),
            ).await {
                warn!("Failed to send pong: {e}");
            }
        }
        ParsedOscMessage::Pong => {
            debug!("Received pong from console");
        }
        ParsedOscMessage::DiscoveryCount { channel_type, count } => {
            // Late discovery message — still apply it
            let mut st = state.write().await;
            if discovery::apply_channel_count(&mut st.config, channel_type, *count) {
                info!(channel_type, count, "Late discovery update");
            }
        }
        ParsedOscMessage::Unknown(path) => {
            tracing::trace!(path, "Unknown OSC message");
        }
    }
}
