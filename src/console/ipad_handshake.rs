use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time;
use tracing::{debug, info, warn};

use crate::model::config::ConsoleConfig;
use crate::osc::client::ReceivedOscMessage;
use crate::osc::ipad_client::IpadSender;
use crate::osc::ipad_parse::{self, BankData, IpadConfigMessage, ParsedIpadMessage};

/// Default timeout for handshake phases.
const DEFAULT_PHASE_TIMEOUT: Duration = Duration::from_secs(5);

/// Result of a successful iPad handshake.
#[derive(Debug)]
pub struct HandshakeResult {
    pub config: ConsoleConfig,
    pub layout_banks: Vec<BankData>,
    pub current_snapshot: Option<i32>,
    /// How many replies the console sent during the handshake.
    ///
    /// A handshake returns `Ok` even when the desk never answers — it simply
    /// times out with defaults — so this is the only evidence the console is
    /// actually reachable. Pad-only connections use it to decide whether to
    /// report a live link or stay in `Connecting`.
    pub responses_seen: u32,
}

/// Tuning for [`perform_handshake_with`].
///
/// The defaults reproduce the S-series behaviour exactly: the nine queries
/// captured from a real iPad session, and both collection phases running
/// their full timeout regardless of what arrives. That path is
/// hardware-verified and its regression run is still outstanding, so it is
/// deliberately left alone.
#[derive(Clone, Debug)]
pub struct HandshakeOptions {
    /// Extra `/?` queries appended to the standard set. Pad-only consoles ask
    /// for the channel counts the S-series learns from GP OSC instead.
    pub extra_config_queries: &'static [&'static str],
    /// Stop collecting as soon as this many replies have arrived rather than
    /// waiting out the timeout. `None` always waits — the S-series behaviour.
    pub config_early_exit_after: Option<u32>,
    /// Whether to query the surface layout banks at all. A Pad-only console
    /// may not implement `/Layout/Layout/Banks`, and waiting a full timeout
    /// for a reply that never comes doubles connect latency for nothing.
    pub query_layout_banks: bool,
    /// Gap between consecutive queries, or `None` to send them back to back.
    ///
    /// The whole enumeration sweep is paced one-query-in-flight because SD
    /// desks are reported to drop bursts — yet this handshake opens with a
    /// dozen-odd queries fired instantly, which is the same burst by another
    /// name. Every reply lost there is a channel count silently left at its
    /// default. S-series stays back-to-back: that timing is what the
    /// hardware-verified path does today.
    pub query_pacing: Option<Duration>,
}

impl Default for HandshakeOptions {
    fn default() -> Self {
        Self {
            extra_config_queries: &[],
            config_early_exit_after: None,
            query_layout_banks: true,
            query_pacing: None,
        }
    }
}

impl HandshakeOptions {
    /// Counts the S-series gets from GP OSC `/console/channel/counts`, which a
    /// Pad-only console has no other way to learn. Without these, matrix,
    /// matrix-input, Control Group and Graphic EQ counts stay at their
    /// defaults and the scope editor shows the wrong channel set.
    ///
    /// The aux and group counts are here for a subtler reason: `BASE_QUERIES`
    /// asks those two buses for their *modes*, never their *count*, and an S21
    /// happens to volunteer the count alongside the modes reply. That is an
    /// observed courtesy of one desk, not a protocol guarantee, so ask
    /// outright rather than let a console that stays quiet leave the app
    /// mirroring the wrong number of buses.
    pub const PAD_ONLY_COUNT_QUERIES: &'static [&'static str] = &[
        "/Console/Aux_Outputs/?",
        "/Console/Group_Outputs/?",
        "/Console/Matrix_Outputs/?",
        "/Console/Matrix_Inputs/?",
        "/Console/Control_Groups/?",
        "/Console/Graphic_EQ/?",
    ];

    /// Options for a Pad-only console (SD/Quantum).
    pub fn pad_only() -> Self {
        Self {
            extra_config_queries: Self::PAD_ONLY_COUNT_QUERIES,
            // One reply per query is the ideal; settle for most of them so a
            // console that ignores an unknown query still connects promptly.
            // Late replies are not lost either way — the mirror loop feeds
            // config messages through the same `apply_config_message`.
            config_early_exit_after: Some(12),
            // The bank phase has no early exit, so it always runs its full
            // timeout — five seconds added to every single connect. The Pad
            // connection then throws `layout_banks` away, so that is five
            // seconds bought for nothing. Skip it.
            query_layout_banks: false,
            // Cheap insurance: fifteen queries spaced this far apart still
            // finish inside a third of a second.
            query_pacing: Some(Duration::from_millis(20)),
        }
    }
}

