use rosc::{OscMessage, OscPacket, OscType};
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, error, trace, warn};

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
    pub async fn new(
        local_addr: SocketAddr,
        console_addr: SocketAddr,
    ) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(local_addr).await?;
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
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("OSC encode error: {e}"))
        })?;
        self.socket.send_to(&buf, self.console_addr).await?;
        debug!(path, "Sent OSC message");
        Ok(())
    }

    /// Split this client into a sender handle and a receive loop.
    /// The receive loop runs as a background task, pushing messages to the returned channel.
    pub fn into_parts(self) -> (OscSender, mpsc::Receiver<ReceivedOscMessage>) {
        let (tx, rx) = mpsc::channel(1024);
        let socket = std::sync::Arc::new(self.socket);

        let sender = OscSender {
            socket: socket.clone(),
            console_addr: self.console_addr,
        };

        // Spawn the receive loop
        tokio::spawn(receive_loop(socket, tx));

        (sender, rx)
    }
}

/// Handle for sending OSC messages (can be cloned and shared).
#[derive(Clone)]
pub struct OscSender {
    socket: std::sync::Arc<UdpSocket>,
    console_addr: SocketAddr,
}

impl OscSender {
    /// Send an OSC message to the console.
    pub async fn send(&self, path: &str, args: Vec<OscType>) -> std::io::Result<()> {
        let msg = OscMessage {
            addr: path.to_string(),
            args,
        };
        let packet = OscPacket::Message(msg);
        let buf = rosc::encoder::encode(&packet).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, format!("OSC encode error: {e}"))
        })?;
        self.socket.send_to(&buf, self.console_addr).await?;
        debug!(path, "Sent OSC message");
        Ok(())
    }
}

/// Background receive loop: reads UDP packets, decodes OSC, and forwards to channel.
async fn receive_loop(
    socket: std::sync::Arc<UdpSocket>,
    tx: mpsc::Sender<ReceivedOscMessage>,
) {
    let mut buf = vec![0u8; 65536];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((size, _src)) => {
                match rosc::decoder::decode_udp(&buf[..size]) {
                    Ok((_, packet)) => {
                        process_packet(packet, &tx).await;
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

/// Recursively process an OSC packet (message or bundle).
async fn process_packet(packet: OscPacket, tx: &mpsc::Sender<ReceivedOscMessage>) {
    match packet {
        OscPacket::Message(msg) => {
            trace!(path = msg.addr, "Received OSC message");
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
                Box::pin(process_packet(p, tx)).await;
            }
        }
    }
}
