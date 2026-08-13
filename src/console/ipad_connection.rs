use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::model::osc_log::OscLog;
use crate::osc::client::{ReceivedOscMessage, format_osc_args};
use crate::osc::ipad_client::{IpadClient, IpadSender};
use crate::osc::ipad_parse::{self, ParsedIpadMessage};

use super::connection::DaemonState;
use super::inbound::{self, InboundSource};
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
    daemon: DaemonState,
    interface_name: Option<&str>,
    osc_log: Option<OscLog>,
) -> Result<(IpadSender, HandshakeResult, JoinHandle<()>), IpadConnectionError> {
    info!(%console_ipad_addr, "Mode 2: connecting to console iPad port...");

    let client = IpadClient::new(local_addr, console_ipad_addr, interface_name).await?;
    let (mut sender, mut rx) = client.into_parts();
    // Outbound iPad sends (handshake + engine writes) appear in the OSC Log.
    sender.set_log(osc_log.clone());

    // Perform handshake
    let handshake_result =
        ipad_handshake::perform_handshake(&sender, &mut rx, HANDSHAKE_TIMEOUT).await?;

    info!(
        name = %handshake_result.config.console_name,
        banks = handshake_result.layout_banks.len(),
        "Mode 2: handshake complete"
    );

    // Seed current_console_snapshot from the handshake reply if present.
    if let Some(n) = handshake_result.current_snapshot {
        daemon.state.write().await.current_console_snapshot = Some(n);
    }

    // Start background state mirror loop
    let log_clone = osc_log.clone();
    let handle = tokio::spawn(async move {
        ipad_state_mirror_loop(rx, daemon, log_clone).await;
    });

    Ok((sender, handshake_result, handle))
}

