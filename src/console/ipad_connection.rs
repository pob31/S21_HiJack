use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{info, warn, debug};

use crate::model::dirty_tracker::DirtyTracker;
use crate::model::state::ConsoleState;
use crate::osc::client::ReceivedOscMessage;
use crate::osc::ipad_client::{IpadClient, IpadSender};
use crate::osc::ipad_parse::{self, ParsedIpadMessage};

use super::ipad_handshake::{self, HandshakeResult};

/// Default handshake timeout.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Errors from iPad connection setup.
#[derive(Debug)]
pub enum IpadConnectionError {
    Io(std::io::Error),
    Handshake(ipad_handshake::HandshakeError),
}

impl std::fmt::Display for IpadConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "iPad connection I/O error: {e}"),
            Self::Handshake(e) => write!(f, "iPad handshake failed: {e}"),
        }
    }
}

impl std::error::Error for IpadConnectionError {}

impl From<std::io::Error> for IpadConnectionError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<ipad_handshake::HandshakeError> for IpadConnectionError {
    fn from(e: ipad_handshake::HandshakeError) -> Self {
        Self::Handshake(e)
    }
}

/// Mode 2: Direct iPad protocol connection.
///
/// Connects to the console's iPad remote port, performs the handshake,
/// and returns a sender for sending iPad-only commands.
/// Also starts a background loop to mirror iPad protocol state.
pub async fn connect_mode2(
    console_ipad_addr: SocketAddr,
    local_addr: SocketAddr,
    state: Arc<RwLock<ConsoleState>>,
    dirty_tracker: Arc<RwLock<DirtyTracker>>,
    interface_name: Option<&str>,
) -> Result<(IpadSender, HandshakeResult, JoinHandle<()>), IpadConnectionError> {
    info!(%console_ipad_addr, "Mode 2: connecting to console iPad port...");

    let client = IpadClient::new(local_addr, console_ipad_addr, interface_name).await?;
    let (sender, mut rx) = client.into_parts();

    // Perform handshake
    let handshake_result = ipad_handshake::perform_handshake(
        &sender,
        &mut rx,
        HANDSHAKE_TIMEOUT,
    ).await?;

    info!(
        name = %handshake_result.config.console_name,
        banks = handshake_result.layout_banks.len(),
        "Mode 2: handshake complete"
    );

    // Start background state mirror loop
    let state_clone = state.clone();
    let dirty_clone = dirty_tracker.clone();
    let handle = tokio::spawn(async move {
        ipad_state_mirror_loop(rx, state_clone, dirty_clone).await;
    });

    Ok((sender, handshake_result, handle))
}

/// Mode 3: Two-socket iPad proxy.
///
/// Console-side socket: bound to `local_console_addr`, sends to `console_ipad_addr`
/// iPad-side socket: bound to `ipad_listen_addr`, sends to `ipad_target` (or auto-detected)
///
/// Starts forwarding immediately without blocking on a handshake.
/// All traffic is logged and captured into the state mirror.
#[allow(clippy::too_many_arguments)]
pub async fn connect_mode3_proxy(
    console_ipad_addr: SocketAddr,
    local_console_addr: SocketAddr,
    ipad_listen_addr: SocketAddr,
    ipad_target: Option<SocketAddr>,
    ipad_reply_port: u16,
    state: Arc<RwLock<ConsoleState>>,
    dirty_tracker: Arc<RwLock<DirtyTracker>>,
    cancel: tokio_util::sync::CancellationToken,
    interface_name: Option<String>,
) -> Result<IpadSender, IpadConnectionError> {
    info!(
        %console_ipad_addr,
        %local_console_addr,
        %ipad_listen_addr,
        ?ipad_target,
        "Mode 3: setting up two-socket iPad proxy..."
    );

    // Socket 1: Console-side (daemon ↔ console) — raw UDP, interface-bound
    let console_socket = crate::ui::net_interfaces::create_bound_udp_socket(
        local_console_addr, interface_name.as_deref(),
    ).await?;
    let actual_console = console_socket.local_addr()?;
    info!(%actual_console, %console_ipad_addr, "Mode 3: console-side socket bound");
    let console_socket = std::sync::Arc::new(console_socket);

    // Also create an IpadSender for snapshot engine use (sends via same socket)
    let ipad_sender = IpadSender::from_socket(console_socket.clone(), console_ipad_addr);

    // Socket 2: iPad-side (daemon ↔ iPad) — raw UDP, interface-bound
    let ipad_socket = crate::ui::net_interfaces::create_bound_udp_socket(
        ipad_listen_addr, interface_name.as_deref(),
    ).await?;
    let actual_listen = ipad_socket.local_addr()?;
    info!(%actual_listen, "Mode 3: iPad-side socket listening");
    let ipad_socket = std::sync::Arc::new(ipad_socket);

    // Start the bidirectional proxy loop immediately (no handshake)
    let state_clone = state.clone();
    let dirty_clone = dirty_tracker.clone();

    tokio::spawn(async move {
        raw_proxy_loop(
            console_socket,
            console_ipad_addr,
            ipad_socket,
            ipad_target,
            ipad_reply_port,
            state_clone,
            dirty_clone,
            cancel,
        ).await;
    });

    Ok(ipad_sender)
}

