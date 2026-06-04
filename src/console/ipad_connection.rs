use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::{RwLock, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::model::dirty_tracker::DirtyTracker;
use crate::model::osc_log::OscLog;
use crate::model::state::ConsoleState;
use crate::osc::client::{ReceivedOscMessage, format_osc_args};
use crate::osc::ipad_client::{IpadClient, IpadSender};
use crate::osc::ipad_parse::{self, ParsedIpadMessage};

use super::ipad_handshake::{self, HandshakeResult};
use super::macro_manager::MacroManager;

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
#[allow(clippy::too_many_arguments)]
pub async fn connect_mode2(
    console_ipad_addr: SocketAddr,
    local_addr: SocketAddr,
    state: Arc<RwLock<ConsoleState>>,
    dirty_tracker: Arc<RwLock<DirtyTracker>>,
    macro_manager: Arc<RwLock<MacroManager>>,
    offline_mode: Arc<AtomicBool>,
    snapshot_event_tx: Option<mpsc::Sender<i32>>,
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
        state.write().await.current_console_snapshot = Some(n);
    }

    // Start background state mirror loop
    let state_clone = state.clone();
    let dirty_clone = dirty_tracker.clone();
    let macro_clone = macro_manager.clone();
    let offline_clone = offline_mode.clone();
    let snap_tx = snapshot_event_tx.clone();
    let log_clone = osc_log.clone();
    let handle = tokio::spawn(async move {
        ipad_state_mirror_loop(
            rx,
            state_clone,
            dirty_clone,
            macro_clone,
            offline_clone,
            snap_tx,
            log_clone,
        )
        .await;
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
    state: Arc<RwLock<ConsoleState>>,
    dirty_tracker: Arc<RwLock<DirtyTracker>>,
    macro_manager: Arc<RwLock<MacroManager>>,
    offline_mode: Arc<AtomicBool>,
    snapshot_event_tx: Option<mpsc::Sender<i32>>,
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

    // Socket 1: Console-side (daemon ↔ console) — raw UDP, interface-bound
    let console_socket = crate::ui::net_interfaces::create_bound_udp_socket(
        local_console_addr,
        interface_name.as_deref(),
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

    // Socket 2: iPad-side (daemon ↔ iPad) — raw UDP, interface-bound
    let ipad_socket = crate::ui::net_interfaces::create_bound_udp_socket(
        ipad_listen_addr,
        interface_name.as_deref(),
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
    let (capture_tx, capture_rx) = mpsc::channel::<ProxyCapture>(256);

    let capture_state = state.clone();
    let capture_dt = dirty_tracker.clone();
    let capture_mgr = macro_manager.clone();
    let capture_snap_tx = snapshot_event_tx.clone();
    let capture_cancel = cancel.clone();
    let capture_log = osc_log.clone();
    tokio::spawn(async move {
        state_capture_loop(
            capture_rx,
            capture_state,
            capture_dt,
            capture_mgr,
            capture_snap_tx,
            capture_cancel,
            capture_log,
        )
        .await;
    });

    // Start the bidirectional proxy loop immediately (no handshake)
    let offline_clone = offline_mode.clone();

    tokio::spawn(async move {
        raw_proxy_loop(
            console_socket,
            console_ipad_addr,
            ipad_socket,
            ipad_target,
            capture_tx,
            offline_clone,
            cancel,
        )
        .await;
    });

    Ok(ipad_sender)
}

/// One captured packet: raw bytes plus the direction that produced them.
/// Sent from `raw_proxy_loop` to `state_capture_loop` over a bounded channel.
struct ProxyCapture {
    bytes: Vec<u8>,
    direction: &'static str,
}

/// Drain captures from the proxy and update the state mirror / dirty tracker /
/// snapshot-follow channel. Runs as a separate task so that state contention
/// (snapshot engine writes, UI reads, gang propagation) cannot delay the
/// raw byte forwarding in `raw_proxy_loop`.
async fn state_capture_loop(
    mut rx: mpsc::Receiver<ProxyCapture>,
    state: Arc<RwLock<ConsoleState>>,
    dirty_tracker: Arc<RwLock<DirtyTracker>>,
    macro_manager: Arc<RwLock<MacroManager>>,
    snapshot_event_tx: Option<mpsc::Sender<i32>>,
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
                        &state,
                        &dirty_tracker,
                        &macro_manager,
                        &snapshot_event_tx,
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
    state: Arc<RwLock<ConsoleState>>,
    dirty_tracker: Arc<RwLock<DirtyTracker>>,
    macro_manager: Arc<RwLock<MacroManager>>,
    offline_mode: Arc<AtomicBool>,
    snapshot_event_tx: Option<mpsc::Sender<i32>>,
    osc_log: Option<OscLog>,
) {
    info!("iPad state mirror loop started");
    while let Some(msg) = rx.recv().await {
        if offline_mode.load(Ordering::Relaxed) {
            debug!(path = %msg.path, "iPad mirror: dropped (offline mode)");
            continue;
        }
        // Validate channel numbers against the live config so a buggy /
        // mis-configured peer can't pollute the mirror with bogus channels.
        let config_snapshot = state.read().await.config.clone();
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
                debug!(%addr, %value, "iPad mirror: parameter update");
                let old = state.write().await.update(addr.clone(), value.clone());
                if let Some(prev) = &old {
                    if prev != &value {
                        dirty_tracker.write().await.mark(&addr);
                    }
                }
                // Feed Learn-mode recording from the iPad path too.
                // Without this the macro-recording UI would only ever
                // capture changes that arrived via GP OSC — for an S21
                // driven via the iPad protocol, that's nothing.
                {
                    let mut mgr = macro_manager.write().await;
                    if mgr.is_recording() {
                        mgr.record_change(addr.clone(), value.clone());
                    }
                }
            }
            ParsedIpadMessage::SnapshotInfo { current } => {
                debug!(current, "iPad mirror: console snapshot is now {current}");
                let prev = {
                    let mut s = state.write().await;
                    let p = s.current_console_snapshot;
                    s.current_console_snapshot = Some(current);
                    p
                };
                // Notify the follow-mode dispatcher only on actual changes.
                if prev != Some(current) {
                    if let Some(tx) = &snapshot_event_tx {
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
                let mut s = state.write().await;
                ipad_handshake::apply_config_message(&mut s.config, &cfg_msg);
            }
            _ => {
                debug!(path = msg.path, "iPad mirror: non-parameter message");
            }
        }
    }
    info!("iPad state mirror loop ended");
}

/// Pure raw bidirectional proxy loop for Mode 3.
///
/// Two raw UDP sockets, no parsing, no wrappers. Just forwards bytes.
/// Parsing is offloaded to `state_capture_loop` via `capture_tx` so that
/// state-mirror contention (snapshot engine writes, UI reads, gang
/// propagation) cannot delay the byte forwarding here.
async fn raw_proxy_loop(
    console_socket: std::sync::Arc<tokio::net::UdpSocket>,
    console_addr: SocketAddr,
    ipad_socket: std::sync::Arc<tokio::net::UdpSocket>,
    ipad_addr: SocketAddr,
    capture_tx: mpsc::Sender<ProxyCapture>,
    offline_mode: Arc<AtomicBool>,
    cancel: tokio_util::sync::CancellationToken,
) {
    info!(%ipad_addr, "Mode 3 raw proxy started");

    let mut console_buf = vec![0u8; 65536];
    let mut ipad_buf = vec![0u8; 65536];
    let mut c2i: u64 = 0;
    let mut i2c: u64 = 0;
    let mut capture_drops: u64 = 0;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!(c2i, i2c, capture_drops, "Mode 3 proxy cancelled");
                break;
            }

            // Console → daemon → iPad
            result = console_socket.recv_from(&mut console_buf) => {
                match result {
                    Ok((size, _src)) => {
                        if offline_mode.load(Ordering::Relaxed) {
                            debug!("Mode 3 C→I dropped (offline mode)");
                            continue;
                        }

                        // Forward FIRST so state-mirror contention can't delay
                        // the byte path — capture happens off the hot path.
                        match ipad_socket.send_to(&console_buf[..size], ipad_addr).await {
                            Ok(sent) => {
                                c2i += 1;
                                if c2i <= 5 {
                                    debug!(c2i, sent, %ipad_addr, "Proxy C→I: forwarded {sent} bytes to {ipad_addr}");
                                }
                            }
                            Err(e) => {
                                warn!(%ipad_addr, "Proxy C→I: send failed: {e}");
                            }
                        }

                        // Best-effort enqueue for state capture. Channel-full
                        // means the capture task is lagging — drop the capture
                        // (mirror briefly stale) rather than block the proxy.
                        if capture_tx
                            .try_send(ProxyCapture {
                                bytes: console_buf[..size].to_vec(),
                                direction: "C→I",
                            })
                            .is_err()
                        {
                            capture_drops = capture_drops.saturating_add(1);
                            if capture_drops.is_power_of_two() {
                                warn!(capture_drops, "Mode 3 capture channel full — state mirror is lagging");
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
                    Ok((size, _src)) => {
                        if offline_mode.load(Ordering::Relaxed) {
                            debug!("Mode 3 I→C dropped (offline mode)");
                            continue;
                        }

                        // Forward FIRST. See C→I branch comment.
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

                        if capture_tx
                            .try_send(ProxyCapture {
                                bytes: ipad_buf[..size].to_vec(),
                                direction: "I→C",
                            })
                            .is_err()
                        {
                            capture_drops = capture_drops.saturating_add(1);
                            if capture_drops.is_power_of_two() {
                                warn!(capture_drops, "Mode 3 capture channel full — state mirror is lagging");
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
    macro_manager: &Arc<RwLock<MacroManager>>,
    snapshot_event_tx: &Option<mpsc::Sender<i32>>,
    osc_log: &Option<OscLog>,
) {
    // Try standard OSC first
    if let Ok((_, packet)) = rosc::decoder::decode_udp(data) {
        let messages = flatten_packet(packet);
        // Snapshot the config once for this packet's worth of messages.
        let config_snapshot = state.read().await.config.clone();
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
                    let old = state.write().await.update(addr.clone(), value.clone());
                    if let Some(prev) = &old {
                        if prev != value {
                            dirty_tracker.write().await.mark(addr);
                        }
                    }
                    // Feed Learn-mode recording (Mode 3 path). Without
                    // this, an S21 driven through the iPad proxy never
                    // hands captured parameter changes to the macro
                    // manager and Learn ends with zero steps.
                    {
                        let mut mgr = macro_manager.write().await;
                        if mgr.is_recording() {
                            mgr.record_change(addr.clone(), value.clone());
                        }
                    }
                }
                ParsedIpadMessage::SnapshotInfo { current } => {
                    debug!(
                        current,
                        "Proxy {direction}: console snapshot is now {current}"
                    );
                    let prev = {
                        let mut s = state.write().await;
                        let p = s.current_console_snapshot;
                        s.current_console_snapshot = Some(*current);
                        p
                    };
                    if prev != Some(*current) {
                        if let Some(tx) = snapshot_event_tx {
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
                    let mut s = state.write().await;
                    ipad_handshake::apply_config_message(&mut s.config, cfg);
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
        let hex: String = data[..data.len().min(32)]
            .iter()
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
