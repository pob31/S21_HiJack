use std::net::SocketAddr;
use std::sync::Arc;

use rosc::{OscMessage, OscPacket, OscType};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::model::cidr::{self, Ipv4Cidr};
use crate::model::monitor::ClientEndpoint;

/// Commands parsed from incoming monitoring client OSC messages.
#[derive(Debug)]
pub enum MonitorCommand {
    /// Client connecting (acts as heartbeat).
    Connect {
        client_name: String,
        endpoint: ClientEndpoint,
    },
    /// Client requests its full permitted state.
    RequestState {
        client_name: String,
        endpoint: ClientEndpoint,
    },
    /// Set a send level: `/monitor/{name}/send/{input}/{aux}/level {value}`
    SetSendLevel {
        client_name: String,
        input_ch: u8,
        aux_ch: u8,
        value: f32,
        endpoint: ClientEndpoint,
    },
    /// Set a send pan: `/monitor/{name}/send/{input}/{aux}/pan {value}`
    SetSendPan {
        client_name: String,
        input_ch: u8,
        aux_ch: u8,
        value: f32,
        endpoint: ClientEndpoint,
    },
    /// Set a send on/off: `/monitor/{name}/send/{input}/{aux}/on {0|1}`
    SetSendOn {
        client_name: String,
        input_ch: u8,
        aux_ch: u8,
        on: bool,
        endpoint: ClientEndpoint,
    },
    /// Set aux output fader: `/monitor/{name}/aux/{aux}/fader {value}`
    SetAuxFader {
        client_name: String,
        aux_ch: u8,
        value: f32,
        endpoint: ClientEndpoint,
    },
    /// Set aux output mute: `/monitor/{name}/aux/{aux}/mute {0|1}`
    SetAuxMute {
        client_name: String,
        aux_ch: u8,
        mute: bool,
        endpoint: ClientEndpoint,
    },
    /// Discovery: `/monitor/discover` — app broadcasts to find daemons on the LAN.
    Discover { endpoint: ClientEndpoint },
    /// PRD 6.4: `/status/console` — query console connection status.
    QueryConsoleStatus { endpoint: ClientEndpoint },
    /// PRD 6.4: `/status/clients` — query connected monitoring client count.
    QueryClientCount { endpoint: ClientEndpoint },
}

/// Binds a UDP socket and spawns a receive loop for monitoring clients.
pub struct MonitorServer;

impl MonitorServer {
    /// Start the monitor server on the given address.
    /// Returns a sender handle (for replies) and a receiver channel for commands.
    pub async fn start(
        listen_addr: SocketAddr,
    ) -> std::io::Result<(MonitorSender, mpsc::Receiver<MonitorCommand>)> {
        Self::start_with_cancel(listen_addr, CancellationToken::new(), None, Vec::new()).await
    }

    /// Start the monitor server with a CancellationToken for clean shutdown
    /// and an optional source-IP CIDR allowlist. An empty allowlist accepts
    /// all sources (current behavior); a non-empty list drops packets from
    /// any source not matching at least one CIDR — see audit C2 for the
    /// closed-LAN deployment rationale.
    pub async fn start_with_cancel(
        listen_addr: SocketAddr,
        cancel: CancellationToken,
        interface_name: Option<&str>,
        allowlist: Vec<Ipv4Cidr>,
    ) -> std::io::Result<(MonitorSender, mpsc::Receiver<MonitorCommand>)> {
        let socket = Arc::new(
            crate::ui::net_interfaces::create_bound_udp_socket(listen_addr, interface_name).await?,
        );
        let (tx, rx) = mpsc::channel(256);

        if allowlist.is_empty() {
            info!(%listen_addr, "Monitor server started (no source allowlist)");
        } else {
            info!(
                %listen_addr,
                allowlist_size = allowlist.len(),
                "Monitor server started with CIDR allowlist"
            );
        }

        let sender = MonitorSender {
            socket: socket.clone(),
        };

        tokio::spawn(listen_loop(socket, tx, cancel, allowlist));

        Ok((sender, rx))
    }
}

