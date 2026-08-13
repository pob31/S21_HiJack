//! Pad-only console connection lifecycle (SD / Quantum).
//!
//! These consoles have no S-series GP OSC dialect, so everything the GP loop
//! in [`super::connection`] provides has to come from the Pad surface instead:
//!
//! | GP OSC                        | here                                    |
//! |-------------------------------|-----------------------------------------|
//! | `/console/channel/counts`     | handshake `/Console/…/?` queries        |
//! | `/console/resend` bulk dump   | paced per-parameter `/?` enumeration    |
//! | `/console/ping` → `/pong`     | periodic `/Console/Name/?`              |
//! | (n/a)                         | session-change resync                   |
//!
//! Everything downstream is dialect-agnostic and reused unchanged: inbound
//! values run through [`super::inbound::apply_inbound_parameter`], so gangs,
//! pan link, the dirty tracker and macro learn behave exactly as on an S21.
//!
//! `ipad_connection.rs` (the S-series Mode 2/3 paths) is deliberately left
//! alone — its behaviour is hardware-verified and its regression run is still
//! outstanding.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{self, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::model::config::ConsoleConfig;
use crate::model::family::ConsoleProfile;
use crate::model::osc_log::OscLog;
use crate::model::parameter::{ParameterAddress, ParameterPath};
use crate::model::recall_progress::RecallKind;
use crate::model::state::ConnectionHealth;
use crate::osc::client::{ReceivedOscMessage, format_osc_args};
use crate::osc::ipad_client::{IpadClient, IpadSender};
use crate::osc::ipad_encode;
use crate::osc::ipad_parse::{self, ParsedIpadMessage};

use super::connection::DaemonState;
use super::inbound::{self, InboundSource};
use super::ipad_handshake::{self, HandshakeOptions};

/// How long the handshake waits in each collection phase.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long one enumeration query waits for its reply before being retried.
const QUERY_TIMEOUT: Duration = Duration::from_millis(250);
/// Retries per enumeration query before it is skipped and logged.
const QUERY_RETRIES: u8 = 2;
/// Heartbeat cadence once enumeration is done.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
/// Missed heartbeats before the link is called `Stale`, then `Lost`.
const HEARTBEAT_MISSES_STALE: u8 = 3;
const HEARTBEAT_MISSES_LOST: u8 = 6;
/// Loop tick — drives query timeouts and the heartbeat.
const TICK: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub enum PadConnectionError {
    Io(std::io::Error),
    Handshake(ipad_handshake::HandshakeError),
}

impl std::fmt::Display for PadConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "Pad connection I/O error: {e}"),
            Self::Handshake(e) => write!(f, "Pad handshake failed: {e}"),
        }
    }
}

impl std::error::Error for PadConnectionError {}

impl From<std::io::Error> for PadConnectionError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<ipad_handshake::HandshakeError> for PadConnectionError {
    fn from(e: ipad_handshake::HandshakeError) -> Self {
        Self::Handshake(e)
    }
}

/// Connect to a Pad-only console: handshake, then run the mirror loop.
///
/// Returns the sender for engine writes plus the loop's join handle. The
/// handshake's discovered config is applied to `daemon.state` before the loop
/// starts, so callers see real channel counts immediately.
pub async fn connect_pad(
    console_addr: SocketAddr,
    local_addr: SocketAddr,
    daemon: DaemonState,
    profile: Arc<ConsoleProfile>,
    cancel: CancellationToken,
    interface_name: Option<&str>,
    osc_log: Option<OscLog>,
) -> Result<(IpadSender, JoinHandle<()>), PadConnectionError> {
    info!(%console_addr, %local_addr, "Pad connection: connecting…");

    let client = IpadClient::new(local_addr, console_addr, interface_name).await?;
    let (mut sender, mut rx) = client.into_parts_with_cancel(cancel.clone());
    sender.set_log(osc_log.clone());
    sender.set_offline_flag(daemon.offline_mode.clone());

    set_health(&daemon, ConnectionHealth::Connecting).await;

    let result = ipad_handshake::perform_handshake_with(
        &sender,
        &mut rx,
        HANDSHAKE_TIMEOUT,
        &HandshakeOptions::pad_only(),
    )
    .await?;

    // A handshake returns Ok even against a desk that never answers, so the
    // reply count is the only real evidence of reachability.
    if result.responses_seen == 0 {
        warn!(
            "Pad handshake completed with no replies — the console may not have \
             External Control enabled, may be pointed at a different port, or may \
             need its Pad command set loaded"
        );
    } else {
        info!(
            name = %result.config.console_name,
            replies = result.responses_seen,
            inputs = result.config.input_channel_count,
            "Pad handshake complete"
        );
    }

    apply_discovered_config(&daemon, &result.config).await;
    if let Some(n) = result.current_snapshot {
        daemon.state.write().await.current_console_snapshot = Some(n);
    }
    if result.responses_seen > 0 {
        set_health(&daemon, ConnectionHealth::Connected).await;
    }

    let handle = tokio::spawn(run_loop(
        sender.clone(),
        rx,
        daemon,
        profile,
        cancel,
        osc_log,
    ));
    Ok((sender, handle))
}