/// Errors that can occur during the iPad handshake.
#[derive(Debug)]
pub enum HandshakeError {
    SendFailed(std::io::Error),
    Timeout { phase: String },
}

impl std::fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendFailed(e) => write!(f, "Failed to send handshake query: {e}"),
            Self::Timeout { phase } => write!(f, "Handshake timed out during {phase}"),
        }
    }
}

impl std::error::Error for HandshakeError {}

impl From<std::io::Error> for HandshakeError {
    fn from(e: std::io::Error) -> Self {
        Self::SendFailed(e)
    }
}

/// Perform the iPad protocol handshake.
///
/// Mimics the query sequence observed in iPad_handshake.txt:
/// 1. Send configuration queries
/// 2. Collect config responses with timeout
/// 3. Send layout bank query
/// 4. Collect bank responses with timeout
pub async fn perform_handshake(
    sender: &IpadSender,
    rx: &mut mpsc::Receiver<ReceivedOscMessage>,
    timeout: Duration,
) -> Result<HandshakeResult, HandshakeError> {
    perform_handshake_with(sender, rx, timeout, &HandshakeOptions::default()).await
}

/// [`perform_handshake`] with explicit options — see [`HandshakeOptions`].
pub async fn perform_handshake_with(
    sender: &IpadSender,
    rx: &mut mpsc::Receiver<ReceivedOscMessage>,
    timeout: Duration,
    opts: &HandshakeOptions,
) -> Result<HandshakeResult, HandshakeError> {
    let mut config = ConsoleConfig::default();
    let mut current_snapshot: Option<i32> = None;
    let mut layout_banks = Vec::new();
    let mut responses_seen = 0u32;

    // Phase 1: Send config queries
    info!("iPad handshake: sending config queries...");
    const BASE_QUERIES: &[&str] = &[
        "/Snapshots/Current_Snapshot/?",
        "/Console/Name/?",
        "/Console/Session/Filename/?",
        "/Console/Channels/?",
        "/Console/Input_Channels/modes/?",
        "/Console/Aux_Outputs/modes/?",
        "/Console/Aux_Outputs/types/?",
        "/Console/Group_Outputs/modes/?",
        "/Console/Multis/?",
    ];

    for (i, query) in BASE_QUERIES
        .iter()
        .chain(opts.extra_config_queries)
        .enumerate()
    {
        if let Some(gap) = opts.query_pacing
            && i > 0
        {
            time::sleep(gap).await;
        }
        sender.send(query, vec![]).await?;
        debug!(query, "Sent handshake query");
    }

    // Phase 2: Collect config responses
    let deadline = time::Instant::now() + timeout;
    let mut config_responses = 0u32;

    loop {
        let remaining = deadline.saturating_duration_since(time::Instant::now());
        if remaining.is_zero() {
            info!(config_responses, "Config phase complete (timeout)");
            break;
        }

        tokio::select! {
            Some(msg) = rx.recv() => {
                let parsed = ipad_parse::parse_ipad_message(&msg.path, &msg.args);
                match parsed {
                    ParsedIpadMessage::ConfigResponse(cfg_msg) => {
                        apply_config_message(&mut config, &cfg_msg);
                        config_responses += 1;
                        responses_seen += 1;
                    }
                    ParsedIpadMessage::SnapshotInfo { current } => {
                        current_snapshot = Some(current);
                        config_responses += 1;
                        responses_seen += 1;
                    }
                    _ => {
                        debug!(path = msg.path, "Handshake: ignoring non-config message");
                    }
                }
                // Stop early once the console has answered enough of the
                // queries — only when the caller opted in, so the S-series
                // path keeps its original always-wait-the-timeout timing.
                if let Some(limit) = opts.config_early_exit_after
                    && config_responses >= limit
                {
                    info!(config_responses, "Config phase complete (all expected replies)");
                    break;
                }
            }
            _ = time::sleep(remaining) => {
                info!(config_responses, "Config phase complete (timeout)");
                break;
            }
        }
    }

    info!(
        name = %config.console_name,
        inputs = config.input_channel_count,
        auxes = config.aux_output_count,
        groups = config.group_output_count,
        "iPad handshake: config collected"
    );

    // Phase 3: Send layout banks query
    if opts.query_layout_banks {
        info!("iPad handshake: querying layout banks...");
        sender.send("/Layout/Layout/Banks/?", vec![]).await?;

        // Phase 4: Collect bank responses
        let bank_deadline = time::Instant::now() + timeout;

        loop {
            let remaining = bank_deadline.saturating_duration_since(time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            tokio::select! {
                Some(msg) = rx.recv() => {
                    let parsed = ipad_parse::parse_ipad_message(&msg.path, &msg.args);
                    match parsed {
                        ParsedIpadMessage::LayoutBank(bank) => {
                            debug!(side = %bank.side, bank = bank.bank_number, "Received layout bank");
                            layout_banks.push(bank);
                            responses_seen += 1;
                        }
                        ParsedIpadMessage::ConfigResponse(cfg_msg) => {
                            // Late config response — still apply
                            apply_config_message(&mut config, &cfg_msg);
                            responses_seen += 1;
                        }
                        _ => {
                            debug!(path = msg.path, "Handshake banks: ignoring message");
                        }
                    }
                }
                _ = time::sleep(remaining) => {
                    break;
                }
            }
        }
    }

    info!(
        bank_count = layout_banks.len(),
        "iPad handshake: layout banks collected"
    );

    // Phase 5: Send meters clear (as the real iPad does)
    if let Err(e) = sender.send("/Meters/clear", vec![]).await {
        warn!("Failed to send /Meters/clear: {e}");
    }

    Ok(HandshakeResult {
        config,
        layout_banks,
        current_snapshot,
        responses_seen,
    })
}

/// Apply a config message to update the ConsoleConfig.
///
/// Used both during the initial handshake and during the steady-state
/// mirror loop — the console may push fresh `Aux_Outputs/modes` (etc.)
/// when the user reconfigures channel stereo/mono on the desk, and the
/// daemon's mirror needs to follow so the Pan Link tab and other
/// config-driven UI track the live state.
pub(crate) fn apply_config_message(config: &mut ConsoleConfig, msg: &IpadConfigMessage) {
    match msg {
        IpadConfigMessage::ConsoleName { name, serial } => {
            config.console_name = name.clone();
            config.console_serial = serial.clone();
            info!(name, serial, "Console identified");
        }
        IpadConfigMessage::SessionFilename(filename) => {
            config.session_filename = filename.clone();
            debug!(filename = ?config.session_filename, "Session filename");
        }
        IpadConfigMessage::ChannelCount {
            channel_type,
            count,
        } => {
            let count = *count;
            match channel_type.as_str() {
                "Input_Channels" | "Channels" => config.input_channel_count = count,
                "Aux_Outputs" => {
                    config.aux_output_count = count;
                    reconcile_bus_counts(config);
                }
                "Group_Outputs" => {
                    config.group_output_count = count;
                    reconcile_bus_counts(config);
                }
                "Matrix_Outputs" => config.matrix_output_count = count,
                "Matrix_Inputs" => config.matrix_input_count = count,
                "Control_Groups" => config.control_group_count = count,
                "Graphic_EQ" => config.graphic_eq_count = count,
                "Talkback_Outputs" => config.talkback_output_count = count,
                "Multis" => { /* Not stored in config currently */ }
                other => {
                    debug!(other, count, "Unknown channel type in handshake");
                }
            }
        }
        IpadConfigMessage::OutputModes {
            channel_type,
            modes,
        } => match channel_type.as_str() {
            "Input_Channels" => config.input_modes = modes.clone(),
            "Aux_Outputs" => config.mix_output_modes = modes.clone(),
            "Group_Outputs" => config.group_modes = modes.clone(),
            other => {
                debug!(other, "Unknown mode channel type");
            }
        },
        IpadConfigMessage::OutputTypes { types } => {
            config.mix_output_types = types.clone();
            reconcile_bus_counts(config);
        }
    }
}

/// Take the aux/group split from the bus types when the counts are the pool.
///
/// An S21 reports its whole pool of mix buses as both the aux and the group
/// count: 17 and 17 for 8 auxes and 9 groups in the captured handshake
/// (`Documentation/iPad_handshake.txt`), with `/Console/Aux_Outputs/types`
/// giving the split. Stored as they came, 17/17 overwrote the GP counts and
/// group 17 collided with the master bus (audit M10). So once the types are
/// known and either count is the pool size, the counts come from the types.
/// A console that reports true per-type counts is left as it reported.
/// (Inferred from the capture; to confirm on a desk with another split.)
///
/// "Either" is for the S family only, where it is what the capture shows.
/// An SD or Quantum desk reporting real per-type counts with an aux-only
/// types list would have its aux count equal the list's length, and its group
/// count would come out 0; there both counts must be the pool.
fn reconcile_bus_counts(config: &mut ConsoleConfig) {
    let Ok(pool) = u16::try_from(config.mix_output_types.len()) else {
        return;
    };
    let (aux_is_pool, group_is_pool) = (
        config.aux_output_count == pool,
        config.group_output_count == pool,
    );
    let pool_counts = if config.family == crate::model::family::ConsoleFamily::SSeries {
        aux_is_pool || group_is_pool
    } else {
        aux_is_pool && group_is_pool
    };
    if pool == 0 || !pool_counts {
        return;
    }
    let aux = config.mix_output_types.iter().filter(|&&t| t).count() as u16;
    config.aux_output_count = aux;
    config.group_output_count = pool - aux;
}

/// Merge what a Mode 2 handshake learned into the live config. Only the iPad
/// protocol reports the bus types and modes and the console's name, and
/// Mode 2 used to log the name and drop the rest, so Pan Link never learned an
/// aux's mode (audit M11). The other channel counts stay the GP link's; the
/// aux/group split follows the types.
pub fn merge_handshake_config(live: &mut ConsoleConfig, handshake: &ConsoleConfig) {
    if !handshake.console_name.is_empty() {
        live.console_name = handshake.console_name.clone();
        live.console_serial = handshake.console_serial.clone();
    }
    if handshake.session_filename.is_some() {
        live.session_filename = handshake.session_filename.clone();
    }
    if !handshake.input_modes.is_empty() {
        live.input_modes = handshake.input_modes.clone();
    }
    if !handshake.mix_output_modes.is_empty() {
        live.mix_output_modes = handshake.mix_output_modes.clone();
    }
    if !handshake.group_modes.is_empty() {
        live.group_modes = handshake.group_modes.clone();
    }
    if !handshake.mix_output_types.is_empty() {
        live.mix_output_types = handshake.mix_output_types.clone();
        let aux = live.mix_output_types.iter().filter(|&&t| t).count();
        live.aux_output_count = aux as u16;
        live.group_output_count = (live.mix_output_types.len() - aux) as u16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::ChannelMode;
    use crate::model::family::ConsoleFamily;
    use rosc::OscType;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::net::UdpSocket;

    /// Time the console side takes to receive `expected` queries, bounded so a
    /// dropped datagram fails the test instead of hanging it.
    async fn query_arrival_spread(
        console_sock: &UdpSocket,
        expected: usize,
    ) -> std::time::Duration {
        let mut buf = [0u8; 2048];
        let mut first = None;
        let mut last = std::time::Instant::now();
        for _ in 0..expected {
            let recv =
                time::timeout(Duration::from_secs(2), console_sock.recv_from(&mut buf)).await;
            assert!(recv.is_ok(), "handshake sent fewer than {expected} queries");
            let now = std::time::Instant::now();
            first.get_or_insert(now);
            last = now;
        }
        last.duration_since(first.unwrap())
    }

    /// The enumeration sweep is paced one-query-in-flight because SD desks are
    /// reported to drop bursts, so the handshake must not undo that by opening
    /// with a dozen queries at once. Every reply lost to a burst is a channel
    /// count left silently at its default.
    #[tokio::test]
    async fn pad_only_handshake_paces_its_queries_and_the_default_does_not() {
        let opts = HandshakeOptions {
            extra_config_queries: &[],
            config_early_exit_after: Some(1),
            query_layout_banks: false,
            query_pacing: Some(Duration::from_millis(20)),
        };
        let base_query_count = 9;

        let (sender, mut rx, console_sock, _) = mock_ipad_pair().await;
        let handshake = tokio::spawn(async move {
            perform_handshake_with(&sender, &mut rx, Duration::from_millis(50), &opts).await
        });
        let paced = query_arrival_spread(&console_sock, base_query_count).await;
        let _ = handshake.await.unwrap();

        // Eight gaps of 20 ms; allow generous slack for a loaded CI machine
        // while still being far above what an unpaced burst could produce.
        assert!(
            paced >= Duration::from_millis(100),
            "paced handshake sent {base_query_count} queries within {paced:?} — pacing lost"
        );

        // The S-series default must keep its original back-to-back timing:
        // that path is hardware-verified and its regression run is outstanding.
        let (sender, mut rx, console_sock, _) = mock_ipad_pair().await;
        let handshake = tokio::spawn(async move {
            perform_handshake_with(
                &sender,
                &mut rx,
                Duration::from_millis(50),
                &HandshakeOptions {
                    query_layout_banks: false,
                    ..HandshakeOptions::default()
                },
            )
            .await
        });
        let unpaced = query_arrival_spread(&console_sock, base_query_count).await;
        let _ = handshake.await.unwrap();

        assert!(
            unpaced < Duration::from_millis(50),
            "default handshake took {unpaced:?} to send {base_query_count} queries — \
             it should still be back to back"
        );
    }

    /// Helper: create a mock sender/receiver pair for testing handshake.
    /// Returns (IpadSender, a handle to inject mock responses, receiver for handshake).
    async fn mock_ipad_pair() -> (
        IpadSender,
        mpsc::Receiver<ReceivedOscMessage>,
        Arc<UdpSocket>,
        SocketAddr,
    ) {
        // Console-side socket (receives queries, sends responses)
        let console_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let console_addr = console_sock.local_addr().unwrap();

        // Create iPad client pointed at console
        let client = crate::osc::ipad_client::IpadClient::new(
            "127.0.0.1:0".parse().unwrap(),
            console_addr,
            None,
        )
        .await
        .unwrap();
        let (sender, rx) = client.into_parts();

        (sender, rx, Arc::new(console_sock), console_addr)
    }

    /// Encode and send a mock OSC response from the "console" socket.
    async fn send_mock_response(
        console_sock: &UdpSocket,
        dest: SocketAddr,
        path: &str,
        args: Vec<OscType>,
    ) {
        use rosc::{OscMessage, OscPacket};
        let msg = OscMessage {
            addr: path.to_string(),
            args,
        };
        let packet = OscPacket::Message(msg);
        let buf = rosc::encoder::encode(&packet).unwrap();
        console_sock.send_to(&buf, dest).await.unwrap();
    }

    #[tokio::test]
    async fn handshake_collects_config() {
        let (sender, mut rx, console_sock, _console_addr) = mock_ipad_pair().await;

        // We need the daemon's local address to send responses back
        // The sender's socket address is what we need to send to
        // Since the sender sends to console_sock, console_sock will see the source addr

        // Spawn the handshake with a short timeout
        let sender_clone = sender.clone();
        let handshake = tokio::spawn(async move {
            perform_handshake(&sender_clone, &mut rx, Duration::from_millis(500)).await
        });

        // Wait briefly for queries to arrive, then send mock responses
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Read a query to learn the daemon's address
        let mut buf = vec![0u8; 65536];
        let (_, daemon_addr) = console_sock.recv_from(&mut buf).await.unwrap();

        // Drain remaining queries
        for _ in 0..20 {
            match tokio::time::timeout(Duration::from_millis(10), console_sock.recv_from(&mut buf))
                .await
            {
                Ok(_) => {}
                Err(_) => break,
            }
        }

        // Send config responses
        send_mock_response(
            &console_sock,
            daemon_addr,
            "/Snapshots/Current_Snapshot",
            vec![OscType::Int(3)],
        )
        .await;

        send_mock_response(
            &console_sock,
            daemon_addr,
            "/Console/Name",
            vec![OscType::String("S21 S21-210385".into())],
        )
        .await;

        send_mock_response(
            &console_sock,
            daemon_addr,
            "/Console/Input_Channels",
            vec![OscType::Int(48)],
        )
        .await;

        send_mock_response(
            &console_sock,
            daemon_addr,
            "/Console/Aux_Outputs",
            vec![OscType::Int(17)],
        )
        .await;

        send_mock_response(
            &console_sock,
            daemon_addr,
            "/Console/Group_Outputs",
            vec![OscType::Int(17)],
        )
        .await;

        send_mock_response(
            &console_sock,
            daemon_addr,
            "/Console/Aux_Outputs/modes",
            vec![OscType::Int(1), OscType::Int(2), OscType::Int(1)],
        )
        .await;

        // Wait for handshake to complete
        let result = handshake.await.unwrap().unwrap();

        assert_eq!(result.config.console_name, "S21");
        assert_eq!(result.config.console_serial, "S21-210385");
        assert_eq!(result.config.input_channel_count, 48);
        assert_eq!(result.config.aux_output_count, 17);
        assert_eq!(result.config.group_output_count, 17);
        assert_eq!(result.current_snapshot, Some(3));
        assert_eq!(result.config.mix_output_modes.len(), 3);
    }

    #[tokio::test]
    async fn handshake_timeout_returns_partial_config() {
        let (sender, mut rx, _console_sock, _) = mock_ipad_pair().await;

        // No responses — should timeout but not error
        let result = perform_handshake(&sender, &mut rx, Duration::from_millis(100)).await;

        // Should succeed with default config (no responses received)
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.config.console_name, ""); // No name received
        assert!(result.layout_banks.is_empty());
        assert_eq!(result.current_snapshot, None);
    }

    fn count(channel_type: &str, count: u16) -> IpadConfigMessage {
        IpadConfigMessage::ChannelCount {
            channel_type: channel_type.into(),
            count,
        }
    }

    /// The captured S21 split: 8 auxes, then 9 groups.
    fn captured_types() -> IpadConfigMessage {
        IpadConfigMessage::OutputTypes {
            types: (0..17).map(|i| i < 8).collect(),
        }
    }

    /// Audit M10: in the captured handshake the desk reports its 17-bus pool
    /// as both the aux and the group count, and the types give the split.
    /// Replies in the capture's order: group count, types, then aux count.
    #[test]
    fn pool_counts_take_the_split_from_the_types() {
        let mut config = ConsoleConfig::default();
        for msg in [
            count("Group_Outputs", 17),
            captured_types(),
            count("Aux_Outputs", 17),
        ] {
            apply_config_message(&mut config, &msg);
        }
        assert_eq!((config.aux_output_count, config.group_output_count), (8, 9));

        // Types first, counts after: the same result.
        let mut config = ConsoleConfig::default();
        for msg in [
            captured_types(),
            count("Aux_Outputs", 17),
            count("Group_Outputs", 17),
        ] {
            apply_config_message(&mut config, &msg);
        }
        assert_eq!((config.aux_output_count, config.group_output_count), (8, 9));
    }

    /// An SD or Quantum desk with per-type counts and an aux-only types list
    /// keeps its group count (audit follow-up to M10: it came out 0).
    #[test]
    fn pad_family_per_type_counts_with_aux_only_types_are_kept() {
        for family in [ConsoleFamily::SdRange, ConsoleFamily::Quantum] {
            let mut config = ConsoleConfig {
                family,
                ..ConsoleConfig::default()
            };
            for msg in [
                IpadConfigMessage::OutputTypes {
                    types: vec![true; 16],
                },
                count("Aux_Outputs", 16),
                count("Group_Outputs", 8),
            ] {
                apply_config_message(&mut config, &msg);
            }
            assert_eq!(
                (config.aux_output_count, config.group_output_count),
                (16, 8),
                "{family:?}"
            );

            // A pool reported as both counts still takes the split.
            let mut config = ConsoleConfig {
                family,
                ..ConsoleConfig::default()
            };
            for msg in [
                count("Group_Outputs", 17),
                captured_types(),
                count("Aux_Outputs", 17),
            ] {
                apply_config_message(&mut config, &msg);
            }
            assert_eq!(
                (config.aux_output_count, config.group_output_count),
                (8, 9),
                "{family:?}"
            );
        }
    }

    /// A console that reports true per-type counts keeps them.
    #[test]
    fn per_type_counts_are_kept() {
        let mut config = ConsoleConfig::default();
        for msg in [
            captured_types(),
            count("Aux_Outputs", 8),
            count("Group_Outputs", 9),
        ] {
            apply_config_message(&mut config, &msg);
        }
        assert_eq!((config.aux_output_count, config.group_output_count), (8, 9));
    }

    /// Audit M11: Mode 2 keeps what only its handshake reports.
    #[test]
    fn mode2_handshake_config_is_merged() {
        let mut live = ConsoleConfig::default();
        live.input_channel_count = 60;
        let mut handshake = ConsoleConfig::default();
        handshake.console_name = "S21".into();
        handshake.input_channel_count = 48; // the GP link's count wins
        handshake.mix_output_modes = vec![ChannelMode::Mono, ChannelMode::Stereo];
        handshake.mix_output_types = vec![true, false, true, false];

        merge_handshake_config(&mut live, &handshake);

        assert_eq!(live.console_name, "S21");
        assert_eq!(live.input_channel_count, 60);
        assert_eq!(live.mix_output_modes, handshake.mix_output_modes);
        assert_eq!(live.mix_output_types, handshake.mix_output_types);
        assert_eq!((live.aux_output_count, live.group_output_count), (2, 2));

        // A handshake that learned nothing changes nothing.
        let before = live.clone();
        merge_handshake_config(&mut live, &ConsoleConfig::default());
        assert_eq!(live.mix_output_types, before.mix_output_types);
        assert_eq!(live.console_name, before.console_name);
    }

    #[test]
    fn apply_config_channel_counts() {
        let mut config = ConsoleConfig::default();

        apply_config_message(
            &mut config,
            &IpadConfigMessage::ChannelCount {
                channel_type: "Input_Channels".into(),
                count: 60,
            },
        );
        assert_eq!(config.input_channel_count, 60);

        apply_config_message(
            &mut config,
            &IpadConfigMessage::ChannelCount {
                channel_type: "Matrix_Outputs".into(),
                count: 8,
            },
        );
        assert_eq!(config.matrix_output_count, 8);

        apply_config_message(
            &mut config,
            &IpadConfigMessage::ChannelCount {
                channel_type: "Control_Groups".into(),
                count: 10,
            },
        );
        assert_eq!(config.control_group_count, 10);
    }

    #[test]
    fn apply_config_console_name() {
        let mut config = ConsoleConfig::default();
        apply_config_message(
            &mut config,
            &IpadConfigMessage::ConsoleName {
                name: "S21".into(),
                serial: "ABC-123".into(),
            },
        );
        assert_eq!(config.console_name, "S21");
        assert_eq!(config.console_serial, "ABC-123");
    }

    #[test]
    fn apply_config_output_modes() {
        let mut config = ConsoleConfig::default();
        apply_config_message(
            &mut config,
            &IpadConfigMessage::OutputModes {
                channel_type: "Aux_Outputs".into(),
                modes: vec![ChannelMode::Mono, ChannelMode::Stereo],
            },
        );
        assert_eq!(config.mix_output_modes.len(), 2);
        assert_eq!(config.mix_output_modes[0], ChannelMode::Mono);
        assert_eq!(config.mix_output_modes[1], ChannelMode::Stereo);
    }

    #[test]
    fn apply_config_output_types() {
        let mut config = ConsoleConfig::default();
        apply_config_message(
            &mut config,
            &IpadConfigMessage::OutputTypes {
                types: vec![true, true, false, false],
            },
        );
        assert_eq!(config.mix_output_types, vec![true, true, false, false]);
    }
}