/// Mode 3: Two-socket iPad proxy.
///
/// Console-side socket: bound to `local_console_addr`, sends to `console_ipad_addr`
/// iPad-side socket: bound to `ipad_listen_addr`, sends to `ipad_target`
///
/// `ipad_target` is the pinned iPad address — there is no autodiscovery; the
/// caller must supply the IP (the DiGiCo iPad app shows it).
///
/// Starts forwarding immediately without blocking on a handshake.
/// All traffic is logged and captured into the state mirror.
#[allow(clippy::too_many_arguments)]
pub async fn connect_mode3_proxy(
    console_ipad_addr: SocketAddr,
    local_console_addr: SocketAddr,
    ipad_listen_addr: SocketAddr,
    ipad_target: SocketAddr,
    daemon: DaemonState,
    cancel: tokio_util::sync::CancellationToken,
    interface_name: Option<String>,
    osc_log: Option<OscLog>,
) -> Result<IpadSender, IpadConnectionError> {
    info!(
        %console_ipad_addr,
        %local_console_addr,
        %ipad_listen_addr,
        %ipad_target,
        "Mode 3: setting up two-socket iPad proxy..."
    );

    // Socket 1: Console-side (daemon ↔ console) — raw UDP, interface-bound.
    // Enlarged buffers: this socket receives the console's initial flood, which
    // can overflow the default OS receive buffer and silently drop datagrams.
    let console_socket = crate::ui::net_interfaces::create_bound_udp_socket_buffered(
        local_console_addr,
        interface_name.as_deref(),
        crate::ui::net_interfaces::PROXY_SOCKET_BUFFER_BYTES,
    )
    .await?;
    let actual_console = console_socket.local_addr()?;
    info!(%actual_console, %console_ipad_addr, "Mode 3: console-side socket bound");
    let console_socket = std::sync::Arc::new(console_socket);

    // Also create an IpadSender for snapshot engine use (sends via same socket).
    // Engine-originated sends (snapshot/gang/macro writes to the console) are
    // logged as iPad → Console; the proxy's own forwards are logged in
    // `log_and_capture_packet`.
    let mut ipad_sender = IpadSender::from_socket(console_socket.clone(), console_ipad_addr);
    ipad_sender.set_log(osc_log.clone());

    // Socket 2: iPad-side (daemon ↔ iPad) — raw UDP, interface-bound.
    // Enlarged buffers to match the console side (see above).
    let ipad_socket = crate::ui::net_interfaces::create_bound_udp_socket_buffered(
        ipad_listen_addr,
        interface_name.as_deref(),
        crate::ui::net_interfaces::PROXY_SOCKET_BUFFER_BYTES,
    )
    .await?;
    let actual_listen = ipad_socket.local_addr()?;
    info!(%actual_listen, "Mode 3: iPad-side socket listening");
    let ipad_socket = std::sync::Arc::new(ipad_socket);

    // Decouple state-mirror updates from the proxy hot path: the proxy
    // forwards bytes immediately and `try_send`s captures into a channel;
    // a dedicated task drains the channel and updates `state` /
    // `dirty_tracker` independently. Bounded buffer drops captures under
    // backpressure rather than slowing forwarding (mirror staleness >
    // forwarding latency for a high-volume Mode 3 stream).
    // Headroom for the one-time connect flood (faders/levels). Meters are
    // excluded from capture in `proxy_direction`, so this only has to absorb the
    // transient state dump, not the continuous meter stream.
    let (capture_tx, capture_rx) = mpsc::channel::<ProxyCapture>(2048);

    let capture_daemon = daemon.clone();
    let capture_cancel = cancel.clone();
    let capture_log = osc_log.clone();
    tokio::spawn(async move {
        state_capture_loop(capture_rx, capture_daemon, capture_cancel, capture_log).await;
    });

    // Start forwarding immediately (no handshake). Each direction is its own
    // task so neither can starve the other's recv: forwarding the console's
    // initial flood (C→I) no longer delays draining the iPad-side socket (and
    // vice versa), which is what was overflowing the kernel recv buffer and
    // dropping matrix/bus responses on the first connection.
    let offline_c2i = daemon.offline_mode.clone();
    let offline_i2c = daemon.offline_mode.clone();
    let cancel_c2i = cancel.clone();
    let cancel_i2c = cancel;
    let capture_tx_c2i = capture_tx.clone();
    let capture_tx_i2c = capture_tx;
    let log_c2i = osc_log.clone();
    let log_i2c = osc_log;

    let cs_c2i = console_socket.clone(); // C→I recv side
    let is_c2i = ipad_socket.clone(); // C→I send side
    let is_i2c = ipad_socket; // I→C recv side
    let cs_i2c = console_socket; // I→C send side

    // Console → daemon → iPad
    tokio::spawn(async move {
        proxy_direction(
            cs_c2i,
            is_c2i,
            ipad_target,
            capture_tx_c2i,
            offline_c2i,
            cancel_c2i,
            "C→I",
            log_c2i,
        )
        .await;
    });

    // iPad → daemon → console
    tokio::spawn(async move {
        proxy_direction(
            is_i2c,
            cs_i2c,
            console_ipad_addr,
            capture_tx_i2c,
            offline_i2c,
            cancel_i2c,
            "I→C",
            log_i2c,
        )
        .await;
    });

    Ok(ipad_sender)
}

/// One captured packet: raw bytes plus the direction that produced them.
/// Sent from `proxy_direction` to `state_capture_loop` over a bounded channel.
struct ProxyCapture {
    bytes: Vec<u8>,
    direction: &'static str,
}