/// Copy the counts and identity the handshake discovered into the live config
/// **in place**.
///
/// Deliberately field-by-field rather than replacing the struct: `family` and
/// `pad_quirk_overrides` describe how we choose to talk to this desk and come
/// from the show file or preferences. The console does not know about them and
/// must never overwrite them.
async fn apply_discovered_config(daemon: &DaemonState, discovered: &ConsoleConfig) {
    let mut state = daemon.state.write().await;
    let cfg = &mut state.config;
    cfg.console_name = discovered.console_name.clone();
    cfg.console_serial = discovered.console_serial.clone();
    cfg.session_filename = discovered.session_filename.clone();
    cfg.input_channel_count = discovered.input_channel_count;
    cfg.aux_output_count = discovered.aux_output_count;
    cfg.group_output_count = discovered.group_output_count;
    cfg.matrix_output_count = discovered.matrix_output_count;
    cfg.matrix_input_count = discovered.matrix_input_count;
    cfg.control_group_count = discovered.control_group_count;
    cfg.graphic_eq_count = discovered.graphic_eq_count;
    cfg.talkback_output_count = discovered.talkback_output_count;
    if !discovered.mix_output_types.is_empty() {
        cfg.mix_output_types = discovered.mix_output_types.clone();
    }
    if !discovered.mix_output_modes.is_empty() {
        cfg.mix_output_modes = discovered.mix_output_modes.clone();
    }
    if !discovered.input_modes.is_empty() {
        cfg.input_modes = discovered.input_modes.clone();
    }
    if !discovered.group_modes.is_empty() {
        cfg.group_modes = discovered.group_modes.clone();
    }
}

/// One outstanding enumeration query.
struct InFlight {
    address: ParameterAddress,
    path: String,
    attempts: u8,
    sent_at: Instant,
}

/// Build the enumeration queue for the discovered console.
///
/// Ordered by usefulness, not by address: the parameters an operator notices
/// first — names, faders, mutes, sends — are queried before the deep
/// processing ones, so gangs and monitoring are usable long before a
/// 256-input desk finishes its ~10k round-trips.
fn build_enumeration_queue(
    config: &ConsoleConfig,
    profile: &ConsoleProfile,
) -> Vec<ParameterAddress> {
    use crate::model::channel::ChannelId;

    let family = profile.family;
    let counts: Vec<ChannelId> = std::iter::empty()
        .chain((1..=config.input_channel_count).map(ChannelId::Input))
        .chain((1..=config.aux_output_count).map(ChannelId::Aux))
        .chain((1..=config.group_output_count).map(ChannelId::Group))
        .chain((1..=config.matrix_output_count).map(ChannelId::Matrix))
        .chain((1..=config.matrix_input_count).map(ChannelId::MatrixInput))
        .chain((1..=config.control_group_count).map(ChannelId::ControlGroup))
        .chain((1..=config.graphic_eq_count).map(ChannelId::GraphicEq))
        .collect();

    let priority = |p: &ParameterPath| match p {
        ParameterPath::Name => 0,
        ParameterPath::Fader | ParameterPath::Mute => 1,
        ParameterPath::SendLevel(_) | ParameterPath::SendEnabled(_) => 2,
        ParameterPath::Pan | ParameterPath::SendPan(_) | ParameterPath::Solo => 3,
        _ => 4,
    };

    let mut out: Vec<(u8, ParameterAddress)> = Vec::new();
    for channel in counts {
        let paths = ParameterPath::applicable_to_for_family(
            &channel,
            config.aux_output_count,
            config.group_output_count,
            config.matrix_output_count,
            family,
        );
        for parameter in paths {
            // Only ask for what this surface can actually address.
            if parameter.to_pad_suffix(&profile.pad_quirks).is_none() {
                continue;
            }
            out.push((
                priority(&parameter),
                ParameterAddress {
                    channel: channel.clone(),
                    parameter,
                },
            ));
        }
    }
    out.sort_by_key(|(p, _)| *p);
    out.into_iter().map(|(_, a)| a).collect()
}

