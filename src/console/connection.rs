use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

// ── Fader change-screening deadbands ───────────────────────────────────
// Motorized faders don't always return to a bit-exact dB after a recall or a
// console layer change (rotary-encoder params do). These dB deadbands let the
// auto-preselect "dirty" screening ignore that mechanical jitter. Bands widen
// as the level drops — the physical track compresses a large dB range into a
// sliver down low, mirroring the `FADER_GANG_FLOOR_DB` reasoning in `parameter`.
const FADER_DEADBAND_HI_DB: f32 = 0.2; //  value >= -10 dB
const FADER_DEADBAND_MID_DB: f32 = 0.3; // -40 <= value < -10 dB
const FADER_DEADBAND_LO_DB: f32 = 0.4; //  value <  -40 dB

/// True when an inbound value is a *meaningful* change vs the stored previous
/// value — i.e. it should mark the cell dirty for the self-actualising scope.
///
/// Fader-controllable level params (see [`ParameterPath::is_fader_level`]) get a
/// dB deadband, banded by the new (settled) value, so post-recall / layer-change
/// fader jitter doesn't auto-preselect. Every other parameter keeps exact
/// equality, matching the original `prev != value` behaviour.
///
/// Note: the comparison is against the immediately-preceding stored value
/// (`ConsoleState::update` overwrites on every message). A very slow continuous
/// fader ride in sub-deadband steps could therefore ride a long way without ever
/// marking dirty — accepted, because this screens recall/layer-change jitter,
/// not live rides (a real ride moves faster than the band per message). True
/// hysteresis would need a separate "last-marked value" map, not a change to the
/// live mirror.
fn is_meaningful_change(
    path: &crate::model::parameter::ParameterPath,
    prev: &crate::model::parameter::ParameterValue,
    new: &crate::model::parameter::ParameterValue,
) -> bool {
    use crate::model::parameter::ParameterValue::Float;
    if path.is_fader_level()
        && let (Float(a), Float(b)) = (prev, new)
    {
        let band = if *b >= -10.0 {
            FADER_DEADBAND_HI_DB
        } else if *b >= -40.0 {
            FADER_DEADBAND_MID_DB
        } else {
            FADER_DEADBAND_LO_DB
        };
        return (a - b).abs() > band;
    }
    prev != new
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
            // TotalGain (GP OSC `total/gain`) is a console-derived, read-only
            // monitor value (post-fader + CG sum). It can't be written back or
            // meaningfully recalled, so drop it on receipt before it pollutes
            // the state mirror, dirty tracker, override registry, gangs,
            // pan-link, or macro recording. Mirrors macro_manager::record_change.
            if matches!(
                addr.parameter,
                crate::model::parameter::ParameterPath::TotalGain
            ) {
                return;
            }
            debug!(%addr, %value, "Parameter update");
            let old_value = daemon
                .state
                .write()
                .await
                .update(addr.clone(), value.clone());

            // Mirror the most recent parameter address for the Macros
            // tab's "track latest OSC" affordance.
            *daemon.last_received.write().await = Some(addr.clone());

            // Mark this cell dirty IF the value actually changed. The dirty
            // tracker is suppression-aware, so echoes from snapshot recall
            // (which set begin_suppression before sending) are ignored. The
            // first sample after a connection comes through old_value=None,
            // which we treat as "this is the baseline" — not a change.
            if let Some(prev) = &old_value
                && is_meaningful_change(&addr.parameter, prev, value)
            {
                daemon.dirty_tracker.write().await.mark(addr);
            }

            // While a console snapshot is loading, DON'T gang/pan-propagate the
            // desk's echo flood: the desk already applied the snapshot
            // coherently, and re-propagating it back fights the desk and can
            // saturate the outbound path into a stall (the App→Console hang).
            let in_console_load = crate::console::snapshot_engine::console_load_active(
                &daemon.console_load_suppression,
            );

            // Operator live-override: if this inbound change is the operator
            // (hands on the desk) grabbing a parameter that a timed recall is
            // currently pre-waiting or fading, the registry drops that one so the
            // running fade/pre-wait stops sending it and the hands-on value wins.
            // Gated on `!in_console_load`: the desk's own snapshot-load flood
            // overlaps the first seconds of a memory-backed recall and would
            // otherwise look like a storm of operator moves, spuriously
            // cancelling the very recall it belongs to.
            if !in_console_load
                && let Some(reg) = &daemon.automation_override
                && matches!(
                    reg.maybe_override(addr, value),
                    crate::console::automation_registry::Override::Operator
                )
            {
                debug!(%addr, "Operator override — automation cancelled for this parameter");
            }

            // Gang propagation — before macro recording so the engineer's
            // original change is what gets recorded, not ganged echoes.
            if !in_console_load {
                let mut engine = daemon.gang_engine.write().await;
                let manager = daemon.gang_manager.read().await;
                engine
                    .process_gang_update(addr, value, old_value.as_ref(), &manager)
                    .await;
            }

            // Pan link propagation — runs after gangs so a gang-driven
            // pan change on an input also pushes to its linked aux sends.
            if !in_console_load {
                let engine = daemon.pan_link_engine.read().await;
                engine.process_pan_update(addr, value).await;
            }

            // Feed into macro learn mode if recording
            let mut mgr = daemon.macro_manager.write().await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parameter::{ParameterPath, ParameterValue};

    fn f(v: f32) -> ParameterValue {
        ParameterValue::Float(v)
    }

    #[test]
    fn fader_hi_band_ignores_small_jitter_marks_real_change() {
        // value >= -10 dB → 0.2 dB deadband.
        assert!(!is_meaningful_change(
            &ParameterPath::Fader,
            &f(-5.0),
            &f(-4.85)
        )); // 0.15 ≤ 0.2
        assert!(is_meaningful_change(
            &ParameterPath::Fader,
            &f(-5.0),
            &f(-4.75)
        )); // 0.25 > 0.2
    }

    #[test]
    fn fader_mid_band() {
        // -40 <= value < -10 dB → 0.3 dB deadband.
        assert!(!is_meaningful_change(
            &ParameterPath::Fader,
            &f(-25.0),
            &f(-25.25)
        )); // 0.25 ≤ 0.3
        assert!(is_meaningful_change(
            &ParameterPath::Fader,
            &f(-25.0),
            &f(-25.35)
        )); // 0.35 > 0.3
    }

    #[test]
    fn fader_lo_band() {
        // value < -40 dB → 0.4 dB deadband.
        assert!(!is_meaningful_change(
            &ParameterPath::Fader,
            &f(-60.0),
            &f(-60.35)
        )); // 0.35 ≤ 0.4
        assert!(is_meaningful_change(
            &ParameterPath::Fader,
            &f(-60.0),
            &f(-60.45)
        )); // 0.45 > 0.4
    }

    #[test]
    fn band_boundaries_use_new_value() {
        // Exactly -10.0 → HI band (>= -10). 0.25 dB is a change at HI(0.2)…
        assert!(is_meaningful_change(
            &ParameterPath::Fader,
            &f(-10.25),
            &f(-10.0)
        ));
        // …but the same 0.25 dB landing at -40.0 → MID band (>= -40) is ignored.
        assert!(!is_meaningful_change(
            &ParameterPath::Fader,
            &f(-40.25),
            &f(-40.0)
        ));
    }

    #[test]
    fn send_levels_share_the_fader_deadband() {
        assert!(!is_meaningful_change(
            &ParameterPath::SendLevel(3),
            &f(-5.0),
            &f(-4.85)
        ));
        assert!(is_meaningful_change(
            &ParameterPath::SendLevel(3),
            &f(-5.0),
            &f(-4.5)
        ));
        assert!(!is_meaningful_change(
            &ParameterPath::CgLevel,
            &f(-5.0),
            &f(-4.85)
        ));
        assert!(!is_meaningful_change(
            &ParameterPath::MatrixSendLevel(1),
            &f(-5.0),
            &f(-4.85)
        ));
    }

    #[test]
    fn encoder_params_keep_exact_equality() {
        // A 0.5 dB EQ-gain move is a real change — no deadband for encoder params.
        assert!(is_meaningful_change(
            &ParameterPath::EqBandGain(1),
            &f(0.0),
            &f(0.5)
        ));
        assert!(is_meaningful_change(
            &ParameterPath::TotalGain,
            &f(0.0),
            &f(0.1)
        ));
        // Identical values are never a change, for any param.
        assert!(!is_meaningful_change(
            &ParameterPath::EqBandGain(1),
            &f(0.0),
            &f(0.0)
        ));
        assert!(!is_meaningful_change(
            &ParameterPath::Fader,
            &f(-5.0),
            &f(-5.0)
        ));
    }

    #[test]
    fn non_float_params_use_exact_equality() {
        assert!(is_meaningful_change(
            &ParameterPath::Mute,
            &ParameterValue::Bool(true),
            &ParameterValue::Bool(false),
        ));
        assert!(!is_meaningful_change(
            &ParameterPath::Mute,
            &ParameterValue::Bool(true),
            &ParameterValue::Bool(true),
        ));
    }

    #[test]
    fn is_fader_level_classification() {
        assert!(ParameterPath::Fader.is_fader_level());
        assert!(ParameterPath::SendLevel(0).is_fader_level());
        assert!(ParameterPath::MatrixSendLevel(0).is_fader_level());
        assert!(ParameterPath::CgLevel.is_fader_level());
        assert!(!ParameterPath::EqBandGain(1).is_fader_level());
        assert!(!ParameterPath::TotalGain.is_fader_level());
        assert!(!ParameterPath::Pan.is_fader_level());
    }

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
