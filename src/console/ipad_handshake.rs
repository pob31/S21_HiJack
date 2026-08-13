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
}

impl Default for HandshakeOptions {
    fn default() -> Self {
        Self {
            extra_config_queries: &[],
            config_early_exit_after: None,
            query_layout_banks: true,
        }
    }
}

impl HandshakeOptions {
    /// Counts the S-series gets from GP OSC `/console/channel/counts`, which a
    /// Pad-only console has no other way to learn. Without these, matrix,
    /// matrix-input, Control Group and Graphic EQ counts stay at their
    /// defaults and the scope editor shows the wrong channel set.
    pub const PAD_ONLY_COUNT_QUERIES: &'static [&'static str] = &[
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
            config_early_exit_after: Some(10),
            query_layout_banks: true,
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

    for query in BASE_QUERIES.iter().chain(opts.extra_config_queries) {
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
                "Aux_Outputs" => config.aux_output_count = count,
                "Group_Outputs" => config.group_output_count = count,
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::ChannelMode;
    use rosc::OscType;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::net::UdpSocket;

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