#[allow(clippy::too_many_arguments)]
async fn run_loop(
    sender: IpadSender,
    mut rx: mpsc::Receiver<ReceivedOscMessage>,
    daemon: DaemonState,
    profile: Arc<ConsoleProfile>,
    cancel: CancellationToken,
    osc_log: Option<OscLog>,
) {
    let wire = crate::model::family::PadWire::new(profile.primary_surface(), profile.pad_quirks);

    let mut queue = {
        let config = daemon.state.read().await.config.clone();
        build_enumeration_queue(&config, &profile)
    };
    queue.reverse(); // pop() takes from the back, so reverse to keep priority order
    let total = queue.len();
    info!(total, "Pad enumeration: starting");
    daemon.recall_progress.begin(RecallKind::Dump, total);

    let mut in_flight: Option<InFlight> = None;
    let mut skipped = 0usize;
    let mut last_heartbeat = Instant::now();
    let mut heartbeat_outstanding = false;
    let mut misses: u8 = 0;
    let mut ticker = time::interval(TICK);
    ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    // Kick off the first query.
    send_next_query(&sender, &mut queue, &mut in_flight, &wire).await;

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                info!("Pad connection cancelled");
                daemon.recall_progress.finish();
                break;
            }
            msg = rx.recv() => {
                let Some(msg) = msg else {
                    info!("Pad receive channel closed");
                    daemon.recall_progress.finish();
                    break;
                };
                if daemon.offline_mode.load(Ordering::Relaxed) {
                    continue;
                }
                // Any inbound traffic proves the link is alive.
                misses = 0;
                heartbeat_outstanding = false;
                set_health(&daemon, ConnectionHealth::Connected).await;

                let matched = handle_message(&msg, &daemon, &wire, &osc_log, in_flight.as_ref()).await;
                if matched {
                    in_flight = None;
                    daemon.recall_progress.bump();
                    send_next_query(&sender, &mut queue, &mut in_flight, &wire).await;
                }
            }
            _ = ticker.tick() => {
                // Enumeration: retry or skip whatever is stuck.
                if let Some(f) = &mut in_flight
                    && f.sent_at.elapsed() >= QUERY_TIMEOUT
                {
                    if f.attempts >= QUERY_RETRIES {
                        debug!(path = %f.path, "Pad enumeration: no reply, skipping");
                        skipped += 1;
                        in_flight = None;
                        daemon.recall_progress.bump();
                        send_next_query(&sender, &mut queue, &mut in_flight, &wire).await;
                    } else {
                        f.attempts += 1;
                        f.sent_at = Instant::now();
                        let _ = sender.send(&f.path, vec![]).await;
                    }
                }

                if in_flight.is_none() && queue.is_empty() {
                    // Enumeration finished — settle into the heartbeat.
                    if daemon.recall_progress.is_active()
                        && daemon.recall_progress.kind() == RecallKind::Dump
                    {
                        info!(total, skipped, "Pad enumeration: complete");
                        daemon.recall_progress.finish();
                    }
                    if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                        last_heartbeat = Instant::now();
                        if heartbeat_outstanding {
                            misses = misses.saturating_add(1);
                            if misses >= HEARTBEAT_MISSES_LOST {
                                set_health(&daemon, ConnectionHealth::Lost).await;
                            } else if misses >= HEARTBEAT_MISSES_STALE {
                                set_health(&daemon, ConnectionHealth::Stale).await;
                            }
                        }
                        heartbeat_outstanding = true;
                        let _ = sender.send("/Console/Name/?", vec![]).await;
                    }
                }
            }
        }
    }
    info!("Pad connection loop ended");
}

/// Send the next queued query, if any.
async fn send_next_query(
    sender: &IpadSender,
    queue: &mut Vec<ParameterAddress>,
    in_flight: &mut Option<InFlight>,
    wire: &crate::model::family::PadWire,
) {
    while let Some(address) = queue.pop() {
        let Some(path) = ipad_encode::encode_pad_query(&address.channel, &address.parameter, wire)
        else {
            continue;
        };
        let _ = sender.send(&path, vec![]).await;
        *in_flight = Some(InFlight {
            address,
            path,
            attempts: 0,
            sent_at: Instant::now(),
        });
        return;
    }
}