/// Background loop for Mode 2: mirrors iPad protocol messages into ConsoleState.
async fn ipad_state_mirror_loop(
    mut rx: tokio::sync::mpsc::Receiver<ReceivedOscMessage>,
    state: Arc<RwLock<ConsoleState>>,
    dirty_tracker: Arc<RwLock<DirtyTracker>>,
) {
    info!("iPad state mirror loop started");
    while let Some(msg) = rx.recv().await {
        let parsed = ipad_parse::parse_ipad_message(&msg.path, &msg.args);
        match parsed {
            ParsedIpadMessage::ParameterUpdate(addr, value) => {
                debug!(%addr, %value, "iPad mirror: parameter update");
                let old = state.write().await.update(addr.clone(), value.clone());
                if let Some(prev) = &old {
                    if prev != &value {
                        dirty_tracker.write().await.mark(&addr);
                    }
                }
            }
            ParsedIpadMessage::MeterValues(_) => {
                // Meters are high-frequency — skip state updates
            }
            _ => {
                // Config/layout/snapshot messages during mirror phase
                debug!(path = msg.path, "iPad mirror: non-parameter message");
            }
        }
    }
    info!("iPad state mirror loop ended");
}

/// Pure raw bidirectional proxy loop for Mode 3.
///
/// Two raw UDP sockets, no parsing, no wrappers. Just forwards bytes.
/// Parsing is best-effort for logging only.
#[allow(clippy::too_many_arguments)]
async fn raw_proxy_loop(
    console_socket: std::sync::Arc<tokio::net::UdpSocket>,
    console_addr: SocketAddr,
    ipad_socket: std::sync::Arc<tokio::net::UdpSocket>,
    ipad_target: Option<SocketAddr>,
    ipad_reply_port: u16,
    state: Arc<RwLock<ConsoleState>>,
    dirty_tracker: Arc<RwLock<DirtyTracker>>,
    cancel: tokio_util::sync::CancellationToken,
) {
    info!("Mode 3 raw proxy started");

    let mut ipad_addr: Option<SocketAddr> = ipad_target;
    let mut console_buf = vec![0u8; 65536];
    let mut ipad_buf = vec![0u8; 65536];
    let mut c2i: u64 = 0;
    let mut i2c: u64 = 0;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!(c2i, i2c, "Mode 3 proxy cancelled");
                break;
            }

            // Console → daemon → iPad
            result = console_socket.recv_from(&mut console_buf) => {
                match result {
                    Ok((size, _src)) => {
                        log_and_capture_packet(&console_buf[..size], "C→I", &state, &dirty_tracker).await;

                        if let Some(dest) = ipad_addr {
                            match ipad_socket.send_to(&console_buf[..size], dest).await {
                                Ok(sent) => {
                                    c2i += 1;
                                    if c2i <= 5 {
                                        debug!(c2i, sent, %dest, "Proxy C→I: forwarded {sent} bytes to {dest}");
                                    }
                                }
                                Err(e) => {
                                    warn!(%dest, "Proxy C→I: send failed: {e}");
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Proxy console recv error: {e}");
                        break;
                    }
                }
            }

            // iPad → daemon → console
            result = ipad_socket.recv_from(&mut ipad_buf) => {
                match result {
                    Ok((size, src)) => {
                        // Use the iPad's IP from the first packet, but reply to the
                        // configured receive port (not the ephemeral send port).
                        // The iPad listens on its configured "Receive Port" for responses.
                        if ipad_addr.is_none() {
                            let detected = SocketAddr::new(src.ip(), ipad_reply_port);
                            info!(%src, %detected, "Proxy: iPad detected (src={src}), replies → {detected}");
                            ipad_addr = Some(detected);
                        }

                        log_and_capture_packet(&ipad_buf[..size], "I→C", &state, &dirty_tracker).await;

                        match console_socket.send_to(&ipad_buf[..size], console_addr).await {
                            Ok(sent) => {
                                i2c += 1;
                                if i2c <= 5 {
                                    debug!(i2c, sent, %console_addr, "Proxy I→C: forwarded {sent} bytes to {console_addr}");
                                }
                            }
                            Err(e) => {
                                warn!(%console_addr, "Proxy I→C: send failed: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Proxy iPad recv error: {e}");
                        break;
                    }
                }
            }
        }
    }
    drop(console_socket);
    drop(ipad_socket);
    info!("Mode 3 proxy ended");
}

/// Best-effort parse and log a raw packet. Captures parameter updates into state.
async fn log_and_capture_packet(
    data: &[u8],
    direction: &str,
    state: &Arc<RwLock<ConsoleState>>,
    dirty_tracker: &Arc<RwLock<DirtyTracker>>,
) {
    // Try standard OSC first
    if let Ok((_, packet)) = rosc::decoder::decode_udp(data) {
        let messages = flatten_packet(packet);
        for msg in messages {
            let parsed = ipad_parse::parse_ipad_message(&msg.path, &msg.args);
            match &parsed {
                ParsedIpadMessage::ParameterUpdate(addr, value) => {
                    debug!(%addr, %value, "Proxy {direction}: param");
                    let old = state.write().await.update(addr.clone(), value.clone());
                    if let Some(prev) = &old {
                        if prev != value {
                            dirty_tracker.write().await.mark(addr);
                        }
                    }
                }
                ParsedIpadMessage::ConfigResponse(cfg) => {
                    info!(?cfg, "Proxy {direction}: config");
                }
                _ => {
                    debug!(path = msg.path, "Proxy {direction}: {}", msg.path);
                }
            }
        }
    } else if let Some(msg) = parse_bare_path(data) {
        // DiGiCo bare-path query
        debug!(path = msg.path, "Proxy {direction}: bare query");
    } else {
        let hex: String = data[..data.len().min(32)].iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        debug!(size = data.len(), %hex, "Proxy {direction}: raw ({} bytes)", data.len());
    }
}

/// Try to parse a bare path packet (DiGiCo non-standard: just path + null, no type tag).
fn parse_bare_path(data: &[u8]) -> Option<ReceivedOscMessage> {
    // Must start with '/'
    if data.first() != Some(&b'/') {
        return None;
    }
    // Find null terminator
    let null_pos = data.iter().position(|&b| b == 0)?;
    let path = std::str::from_utf8(&data[..null_pos]).ok()?;
    Some(ReceivedOscMessage {
        path: path.to_string(),
        args: vec![],
    })
}

/// Flatten an OSC packet (message or bundle) into a Vec of messages.
fn flatten_packet(packet: rosc::OscPacket) -> Vec<ReceivedOscMessage> {
    let mut out = Vec::new();
    match packet {
        rosc::OscPacket::Message(msg) => {
            out.push(ReceivedOscMessage {
                path: msg.addr,
                args: msg.args,
            });
        }
        rosc::OscPacket::Bundle(bundle) => {
            for p in bundle.content {
                out.extend(flatten_packet(p));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display() {
        let io_err = IpadConnectionError::Io(
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused"),
        );
        assert!(io_err.to_string().contains("refused"));

        let hs_err = IpadConnectionError::Handshake(
            ipad_handshake::HandshakeError::Timeout { phase: "config".into() },
        );
        assert!(hs_err.to_string().contains("config"));
    }
}
