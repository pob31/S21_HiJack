//! Transport-neutral monitor output events + the UDP fan-out task.
//!
//! The [`MonitorEngine`](super::monitor_engine::MonitorEngine) no longer sends
//! OSC to clients directly. Instead every server→client output is published as
//! a neutral [`MonitorStateEvent`] on a `tokio::sync::broadcast`. Two kinds of
//! subscriber consume it:
//!
//! 1. [`run_udp_fanout`] — reproduces today's exact OSC packets to the
//!    connected native/UDP clients (so the UDP path is byte-identical to
//!    before the bridge existed).
//! 2. Each WebSocket connection (added in a later phase) — filters by its
//!    client's permissions and serializes to JSON.
//!
//! Permission/visibility filtering lives in the subscribers, not the engine:
//! broadcast events (`SendEcho`/`SendState`/`AuxState`) carry only the change,
//! and each transport decides which of *its* clients should receive it.
//! Directed events (`ClientState`/`Discovered`/`ConsoleStatus`/`ClientCount`)
//! carry the target [`ClientEndpoint`]; a transport ignores them unless the
//! endpoint is one of its own.

use std::sync::Arc;

use rosc::OscType;
use tokio::sync::{RwLock, broadcast};
use tracing::{debug, warn};

use crate::model::monitor::ClientEndpoint;
use crate::model::parameter::ParameterValue;
use crate::osc::monitor_server::MonitorSender;

use super::monitor_engine::SendParam;
use super::monitor_manager::MonitorManager;

/// Full per-client state snapshot — the reply to a `Connect`/`RequestState`.
/// Field order matches the legacy send order in `send_client_state`: sends,
/// then input names, then aux names, then aux strips.
#[derive(Clone, Debug)]
pub struct ClientStateSnapshot {
    /// `(input, aux, level, pan, on)` for every permitted send.
    pub sends: Vec<(u8, u8, f32, f32, bool)>,
    /// `(input, name)` for each visible input that has a name.
    pub input_names: Vec<(u8, String)>,
    /// `(aux, name)` for each permitted aux that has a name.
    pub aux_names: Vec<(u8, String)>,
    /// `(aux, fader, mute)` for each permitted aux.
    pub aux_strips: Vec<(u8, f32, bool)>,
}

/// A transport-neutral server→client output, published to the broadcast and
/// consumed by the UDP fan-out task and each WebSocket connection.
#[derive(Clone, Debug)]
pub enum MonitorStateEvent {
    /// Full snapshot directed at one client (reply to `Connect`/`RequestState`).
    ClientState {
        endpoint: ClientEndpoint,
        snapshot: ClientStateSnapshot,
    },
    /// A single send field changed, command-driven. Echoed to every connected
    /// client permitted for `aux`, **except** the originating `source`.
    /// UDP path: `/monitor/state/send/{input}/{aux}/{level|pan|on}` `[value]`.
    SendEcho {
        source: ClientEndpoint,
        input: u8,
        aux: u8,
        param: SendParam,
        value: ParameterValue,
    },
    /// A send's full `(level, pan, on)`, poll-driven. Sent to every connected
    /// client permitted for `aux` with `input` visible.
    /// UDP path: `/monitor/state/send/{input}/{aux}` `[level, pan, on]`.
    SendState {
        input: u8,
        aux: u8,
        level: f32,
        pan: f32,
        on: bool,
    },
    /// An aux master's `(fader, mute)`, poll-driven. Sent to every connected
    /// client permitted for `aux`. UDP path: `/monitor/state/aux/{aux}` `[fader, mute]`.
    AuxState { aux: u8, fader: f32, mute: bool },
    /// Discovery reply (UDP-native; WS ignores). The UDP port is filled in by
    /// the fan-out from its own socket. Path: `/monitor/discovered` `[name, port]`.
    Discovered {
        endpoint: ClientEndpoint,
        console_name: String,
    },
    /// Console-status reply to a pull query. Path: `/status/console` `[0|1]`.
    ConsoleStatus {
        endpoint: ClientEndpoint,
        connected: bool,
    },
    /// Client-count reply to a pull query. Path: `/status/clients` `[count]`.
    ClientCount {
        endpoint: ClientEndpoint,
        count: i32,
    },
}

/// UDP fan-out task: drains the broadcast and reproduces today's OSC packets to
/// the connected native clients. One per monitor-server instance. Stops when
/// every `broadcast::Sender` is dropped (i.e. the engine loop ends).
pub async fn run_udp_fanout(
    mut events: broadcast::Receiver<MonitorStateEvent>,
    sender: MonitorSender,
    manager: Arc<RwLock<MonitorManager>>,
) {
    loop {
        match events.recv().await {
            Ok(event) => dispatch_udp(event, &sender, &manager).await,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                // Under extreme burst a slow drainer can fall behind; for UDP
                // the next console change re-pushes, so just note the gap.
                warn!(
                    skipped = n,
                    "Monitor UDP fan-out lagged; dropped state updates (next change re-syncs)"
                );
            }
            Err(broadcast::error::RecvError::Closed) => {
                debug!("Monitor UDP fan-out: event channel closed, stopping");
                break;
            }
        }
    }
}

