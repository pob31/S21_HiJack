use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{info, warn, debug};

use crate::model::state::ConsoleState;
use crate::osc::client::ReceivedOscMessage;
use crate::osc::ipad_client::{IpadClient, IpadForwarder, IpadListener, IpadSender};
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
) -> Result<(IpadSender, HandshakeResult, JoinHandle<()>), IpadConnectionError> {
    info!(%console_ipad_addr, "Mode 2: connecting to console iPad port...");

    let client = IpadClient::new(local_addr, console_ipad_addr).await?;
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
    let handle = tokio::spawn(async move {
        ipad_state_mirror_loop(rx, state_clone).await;
    });

    Ok((sender, handshake_result, handle))
}

/// Mode 3: iPad proxy connection.
///
/// 1. Connects to the console's iPad remote port (daemon→console).
/// 2. Listens for the real iPad to connect (iPad→daemon).
/// 3. Forwards traffic bidirectionally while capturing state.
pub async fn connect_mode3(
    console_ipad_addr: SocketAddr,
    local_listen_addr: SocketAddr,
    local_outbound_addr: SocketAddr,
    state: Arc<RwLock<ConsoleState>>,
) -> Result<(IpadSender, IpadForwarder, HandshakeResult, JoinHandle<()>), IpadConnectionError> {
    info!(
        %console_ipad_addr,
        %local_listen_addr,
        "Mode 3: setting up iPad proxy..."
    );

    // 1. Connect to console's iPad port
    let client = IpadClient::new(local_outbound_addr, console_ipad_addr).await?;
    let (console_sender, mut console_rx) = client.into_parts();

    // 2. Perform handshake with console
    let handshake_result = ipad_handshake::perform_handshake(
        &console_sender,
        &mut console_rx,
        HANDSHAKE_TIMEOUT,
    ).await?;

    info!(
        name = %handshake_result.config.console_name,
        "Mode 3: console handshake complete"
    );

    // 3. Listen for real iPad connections
    let listener = IpadListener::new(local_listen_addr).await?;
    let (ipad_forwarder, ipad_rx) = listener.into_parts();

    info!(%local_listen_addr, "Mode 3: listening for iPad connections");

    // 4. Start bidirectional forwarding loop
    let state_clone = state.clone();
    let console_sender_clone = console_sender.clone();
    let ipad_forwarder_clone = ipad_forwarder.clone();

    let handle = tokio::spawn(async move {
        proxy_loop(
            console_rx,
            ipad_rx,
            console_sender_clone,
            ipad_forwarder_clone,
            state_clone,
        ).await;
    });

    Ok((console_sender, ipad_forwarder, handshake_result, handle))
}

/// Background loop for Mode 2: mirrors iPad protocol messages into ConsoleState.
async fn ipad_state_mirror_loop(
    mut rx: tokio::sync::mpsc::Receiver<ReceivedOscMessage>,
    state: Arc<RwLock<ConsoleState>>,
) {
    info!("iPad state mirror loop started");
    while let Some(msg) = rx.recv().await {
        let parsed = ipad_parse::parse_ipad_message(&msg.path, &msg.args);
        match parsed {
            ParsedIpadMessage::ParameterUpdate(addr, value) => {
                debug!(%addr, %value, "iPad mirror: parameter update");
                state.write().await.update(addr, value);
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

/// Bidirectional proxy loop for Mode 3.
///
/// - Console→daemon: parse, capture state, forward to iPad
/// - iPad→daemon: parse, capture state, forward to console
async fn proxy_loop(
    mut console_rx: tokio::sync::mpsc::Receiver<ReceivedOscMessage>,
    mut ipad_rx: tokio::sync::mpsc::Receiver<(ReceivedOscMessage, SocketAddr)>,
    console_sender: IpadSender,
    ipad_forwarder: IpadForwarder,
    state: Arc<RwLock<ConsoleState>>,
) {
    info!("Mode 3 proxy loop started");
    loop {
        tokio::select! {
            // Console → daemon → iPad
            Some(msg) = console_rx.recv() => {
                let parsed = ipad_parse::parse_ipad_message(&msg.path, &msg.args);
                if let ParsedIpadMessage::ParameterUpdate(addr, value) = &parsed {
                    debug!(%addr, "Proxy: console→iPad parameter");
                    state.write().await.update(addr.clone(), value.clone());
                }

                // Forward raw to iPad
                // Re-encode the message for forwarding
                let osc_msg = rosc::OscMessage {
                    addr: msg.path.clone(),
                    args: msg.args.clone(),
                };
                let packet = rosc::OscPacket::Message(osc_msg);
                if let Ok(buf) = rosc::encoder::encode(&packet) {
                    if let Err(e) = ipad_forwarder.forward_raw(&buf).await {
                        warn!("Proxy: failed to forward to iPad: {e}");
                    }
                }
            }

            // iPad → daemon → console
            Some((msg, _src)) = ipad_rx.recv() => {
                let parsed = ipad_parse::parse_ipad_message(&msg.path, &msg.args);
                if let ParsedIpadMessage::ParameterUpdate(addr, value) = &parsed {
                    debug!(%addr, "Proxy: iPad→console parameter");
                    state.write().await.update(addr.clone(), value.clone());
                }

                // Forward to console
                if let Err(e) = console_sender.send(&msg.path, msg.args.clone()).await {
                    warn!("Proxy: failed to forward to console: {e}");
                }
            }

            else => {
                info!("Proxy loop: both channels closed");
                break;
            }
        }
    }
    info!("Mode 3 proxy loop ended");
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
