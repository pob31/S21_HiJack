use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tokio::time;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::console::console_tx::SentLog;
use crate::console::discovery::apply_channel_counts;
use crate::console::gang_engine::GangEngine;
use crate::console::gang_manager::GangManager;
use crate::console::inbound::{self, InboundSource};
use crate::console::macro_manager::MacroManager;
use crate::console::pan_link_engine::PanLinkEngine;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::recall_progress::{RecallKind, RecallProgress};
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

/// Bundle of daemon-wide handles that travel together through the connection
/// pipeline (audit M10). Cheap to clone — every field is `Arc<…>`. Replaces
/// the 9-argument cluster that `connect_from_parts` / `run_loop` /
/// `process_message` used to take. iPad-protocol setup functions in
/// [`crate::console::ipad_connection`] only need a 3-field subset (state /
/// dirty_tracker / offline_mode) and continue to take those individually
/// rather than carry dead fields.
#[derive(Clone)]
pub struct DaemonState {
    pub state: Arc<RwLock<ConsoleState>>,
    pub macro_manager: Arc<RwLock<MacroManager>>,
    pub gang_engine: Arc<RwLock<GangEngine>>,
    pub gang_manager: Arc<RwLock<GangManager>>,
    pub pan_link_engine: Arc<RwLock<PanLinkEngine>>,
    pub dirty_tracker: Arc<RwLock<DirtyTracker>>,
    pub offline_mode: Arc<AtomicBool>,
    /// Shared progress handle for long parameter transfers. The dump loop here
    /// drives it on connect / recovery resend; the recall engines drive it for
    /// snapshot/cue/macro recalls; the UI polls it to render the progress line.
    pub recall_progress: Arc<RecallProgress>,
    /// Address of the most recently dispatched parameter from the
    /// console. Used by the Macros tab "track latest OSC" affordance to
    /// mirror the operator's currently-touched parameter into the Add
    /// Step form. Updated unconditionally on every inbound parameter
    /// (one address clone per message — negligible).
    pub last_received: Arc<RwLock<Option<crate::model::parameter::ParameterAddress>>>,
    /// Forwards the desk's current snapshot row (from inbound GP OSC) to the
    /// follow-mode dispatcher so Console→App follow works in every mode.
    /// `None` when there is no dispatcher to feed (e.g. headless).
    pub console_snapshot_tx: Option<tokio::sync::mpsc::Sender<i32>>,
    /// Lock-free console-load suppression window. While active (just after a
    /// memory fire or a desk-reported load), `process_message` skips gang/pan
    /// propagation so the app doesn't re-propagate the desk's coherent
    /// snapshot-load flood back at the console.
    pub console_load_suppression: crate::console::snapshot_engine::ConsoleLoadSuppression,
    /// Shared live-override registry for timed cue recalls. The inbound path
    /// consults it so that an operator move (hands on the console) during a
    /// pre-wait or fade cancels the automation for exactly that
    /// `(channel, parameter)`. `None` when no recall engine is attached.
    pub automation_override: Option<crate::console::automation_registry::AutomationOverride>,
    /// Shared recently-sent log: every engine `ConsoleTx` records successful
    /// writes here, and the inbound chain consults it so the iPad/Pad link's
    /// echo of our own write isn't mistaken for an operator move (GP OSC
    /// doesn't echo, so it's inert for GP-only sessions).
    pub sent_log: SentLog,
}

/// Connection manager handles the lifecycle of the console connection.
pub struct ConnectionManager {
    sender: OscSender,
    daemon: DaemonState,
}

impl ConnectionManager {
    /// Get a reference to the shared console state.
    pub fn state(&self) -> Arc<RwLock<ConsoleState>> {
        self.daemon.state.clone()
    }

    /// Get a clone of the sender for sending commands.
    pub fn sender(&self) -> OscSender {
        self.sender.clone()
    }

    /// Get a reference to the macro manager.
    pub fn macro_manager(&self) -> Arc<RwLock<MacroManager>> {
        self.daemon.macro_manager.clone()
    }

    /// Build a ConnectionManager from pre-created parts (when OscClient was created externally).
    /// Spawns the state mirror loop with cancellation support.
    ///
    /// On connect, the loop sends `/console/resend` and `/console/channel/counts` to
    /// populate the parameter database and discover the bus layout. While running, an
    /// idle-triggered ping/pong heartbeat watches link health; if pings go unanswered
    /// for long enough, a recovery `/console/resend` is issued.
    pub fn connect_from_parts(
        sender: OscSender,
        rx: tokio::sync::mpsc::Receiver<ReceivedOscMessage>,
        daemon: DaemonState,
        cancel: CancellationToken,
    ) -> Self {
        info!("ConnectionManager created from parts");

        tokio::spawn(run_loop(sender.clone(), rx, daemon.clone(), cancel.clone()));

        Self { sender, daemon }
    }