/// Drain captures from the proxy and update the state mirror / dirty tracker /
/// snapshot-follow channel. Runs as a separate task so that state contention
/// (snapshot engine writes, UI reads, gang propagation) cannot delay the
/// raw byte forwarding in the `proxy_direction` tasks.
async fn state_capture_loop(
    mut rx: mpsc::Receiver<ProxyCapture>,
    daemon: DaemonState,
    cancel: tokio_util::sync::CancellationToken,
    osc_log: Option<OscLog>,
) {
    info!("Mode 3 state-capture loop started");
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            msg = rx.recv() => match msg {
                Some(capture) => {
                    log_and_capture_packet(
                        &capture.bytes,
                        capture.direction,
                        &daemon,
                        &osc_log,
                    )
                    .await;
                }
                None => break,
            },
        }
    }
    info!("Mode 3 state-capture loop ended");
}

/// Background loop for Mode 2: mirrors iPad protocol messages into ConsoleState.
async fn ipad_state_mirror_loop(
    mut rx: tokio::sync::mpsc::Receiver<ReceivedOscMessage>,
    daemon: DaemonState,
    osc_log: Option<OscLog>,
) {
    info!("iPad state mirror loop started");
    while let Some(msg) = rx.recv().await {
        if daemon.offline_mode.load(Ordering::Relaxed) {
            debug!(path = %msg.path, "iPad mirror: dropped (offline mode)");
            continue;
        }
        // Validate channel numbers against the live config so a buggy /
        // mis-configured peer can't pollute the mirror with bogus channels.
        let config_snapshot = daemon.state.read().await.config.clone();
        let parsed = ipad_parse::parse_ipad_message_with_config(
            &msg.path,
            &msg.args,
            Some(&config_snapshot),
        );
        // Log inbound iPad traffic (Console → iPad). Skip high-frequency
        // meters so the log isn't flooded.
        if let Some(ref log) = osc_log {
            if !matches!(parsed, ParsedIpadMessage::MeterValues(_)) {
                log.log_ipad_in(&msg.path, &format_osc_args(&msg.args));
            }
        }
        match parsed {
            ParsedIpadMessage::ParameterUpdate(addr, value) => {
                // Full shared side-effect chain (mirror, dirty screening,
                // operator override, gang + pan-link propagation, macro
                // learn) — same as the GP loop. Historically this loop
                // only mirrored + dirty-marked, so a desk move seen via
                // the iPad protocol never gang-propagated.
                inbound::apply_inbound_parameter(&daemon, &addr, &value, InboundSource::Pad).await;
            }
            ParsedIpadMessage::SnapshotInfo { current } => {
                debug!(current, "iPad mirror: console snapshot is now {current}");
                let prev = {
                    let mut s = daemon.state.write().await;
                    let p = s.current_console_snapshot;
                    s.current_console_snapshot = Some(current);
                    p
                };
                // Notify the follow-mode dispatcher only on actual changes.
                if prev != Some(current) {
                    if let Some(tx) = &daemon.console_snapshot_tx {
                        let _ = tx.try_send(current);
                    }
                }
            }
            ParsedIpadMessage::MeterValues(_) => {
                // Meters are high-frequency — skip state updates
            }
            ParsedIpadMessage::ConfigResponse(cfg_msg) => {
                // Mirror runtime config pushes — e.g. when an aux is
                // reconfigured stereo↔mono the console resends
                // `/Console/Aux_Outputs/modes` and the Pan Link tab's
                // aux-mode read needs to follow.
                debug!(?cfg_msg, "iPad mirror: config update");
                let mut s = daemon.state.write().await;
                ipad_handshake::apply_config_message(&mut s.config, &cfg_msg);
            }
            _ => {
                debug!(path = msg.path, "iPad mirror: non-parameter message");
            }
        }
    }
    info!("iPad state mirror loop ended");
}

