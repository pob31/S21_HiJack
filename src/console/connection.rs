use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tokio::time;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::console::discovery::apply_channel_counts;
use crate::console::gang_engine::GangEngine;
use crate::console::gang_manager::GangManager;
use crate::console::macro_manager::MacroManager;
use crate::console::pan_link_engine::PanLinkEngine;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::state::{ConnectionHealth, ConsoleState};
use crate::osc::client::{OscSender, ReceivedOscMessage};
use crate::osc::encode::SystemCommand;
use crate::osc::parse::{self, ParsedOscMessage};

// ── Connection-health tunables ─────────────────────────────────────────
// Tweak these once we test against the real S21+ — `/console/resend` is known
// to be slow on first reply, so don't make IDLE_THRESHOLD too aggressive.

/// No inbound traffic for this long → start sending pings.
const IDLE_THRESHOLD: Duration = Duration::from_secs(5);
/// While idle, send a ping this often.
const PING_INTERVAL: Duration = Duration::from_secs(2);
/// After this many unanswered pings, force a `/console/resend` to refresh.
const MAX_UNANSWERED_PINGS: u8 = 3;
/// Loop tick rate for the idle/ping check.
const TICK_INTERVAL: Duration = Duration::from_secs(1);

/// Connection manager handles the lifecycle of the console connection.
pub struct ConnectionManager {
    state: Arc<RwLock<ConsoleState>>,
    sender: OscSender,
    macro_manager: Arc<RwLock<MacroManager>>,
    gang_engine: Arc<RwLock<GangEngine>>,
    gang_manager: Arc<RwLock<GangManager>>,
    pan_link_engine: Arc<RwLock<PanLinkEngine>>,
    dirty_tracker: Arc<RwLock<DirtyTracker>>,
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
    /// On connect, the loop sends `/console/resend` and `/console/channel/counts` to
    /// populate the parameter database and discover the bus layout. While running, an
    /// idle-triggered ping/pong heartbeat watches link health; if pings go unanswered
    /// for long enough, a recovery `/console/resend` is issued.
    #[allow(clippy::too_many_arguments)]
    pub fn connect_from_parts(
        sender: OscSender,
        rx: tokio::sync::mpsc::Receiver<ReceivedOscMessage>,
        state: Arc<RwLock<ConsoleState>>,
        macro_manager: Arc<RwLock<MacroManager>>,
        gang_engine: Arc<RwLock<GangEngine>>,
        gang_manager: Arc<RwLock<GangManager>>,
        pan_link_engine: Arc<RwLock<PanLinkEngine>>,
        dirty_tracker: Arc<RwLock<DirtyTracker>>,
        cancel: CancellationToken,
    ) -> Self {
        info!("ConnectionManager created from parts");

        tokio::spawn(run_loop(
            sender.clone(), rx, state.clone(), macro_manager.clone(),
            gang_engine.clone(), gang_manager.clone(), pan_link_engine.clone(),
            dirty_tracker.clone(), cancel.clone(),
        ));

        Self {
            state,
            sender,
            macro_manager,
            gang_engine,
            gang_manager,
            pan_link_engine,
            dirty_tracker,
        }
    }

    /// Get a reference to the dirty tracker.
    pub fn dirty_tracker(&self) -> Arc<RwLock<DirtyTracker>> {
        self.dirty_tracker.clone()
    }

    /// Get a reference to the gang engine.
    pub fn gang_engine(&self) -> Arc<RwLock<GangEngine>> {
        self.gang_engine.clone()
    }

    /// Get a reference to the gang manager.
    pub fn gang_manager(&self) -> Arc<RwLock<GangManager>> {
        self.gang_manager.clone()
    }

    /// Get a reference to the pan link engine.
    pub fn pan_link_engine(&self) -> Arc<RwLock<PanLinkEngine>> {
        self.pan_link_engine.clone()
    }
}