/// Handle for sending OSC replies back to monitoring clients.
#[derive(Clone)]
pub struct MonitorSender {
    socket: Arc<UdpSocket>,
}

impl MonitorSender {
    /// Local UDP port the monitor server is bound to. Returned in the
    /// `/monitor/discovered` reply so clients can autodiscover both the
    /// daemon's IP (from the datagram source) and its monitor port.
    pub fn local_port(&self) -> u16 {
        self.socket.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    /// Send an OSC message to a monitoring client.
    pub async fn send_to(
        &self,
        addr: SocketAddr,
        path: &str,
        args: Vec<OscType>,
    ) -> std::io::Result<()> {
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
        self.socket.send_to(&buf, addr).await?;
        Ok(())
    }

    /// Send the full state of a client's permitted sends.
    /// Each tuple: (input_ch, aux_ch, level, pan, on)
    pub async fn send_client_state(
        &self,
        addr: SocketAddr,
        sends: &[(u8, u8, f32, f32, bool)],
    ) -> std::io::Result<()> {
        for &(input, aux, level, pan, on) in sends {
            self.send_to(
                addr,
                &format!("/monitor/state/send/{input}/{aux}"),
                vec![
                    OscType::Float(level),
                    OscType::Float(pan),
                    OscType::Bool(on),
                ],
            )
            .await?;
        }
        Ok(())
    }
}

/// Background receive loop.
async fn listen_loop(
    socket: Arc<UdpSocket>,
    tx: mpsc::Sender<MonitorCommand>,
    cancel: CancellationToken,
    allowlist: Vec<Ipv4Cidr>,
) {
    let mut buf = vec![0u8; 4096];
    let mut blocked = 0u64;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("Monitor server cancelled — releasing port");
                break;
            }
            result = socket.recv_from(&mut buf) => {
                match result {
                    Ok((size, src)) => {
                        if !cidr::ip_allowed(src.ip(), &allowlist) {
                            blocked = blocked.saturating_add(1);
                            // Surface at exponentially-spaced thresholds so a
                            // chatty mis-configured peer doesn't flood logs.
                            if blocked.is_power_of_two() {
                                warn!(%src, blocked, "Monitor server: dropped packet (source not in CIDR allowlist)");
                            }
                            continue;
                        }
                        match rosc::decoder::decode_udp(&buf[..size]) {
                            Ok((_, packet)) => {
                                process_packet(packet, src, &tx).await;
                            }
                            Err(e) => {
                                warn!("Monitor server: failed to decode OSC from {src}: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        error!("Monitor server: UDP receive error: {e}");
                        break;
                    }
                }
            }
        }
    }
    drop(socket);
}

async fn process_packet(packet: OscPacket, src: SocketAddr, tx: &mpsc::Sender<MonitorCommand>) {
    match packet {
        OscPacket::Message(msg) => {
            if let Some(cmd) = parse_monitor_message(&msg.addr, &msg.args, src) {
                debug!(path = msg.addr, "Monitor command received");
                if tx.send(cmd).await.is_err() {
                    error!("Monitor command channel closed");
                }
            } else {
                debug!(path = msg.addr, %src, "Unknown monitor message");
            }
        }
        OscPacket::Bundle(bundle) => {
            for p in bundle.content {
                Box::pin(process_packet(p, src, tx)).await;
            }
        }
    }
}

/// Parse a monitor OSC message into a MonitorCommand.
///
/// Supported paths:
/// - `/monitor/{name}/connect`
/// - `/monitor/{name}/state`
/// - `/monitor/{name}/send/{input}/{aux}/level {f32}`
/// - `/monitor/{name}/send/{input}/{aux}/pan {f32}`
/// - `/monitor/{name}/send/{input}/{aux}/on {0|1}`
/// - `/status/console`
/// - `/status/clients`
pub fn parse_monitor_message(
    path: &str,
    args: &[OscType],
    src: SocketAddr,
) -> Option<MonitorCommand> {
    // Status endpoints (PRD 6.4)
    if path == "/status/console" {
        return Some(MonitorCommand::QueryConsoleStatus {
            endpoint: ClientEndpoint::Udp(src),
        });
    }
    if path == "/status/clients" {
        return Some(MonitorCommand::QueryClientCount {
            endpoint: ClientEndpoint::Udp(src),
        });
    }

    // Discovery: /monitor/discover (no client name)
    if path == "/monitor/discover" {
        return Some(MonitorCommand::Discover {
            endpoint: ClientEndpoint::Udp(src),
        });
    }

    // Monitor paths: /monitor/{name}/...
    let rest = path.strip_prefix("/monitor/")?;
    let (name, action) = rest.split_once('/')?;

    if name.is_empty() {
        return None;
    }

    let client_name = name.to_string();

    match action {
        "connect" => Some(MonitorCommand::Connect {
            client_name,
            endpoint: ClientEndpoint::Udp(src),
        }),
        "state" => Some(MonitorCommand::RequestState {
            client_name,
            endpoint: ClientEndpoint::Udp(src),
        }),
        _ => {
            // Try aux path: aux/{aux}/{param}
            if let Some(aux_rest) = action.strip_prefix("aux/") {
                let (aux_str, param) = aux_rest.split_once('/')?;
                let aux_ch: u8 = aux_str.parse().ok()?;
                return match param {
                    "fader" => {
                        let value = match args.first() {
                            Some(OscType::Float(f)) => *f,
                            Some(OscType::Int(i)) => *i as f32,
                            _ => return None,
                        };
                        Some(MonitorCommand::SetAuxFader {
                            client_name,
                            aux_ch,
                            value,
                            endpoint: ClientEndpoint::Udp(src),
                        })
                    }
                    "mute" => {
                        let mute = match args.first() {
                            Some(OscType::Bool(b)) => *b,
                            Some(OscType::Int(i)) => *i != 0,
                            Some(OscType::Float(f)) => *f != 0.0,
                            _ => return None,
                        };
                        Some(MonitorCommand::SetAuxMute {
                            client_name,
                            aux_ch,
                            mute,
                            endpoint: ClientEndpoint::Udp(src),
                        })
                    }
                    _ => None,
                };
            }

            // Try send path: send/{input}/{aux}/{param}
            let send_rest = action.strip_prefix("send/")?;
            let parts: Vec<&str> = send_rest.split('/').collect();
            if parts.len() != 3 {
                return None;
            }
            let input_ch: u8 = parts[0].parse().ok()?;
            let aux_ch: u8 = parts[1].parse().ok()?;
            let param = parts[2];

            match param {
                "level" => {
                    let value = match args.first() {
                        Some(OscType::Float(f)) => *f,
                        Some(OscType::Int(i)) => *i as f32,
                        _ => return None,
                    };
                    Some(MonitorCommand::SetSendLevel {
                        client_name,
                        input_ch,
                        aux_ch,
                        value,
                        endpoint: ClientEndpoint::Udp(src),
                    })
                }
                "pan" => {
                    let value = match args.first() {
                        Some(OscType::Float(f)) => *f,
                        Some(OscType::Int(i)) => *i as f32,
                        _ => return None,
                    };
                    Some(MonitorCommand::SetSendPan {
                        client_name,
                        input_ch,
                        aux_ch,
                        value,
                        endpoint: ClientEndpoint::Udp(src),
                    })
                }
                "on" => {
                    let on = match args.first() {
                        Some(OscType::Bool(b)) => *b,
                        Some(OscType::Int(i)) => *i != 0,
                        Some(OscType::Float(f)) => *f != 0.0,
                        _ => return None,
                    };
                    Some(MonitorCommand::SetSendOn {
                        client_name,
                        input_ch,
                        aux_ch,
                        on,
                        endpoint: ClientEndpoint::Udp(src),
                    })
                }
                _ => None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src() -> SocketAddr {
        "192.168.1.100:9000".parse().unwrap()
    }

    #[test]
    fn parse_connect() {
        let cmd = parse_monitor_message("/monitor/drummer/connect", &[], src()).unwrap();
        match cmd {
            MonitorCommand::Connect {
                client_name,
                endpoint,
            } => {
                assert_eq!(client_name, "drummer");
                assert_eq!(endpoint, ClientEndpoint::Udp(src()));
            }
            _ => panic!("Expected Connect"),
        }
    }

    #[test]
    fn parse_request_state() {
        let cmd = parse_monitor_message("/monitor/keys/state", &[], src()).unwrap();
        match cmd {
            MonitorCommand::RequestState {
                client_name,
                endpoint,
            } => {
                assert_eq!(client_name, "keys");
                assert_eq!(endpoint, ClientEndpoint::Udp(src()));
            }
            _ => panic!("Expected RequestState"),
        }
    }

    #[test]
    fn parse_send_level() {
        let cmd = parse_monitor_message(
            "/monitor/drummer/send/1/1/level",
            &[OscType::Float(0.75)],
            src(),
        )
        .unwrap();
        match cmd {
            MonitorCommand::SetSendLevel {
                client_name,
                input_ch,
                aux_ch,
                value,
                ..
            } => {
                assert_eq!(client_name, "drummer");
                assert_eq!(input_ch, 1);
                assert_eq!(aux_ch, 1);
                assert!((value - 0.75).abs() < 0.001);
            }
            _ => panic!("Expected SetSendLevel"),
        }
    }

    #[test]
    fn parse_send_pan() {
        let cmd =
            parse_monitor_message("/monitor/bass/send/5/2/pan", &[OscType::Float(-0.5)], src())
                .unwrap();
        match cmd {
            MonitorCommand::SetSendPan {
                client_name,
                input_ch,
                aux_ch,
                value,
                ..
            } => {
                assert_eq!(client_name, "bass");
                assert_eq!(input_ch, 5);
                assert_eq!(aux_ch, 2);
                assert!((value - (-0.5)).abs() < 0.001);
            }
            _ => panic!("Expected SetSendPan"),
        }
    }

    #[test]
    fn parse_send_on() {
        let cmd = parse_monitor_message("/monitor/vocals/send/3/1/on", &[OscType::Int(1)], src())
            .unwrap();
        match cmd {
            MonitorCommand::SetSendOn {
                client_name,
                input_ch,
                aux_ch,
                on,
                ..
            } => {
                assert_eq!(client_name, "vocals");
                assert_eq!(input_ch, 3);
                assert_eq!(aux_ch, 1);
                assert!(on);
            }
            _ => panic!("Expected SetSendOn"),
        }
    }

    #[test]
    fn parse_send_on_off() {
        let cmd = parse_monitor_message("/monitor/vocals/send/3/1/on", &[OscType::Int(0)], src())
            .unwrap();
        match cmd {
            MonitorCommand::SetSendOn { on, .. } => assert!(!on),
            _ => panic!("Expected SetSendOn"),
        }
    }

    #[test]
    fn parse_status_console() {
        let cmd = parse_monitor_message("/status/console", &[], src()).unwrap();
        assert!(matches!(cmd, MonitorCommand::QueryConsoleStatus { .. }));
    }

    #[test]
    fn parse_status_clients() {
        let cmd = parse_monitor_message("/status/clients", &[], src()).unwrap();
        assert!(matches!(cmd, MonitorCommand::QueryClientCount { .. }));
    }

    #[test]
    fn parse_unknown_returns_none() {
        assert!(parse_monitor_message("/unknown/path", &[], src()).is_none());
    }

    #[test]
    fn parse_invalid_monitor_path_returns_none() {
        // Empty name
        assert!(parse_monitor_message("/monitor//connect", &[], src()).is_none());
        // Missing action
        assert!(parse_monitor_message("/monitor/drummer", &[], src()).is_none());
        // Unknown action
        assert!(parse_monitor_message("/monitor/drummer/unknown", &[], src()).is_none());
        // Bad send format (missing parts)
        assert!(parse_monitor_message("/monitor/drummer/send/1/level", &[], src()).is_none());
    }

    #[test]
    fn parse_send_level_missing_arg_returns_none() {
        assert!(parse_monitor_message("/monitor/drummer/send/1/1/level", &[], src()).is_none());
    }
}
