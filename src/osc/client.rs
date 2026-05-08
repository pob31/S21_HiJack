use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType};
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

    /// Send several OSC messages as a single OSC 1.0 bundle. The bundle
    /// uses the "immediate" timetag sentinel `(0, 1)` so the console
    /// processes the contained messages without delay.
    ///
    /// Bundling guarantees that the console sees all messages as one
    /// UDP datagram delivery — preventing the inter-packet reordering
    /// and drops we'd see with N back-to-back individual sends. Used by
    /// the gang and pan-link engines when one inbound parameter event
    /// fans out to multiple downstream writes.
    ///
    /// If the encoded bundle exceeds [`MAX_BUNDLE_BYTES`] it's split
    /// into successive bundles by halving the message list until each
    /// chunk fits. The split preserves overall message order.
    pub async fn send_bundle(&self, messages: Vec<OscMessage>) -> std::io::Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        // Offline gate.
        if let Some(ref flag) = self.offline_mode {
            if flag.load(Ordering::Relaxed) {
                trace!(count = messages.len(), "OSC bundle dropped (offline mode)");
                return Ok(());
            }
        }
        // Log each message individually so the OSC Log tab keeps its
        // per-path granularity.
        if let Some(ref log) = self.log {
            for m in &messages {
                log.log_out(&m.addr, &format_osc_args(&m.args));
            }
        }
        send_bundle_chunked(&self.socket, self.console_addr, messages).await
    }
}

/// Conservative upper bound on bundle size before we split. Typical
/// Ethernet path MTU is 1500 bytes minus IP+UDP headers (~28 bytes);
/// 1200 leaves headroom for the OSC bundle envelope and any future
/// header growth.
pub(crate) const MAX_BUNDLE_BYTES: usize = 1200;

/// Build an OscBundle with the immediate-execute timetag and encode it.
pub(crate) fn encode_bundle(messages: Vec<OscMessage>) -> std::io::Result<Vec<u8>> {
    let bundle = OscBundle {
        // OSC 1.0 sentinel for "execute immediately".
        timetag: OscTime {
            seconds: 0,
            fractional: 1,
        },
        content: messages.into_iter().map(OscPacket::Message).collect(),
    };
    rosc::encoder::encode(&OscPacket::Bundle(bundle)).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("OSC bundle encode error: {e}"),
        )
    })
}

/// Send a list of messages as one or more bundles, splitting when the
/// encoded size exceeds [`MAX_BUNDLE_BYTES`]. Order is preserved.
pub(crate) async fn send_bundle_chunked(
    socket: &UdpSocket,
    target: SocketAddr,
    messages: Vec<OscMessage>,
) -> std::io::Result<()> {
    if messages.is_empty() {
        return Ok(());
    }
    // Greedy chunking: try to send the whole list as one bundle; if
    // it's too large, split in half and recurse. Avoids a costly
    // per-message encode-and-measure pass when bundles fit (the common
    // case).
    fn chunks(messages: Vec<OscMessage>, out: &mut Vec<Vec<u8>>) -> std::io::Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let buf = encode_bundle(messages.clone())?;
        if buf.len() <= MAX_BUNDLE_BYTES || messages.len() == 1 {
            out.push(buf);
            return Ok(());
        }
        let mid = messages.len() / 2;
        let mut iter = messages.into_iter();
        let left: Vec<OscMessage> = (&mut iter).take(mid).collect();
        let right: Vec<OscMessage> = iter.collect();
        chunks(left, out)?;
        chunks(right, out)?;
        Ok(())
    }
    let mut bufs: Vec<Vec<u8>> = Vec::new();
    chunks(messages, &mut bufs)?;
    for buf in bufs {
        socket.send_to(&buf, target).await?;
    }
    debug!("Sent OSC bundle(s)");
    Ok(())
}