/// Main state mirror loop.
///
/// On startup: send `/console/resend` and `/console/channel/counts` to populate
/// the parameter database and the bus layout. While running:
///   - Any inbound message updates `last_inbound_at` and clears `unanswered_pings`.
///   - When idle for `IDLE_THRESHOLD`, the loop sends `/console/ping` every `PING_INTERVAL`.
///   - After `MAX_UNANSWERED_PINGS` consecutive pings without a reply, fire one
///     `/console/resend` as a recovery refresh; reset the counter and re-arm.
///   - If pings still go unanswered after the recovery resend, mark health as `Lost`.
#[allow(clippy::too_many_arguments)]
async fn run_loop(
    sender: OscSender,
    mut rx: tokio::sync::mpsc::Receiver<ReceivedOscMessage>,
    state: Arc<RwLock<ConsoleState>>,
    macro_manager: Arc<RwLock<MacroManager>>,
    gang_engine: Arc<RwLock<GangEngine>>,
    gang_manager: Arc<RwLock<GangManager>>,
    pan_link_engine: Arc<RwLock<PanLinkEngine>>,
    dirty_tracker: Arc<RwLock<DirtyTracker>>,
    cancel: CancellationToken,
) {
    info!("GP OSC state mirror started — querying channel counts before full dump");

    // Step 1: Query channel counts and wait for the reply before requesting
    // the full state dump. This ensures the config is populated so the UI
    // can display the correct channel counts and send counts.
    send_system(&sender, SystemCommand::ChannelCountsQuery).await;

    let mut counts_received = false;
    let counts_timeout = Instant::now() + Duration::from_secs(3);

    while !counts_received && Instant::now() < counts_timeout {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("Connection cancelled during channel counts wait");
                return;
            }
            Some(msg) = rx.recv() => {
                let parsed = parse::parse_gp_osc(&msg.path, &msg.args);
                process_message(&parsed, &state, &macro_manager, &gang_engine, &gang_manager, &pan_link_engine, &dirty_tracker, &sender).await;
                match &parsed {
                    ParsedOscMessage::ChannelCounts { .. } | ParsedOscMessage::DiscoveryCount { .. } => {
                        counts_received = true;
                        info!("Channel counts received — requesting full state dump");
                    }
                    _ => {}
                }
            }
            _ = time::sleep(Duration::from_millis(100)) => {}
        }
    }

    if !counts_received {
        warn!("Channel counts not received within 3s — proceeding with resend anyway");
    }

    // Step 2: Request full state dump now that config is populated.
    send_system(&sender, SystemCommand::Resend).await;

    let mut last_inbound_at = Instant::now();
    let mut last_ping_at: Option<Instant> = None;
    let mut unanswered_pings: u8 = 0;
    let mut recovery_attempted = false;

    let mut param_count_log_interval = time::interval(Duration::from_secs(10));
    let mut tick_interval = time::interval(TICK_INTERVAL);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("Connection cancelled — shutting down state mirror");
                return;
            }

            // Process incoming OSC messages
            Some(msg) = rx.recv() => {
                let mix_types = {
                    let s = state.read().await;
                    if s.config.mix_output_types.is_empty() { None } else { Some(s.config.mix_output_types.clone()) }
                };
                let parsed = parse::parse_gp_osc_with_config(&msg.path, &msg.args, mix_types.as_deref());
                process_message(&parsed, &state, &macro_manager, &gang_engine, &gang_manager, &pan_link_engine, &dirty_tracker, &sender).await;

                // Any inbound traffic counts as "alive": reset idle/ping bookkeeping.
                last_inbound_at = Instant::now();
                last_ping_at = None;
                unanswered_pings = 0;
                recovery_attempted = false;
                set_health(&state, ConnectionHealth::Connected).await;
            }

            // Idle / ping / recovery tick
            _ = tick_interval.tick() => {
                let now = Instant::now();
                let idle_for = now.duration_since(last_inbound_at);

                if idle_for >= IDLE_THRESHOLD {
                    // Time to ping?
                    let ping_due = match last_ping_at {
                        None => true,
                        Some(t) => now.duration_since(t) >= PING_INTERVAL,
                    };
                    if ping_due {
                        // Bookkeeping: this ping is "unanswered" until inbound traffic arrives.
                        unanswered_pings = unanswered_pings.saturating_add(1);
                        last_ping_at = Some(now);
                        debug!(unanswered_pings, "GP OSC idle — sending /console/ping");
                        send_system(&sender, SystemCommand::Ping).await;

                        if unanswered_pings >= MAX_UNANSWERED_PINGS {
                            if !recovery_attempted {
                                warn!(
                                    unanswered_pings,
                                    "GP OSC unresponsive — issuing recovery /console/resend"
                                );
                                send_system(&sender, SystemCommand::Resend).await;
                                recovery_attempted = true;
                                unanswered_pings = 0; // re-arm; new pings count fresh
                                set_health(&state, ConnectionHealth::Stale).await;
                            } else {
                                warn!("GP OSC still unresponsive after recovery resend — marking link Lost");
                                set_health(&state, ConnectionHealth::Lost).await;
                            }
                        } else {
                            // Pinging but threshold not yet hit.
                            set_health(&state, ConnectionHealth::Idle).await;
                        }
                    }
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

/// Send a system command, logging any send errors but not failing the loop.
async fn send_system(sender: &OscSender, cmd: SystemCommand) {
    let path = cmd.path().to_string();
    let args = cmd.args();
    if let Err(e) = sender.send(&path, args).await {
        warn!(path = %path, error = %e, "Failed to send GP OSC system command");
    }
}

/// Update the connection-health field on shared state.
async fn set_health(state: &Arc<RwLock<ConsoleState>>, health: ConnectionHealth) {
    let mut s = state.write().await;
    if s.health != health {
        debug!(?health, "GP OSC connection health changed");
        s.health = health;
    }
}

/// Process a parsed GP OSC message — update state mirror, propagate gangs, record macros.
#[allow(clippy::too_many_arguments)]
async fn process_message(
    parsed: &ParsedOscMessage,
    state: &Arc<RwLock<ConsoleState>>,
    macro_manager: &Arc<RwLock<MacroManager>>,
    gang_engine: &Arc<RwLock<GangEngine>>,
    gang_manager: &Arc<RwLock<GangManager>>,
    pan_link_engine: &Arc<RwLock<PanLinkEngine>>,
    dirty_tracker: &Arc<RwLock<DirtyTracker>>,
    sender: &OscSender,
) {
    match parsed {
        ParsedOscMessage::ParameterUpdate(addr, value) => {
            debug!(%addr, %value, "Parameter update");
            let old_value = state.write().await.update(addr.clone(), value.clone());

            // Mark this cell dirty IF the value actually changed. The dirty
            // tracker is suppression-aware, so echoes from snapshot recall
            // (which set begin_suppression before sending) are ignored. The
            // first sample after a connection comes through old_value=None,
            // which we treat as "this is the baseline" — not a change.
            if let Some(prev) = &old_value {
                if prev != value {
                    dirty_tracker.write().await.mark(addr);
                }
            }

            // Gang propagation — before macro recording so the engineer's
            // original change is what gets recorded, not ganged echoes.
            {
                let mut engine = gang_engine.write().await;
                let manager = gang_manager.read().await;
                engine
                    .process_gang_update(addr, value, old_value.as_ref(), &manager)
                    .await;
            }

            // Pan link propagation — runs after gangs so a gang-driven
            // pan change on an input also pushes to its linked aux sends.
            {
                let engine = pan_link_engine.read().await;
                engine.process_pan_update(addr, value).await;
            }

            // Feed into macro learn mode if recording
            let mut mgr = macro_manager.write().await;
            if mgr.is_recording() {
                mgr.record_change(addr.clone(), value.clone());
            }
        }
        ParsedOscMessage::Pong => {
            debug!("Received /console/pong");
            // Health/idle bookkeeping is handled in run_loop on any inbound message.
        }
        ParsedOscMessage::Ping => {
            // The console doesn't normally ping us, but reply if it ever does.
            debug!("Received /console/ping — replying with /console/pong");
            send_system(sender, SystemCommand::Pong).await;
        }
        ParsedOscMessage::ChannelCounts {
            inputs,
            aux,
            groups,
            control_groups,
            matrices,
            master,
        } => {
            info!(
                inputs, aux, groups, control_groups, matrices, master,
                "Received /console/channel/counts — applying to config"
            );
            let mut s = state.write().await;
            apply_channel_counts(
                &mut s.config,
                *inputs,
                *aux,
                *groups,
                *control_groups,
                *matrices,
                *master,
            );
        }
        ParsedOscMessage::DiscoveryCount { channel_type, count } => {
            debug!(channel_type, count, "Per-type channel count (back-compat path)");
            let mut s = state.write().await;
            crate::console::discovery::apply_channel_count(&mut s.config, channel_type, *count);
        }
        ParsedOscMessage::Unknown(path) => {
            tracing::trace!(path, "Unknown OSC message");
        }
    }
}
