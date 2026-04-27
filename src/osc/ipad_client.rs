use rosc::{OscMessage, OscPacket, OscType};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, error, trace, warn};

use super::client::ReceivedOscMessage;

/// Async iPad protocol UDP client (connects to console's iPad remote port).
pub struct IpadClient {
    socket: UdpSocket,
    console_addr: SocketAddr,
}

impl IpadClient {
    /// Create a new iPad client bound to `local_addr`, sending to `console_addr`.
    /// If `interface_name` is provided, the socket is pinned to that network interface.
    pub async fn new(
        local_addr: SocketAddr,
        console_addr: SocketAddr,
        interface_name: Option<&str>,
    ) -> std::io::Result<Self> {
        let socket =
            crate::ui::net_interfaces::create_bound_udp_socket(local_addr, interface_name).await?;
        let actual_local = socket.local_addr()?;
        tracing::info!(
            %actual_local,
            %console_addr,
            "iPad client: bound local={actual_local}, sending to console={console_addr}"
        );
        Ok(Self {
            socket,
            console_addr,
        })
    }

    /// Split into a sender handle and a receive channel.
    pub fn into_parts(self) -> (IpadSender, mpsc::Receiver<ReceivedOscMessage>) {
        let (tx, rx) = mpsc::channel(1024);
        let socket = std::sync::Arc::new(self.socket);

        let sender = IpadSender {
            socket: socket.clone(),
            console_addr: self.console_addr,
            offline_mode: None,
        };

        tokio::spawn(receive_loop(socket, tx));

        (sender, rx)
    }
}

/// Handle for sending iPad protocol OSC messages (can be cloned and shared).
#[derive(Clone)]
pub struct IpadSender {
    socket: std::sync::Arc<UdpSocket>,
    console_addr: SocketAddr,
    /// When set and `true`, all sends become no-ops (offline mode).
    offline_mode: Option<Arc<AtomicBool>>,
}

impl IpadSender {
    /// Create a sender from an existing socket (for Mode 3 where we manage the socket directly).
    pub fn from_socket(socket: std::sync::Arc<UdpSocket>, console_addr: SocketAddr) -> Self {
        Self {
            socket,
            console_addr,
            offline_mode: None,
        }
    }

    /// Attach the shared offline-mode flag. When `true`, both `send` and
    /// `send_raw` become no-ops without touching the socket.
    pub fn set_offline_flag(&mut self, flag: Arc<AtomicBool>) {
        self.offline_mode = Some(flag);
    }

    fn is_offline(&self) -> bool {
        self.offline_mode
            .as_ref()
            .map(|f| f.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    /// Send raw bytes to the console (for forwarding non-standard packets as-is).
    pub async fn send_raw(&self, data: &[u8]) -> std::io::Result<()> {
        if self.is_offline() {
            trace!("iPad raw send dropped (offline mode)");
            return Ok(());
        }
        self.socket.send_to(data, self.console_addr).await?;
        debug!("Sent raw iPad packet ({} bytes)", data.len());
        Ok(())
    }

    /// Get a reference to the underlying socket (for raw proxy recv).
    pub fn socket(&self) -> &std::sync::Arc<UdpSocket> {
        &self.socket
    }

    /// Send an OSC message via the iPad protocol.
    pub async fn send(&self, path: &str, args: Vec<OscType>) -> std::io::Result<()> {
        if self.is_offline() {
            trace!(path, "iPad send dropped (offline mode)");
            return Ok(());
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
        debug!(path, "Sent iPad OSC message");
        Ok(())
    }
}

/// Background receive loop for iPad protocol messages.
/// Handles both standard OSC and DiGiCo's non-standard bare-path packets.
async fn receive_loop(socket: std::sync::Arc<UdpSocket>, tx: mpsc::Sender<ReceivedOscMessage>) {
    let mut buf = vec![0u8; 65536];
    let mut first_message = true;
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((size, src)) => {
                if first_message {
                    tracing::info!(%src, size, "iPad: first packet received from {src} ({size} bytes)");
                    first_message = false;
                }
                match rosc::decoder::decode_udp(&buf[..size]) {
                    Ok((_, packet)) => {
                        process_packet(packet, &tx).await;
                    }
                    Err(_) => {
                        // DiGiCo iPad protocol may use non-standard encoding:
                        // bare path + null (no type tag) for queries.
                        // Try to parse as bare path with optional inline args.
                        if let Some(msg) = parse_digico_packet(&buf[..size]) {
                            debug!(path = msg.path, "iPad recv: DiGiCo non-standard packet");
                            if tx.send(msg).await.is_err() {
                                error!("iPad OSC receive channel closed");
                            }
                        } else {
                            let hex: String = buf[..size.min(32)]
                                .iter()
                                .map(|b| format!("{b:02x}"))
                                .collect::<Vec<_>>()
                                .join(" ");
                            warn!(%src, size, %hex, "iPad: unrecognized packet format");
                        }
                    }
                }
            }
            Err(e) => {
                error!("iPad UDP receive error: {e}");
                break;
            }
        }
    }
}

/// Parse a DiGiCo non-standard packet.
/// Format: path (null-terminated, 4-byte aligned) optionally followed by inline values.
/// Queries: just `/path/?\0` padded to 4 bytes.
/// Responses: `/path\0` padded, then space-separated values encoded after the path.
fn parse_digico_packet(data: &[u8]) -> Option<ReceivedOscMessage> {
    if data.first() != Some(&b'/') {
        return None;
    }
    // Find the null terminator for the path
    let null_pos = data.iter().position(|&b| b == 0)?;
    let path = std::str::from_utf8(&data[..null_pos]).ok()?;

    // After the path (null-terminated, padded to 4 bytes), there might be args
    // For bare queries like /Console/Name/?\0 — no args
    // For responses, args follow after 4-byte alignment of the path
    let padded_end = (null_pos + 4) & !3; // round up to 4-byte boundary
    let args = if padded_end < data.len() {
        // There's data after the path — try to interpret remaining bytes as inline args
        // DiGiCo seems to use space-separated text values inline after the path
        // For now, try to extract as a string
        let rest = &data[padded_end..];
        if let Ok(s) = std::str::from_utf8(rest) {
            let trimmed = s.trim_end_matches('\0').trim();
            if !trimmed.is_empty() {
                // Try to parse as numeric values, fall back to string
                let mut osc_args = Vec::new();
                for part in trimmed.split_whitespace() {
                    if let Ok(i) = part.parse::<i32>() {
                        osc_args.push(OscType::Int(i));
                    } else if let Ok(f) = part.parse::<f32>() {
                        osc_args.push(OscType::Float(f));
                    } else {
                        osc_args.push(OscType::String(part.to_string()));
                    }
                }
                osc_args
            } else {
                vec![]
            }
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    Some(ReceivedOscMessage {
        path: path.to_string(),
        args,
    })
}

async fn process_packet(packet: OscPacket, tx: &mpsc::Sender<ReceivedOscMessage>) {
    match packet {
        OscPacket::Message(msg) => {
            trace!(path = msg.addr, "Received iPad OSC message");
            let received = ReceivedOscMessage {
                path: msg.addr,
                args: msg.args,
            };
            if tx.send(received).await.is_err() {
                error!("iPad OSC receive channel closed");
            }
        }
        OscPacket::Bundle(bundle) => {
            for p in bundle.content {
                Box::pin(process_packet(p, tx)).await;
            }
        }
    }
}
