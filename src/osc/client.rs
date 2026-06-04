use rosc::{OscMessage, OscPacket, OscType};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::model::osc_log::OscLog;

/// A received OSC message with its path and arguments.
#[derive(Debug, Clone)]
pub struct ReceivedOscMessage {
    pub path: String,
    pub args: Vec<OscType>,
}

/// Async GP OSC UDP client.
pub struct OscClient {
    socket: UdpSocket,
    console_addr: SocketAddr,
}

impl OscClient {
    /// Create a new OSC client bound to `local_addr`, sending to `console_addr`.
    /// If `interface_name` is provided, the socket is pinned to that network interface.
    pub async fn new(
        local_addr: SocketAddr,
        console_addr: SocketAddr,
        interface_name: Option<&str>,
    ) -> std::io::Result<Self> {
        let socket =
            crate::ui::net_interfaces::create_bound_udp_socket(local_addr, interface_name).await?;
        Ok(Self {
            socket,
            console_addr,
        })
    }

    /// Send an OSC message to the console.
    pub async fn send(&self, path: &str, args: Vec<OscType>) -> std::io::Result<()> {
        let msg = OscMessage {
            addr: path.to_string(),
            args,
        };
        let packet = OscPacket::Message(msg);
        let buf = rosc::encoder::encode(&packet).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("OSC encode error: {e}"),
            )
        })?;
        self.socket.send_to(&buf, self.console_addr).await?;
        debug!(path, "Sent OSC message");
        Ok(())
    }

    /// Split this client into a sender handle and a receive loop (no logging, no cancellation).
    pub fn into_parts(self) -> (OscSender, mpsc::Receiver<ReceivedOscMessage>) {
        self.into_parts_with_log(None, CancellationToken::new())
    }

    /// Split with an optional OscLog and a CancellationToken for clean shutdown.
    pub fn into_parts_with_log(
        self,
        log: Option<OscLog>,
        cancel: CancellationToken,
    ) -> (OscSender, mpsc::Receiver<ReceivedOscMessage>) {
        let (tx, rx) = mpsc::channel(1024);
        let socket = std::sync::Arc::new(self.socket);

        let sender = OscSender {
            socket: socket.clone(),
            console_addr: self.console_addr,
            log: log.clone(),
            offline_mode: None,
        };

        // Spawn the receive loop with cancellation support
        tokio::spawn(receive_loop(socket, tx, log, cancel));

        (sender, rx)
    }
}

/// Handle for sending OSC messages (can be cloned and shared).
#[derive(Clone)]
pub struct OscSender {
    socket: std::sync::Arc<UdpSocket>,
    console_addr: SocketAddr,
    log: Option<OscLog>,
    /// When set and `true`, all sends become no-ops (offline mode).
    /// Shared with the inbound dispatcher and the iPad sender so the
    /// app can freeze all OSC traffic in both directions.
    offline_mode: Option<Arc<AtomicBool>>,
}

impl OscSender {
    /// Create a sender from an existing socket and target address.
    pub fn new(socket: std::sync::Arc<UdpSocket>, console_addr: SocketAddr) -> Self {
        Self {
            socket,
            console_addr,
            log: None,
            offline_mode: None,
        }
    }

    /// Create a sender with logging.
    pub fn new_with_log(
        socket: std::sync::Arc<UdpSocket>,
        console_addr: SocketAddr,
        log: OscLog,
    ) -> Self {
        Self {
            socket,
            console_addr,
            log: Some(log),
            offline_mode: None,
        }
    }

    /// Attach the shared offline-mode flag. When the flag is `true`,
    /// `send` becomes a no-op without touching the socket or logging.
    pub fn set_offline_flag(&mut self, flag: Arc<AtomicBool>) {
        self.offline_mode = Some(flag);
    }

    /// Send an OSC message to the console.
    pub async fn send(&self, path: &str, args: Vec<OscType>) -> std::io::Result<()> {
        // Offline gate: drop without touching the socket or the OSC log.
        if let Some(ref flag) = self.offline_mode {
            if flag.load(Ordering::Relaxed) {
                trace!(path, "OSC send dropped (offline mode)");
                return Ok(());
            }
        }

        // Log outgoing message
        if let Some(ref log) = self.log {
            log.log_out(path, &format_osc_args(&args));
        }

        let msg = OscMessage {
            addr: path.to_string(),
            args,
        };
        let packet = OscPacket::Message(msg);
        let buf = rosc::encoder::encode(&packet).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("OSC encode error: {e}"),
            )
        })?;
        self.socket.send_to(&buf, self.console_addr).await?;
        debug!(path, "Sent OSC message");
        Ok(())
    }
}

/// Format OSC arguments as a compact string for logging.
pub(crate) fn format_osc_args(args: &[OscType]) -> String {
    if args.is_empty() {
        return String::new();
    }
    args.iter()
        .map(|a| match a {
            OscType::Int(v) => format!("{v}"),
            OscType::Float(v) => format!("{v:.3}"),
            OscType::String(v) => format!("\"{v}\""),
            OscType::Bool(v) => format!("{v}"),
            OscType::Long(v) => format!("{v}L"),
            OscType::Double(v) => format!("{v:.3}d"),
            OscType::Blob(v) => format!("blob[{}]", v.len()),
            OscType::Nil => "nil".into(),
            OscType::Inf => "inf".into(),
            _ => "?".into(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Background receive loop: reads UDP packets, decodes OSC, and forwards to channel.
/// Exits cleanly when the CancellationToken is cancelled, dropping the socket.
async fn receive_loop(
    socket: std::sync::Arc<UdpSocket>,
    tx: mpsc::Sender<ReceivedOscMessage>,
    log: Option<OscLog>,
    cancel: CancellationToken,
) {
    let mut buf = vec![0u8; 65536];
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("OSC receive loop cancelled — releasing port");
                break;
            }
            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((size, _src)) => {
                        match rosc::decoder::decode_udp(&buf[..size]) {
                            Ok((_, packet)) => {
                                process_packet(packet, &tx, &log).await;
                            }
                            Err(e) => {
                                warn!("Failed to decode OSC packet: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        error!("UDP receive error: {e}");
                        break;
                    }
                }
            }
        }
    }
    // Drop socket Arc ref so the port can be freed
    drop(socket);
}

/// Recursively process an OSC packet (message or bundle).
async fn process_packet(
    packet: OscPacket,
    tx: &mpsc::Sender<ReceivedOscMessage>,
    log: &Option<OscLog>,
) {
    match packet {
        OscPacket::Message(msg) => {
            trace!(path = msg.addr, "Received OSC message");

            // Log incoming message
            if let Some(log) = log {
                log.log_in(&msg.addr, &format_osc_args(&msg.args));
            }

            let received = ReceivedOscMessage {
                path: msg.addr,
                args: msg.args,
            };
            if tx.send(received).await.is_err() {
                error!("OSC receive channel closed");
            }
        }
        OscPacket::Bundle(bundle) => {
            for p in bundle.content {
                Box::pin(process_packet(p, tx, log)).await;
            }
        }
    }
}