/// Process one inbound message. Returns true if it answered `in_flight`.
async fn handle_message(
    msg: &ReceivedOscMessage,
    daemon: &DaemonState,
    wire: &crate::model::family::PadWire,
    osc_log: &Option<OscLog>,
    in_flight: Option<&InFlight>,
) -> bool {
    let config = daemon.state.read().await.config.clone();
    let parsed = ipad_parse::parse_pad_message(&msg.path, &msg.args, Some(&config), wire);

    if let Some(log) = osc_log
        && !matches!(parsed, ParsedIpadMessage::MeterValues(_))
    {
        log.log_ipad_in(&msg.path, &format_osc_args(&msg.args));
    }

    match parsed {
        ParsedIpadMessage::ParameterUpdate(addr, value) => {
            let answered = in_flight.is_some_and(|f| f.address == addr);
            inbound::apply_inbound_parameter(daemon, &addr, &value, InboundSource::Pad).await;
            answered
        }
        ParsedIpadMessage::SnapshotInfo { current } => {
            let prev = {
                let mut s = daemon.state.write().await;
                let p = s.current_console_snapshot;
                s.current_console_snapshot = Some(current);
                p
            };
            if prev != Some(current)
                && let Some(tx) = &daemon.console_snapshot_tx
            {
                let _ = tx.try_send(current);
            }
            false
        }
        ParsedIpadMessage::ConfigResponse(cfg_msg) => {
            // The desk re-pushes config when the session changes or a bus is
            // reconfigured; follow it so the UI tracks the console live.
            let mut s = daemon.state.write().await;
            ipad_handshake::apply_config_message(&mut s.config, &cfg_msg);
            false
        }
        ParsedIpadMessage::MeterValues(_) => false,
        _ => false,
    }
}

async fn set_health(daemon: &DaemonState, health: ConnectionHealth) {
    let mut s = daemon.state.write().await;
    if s.health != health {
        debug!(?health, "Pad connection health changed");
        s.health = health;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::family::ConsoleFamily;

    fn quantum_config() -> ConsoleConfig {
        ConsoleConfig {
            input_channel_count: 4,
            aux_output_count: 2,
            group_output_count: 0,
            matrix_output_count: 0,
            matrix_input_count: 0,
            control_group_count: 0,
            graphic_eq_count: 0,
            family: ConsoleFamily::Quantum,
            ..Default::default()
        }
    }

    #[test]
    fn enumeration_queue_is_priority_ordered_and_pad_addressable() {
        let cfg = quantum_config();
        let profile = ConsoleProfile::for_family(ConsoleFamily::Quantum);
        let queue = build_enumeration_queue(&cfg, &profile);
        assert!(!queue.is_empty());

        // Every entry must be expressible on the Pad tree — a query we cannot
        // encode would stall the one-in-flight pump.
        for addr in &queue {
            assert!(
                addr.parameter.to_pad_suffix(&profile.pad_quirks).is_some(),
                "{addr} has no Pad path but was queued"
            );
        }

        // Names and faders come before deep processing parameters.
        let first_deep = queue
            .iter()
            .position(|a| matches!(a.parameter, ParameterPath::EqBandGain(_)))
            .expect("EQ should be enumerated somewhere");
        let last_fader = queue
            .iter()
            .rposition(|a| matches!(a.parameter, ParameterPath::Fader | ParameterPath::Name))
            .expect("faders and names should be enumerated");
        assert!(
            last_fader < first_deep,
            "faders/names must be queried before EQ bands"
        );
    }

    #[test]
    fn enumeration_queue_covers_the_extra_sd_output_eq_bands() {
        let cfg = quantum_config();
        let profile = ConsoleProfile::for_family(ConsoleFamily::Quantum);
        let queue = build_enumeration_queue(&cfg, &profile);
        assert!(
            queue
                .iter()
                .any(|a| a.channel == ChannelId::Aux(1)
                    && a.parameter == ParameterPath::EqBandGain(8)),
            "a Quantum aux has eight EQ bands, so band 8 must be enumerated"
        );
    }

    #[tokio::test]
    async fn discovered_config_never_clobbers_family_or_quirk_overrides() {
        use crate::model::family::PadQuirks;
        use crate::model::state::ConsoleState;
        use tokio::sync::RwLock;

        // The operator's family choice and hand-tuned quirks live in the show
        // file; the desk knows nothing about them.
        let overrides = PadQuirks {
            eq_bands_reversed: true,
            ..PadQuirks::SD_HYPOTHESIS
        };
        let mut starting = ConsoleConfig {
            family: ConsoleFamily::Quantum,
            pad_quirk_overrides: Some(overrides),
            ..Default::default()
        };
        starting.input_channel_count = 1;
        let state = Arc::new(RwLock::new(ConsoleState::new(starting)));
        let daemon =
            crate::console::connection::test_support::daemon_with_state(state.clone()).await;

        let mut discovered = ConsoleConfig::default();
        discovered.input_channel_count = 128;
        discovered.console_name = "Quantum338".into();
        apply_discovered_config(&daemon, &discovered).await;

        let cfg = &state.read().await.config;
        assert_eq!(cfg.input_channel_count, 128, "counts come from the desk");
        assert_eq!(cfg.console_name, "Quantum338");
        assert_eq!(
            cfg.family,
            ConsoleFamily::Quantum,
            "family is ours, not the desk's"
        );
        assert_eq!(cfg.pad_quirk_overrides, Some(overrides));
    }
}