    /// Get a reference to the dirty tracker.
    pub fn dirty_tracker(&self) -> Arc<RwLock<DirtyTracker>> {
        self.daemon.dirty_tracker.clone()
    }

    /// Get a reference to the gang engine.
    pub fn gang_engine(&self) -> Arc<RwLock<GangEngine>> {
        self.daemon.gang_engine.clone()
    }

    /// Get a reference to the gang manager.
    pub fn gang_manager(&self) -> Arc<RwLock<GangManager>> {
        self.daemon.gang_manager.clone()
    }

    /// Get a reference to the pan link engine.
    pub fn pan_link_engine(&self) -> Arc<RwLock<PanLinkEngine>> {
        self.daemon.pan_link_engine.clone()
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
async fn run_loop(
    sender: OscSender,
    mut rx: tokio::sync::mpsc::Receiver<ReceivedOscMessage>,
    daemon: DaemonState,
    cancel: CancellationToken,
) {
    info!("GP OSC state mirror started — querying channel counts before full dump");

    // A bound UDP socket doesn't prove the desk is reachable, so present the
    // link as "Connecting…" (yellow) until the console actually replies. Reset
    // here so a reconnect on a reused state doesn't inherit a stale health.
    set_health(&daemon.state, ConnectionHealth::Connecting).await;
    // Becomes true the first time any inbound message arrives. Until then we
    // never show the optimistic green "Idle" state from the heartbeat below.
    let mut ever_confirmed = false;

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
                if daemon.offline_mode.load(Ordering::Relaxed) {
                    debug!(path = %msg.path, "Inbound OSC dropped (offline mode)");
                    continue;
                }
                let parsed = parse::parse_gp_osc(&msg.path, &msg.args);
                process_message(&parsed, &daemon, &sender).await;
                // The console answered — the link is real.
                ever_confirmed = true;
                set_health(&daemon.state, ConnectionHealth::Connected).await;
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
    // Start the progress line for the dump. The estimate uses the channel
    // counts we just received; the bar fills as params flood in and snaps to
    // 100 % when the inbound stream plateaus (tick arm below).
    {
        let total = estimate_expected_param_count(&daemon.state.read().await.config);
        daemon.recall_progress.begin(RecallKind::Dump, total);
    }
    // When the current dump began, so the completion logic can tell "the flood
    // hasn't started yet" (handshake delay) from "the flood finished".
    let mut dump_begin: Option<Instant> = Some(Instant::now());

    let mut last_inbound_at = Instant::now();
    let mut last_ping_at: Option<Instant> = None;
    let mut unanswered_pings: u8 = 0;
    let mut recovery_attempted = false;

    let mut param_count_log_interval = time::interval(Duration::from_secs(10));
    let mut tick_interval = time::interval(TICK_INTERVAL);

    loop {
        tokio::select! {
            // Biased: always check cancellation FIRST so a continuous inbound
            // flood (e.g. a snapshot-load echo storm) can never starve the
            // cancel branch and block Disconnect.
            biased;

            _ = cancel.cancelled() => {
                info!("Connection cancelled — shutting down state mirror");
                daemon.recall_progress.finish();
                return;
            }

            // Process incoming OSC messages
            Some(msg) = rx.recv() => {
                if daemon.offline_mode.load(Ordering::Relaxed) {
                    debug!(path = %msg.path, "Inbound OSC dropped (offline mode)");
                    continue;
                }
                let mix_types = {
                    let s = daemon.state.read().await;
                    if s.config.mix_output_types.is_empty() { None } else { Some(s.config.mix_output_types.clone()) }
                };
                let parsed = parse::parse_gp_osc_with_config(&msg.path, &msg.args, mix_types.as_deref());
                process_message(&parsed, &daemon, &sender).await;

                // Advance the dump progress bar per inbound parameter — only
                // while a Dump op is active. A snapshot recall's inbound echoes
                // are counted by the recall engine on the send side, so the
                // `kind == Dump` guard keeps the two sources from double-counting.
                if matches!(parsed, ParsedOscMessage::ParameterUpdate(..))
                    && daemon.recall_progress.is_active()
                    && daemon.recall_progress.kind() == RecallKind::Dump
                {
                    daemon.recall_progress.bump();
                }

                // Any inbound traffic counts as "alive": reset idle/ping bookkeeping.
                last_inbound_at = Instant::now();
                last_ping_at = None;
                unanswered_pings = 0;
                recovery_attempted = false;
                ever_confirmed = true;
                set_health(&daemon.state, ConnectionHealth::Connected).await;
            }

            // Idle / ping / recovery tick
            _ = tick_interval.tick() => {
                let now = Instant::now();
                let idle_for = now.duration_since(last_inbound_at);

                // Dump completion / abandonment. Crucially, do NOT finish before
                // the flood has actually started: after `/console/resend` the
                // desk can take a few seconds (handshake) to begin streaming, and
                // finishing during that gap makes the bar flash and then sit out
                // the real dump. So gate the plateau on having received at least
                // one parameter; if none ever arrive, abandon after a generous
                // wait so a non-responding desk can't leave a stuck 0 % bar.
                if daemon.recall_progress.is_active()
                    && daemon.recall_progress.kind() == RecallKind::Dump
                {
                    if daemon.recall_progress.done() > 0 {
                        if idle_for >= Duration::from_millis(1000) {
                            daemon.recall_progress.finish();
                            dump_begin = None;
                        }
                    } else if let Some(t) = dump_begin
                        && now.duration_since(t) >= Duration::from_secs(12)
                    {
                        daemon.recall_progress.finish();
                        dump_begin = None;
                    }
                }

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
                                // Recovery resend re-floods the state — drive a
                                // fresh dump bar so the operator sees it refill.
                                {
                                    let total = estimate_expected_param_count(
                                        &daemon.state.read().await.config,
                                    );
                                    daemon.recall_progress.begin(RecallKind::Dump, total);
                                }
                                dump_begin = Some(now);
                                recovery_attempted = true;
                                unanswered_pings = 0; // re-arm; new pings count fresh
                                set_health(&daemon.state, ConnectionHealth::Stale).await;
                            } else {
                                warn!("GP OSC still unresponsive after recovery resend — marking link Lost");
                                set_health(&daemon.state, ConnectionHealth::Lost).await;
                                // Link is dead — clear any lingering dump bar.
                                daemon.recall_progress.finish();
                                dump_begin = None;
                            }
                        } else if ever_confirmed {
                            // Pinging but threshold not yet hit. Only an
                            // established link earns the optimistic green
                            // "Idle"; a link that has never replied stays
                            // yellow "Connecting…".
                            set_health(&daemon.state, ConnectionHealth::Idle).await;
                        } else {
                            set_health(&daemon.state, ConnectionHealth::Connecting).await;
                        }
                    }
                }
            }