/// Reproduce one event as UDP/OSC, byte-identical to the pre-bridge engine.
async fn dispatch_udp(
    event: MonitorStateEvent,
    sender: &MonitorSender,
    manager: &Arc<RwLock<MonitorManager>>,
) {
    match event {
        MonitorStateEvent::ClientState { endpoint, snapshot } => {
            let ClientEndpoint::Udp(addr) = endpoint else {
                return;
            };
            // Order matches the legacy send_client_state: sends, input names,
            // aux names, then aux strips.
            let _ = sender.send_client_state(addr, &snapshot.sends).await;
            for (input, name) in &snapshot.input_names {
                let _ = sender
                    .send_to(
                        addr,
                        &format!("/monitor/state/name/input/{input}"),
                        vec![OscType::String(name.clone())],
                    )
                    .await;
            }
            for (aux, name) in &snapshot.aux_names {
                let _ = sender
                    .send_to(
                        addr,
                        &format!("/monitor/state/name/aux/{aux}"),
                        vec![OscType::String(name.clone())],
                    )
                    .await;
            }
            for &(aux, fader, mute) in &snapshot.aux_strips {
                let _ = sender
                    .send_to(
                        addr,
                        &format!("/monitor/state/aux/{aux}"),
                        vec![OscType::Float(fader), OscType::Bool(mute)],
                    )
                    .await;
            }
        }
        MonitorStateEvent::SendEcho {
            source,
            input,
            aux,
            param,
            value,
        } => {
            let suffix = match param {
                SendParam::Level => "level",
                SendParam::Pan => "pan",
                SendParam::On => "on",
            };
            let path = format!("/monitor/state/send/{input}/{aux}/{suffix}");
            let args = match value {
                ParameterValue::Float(f) => vec![OscType::Float(f)],
                ParameterValue::Bool(b) => vec![OscType::Bool(b)],
                ParameterValue::Int(i) => vec![OscType::Int(i)],
                ParameterValue::String(s) => vec![OscType::String(s)],
            };
            // Legacy echo recipient rule: connected + aux-permitted, excluding
            // the source. Note it deliberately does NOT test `visible_inputs`.
            let mgr = manager.read().await;
            for client in mgr.clients.values() {
                let Some(ClientEndpoint::Udp(addr)) = client.endpoint else {
                    continue;
                };
                if client.endpoint == Some(source) {
                    continue;
                }
                if !client.is_connected() || !client.permitted_auxes.contains(&aux) {
                    continue;
                }
                let _ = sender.send_to(addr, &path, args.clone()).await;
            }
        }
        MonitorStateEvent::SendState {
            input,
            aux,
            level,
            pan,
            on,
        } => {
            let path = format!("/monitor/state/send/{input}/{aux}");
            let mgr = manager.read().await;
            for client in mgr.clients.values() {
                let Some(ClientEndpoint::Udp(addr)) = client.endpoint else {
                    continue;
                };
                if !client.is_connected() || !client.permitted_auxes.contains(&aux) {
                    continue;
                }
                let input_visible =
                    client.visible_inputs.is_empty() || client.visible_inputs.contains(&input);
                if !input_visible {
                    continue;
                }
                let _ = sender
                    .send_to(
                        addr,
                        &path,
                        vec![
                            OscType::Float(level),
                            OscType::Float(pan),
                            OscType::Bool(on),
                        ],
                    )
                    .await;
            }
        }
        MonitorStateEvent::AuxState { aux, fader, mute } => {
            let path = format!("/monitor/state/aux/{aux}");
            let mgr = manager.read().await;
            for client in mgr.clients.values() {
                let Some(ClientEndpoint::Udp(addr)) = client.endpoint else {
                    continue;
                };
                if !client.is_connected() || !client.permitted_auxes.contains(&aux) {
                    continue;
                }
                let _ = sender
                    .send_to(
                        addr,
                        &path,
                        vec![OscType::Float(fader), OscType::Bool(mute)],
                    )
                    .await;
            }
        }
        MonitorStateEvent::Discovered {
            endpoint,
            console_name,
        } => {
            let ClientEndpoint::Udp(addr) = endpoint else {
                return;
            };
            let port = sender.local_port();
            let _ = sender
                .send_to(
                    addr,
                    "/monitor/discovered",
                    vec![OscType::String(console_name), OscType::Int(port as i32)],
                )
                .await;
        }
        MonitorStateEvent::ConsoleStatus {
            endpoint,
            connected,
        } => {
            let ClientEndpoint::Udp(addr) = endpoint else {
                return;
            };
            let _ = sender
                .send_to(
                    addr,
                    "/status/console",
                    vec![OscType::Int(if connected { 1 } else { 0 })],
                )
                .await;
        }
        MonitorStateEvent::ClientCount { endpoint, count } => {
            let ClientEndpoint::Udp(addr) = endpoint else {
                return;
            };
            let _ = sender
                .send_to(addr, "/status/clients", vec![OscType::Int(count)])
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::monitor::MonitorClient;
    use crate::osc::monitor_server::MonitorServer;
    use rosc::OscPacket;
    use std::time::{Duration, Instant};
    use tokio::net::UdpSocket;

    /// Receive one OSC message on `sock`, or `None` if nothing arrives within
    /// 200 ms (long enough on loopback to be a reliable "nothing was sent").
    async fn recv_osc(sock: &UdpSocket) -> Option<OscPacket> {
        let mut buf = [0u8; 4096];
        match tokio::time::timeout(Duration::from_millis(200), sock.recv_from(&mut buf)).await {
            Ok(Ok((n, _))) => rosc::decoder::decode_udp(&buf[..n]).ok().map(|(_, p)| p),
            _ => None,
        }
    }

    fn msg(packet: &OscPacket) -> (&str, &[OscType]) {
        match packet {
            OscPacket::Message(m) => (m.addr.as_str(), m.args.as_slice()),
            _ => panic!("expected an OSC message, got a bundle"),
        }
    }

    /// The UDP fan-out reproduces the legacy recipient rules: a command echo
    /// reaches permitted peers but not its source; a poll push reaches every
    /// permitted client and skips clients lacking the aux permission.
    #[tokio::test]
    async fn udp_fanout_reproduces_echo_and_poll_recipients() {
        // Two simulated native clients on loopback.
        let sock_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sock_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = sock_a.local_addr().unwrap();
        let addr_b = sock_b.local_addr().unwrap();

        // Manager: A permits aux 1+2, B permits aux 1 only; both connected.
        let mut mgr = MonitorManager::new();
        let mut a = MonitorClient::new("A".into(), vec![1, 2], vec![]);
        a.endpoint = Some(ClientEndpoint::Udp(addr_a));
        a.last_seen = Some(Instant::now());
        let mut b = MonitorClient::new("B".into(), vec![1], vec![]);
        b.endpoint = Some(ClientEndpoint::Udp(addr_b));
        b.last_seen = Some(Instant::now());
        mgr.add_client(a);
        mgr.add_client(b);
        let manager = Arc::new(RwLock::new(mgr));

        // A reply socket for the fan-out (its own UDP port).
        let (sender, _rx) = MonitorServer::start("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();

        let (tx, rx) = broadcast::channel(64);
        tokio::spawn(run_udp_fanout(rx, sender, manager.clone()));

        // 1. Echo from A on aux 1 → B receives it, A (source) does not.
        tx.send(MonitorStateEvent::SendEcho {
            source: ClientEndpoint::Udp(addr_a),
            input: 5,
            aux: 1,
            param: SendParam::Level,
            value: ParameterValue::Float(-6.0),
        })
        .unwrap();
        let got_b = recv_osc(&sock_b).await.expect("B should receive the echo");
        let (path, args) = msg(&got_b);
        assert_eq!(path, "/monitor/state/send/5/1/level");
        assert!(matches!(args, [OscType::Float(f)] if (*f - -6.0).abs() < 1e-6));
        assert!(
            recv_osc(&sock_a).await.is_none(),
            "source A must not receive its own echo"
        );

        // 2. Poll push on aux 2 → only A is permitted; B must not receive it.
        tx.send(MonitorStateEvent::SendState {
            input: 7,
            aux: 2,
            level: -3.0,
            pan: 0.5,
            on: true,
        })
        .unwrap();
        let got_a = recv_osc(&sock_a)
            .await
            .expect("A should receive aux-2 push");
        let (path, args) = msg(&got_a);
        assert_eq!(path, "/monitor/state/send/7/2");
        assert!(
            matches!(args, [OscType::Float(l), OscType::Float(p), OscType::Bool(o)]
                if (*l - -3.0).abs() < 1e-6 && (*p - 0.5).abs() < 1e-6 && *o)
        );
        assert!(
            recv_osc(&sock_b).await.is_none(),
            "B is not permitted aux 2"
        );

        // 3. Poll push on aux 1 → both A and B receive it.
        tx.send(MonitorStateEvent::SendState {
            input: 7,
            aux: 1,
            level: -9.0,
            pan: 0.0,
            on: false,
        })
        .unwrap();
        assert_eq!(
            msg(&recv_osc(&sock_a).await.unwrap()).0,
            "/monitor/state/send/7/1"
        );
        assert_eq!(
            msg(&recv_osc(&sock_b).await.unwrap()).0,
            "/monitor/state/send/7/1"
        );
    }
}