/// One direction of the Mode 3 raw proxy: recv on `recv_socket`, forward to
/// `dest` on `send_socket`, then best-effort enqueue a capture.
///
/// Runs as its own task (one per direction) so the two directions never compete
/// for drain time — forwarding the console's initial flood no longer starves
/// the iPad-side recv (and vice versa), which is what was overflowing the kernel
/// recv buffer and dropping matrix/bus responses. Parsing is offloaded to
/// `state_capture_loop` via `capture_tx` so state-mirror contention (snapshot
/// engine writes, UI reads, gang propagation) cannot delay byte forwarding here.
async fn proxy_direction(
    recv_socket: std::sync::Arc<tokio::net::UdpSocket>,
    send_socket: std::sync::Arc<tokio::net::UdpSocket>,
    dest: SocketAddr,
    capture_tx: mpsc::Sender<ProxyCapture>,
    offline_mode: Arc<AtomicBool>,
    cancel: tokio_util::sync::CancellationToken,
    direction: &'static str,
    osc_log: Option<OscLog>,
) {
    info!(%dest, direction, "Mode 3 proxy direction started");

    let mut buf = vec![0u8; 65536];
    let mut forwarded: u64 = 0;
    let mut capture_drops: u64 = 0;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!(direction, forwarded, capture_drops, "Mode 3 proxy direction cancelled");
                break;
            }

            result = recv_socket.recv_from(&mut buf) => {
                match result {
                    Ok((size, _src)) => {
                        if offline_mode.load(Ordering::Relaxed) {
                            debug!(direction, "Mode 3 forward dropped (offline mode)");
                            continue;
                        }

                        // Forward FIRST so state-mirror contention can't delay
                        // the byte path — capture happens off the hot path.
                        match send_socket.send_to(&buf[..size], dest).await {
                            Ok(sent) => {
                                forwarded += 1;
                                if forwarded <= 5 {
                                    debug!(direction, forwarded, sent, %dest, "Proxy forwarded {sent} bytes to {dest}");
                                }
                            }
                            Err(e) => {
                                warn!(direction, %dest, "Proxy send failed: {e}");
                            }
                        }

                        // The console's continuous `/Meters/values` stream is the
                        // dominant load and is never mirrored (state ignores it),
                        // so don't even allocate + enqueue it for capture — that
                        // keeps the capture channel free for keepalive / control
                        // traffic and keeps this recv loop lean during the flood.
                        // Meters are still forwarded above; only parse/log/state
                        // is skipped. (Faders/levels are NOT skipped.)
                        if buf[..size].starts_with(b"/Meters/values") {
                            continue;
                        }

                        // Best-effort enqueue for state capture. Channel-full
                        // means the capture task is lagging — drop the capture
                        // (mirror briefly stale) rather than block the proxy.
                        if capture_tx
                            .try_send(ProxyCapture {
                                bytes: buf[..size].to_vec(),
                                direction,
                            })
                            .is_err()
                        {
                            capture_drops = capture_drops.saturating_add(1);
                            if capture_drops.is_power_of_two() {
                                warn!(direction, capture_drops, "Mode 3 capture channel full — state mirror is lagging");
                                // Surface the drop in the OSC log so the operator
                                // can see exactly when the proxy fell behind.
                                if let Some(ref log) = osc_log {
                                    let note = format!("{capture_drops} packets dropped from log/mirror");
                                    if direction == "I→C" {
                                        log.log_ipad_out("⚠ proxy log overflow", &note);
                                    } else {
                                        log.log_ipad_in("⚠ proxy log overflow", &note);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // A fatal recv error on one direction tears down the
                        // whole proxy: cancel so the sibling direction and the
                        // capture loop exit cleanly too.
                        warn!(direction, "Proxy recv error: {e}");
                        cancel.cancel();
                        break;
                    }
                }
            }
        }
    }
    info!(direction, "Mode 3 proxy direction ended");
}

/// Best-effort parse and log a raw packet. Captures parameter updates into state.
async fn log_and_capture_packet(
    data: &[u8],
    direction: &str,
    daemon: &DaemonState,
    osc_log: &Option<OscLog>,
) {
    // Try standard OSC first (alignment-tolerant — SD/Quantum and the iPad
    // link emit packets whose length is not a multiple of 4).
    if let Some(packet) = crate::osc::decode_udp_tolerant(data) {
        let messages = flatten_packet(packet);
        // Snapshot the config once for this packet's worth of messages.
        let config_snapshot = daemon.state.read().await.config.clone();
        for msg in messages {
            let parsed = ipad_parse::parse_ipad_message_with_config(
                &msg.path,
                &msg.args,
                Some(&config_snapshot),
            );
            // Log proxied iPad traffic, mapping the wire direction:
            // I→C = iPad → Console, C→I = Console → iPad. Skip meters.
            if let Some(log) = osc_log {
                if !matches!(parsed, ParsedIpadMessage::MeterValues(_)) {
                    let args = format_osc_args(&msg.args);
                    if direction == "I→C" {
                        log.log_ipad_out(&msg.path, &args);
                    } else {
                        log.log_ipad_in(&msg.path, &args);
                    }
                }
            }
            match &parsed {
                ParsedIpadMessage::ParameterUpdate(addr, value) => {
                    debug!(%addr, %value, "Proxy {direction}: param");
                    // Full shared side-effect chain — same as the GP and
                    // Mode-2 loops. I→C carries the real iPad operator's
                    // moves, C→I carries the desk surface's; both are
                    // operator input (engine-write echoes are screened by
                    // the SentLog inside the chain).
                    inbound::apply_inbound_parameter(daemon, addr, value, InboundSource::Pad).await;
                }
                ParsedIpadMessage::SnapshotInfo { current } => {
                    debug!(
                        current,
                        "Proxy {direction}: console snapshot is now {current}"
                    );
                    let prev = {
                        let mut s = daemon.state.write().await;
                        let p = s.current_console_snapshot;
                        s.current_console_snapshot = Some(*current);
                        p
                    };
                    if prev != Some(*current) {
                        if let Some(tx) = &daemon.console_snapshot_tx {
                            let _ = tx.try_send(*current);
                        }
                    }
                }
                ParsedIpadMessage::ConfigResponse(cfg) => {
                    info!(?cfg, "Proxy {direction}: config");
                    // Same as the Mode-2 mirror loop: track live config
                    // pushes (aux-mode reconfig, name/serial/session
                    // changes, channel-count changes) so config-driven
                    // UI (Pan Link, Setup, …) follows the desk in real
                    // time instead of needing a reconnect.
                    let mut s = daemon.state.write().await;
                    ipad_handshake::apply_config_message(&mut s.config, cfg);
                }
                _ => {
                    debug!(path = msg.path, "Proxy {direction}: {}", msg.path);
                }
            }
        }
    } else if let Some(msg) = parse_bare_path(data) {
        // DiGiCo bare-path query (path + null, no type tag). Log it too so the
        // OSC log isn't blind to the non-standard packets the proxy moves.
        debug!(path = msg.path, "Proxy {direction}: bare query");
        if let Some(log) = osc_log {
            if direction == "I→C" {
                log.log_ipad_out(&msg.path, "");
            } else {
                log.log_ipad_in(&msg.path, "");
            }
        }
    } else {
        let hex: String = data[..data.len().min(32)]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        debug!(size = data.len(), %hex, "Proxy {direction}: raw ({} bytes)", data.len());
        // Surface undecodable packets in the OSC log as well, with a byte count
        // and a short hex preview, so nothing the proxy forwards is invisible.
        if let Some(log) = osc_log {
            let note = format!("{hex} ({} bytes)", data.len());
            if direction == "I→C" {
                log.log_ipad_out("‹raw›", &note);
            } else {
                log.log_ipad_in("‹raw›", &note);
            }
        }
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
        let io_err = IpadConnectionError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "refused",
        ));
        assert!(io_err.to_string().contains("refused"));

        let hs_err = IpadConnectionError::Handshake(ipad_handshake::HandshakeError::Timeout {
            phase: "config".into(),
        });
        assert!(hs_err.to_string().contains("config"));
    }
}