/// Format OSC arguments as a compact string for logging.
fn format_osc_args(args: &[OscType]) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::OscType;

    fn msg(addr: &str, v: f32) -> OscMessage {
        OscMessage {
            addr: addr.to_string(),
            args: vec![OscType::Float(v)],
        }
    }

    #[test]
    fn encode_bundle_uses_immediate_timetag() {
        let buf = encode_bundle(vec![msg("/Input/1/pan", 0.5)]).unwrap();
        let (_, packet) = rosc::decoder::decode_udp(&buf).unwrap();
        match packet {
            OscPacket::Bundle(b) => {
                assert_eq!(b.timetag.seconds, 0);
                assert_eq!(b.timetag.fractional, 1);
                assert_eq!(b.content.len(), 1);
            }
            _ => panic!("expected bundle"),
        }
    }

    #[test]
    fn encode_bundle_round_trips_messages_in_order() {
        let messages = vec![
            msg("/Input/1/send/5/pan", 0.1),
            msg("/Input/2/send/5/pan", 0.2),
            msg("/Input/3/send/5/pan", 0.3),
        ];
        let buf = encode_bundle(messages.clone()).unwrap();
        let (_, packet) = rosc::decoder::decode_udp(&buf).unwrap();
        let OscPacket::Bundle(b) = packet else {
            panic!("expected bundle");
        };
        let decoded: Vec<OscMessage> = b
            .content
            .into_iter()
            .map(|p| match p {
                OscPacket::Message(m) => m,
                _ => panic!("nested bundle"),
            })
            .collect();
        assert_eq!(decoded.len(), 3);
        for (a, b) in decoded.iter().zip(messages.iter()) {
            assert_eq!(a.addr, b.addr);
            assert_eq!(a.args.len(), b.args.len());
        }
    }

    #[tokio::test]
    async fn send_bundle_chunked_splits_when_over_mtu() {
        // Build enough fat messages that the encoded bundle blows
        // past MAX_BUNDLE_BYTES — verify the helper splits the
        // payload across multiple datagrams instead of refusing.
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let std_send = std::net::UdpSocket::bind(bind).unwrap();
        std_send.set_nonblocking(true).unwrap();
        let send_socket = UdpSocket::from_std(std_send).unwrap();
        let std_recv = std::net::UdpSocket::bind(bind).unwrap();
        let recv_addr = std_recv.local_addr().unwrap();
        std_recv.set_nonblocking(true).unwrap();
        let recv_socket = UdpSocket::from_std(std_recv).unwrap();

        // ~200 bytes per message => 8+ messages overflow 1200.
        let messages: Vec<OscMessage> = (0..30)
            .map(|i| OscMessage {
                addr: format!("/very/long/path/that/inflates/the/encoded/size/{i:04}"),
                args: vec![
                    OscType::Float(i as f32),
                    OscType::String("padding-padding-padding".into()),
                ],
            })
            .collect();

        send_bundle_chunked(&send_socket, recv_addr, messages)
            .await
            .expect("send ok");

        // Drain datagrams; assert at least 2 arrived (chunked) and
        // each is under the cap. Use blocking recv with a short
        // timeout for the first packet (loopback may need a tick),
        // then try_recv for the rest.
        let mut buf = vec![0u8; 65536];
        let first = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            recv_socket.recv_from(&mut buf),
        )
        .await
        .expect("first datagram timed out")
        .expect("recv ok");
        assert!(
            first.0 <= MAX_BUNDLE_BYTES,
            "{} > {MAX_BUNDLE_BYTES}",
            first.0
        );
        let mut datagrams = 1;
        // Give the scheduler a chance to deliver remaining packets,
        // then drain non-blocking.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        while let Ok((n, _)) = recv_socket.try_recv_from(&mut buf) {
            assert!(n <= MAX_BUNDLE_BYTES, "{n} > {MAX_BUNDLE_BYTES}");
            datagrams += 1;
        }
        assert!(datagrams >= 2, "expected split, got {datagrams} datagrams");
    }

    #[tokio::test]
    async fn send_bundle_chunked_one_packet_when_under_mtu() {
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let std_send = std::net::UdpSocket::bind(bind).unwrap();
        std_send.set_nonblocking(true).unwrap();
        let send_socket = UdpSocket::from_std(std_send).unwrap();
        let std_recv = std::net::UdpSocket::bind(bind).unwrap();
        let recv_addr = std_recv.local_addr().unwrap();
        std_recv.set_nonblocking(true).unwrap();
        let recv_socket = UdpSocket::from_std(std_recv).unwrap();

        let messages = vec![msg("/a", 0.1), msg("/b", 0.2), msg("/c", 0.3)];
        send_bundle_chunked(&send_socket, recv_addr, messages)
            .await
            .unwrap();

        let mut buf = vec![0u8; 65536];
        let n = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            recv_socket.recv_from(&mut buf),
        )
        .await
        .expect("datagram timed out")
        .unwrap()
        .0;
        // Single datagram; second receive should not be available.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(recv_socket.try_recv_from(&mut buf).is_err());
        let (_, packet) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
        let OscPacket::Bundle(b) = packet else {
            panic!("expected bundle");
        };
        assert_eq!(b.content.len(), 3);
    }
}
