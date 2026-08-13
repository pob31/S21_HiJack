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
    let (mut sender, rx) = client.into_parts_with_cancel(cancel.clone());
    // Per-clone fields: set them before the sender is cloned anywhere, or the
    // clones log nothing and ignore offline mode.
    sender.set_log(osc_log.clone());
    sender.set_offline_flag(daemon.offline_mode.clone());

    let handle =
        connect_pad_from_parts(sender.clone(), rx, daemon, profile, cancel, osc_log).await?;
    Ok((sender, handle))
}

/// [`connect_pad`] from pre-built parts — the route the GUI takes.
///
/// The engines inside `daemon` need a [`ConsoleTx`](super::console_tx::ConsoleTx)
/// built on the Pad sender, so the sender has to exist *before* the
/// `DaemonState` does. That inverts [`connect_pad`]'s ordering, exactly as
/// [`ConnectionManager::connect_from_parts`](super::connection::ConnectionManager::connect_from_parts)
/// does for the S-series GP path.
///
/// The caller keeps its own clone of `sender` for engine writes; the returned
/// handle belongs to the mirror loop, which also stops on `cancel`.
pub async fn connect_pad_from_parts(
    sender: IpadSender,
    mut rx: mpsc::Receiver<ReceivedOscMessage>,
    daemon: DaemonState,
    profile: Arc<ConsoleProfile>,
    cancel: CancellationToken,
    osc_log: Option<OscLog>,
) -> Result<JoinHandle<()>, PadConnectionError> {
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

    Ok(tokio::spawn(run_loop(
        sender, rx, daemon, profile, cancel, osc_log,
    )))
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

    // ── The live loop, against a scripted desk ───────────────────────────
    //
    // These drive `connect_pad_from_parts` end to end over a real loopback
    // UDP pair, with a scripted responder playing the console side — the same
    // shape as `ipad_handshake`'s `mock_ipad_pair`. What they assert is what a
    // real desk would see on the wire, and what the mirror ends up holding.

    use rosc::{OscMessage, OscPacket, OscType};
    use std::collections::{HashSet, VecDeque};
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::AtomicBool;
    use tokio::net::UdpSocket;
    use tokio::sync::RwLock;

    use crate::console::console_tx::{ConsoleTx, SentLog};
    use crate::console::gang_engine::GangEngine;
    use crate::console::gang_manager::GangManager;
    use crate::model::gang::{GangGroup, GangMode};
    use crate::model::parameter::{ParameterSection, ParameterValue};
    use crate::model::state::ConsoleState;
    use crate::osc::ipad_client::IpadClient;

    /// The desk every script plays: a Quantum with two inputs and no buses.
    /// Small on purpose — enumeration is one round trip per parameter per
    /// channel, so a realistic 128-input desk would make these tests minutes
    /// long without testing anything more.
    const SCRIPT_INPUTS: u16 = 2;
    /// What the scripted desk reports for every numeric parameter.
    const SCRIPT_FLOAT: f32 = -3.0;
    /// Where the operator drags input 1's fader in the gang test. Distinct
    /// from [`SCRIPT_FLOAT`] so the ganged write can't be confused with a
    /// value enumeration already pushed.
    const MOVED_DB: f32 = -14.5;

    /// How the scripted desk behaves. Each test bends one axis.
    struct DeskScript {
        /// Query paths (trailing `/?` included) the desk deliberately drops.
        silent: Vec<String>,
        /// Held back before each enumeration reply.
        reply_delay: Duration,
        /// Clear to make the desk stop answering the `/Console/Name/?`
        /// heartbeat; set again to bring the link back.
        answer_heartbeat: Arc<AtomicBool>,
        /// Watch for a second query arriving while one is still unanswered.
        watch_pacing: bool,
    }

    impl Default for DeskScript {
        fn default() -> Self {
            Self {
                silent: Vec::new(),
                reply_delay: Duration::ZERO,
                answer_heartbeat: Arc::new(AtomicBool::new(true)),
                watch_pacing: false,
            }
        }
    }

    /// What the scripted desk saw, for assertions after the fact.
    #[derive(Default)]
    struct DeskLog {
        /// Every message the app sent, in arrival order.
        received: Vec<(String, Vec<OscType>)>,
        /// How many times a query arrived while the previous one was still
        /// unanswered — the pacing failure that makes real SD desks drop
        /// queries.
        overlaps: usize,
        /// The app's ephemeral address, learned from its first datagram.
        app_addr: Option<SocketAddr>,
    }

    /// The console side of one connection.
    struct Desk {
        sock: Arc<UdpSocket>,
        log: Arc<StdMutex<DeskLog>>,
        cancel: CancellationToken,
    }

    /// Cancelling on drop stops the desk task and the connection loop the
    /// moment a test ends — including when it ends by panicking.
    impl Drop for Desk {
        fn drop(&mut self) {
            self.cancel.cancel();
        }
    }

    /// Start a scripted desk and a Pad client aimed at it.
    async fn scripted_desk(
        script: DeskScript,
    ) -> (Desk, IpadSender, mpsc::Receiver<ReceivedOscMessage>) {
        let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let console_addr = sock.local_addr().unwrap();
        let client = IpadClient::new("127.0.0.1:0".parse().unwrap(), console_addr, None)
            .await
            .unwrap();
        let (sender, rx) = client.into_parts();

        let log = Arc::new(StdMutex::new(DeskLog::default()));
        let cancel = CancellationToken::new();
        tokio::spawn(run_desk(sock.clone(), script, log.clone(), cancel.clone()));
        (Desk { sock, log, cancel }, sender, rx)
    }

    /// Play the console side of the Pad protocol until cancelled.
    ///
    /// Handshake queries get the canned config below; an enumeration `/?`
    /// query gets its answer on the un-suffixed path, which is what advances
    /// the app's one-in-flight pump.
    async fn run_desk(
        sock: Arc<UdpSocket>,
        script: DeskScript,
        log: Arc<StdMutex<DeskLog>>,
        cancel: CancellationToken,
    ) {
        let mut buf = vec![0u8; 65536];
        let mut backlog: VecDeque<(Vec<u8>, SocketAddr)> = VecDeque::new();

        loop {
            let (data, app) = match backlog.pop_front() {
                Some(next) => next,
                None => {
                    let recv = tokio::select! {
                        biased;
                        () = cancel.cancelled() => break,
                        r = sock.recv_from(&mut buf) => r,
                    };
                    let Ok((n, app)) = recv else { break };
                    (buf[..n].to_vec(), app)
                }
            };

            let Some((path, args)) = decode_osc(&data) else {
                continue;
            };
            {
                let mut log = log.lock().unwrap();
                log.app_addr = Some(app);
                log.received.push((path.clone(), args));
            }

            // Anything without the query suffix is a write (or `/Meters/clear`)
            // — the desk has nothing to say back.
            let Some(target) = path.strip_suffix("/?") else {
                continue;
            };
            if script.silent.contains(&path) {
                continue;
            }
            if target == "/Console/Name" && !script.answer_heartbeat.load(Ordering::Relaxed) {
                continue;
            }

            let replies = if target.starts_with("/Input_Channels/") {
                if !script.reply_delay.is_zero() {
                    time::sleep(script.reply_delay).await;
                }
                if script.watch_pacing {
                    let waiting = drain_pending(&sock, &mut backlog).await;
                    if waiting > 0 {
                        log.lock().unwrap().overlaps += waiting;
                    }
                }
                vec![(target.to_string(), parameter_reply(target))]
            } else {
                handshake_answers(target)
            };
            for (path, args) in replies {
                send_from_desk(&sock, app, &path, args).await;
            }
        }
    }

    /// Pull anything already waiting on the desk's socket into `backlog`,
    /// returning how many datagrams were there. Called while a query is still
    /// unanswered, where the answer has to be none.
    async fn drain_pending(
        sock: &UdpSocket,
        backlog: &mut VecDeque<(Vec<u8>, SocketAddr)>,
    ) -> usize {
        let mut waiting = 0;
        let mut probe = [0u8; 1024];
        for _ in 0..32 {
            match time::timeout(Duration::from_millis(2), sock.recv_from(&mut probe)).await {
                Ok(Ok((n, from))) => {
                    backlog.push_back((probe[..n].to_vec(), from));
                    waiting += 1;
                }
                _ => break,
            }
        }
        waiting
    }

    /// The scripted desk's answers to one handshake query.
    ///
    /// `/Console/Channels/?` drags every other count along with it. The
    /// handshake asks only for the input count, and a Pad-only console has no
    /// GP OSC `/console/channel/counts` to supply the rest, so a real desk
    /// pushing its configuration unprompted is what fills them in — see
    /// `ipad_handshake::tests::handshake_collects_config`. Without them the
    /// daemon keeps `ConsoleConfig::default()`'s eight auxes and eight groups
    /// and enumerates hundreds of channels this desk does not have.
    fn handshake_answers(target: &str) -> Vec<(String, Vec<OscType>)> {
        let one = |args: Vec<OscType>| vec![(target.to_string(), args)];
        match target {
            "/Snapshots/Current_Snapshot" => one(vec![OscType::Int(3)]),
            "/Console/Name" => one(vec![OscType::String("Quantum338 QT-4242".into())]),
            "/Console/Session/Filename" => one(vec![OscType::String("Test.ses".into())]),
            "/Console/Channels" => vec![
                (
                    "/Console/Channels".into(),
                    vec![OscType::Int(i32::from(SCRIPT_INPUTS))],
                ),
                ("/Console/Aux_Outputs".into(), vec![OscType::Int(0)]),
                ("/Console/Group_Outputs".into(), vec![OscType::Int(0)]),
                ("/Console/Matrix_Outputs".into(), vec![OscType::Int(0)]),
                ("/Console/Matrix_Inputs".into(), vec![OscType::Int(0)]),
                ("/Console/Control_Groups".into(), vec![OscType::Int(0)]),
                ("/Console/Graphic_EQ".into(), vec![OscType::Int(0)]),
            ],
            "/Console/Input_Channels/modes" => one(vec![OscType::Int(1); SCRIPT_INPUTS as usize]),
            "/Console/Multis"
            | "/Console/Matrix_Outputs"
            | "/Console/Matrix_Inputs"
            | "/Console/Control_Groups"
            | "/Console/Graphic_EQ" => one(vec![OscType::Int(0)]),
            // A desk with no buses reports no bus modes, and this one has no
            // layout banks to offer either.
            _ => Vec::new(),
        }
    }

    /// The desk's value for one enumeration query. Names come back as strings
    /// (the parser demands one); every other parameter takes a float, which
    /// the parser also accepts for the bool- and int-valued paths.
    fn parameter_reply(target: &str) -> Vec<OscType> {
        if target.ends_with("/name") {
            vec![OscType::String(format!("Ch{}", pad_channel_number(target)))]
        } else {
            vec![OscType::Float(SCRIPT_FLOAT)]
        }
    }

    /// The channel number out of a Pad address (`/Input_Channels/2/fader`).
    fn pad_channel_number(path: &str) -> u16 {
        path.split('/')
            .nth(2)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    fn decode_osc(data: &[u8]) -> Option<(String, Vec<OscType>)> {
        match rosc::decoder::decode_udp(data).ok()?.1 {
            OscPacket::Message(m) => Some((m.addr, m.args)),
            _ => None,
        }
    }

    async fn send_from_desk(sock: &UdpSocket, app: SocketAddr, path: &str, args: Vec<OscType>) {
        let packet = OscPacket::Message(OscMessage {
            addr: path.to_string(),
            args,
        });
        let buf = rosc::encoder::encode(&packet).unwrap();
        sock.send_to(&buf, app).await.unwrap();
    }

    /// Every set of args the desk received on `path`.
    fn desk_saw(log: &Arc<StdMutex<DeskLog>>, path: &str) -> Vec<Vec<OscType>> {
        log.lock()
            .unwrap()
            .received
            .iter()
            .filter(|(p, _)| p == path)
            .map(|(_, args)| args.clone())
            .collect()
    }

    /// Poll until `cond` holds. There is no fake clock here (real sockets),
    /// so waiting is the only way to observe the loop's timers.
    async fn settle<F: FnMut() -> bool>(what: &str, limit: Duration, mut cond: F) {
        time::timeout(limit, async {
            while !cond() {
                time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out after {limit:?} waiting for {what}"));
    }

    async fn await_desk<F: FnMut(&DeskLog) -> bool>(
        desk: &Desk,
        what: &str,
        limit: Duration,
        mut cond: F,
    ) {
        let log = desk.log.clone();
        settle(what, limit, move || cond(&log.lock().unwrap())).await;
    }

    /// Wait for the enumeration sweep to finish. `total` stays zero until the
    /// loop has begun, so this cannot mistake "not started yet" for "done".
    async fn await_enumeration(daemon: &DaemonState, limit: Duration) {
        let progress = daemon.recall_progress.clone();
        settle("the enumeration sweep to finish", limit, move || {
            let v = progress.snapshot();
            !v.active && v.total > 0
        })
        .await;
    }

    /// Wait for the mirror's health to reach `want`, returning how long it took.
    async fn await_health(
        state: &Arc<RwLock<ConsoleState>>,
        want: ConnectionHealth,
        limit: Duration,
    ) -> Duration {
        let started = Instant::now();
        time::timeout(limit, async {
            while state.read().await.health != want {
                time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("health never reached {want:?} within {limit:?}"));
        started.elapsed()
    }

    fn quantum_state() -> Arc<RwLock<ConsoleState>> {
        Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig {
            family: ConsoleFamily::Quantum,
            ..Default::default()
        })))
    }

    fn input_fader(n: u16) -> ParameterAddress {
        ParameterAddress {
            channel: ChannelId::Input(n),
            parameter: ParameterPath::Fader,
        }
    }

    fn input_name(n: u16) -> ParameterAddress {
        ParameterAddress {
            channel: ChannelId::Input(n),
            parameter: ParameterPath::Name,
        }
    }

    struct Harness {
        desk: Desk,
        state: Arc<RwLock<ConsoleState>>,
        daemon: DaemonState,
        _connection: JoinHandle<()>,
    }

    /// Connect a Pad-only daemon to a desk playing `script`. Returns once the
    /// handshake is done and the enumeration sweep is under way.
    async fn connect_to_scripted_desk(script: DeskScript) -> Harness {
        let (desk, sender, rx) = scripted_desk(script).await;
        let state = quantum_state();
        let daemon =
            crate::console::connection::test_support::daemon_with_state(state.clone()).await;
        let profile = Arc::new(ConsoleProfile::for_family(ConsoleFamily::Quantum));
        let connection = connect_pad_from_parts(
            sender,
            rx,
            daemon.clone(),
            profile,
            desk.cancel.clone(),
            None,
        )
        .await
        .expect("handshake against the scripted desk");
        Harness {
            desk,
            state,
            daemon,
            _connection: connection,
        }
    }

    #[tokio::test]
    async fn pad_sweep_fills_the_mirror_and_settles_connected() {
        let h = connect_to_scripted_desk(DeskScript::default()).await;
        await_enumeration(&h.daemon, Duration::from_secs(30)).await;

        let progress = h.daemon.recall_progress.snapshot();
        assert!(
            progress.total > 20,
            "two inputs' worth of parameters is a real queue, got {}",
            progress.total
        );
        assert_eq!(
            progress.done, progress.total,
            "every queued query must be answered or skipped before the sweep ends"
        );

        let state = h.state.read().await;
        assert_eq!(state.config.input_channel_count, SCRIPT_INPUTS);
        assert_eq!(state.config.console_name, "Quantum338");
        assert_eq!(
            state.get(&input_fader(1)),
            Some(&ParameterValue::Float(SCRIPT_FLOAT))
        );
        assert_eq!(
            state.get(&input_fader(2)),
            Some(&ParameterValue::Float(SCRIPT_FLOAT)),
            "the sweep must cover every discovered channel, not just the first"
        );
        assert_eq!(
            state.get(&input_name(2)),
            Some(&ParameterValue::String("Ch2".into())),
            "string parameters decode too"
        );
        assert_eq!(state.health, ConnectionHealth::Connected);
    }

    #[tokio::test]
    async fn enumeration_keeps_only_one_query_in_flight() {
        // Each reply is held back briefly, so a second query — had the pump
        // sent one before the first was answered — would already be sitting on
        // the desk's socket when we look. Real SD desks drop queries that
        // arrive while they are still answering.
        let h = connect_to_scripted_desk(DeskScript {
            reply_delay: Duration::from_millis(10),
            watch_pacing: true,
            ..DeskScript::default()
        })
        .await;
        await_enumeration(&h.daemon, Duration::from_secs(60)).await;

        let log = h.desk.log.lock().unwrap();
        let queries = log
            .received
            .iter()
            .filter(|(p, _)| p.starts_with("/Input_Channels/") && p.ends_with("/?"))
            .count();
        assert!(
            queries > 20,
            "the pacing check is only meaningful over a real sweep, saw {queries} queries"
        );
        assert_eq!(
            log.overlaps, 0,
            "the desk was handed a query while the previous one was unanswered"
        );
    }

    #[tokio::test]
    async fn an_unanswered_query_is_retried_then_skipped() {
        const DROPPED: &str = "/Input_Channels/1/fader/?";

        let h = connect_to_scripted_desk(DeskScript {
            silent: vec![DROPPED.to_string()],
            ..DeskScript::default()
        })
        .await;
        await_enumeration(&h.daemon, Duration::from_secs(30)).await;

        assert_eq!(
            desk_saw(&h.desk.log, DROPPED).len(),
            1 + QUERY_RETRIES as usize,
            "the desk should see the query once plus {QUERY_RETRIES} retries"
        );
        {
            let state = h.state.read().await;
            assert!(
                state.get(&input_fader(1)).is_none(),
                "nothing may be invented for the skipped parameter"
            );
            assert_eq!(
                state.get(&input_fader(2)),
                Some(&ParameterValue::Float(SCRIPT_FLOAT)),
                "the sweep must carry on past the skip"
            );
        }
        let progress = h.daemon.recall_progress.snapshot();
        assert_eq!(
            progress.done, progress.total,
            "the skipped query still has to advance the progress count"
        );

        // Steady state: the heartbeat only starts once the sweep is done.
        await_desk(
            &h.desk,
            "a heartbeat after the last enumeration query",
            Duration::from_secs(10),
            |log| {
                let last_query = log
                    .received
                    .iter()
                    .rposition(|(p, _)| p.starts_with("/Input_Channels/"));
                let last_beat = log
                    .received
                    .iter()
                    .rposition(|(p, _)| p == "/Console/Name/?");
                matches!((last_beat, last_query), (Some(b), Some(q)) if b > q)
            },
        )
        .await;
        assert_eq!(h.state.read().await.health, ConnectionHealth::Connected);
    }

    #[tokio::test]
    async fn heartbeat_goes_stale_then_lost_then_recovers() {
        // ~15 s of real time, by construction: the health machine counts three
        // missed 2 s beats to Stale and six to Lost. `tokio::time::pause` can't
        // shorten it — the loop is fed by a real UDP socket and the codebase
        // has no fake clock — so the bounds below are wide enough that a loaded
        // machine cannot flake them, with the timeouts as the upper bound.
        let answering = Arc::new(AtomicBool::new(true));
        let h = connect_to_scripted_desk(DeskScript {
            answer_heartbeat: answering.clone(),
            ..DeskScript::default()
        })
        .await;
        // The desk falls silent the moment enumeration hands over to the
        // heartbeat: nothing but `/Console/Name/?` is sent after that.
        answering.store(false, Ordering::Relaxed);
        await_enumeration(&h.daemon, Duration::from_secs(30)).await;
        assert_eq!(h.state.read().await.health, ConnectionHealth::Connected);

        let stale_after =
            await_health(&h.state, ConnectionHealth::Stale, Duration::from_secs(30)).await;
        assert!(
            stale_after >= Duration::from_secs(4),
            "Stale after only {stale_after:?} — {HEARTBEAT_MISSES_STALE} beats must be missed first"
        );

        let lost_after =
            await_health(&h.state, ConnectionHealth::Lost, Duration::from_secs(30)).await;
        assert!(
            lost_after >= Duration::from_secs(3),
            "Lost only {lost_after:?} after Stale — that is fewer than the \
             {HEARTBEAT_MISSES_LOST}-miss threshold allows"
        );

        // The desk answers again: the next beat brings the link straight back.
        answering.store(true, Ordering::Relaxed);
        await_health(
            &h.state,
            ConnectionHealth::Connected,
            Duration::from_secs(15),
        )
        .await;
    }

    /// A Pad-only daemon whose gang engine writes through the Pad sender —
    /// the wiring an SD/Quantum session gets, and the reason the sender has to
    /// exist before the `DaemonState` does.
    async fn pad_daemon_with_input_gang(
        sender: &IpadSender,
        profile: Arc<ConsoleProfile>,
        state: Arc<RwLock<ConsoleState>>,
    ) -> DaemonState {
        use crate::console::macro_manager::MacroManager;
        use crate::console::pan_link_engine::PanLinkEngine;
        use crate::model::dirty_tracker::DirtyTracker;

        let sent_log = SentLog::new();
        let mut tx = ConsoleTx::pad_only(sender.clone(), profile);
        tx.set_sent_log(sent_log.clone());
        let gang_engine = Arc::new(RwLock::new(GangEngine::from_tx(state.clone(), tx)));

        let mut gang = GangGroup::new(
            "Pad gang".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        // Absolute, so the dB the desk must receive is exactly the dB the
        // operator moved to rather than a delta off whatever the sweep left in
        // the mirror.
        gang.mode = GangMode::Absolute;
        let mut manager = GangManager::new();
        manager.add_group(gang);
        let gang_manager = Arc::new(RwLock::new(manager));

        // Pan link is inert here (no bindings) but still needs a GP sender; a
        // loopback port nobody listens on is enough.
        let gp = crate::osc::client::OscClient::new(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:1".parse().unwrap(),
            None,
        )
        .await
        .unwrap();
        let (gp_sender, _gp_rx) = gp.into_parts();
        let dirty_tracker = Arc::new(RwLock::new(DirtyTracker::new()));
        let pan_link_engine = Arc::new(RwLock::new(PanLinkEngine::new(
            state.clone(),
            gp_sender,
            Arc::new(RwLock::new(
                crate::model::pan_link::PanLinkBindings::default(),
            )),
            dirty_tracker.clone(),
            gang_manager.clone(),
        )));

        DaemonState {
            state,
            macro_manager: Arc::new(RwLock::new(MacroManager::new())),
            gang_engine,
            gang_manager,
            pan_link_engine,
            dirty_tracker,
            offline_mode: Arc::new(AtomicBool::new(false)),
            recall_progress: Arc::new(crate::model::recall_progress::RecallProgress::new()),
            last_received: Arc::new(RwLock::new(None)),
            console_snapshot_tx: None,
            console_load_suppression: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            automation_override: None,
            sent_log,
        }
    }

    /// The offline proof that ganging works on a Pad-only console: a fader
    /// move made on the desk itself has to come back out of the app as a
    /// write for the ganged channel. Covers the whole chain — inbound decode,
    /// `apply_inbound_parameter`, the gang engine, and `ConsoleTx`'s Pad
    /// routing — none of which the S-series tests exercise without GP OSC.
    #[tokio::test]
    async fn a_desk_fader_move_gangs_out_to_the_linked_channel() {
        let (desk, sender, rx) = scripted_desk(DeskScript::default()).await;
        let state = quantum_state();
        let profile = Arc::new(ConsoleProfile::for_family(ConsoleFamily::Quantum));
        let daemon = pad_daemon_with_input_gang(&sender, profile.clone(), state.clone()).await;
        let _connection = connect_pad_from_parts(
            sender,
            rx,
            daemon.clone(),
            profile,
            desk.cancel.clone(),
            None,
        )
        .await
        .expect("handshake against the scripted desk");

        await_enumeration(&daemon, Duration::from_secs(30)).await;
        let app = desk
            .log
            .lock()
            .unwrap()
            .app_addr
            .expect("the desk has seen the app");
        desk.log.lock().unwrap().received.clear();

        // The operator pulls input 1's fader on the desk; the desk pushes that
        // move at us unprompted, as it does for anything touched by hand.
        send_from_desk(
            &desk.sock,
            app,
            "/Input_Channels/1/fader",
            vec![OscType::Float(MOVED_DB)],
        )
        .await;

        await_desk(
            &desk,
            "the ganged fader write for input 2",
            Duration::from_secs(5),
            |log| {
                log.received.iter().any(|(p, args)| {
                    p == "/Input_Channels/2/fader"
                        && matches!(args.first(), Some(OscType::Float(f)) if (f - MOVED_DB).abs() < 0.01)
                })
            },
        )
        .await;
        assert_eq!(
            state.read().await.get(&input_fader(2)),
            Some(&ParameterValue::Float(MOVED_DB)),
            "the mirror must follow what was actually sent"
        );

        // The desk's echo of that write is not a fresh move: it must not gang
        // back to input 1 and start a ping-pong.
        desk.log.lock().unwrap().received.clear();
        send_from_desk(
            &desk.sock,
            app,
            "/Input_Channels/2/fader",
            vec![OscType::Float(MOVED_DB)],
        )
        .await;
        time::sleep(Duration::from_millis(250)).await;
        assert!(
            desk_saw(&desk.log, "/Input_Channels/1/fader").is_empty(),
            "the echo of our own ganged write bounced back at the desk"
        );
    }
}