            // Periodically log state mirror size
            _ = param_count_log_interval.tick() => {
                let count = daemon.state.read().await.parameter_count();
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

/// Expected GP-OSC parameter count for the connection dump — the denominator
/// for the progress line. Sums, per channel type, the channel count × the number
/// of *GP-OSC-reachable* applicable paths (those with a `to_gp_osc_suffix`),
/// i.e. exactly the parameters the console floods back on `/console/resend`.
/// iPad-only paths are excluded because they don't arrive on this link, so the
/// bar tracks real progress instead of under-filling and snapping at the end.
/// It can still be a few percent off (e.g. mono channels omit balance/width);
/// the 0.99 cap + plateau-snap on the UI side absorb the remainder.
fn estimate_expected_param_count(config: &crate::model::config::ConsoleConfig) -> usize {
    use crate::model::channel::ChannelId;
    use crate::model::parameter::ParameterPath;
    let aux = config.aux_output_count;
    let grp = config.group_output_count;
    let mtx = config.matrix_output_count;
    let per = |ch: &ChannelId| {
        ParameterPath::applicable_to(ch, aux, grp, mtx)
            .iter()
            .filter(|p| p.to_gp_osc_suffix().is_some())
            .count()
    };
    config.input_channel_count as usize * per(&ChannelId::Input(1))
        + config.aux_output_count as usize * per(&ChannelId::Aux(1))
        + config.group_output_count as usize * per(&ChannelId::Group(1))
        + config.matrix_output_count as usize * per(&ChannelId::Matrix(1))
        + config.control_group_count as usize * per(&ChannelId::ControlGroup(1))
        + config.graphic_eq_count as usize * per(&ChannelId::GraphicEq(1))
        + config.matrix_input_count as usize * per(&ChannelId::MatrixInput(1))
}

/// Process a parsed GP OSC message — update state mirror, propagate gangs, record macros.
async fn process_message(parsed: &ParsedOscMessage, daemon: &DaemonState, sender: &OscSender) {
    match parsed {
        ParsedOscMessage::ParameterUpdate(addr, value) => {
            // Full shared side-effect chain: mirror update, dirty screening,
            // operator-override detection, gang + pan-link propagation,
            // macro-learn capture. See `console::inbound`.
            inbound::apply_inbound_parameter(daemon, addr, value, InboundSource::GpOsc).await;
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
                inputs,
                aux,
                groups,
                control_groups,
                matrices,
                master,
                "Received /console/channel/counts — applying to config"
            );
            let mut s = daemon.state.write().await;
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
        ParsedOscMessage::DiscoveryCount {
            channel_type,
            count,
        } => {
            debug!(
                channel_type,
                count, "Per-type channel count (back-compat path)"
            );
            let mut s = daemon.state.write().await;
            crate::console::discovery::apply_channel_count(&mut s.config, channel_type, *count);
        }
        ParsedOscMessage::CurrentSnapshot(row) => {
            // The desk reported (or echoed) a snapshot load. Normalize the wire
            // value back to the app's STORED 1-based base so the mirror,
            // suppression map, and follow dispatcher all speak one base. With
            // the default offset of 0 this is a no-op; see
            // `CONSOLE_SNAPSHOT_WIRE_OFFSET` for how to confirm the desk's base.
            let stored = *row - crate::console::snapshot_engine::CONSOLE_SNAPSHOT_WIRE_OFFSET;
            // Logged at info so the operator can read the desk's wire base
            // straight from the app log when calibrating Console→App follow.
            info!(wire = *row, stored, "Console current-snapshot (GP OSC)");
            // A snapshot is loading on the desk (our own fire, or a desk-driven
            // load): arm the suppression window so the resulting parameter
            // flood isn't gang/pan-propagated back at the console.
            crate::console::snapshot_engine::arm_console_load(&daemon.console_load_suppression);
            let changed = {
                let mut s = daemon.state.write().await;
                let prev = s.current_console_snapshot;
                s.current_console_snapshot = Some(stored);
                prev != Some(stored)
            };
            if changed {
                if let Some(tx) = &daemon.console_snapshot_tx {
                    let _ = tx.try_send(stored);
                }
            }
        }
        ParsedOscMessage::Unknown(path) => {
            tracing::trace!(path, "Unknown OSC message");
        }
    }
}

/// Builders for tests in this crate that need a wired-up [`DaemonState`].
///
/// Lives here because `DaemonState` does, and it is shared by the inbound
/// chain's tests and the Pad connection's.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::model::state::ConsoleState;

    /// A `DaemonState` around an existing console-state handle, with real (if
    /// unconnected) engines behind it. Sends go to a loopback port nobody
    /// listens on — enough for anything that only inspects the mirror.
    pub async fn daemon_with_state(state: Arc<RwLock<ConsoleState>>) -> DaemonState {
        let client = crate::osc::client::OscClient::new(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:1".parse().unwrap(),
            None,
        )
        .await
        .unwrap();
        let (sender, _rx) = client.into_parts();

        let gang_manager = Arc::new(RwLock::new(GangManager::new()));
        let dirty_tracker = Arc::new(RwLock::new(DirtyTracker::new()));
        DaemonState {
            state: state.clone(),
            macro_manager: Arc::new(RwLock::new(MacroManager::new())),
            gang_engine: Arc::new(RwLock::new(GangEngine::new(state.clone(), sender.clone()))),
            gang_manager: gang_manager.clone(),
            pan_link_engine: Arc::new(RwLock::new(PanLinkEngine::new(
                state,
                sender,
                Arc::new(RwLock::new(
                    crate::model::pan_link::PanLinkBindings::default(),
                )),
                dirty_tracker.clone(),
                gang_manager,
            ))),
            dirty_tracker,
            offline_mode: Arc::new(AtomicBool::new(false)),
            recall_progress: Arc::new(crate::model::recall_progress::RecallProgress::new()),
            last_received: Arc::new(RwLock::new(None)),
            console_snapshot_tx: None,
            console_load_suppression: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            automation_override: None,
            sent_log: SentLog::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_estimate_counts_only_gp_osc_paths_and_is_realistic() {
        use crate::model::config::ConsoleConfig;
        // The 60-in / 24-mix (10 aux + 14 group) extension reports 11737 GP-mode
        // params on the real desk; our estimate should land close to that.
        let mut cfg = ConsoleConfig::default();
        cfg.input_channel_count = 60;
        cfg.aux_output_count = 10;
        cfg.group_output_count = 14;
        cfg.matrix_output_count = 8;
        cfg.control_group_count = 10;
        cfg.graphic_eq_count = 16;
        cfg.matrix_input_count = 10;
        let est = estimate_expected_param_count(&cfg);
        // Within ~15% of the desk-reported 11737 (mono channels omit a few
        // params, hence the band rather than an exact match).
        assert!(
            (10_000..=13_500).contains(&est),
            "dump estimate {est} not close to the desk-reported 11737"
        );
    }
}
