use rosc::{OscMessage, OscPacket, OscType};
use std::net::SocketAddr;
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

    /// Split into a sender handle and a receive channel.
    pub fn into_parts(self) -> (IpadSender, mpsc::Receiver<ReceivedOscMessage>) {
        let (tx, rx) = mpsc::channel(1024);
        let socket = std::sync::Arc::new(self.socket);

        let sender = IpadSender {
            socket: socket.clone(),
            console_addr: self.console_addr,
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
}

impl IpadSender {
    /// Send an OSC message via the iPad protocol.
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
        debug!(path, "Sent iPad OSC message");
        Ok(())
    }
}

/// Background receive loop for iPad protocol messages.
async fn receive_loop(socket: std::sync::Arc<UdpSocket>, tx: mpsc::Sender<ReceivedOscMessage>) {
    let mut buf = vec![0u8; 65536];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((size, _src)) => match rosc::decoder::decode_udp(&buf[..size]) {
                Ok((_, packet)) => {
                    process_packet(packet, &tx).await;
                }
                Err(e) => {
                    warn!("Failed to decode iPad OSC packet: {e}");
                }
            },
            Err(e) => {
                error!("iPad UDP receive error: {e}");
                break;
            }
        }
    }
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

/// iPad listener (for Mode 3 proxy — daemon-side socket that the iPad connects to).
pub struct IpadListener {
    socket: UdpSocket,
}

impl IpadListener {
    /// Bind a listener on the given address.
    pub async fn new(listen_addr: SocketAddr) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(listen_addr).await?;
        Ok(Self { socket })
    }

    /// Start receiving and forwarding. Returns a forwarder and receive channel.
    /// The forwarder learns the iPad's address from the first message received.
    pub fn into_parts(self) -> (IpadForwarder, mpsc::Receiver<(ReceivedOscMessage, SocketAddr)>) {
        let (tx, rx) = mpsc::channel(1024);
        let socket = std::sync::Arc::new(self.socket);

        let forwarder = IpadForwarder {
            socket: socket.clone(),
            ipad_addr: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        };

        let ipad_addr = forwarder.ipad_addr.clone();
        tokio::spawn(listener_receive_loop(socket, tx, ipad_addr));

        (forwarder, rx)
    }
}

/// Forwards messages back to the real iPad (for Mode 3 proxy).
#[derive(Clone)]
pub struct IpadForwarder {
    socket: std::sync::Arc<UdpSocket>,
    ipad_addr: std::sync::Arc<tokio::sync::RwLock<Option<SocketAddr>>>,
}

impl IpadForwarder {
    /// Send a message to the connected iPad. Returns Ok(false) if no iPad is connected yet.
    pub async fn send_to_ipad(&self, path: &str, args: Vec<OscType>) -> std::io::Result<bool> {
        let addr = *self.ipad_addr.read().await;
        let Some(dest) = addr else {
            return Ok(false);
        };

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
        self.socket.send_to(&buf, dest).await?;
        debug!(path, %dest, "Forwarded to iPad");
        Ok(true)
    }

    /// Forward raw bytes to the connected iPad.
    pub async fn forward_raw(&self, data: &[u8]) -> std::io::Result<bool> {
        let addr = *self.ipad_addr.read().await;
        let Some(dest) = addr else {
            return Ok(false);
        };
        self.socket.send_to(data, dest).await?;
        Ok(true)
    }

    /// Pre-configure the iPad's address (e.g., from --ipad-ip CLI arg).
    /// Will be overwritten if a packet arrives from a different source.
    pub async fn set_ipad_addr(&self, addr: SocketAddr) {
        *self.ipad_addr.write().await = Some(addr);
    }

    /// Check if an iPad has connected.
    pub async fn ipad_connected(&self) -> bool {
        self.ipad_addr.read().await.is_some()
    }
}

/// Receive loop for the listener socket. Tracks the iPad's source address.
async fn listener_receive_loop(
    socket: std::sync::Arc<UdpSocket>,
    tx: mpsc::Sender<(ReceivedOscMessage, SocketAddr)>,
    ipad_addr: std::sync::Arc<tokio::sync::RwLock<Option<SocketAddr>>>,
) {
    let mut buf = vec![0u8; 65536];
    loop {
        match socket.recv_from(&mut buf).await {
            Ok((size, src)) => {
                // Track the iPad's address
                {
                    let mut addr = ipad_addr.write().await;
                    if *addr != Some(src) {
                        tracing::info!(%src, "iPad connected to proxy");
                        *addr = Some(src);
                    }
                }

                match rosc::decoder::decode_udp(&buf[..size]) {
                    Ok((_, packet)) => {
                        process_packet_with_addr(packet, &tx, src).await;
                    }
                    Err(e) => {
                        warn!("Failed to decode iPad proxy OSC packet: {e}");
                    }
                }
            }
            Err(e) => {
                error!("iPad listener receive error: {e}");
                break;
            }
        }
    }
}

async fn process_packet_with_addr(
    packet: OscPacket,
    tx: &mpsc::Sender<(ReceivedOscMessage, SocketAddr)>,
    src: SocketAddr,
) {
    match packet {
        OscPacket::Message(msg) => {
            trace!(path = msg.addr, "Received from iPad");
            let received = ReceivedOscMessage {
                path: msg.addr,
                args: msg.args,
            };
            if tx.send((received, src)).await.is_err() {
                error!("iPad listener channel closed");
            }
        }
        OscPacket::Bundle(bundle) => {
            for p in bundle.content {
                Box::pin(process_packet_with_addr(p, tx, src)).await;
            }
        }
    }
}
