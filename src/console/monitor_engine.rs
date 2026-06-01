use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, broadcast};
use tracing::{debug, info, warn};

use crate::model::channel::ChannelId;
use crate::model::monitor::MonitorClient;
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::encode;
use crate::osc::ipad_client::IpadSender;
use crate::osc::ipad_encode;
use crate::osc::monitor_server::MonitorCommand;

use super::monitor_event::{ClientStateSnapshot, MonitorStateEvent};
use super::monitor_manager::MonitorManager;

/// Which send parameter is being changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendParam {
    Level,
    Pan,
    On,
}

/// Processes monitoring client commands: validates permissions, forwards to
/// the console, and publishes transport-neutral [`MonitorStateEvent`]s for the
/// client-facing transports (the UDP fan-out today, WebSocket later) to deliver.
pub struct MonitorEngine {
    state: Arc<RwLock<ConsoleState>>,
    sender: OscSender,
    ipad_sender: Option<IpadSender>,
    /// Publishes server→client output; subscribed by the UDP fan-out task and
    /// (later) each WebSocket connection. See [`super::monitor_event`].
    events: broadcast::Sender<MonitorStateEvent>,
}

impl MonitorEngine {
    pub fn new(
        state: Arc<RwLock<ConsoleState>>,
        sender: OscSender,
        events: broadcast::Sender<MonitorStateEvent>,
    ) -> Self {
        Self {
            state,
            sender,
            ipad_sender: None,
            events,
        }
    }

    pub fn set_ipad_sender(&mut self, sender: Option<IpadSender>) {
        self.ipad_sender = sender;
    }

    /// Publish a server→client event to all transports. Best-effort: a send
    /// error just means there are currently no subscribers.
    fn publish(&self, event: MonitorStateEvent) {
        let _ = self.events.send(event);
    }

    /// Handle a single monitor command.
    pub async fn handle_command(
        &self,
        cmd: MonitorCommand,
        manager: &mut MonitorManager,
        console_connected: bool,
    ) {
        match cmd {
            MonitorCommand::Connect {
                client_name,
                endpoint,
            } => {
                if manager.find_by_name(&client_name).is_none() {
                    warn!(name = %client_name, "Monitor connect: unknown client");
                    return;
                }
                manager.update_last_seen(&client_name, endpoint);
                info!(name = %client_name, ?endpoint, "Monitor client connected");

                // Publish full permitted state to the connecting client.
                if let Some(client) = manager.find_by_name(&client_name) {
                    let client = client.clone();
                    self.publish_client_state(&client).await;
                }
            }
            MonitorCommand::RequestState {
                client_name,
                endpoint,
            } => {
                manager.update_last_seen(&client_name, endpoint);
                if let Some(client) = manager.find_by_name(&client_name) {
                    let client = client.clone();
                    self.publish_client_state(&client).await;
                } else {
                    warn!(name = %client_name, "Monitor state: unknown client");
                }
            }
            MonitorCommand::SetSendLevel {
                client_name,
                input_ch,
                aux_ch,
                value,
                endpoint,
            } => {
                manager.update_last_seen(&client_name, endpoint);
                self.handle_send_change(
                    &client_name,
                    input_ch,
                    aux_ch,
                    SendParam::Level,
                    ParameterValue::Float(value),
                    manager,
                )
                .await;
            }
            MonitorCommand::SetSendPan {
                client_name,
                input_ch,
                aux_ch,
                value,
                endpoint,
            } => {
                manager.update_last_seen(&client_name, endpoint);
                self.handle_send_change(
                    &client_name,
                    input_ch,
                    aux_ch,
                    SendParam::Pan,
                    ParameterValue::Float(value),
                    manager,
                )
                .await;
            }
            MonitorCommand::SetSendOn {
                client_name,
                input_ch,
                aux_ch,
                on,
                endpoint,
            } => {
                manager.update_last_seen(&client_name, endpoint);
                self.handle_send_change(
                    &client_name,
                    input_ch,
                    aux_ch,
                    SendParam::On,
                    ParameterValue::Bool(on),
                    manager,
                )
                .await;
            }
            MonitorCommand::SetAuxFader {
                client_name,
                aux_ch,
                value,
                endpoint,
            } => {
                manager.update_last_seen(&client_name, endpoint);
                self.handle_aux_change(
                    &client_name,
                    aux_ch,
                    "fader",
                    ParameterValue::Float(value),
                    manager,
                )
                .await;
            }
            MonitorCommand::SetAuxMute {
                client_name,
                aux_ch,
                mute,
                endpoint,
            } => {
                manager.update_last_seen(&client_name, endpoint);
                self.handle_aux_change(
                    &client_name,
                    aux_ch,
                    "mute",
                    ParameterValue::Bool(mute),
                    manager,
                )
                .await;
            }
            MonitorCommand::Discover { endpoint } => {
                let state = self.state.read().await;
                let name = if state.config.console_name.is_empty() {
                    "S21_HiJack".to_string()
                } else {
                    state.config.console_name.clone()
                };
                drop(state);
                self.publish(MonitorStateEvent::Discovered {
                    endpoint,
                    console_name: name,
                });
                info!(?endpoint, "Monitor discovery reply queued");
            }
            MonitorCommand::QueryConsoleStatus { endpoint } => {
                self.publish(MonitorStateEvent::ConsoleStatus {
                    endpoint,
                    connected: console_connected,
                });
            }
            MonitorCommand::QueryClientCount { endpoint } => {
                let count = manager.connected_count() as i32;
                self.publish(MonitorStateEvent::ClientCount { endpoint, count });
            }
        }
    }

    /// Handle an aux output parameter change (fader or mute).
    async fn handle_aux_change(
        &self,
        client_name: &str,
        aux_ch: u8,
        param: &str,
        value: ParameterValue,
        manager: &MonitorManager,
    ) {
        let client = match manager.find_by_name(client_name) {
            Some(c) => c,
            None => {
                warn!(name = %client_name, "Monitor aux: unknown client");
                return;
            }
        };
        // Aux fader/mute changes only involve an aux channel — there is no
        // input channel, so check `permitted_auxes` directly rather than
        // going through `is_permitted`, which would also test `visible_inputs`.
        if !client.permitted_auxes.contains(&aux_ch) {
            warn!(name = %client_name, aux_ch, "Monitor aux: not permitted");
            return;
        }

        let addr = match param {
            "fader" => ParameterAddress {
                channel: ChannelId::Aux(aux_ch),
                parameter: ParameterPath::Fader,
            },
            "mute" => ParameterAddress {
                channel: ChannelId::Aux(aux_ch),
                parameter: ParameterPath::Mute,
            },
            _ => return,
        };

        // Optimistic mirror update *before* the send, so the 20Hz
        // poll-and-push loop pushing to other clients sees the operator's
        // intent without waiting for the network round-trip. UDP sendto
        // rarely fails in practice; on failure the next echo from the
        // console (or another operator action) heals any discrepancy.
        self.state.write().await.update(addr.clone(), value.clone());

        // Forward to console via GP OSC (with iPad fallback)
        match encode::encode_parameter(&addr, &value) {
            Some((path, args)) => {
                if let Err(e) = self.sender.send(&path, args).await {
                    warn!(%addr, "Monitor aux: send failed: {e}");
                }
            }
            None => {
                if let Some(ref ipad) = self.ipad_sender
                    && let Some((path, args)) = ipad_encode::encode_ipad_parameter(&addr, &value)
                {
                    let _ = ipad.send(&path, args).await;
                }
            }
        }
    }

    /// Process a send parameter change: validate, forward, echo.
    async fn handle_send_change(
        &self,
        client_name: &str,
        input_ch: u8,
        aux_ch: u8,
        param: SendParam,
        value: ParameterValue,
        manager: &MonitorManager,
    ) {
        // Validate, and capture the originating endpoint so the echo can skip
        // the source. `update_last_seen` ran just before this, so a connected
        // client has its endpoint set.
        let source = match manager.find_by_name(client_name) {
            Some(c) => {
                if !c.is_permitted(input_ch, aux_ch) {
                    warn!(
                        name = %client_name, input_ch, aux_ch,
                        "Monitor send change: permission denied"
                    );
                    return;
                }
                c.endpoint
            }
            None => {
                warn!(name = %client_name, "Monitor send change: unknown client");
                return;
            }
        };

        // Forward to console.
        let forwarded = self
            .forward_send_change(input_ch, aux_ch, param, &value)
            .await;

        if forwarded {
            debug!(
                name = %client_name, input_ch, aux_ch, ?param,
                "Monitor: forwarded send change to console"
            );

            // Echo to other connected clients with overlapping aux permissions.
            // The fan-out/WS subscribers apply the per-client permission filter
            // and skip the source endpoint.
            if let Some(source) = source {
                self.publish(MonitorStateEvent::SendEcho {
                    source,
                    input: input_ch,
                    aux: aux_ch,
                    param,
                    value,
                });
            }
        }
    }

    /// Build and publish the full permitted-state snapshot for a client
    /// (reply to `Connect`/`RequestState`). The transport delivers it to the
    /// client identified by `client.endpoint`.
    async fn publish_client_state(&self, client: &MonitorClient) {
        let Some(endpoint) = client.endpoint else {
            return;
        };

        let state = self.state.read().await;

        // Determine input range
        let inputs: Vec<u8> = if client.visible_inputs.is_empty() {
            (1..=60).collect() // All inputs
        } else {
            client.visible_inputs.clone()
        };

        let mut sends = Vec::new();
        for &input in &inputs {
            for &aux in &client.permitted_auxes {
                let level = state
                    .get(&ParameterAddress {
                        channel: ChannelId::Input(input),
                        parameter: ParameterPath::SendLevel(aux),
                    })
                    .and_then(|v| v.as_float())
                    .unwrap_or(0.0);

                let pan = state
                    .get(&ParameterAddress {
                        channel: ChannelId::Input(input),
                        parameter: ParameterPath::SendPan(aux),
                    })
                    .and_then(|v| v.as_float())
                    .unwrap_or(0.0);

                let on = state
                    .get(&ParameterAddress {
                        channel: ChannelId::Input(input),
                        parameter: ParameterPath::SendEnabled(aux),
                    })
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                sends.push((input, aux, level, pan, on));
            }
        }

        // Channel names for inputs and permitted auxes (only those that have one).
        let mut input_names = Vec::new();
        for &input in &inputs {
            if let Some(ParameterValue::String(s)) = state.get(&ParameterAddress {
                channel: ChannelId::Input(input),
                parameter: ParameterPath::Name,
            }) {
                input_names.push((input, s.clone()));
            }
        }
        let mut aux_names = Vec::new();
        for &aux in &client.permitted_auxes {
            if let Some(ParameterValue::String(s)) = state.get(&ParameterAddress {
                channel: ChannelId::Aux(aux),
                parameter: ParameterPath::Name,
            }) {
                aux_names.push((aux, s.clone()));
            }
        }

        // Aux fader/mute for each permitted aux.
        let mut aux_strips = Vec::new();
        for &aux in &client.permitted_auxes {
            let fader = state
                .get(&ParameterAddress {
                    channel: ChannelId::Aux(aux),
                    parameter: ParameterPath::Fader,
                })
                .and_then(|v| v.as_float())
                .unwrap_or(-150.0);
            let mute = state
                .get(&ParameterAddress {
                    channel: ChannelId::Aux(aux),
                    parameter: ParameterPath::Mute,
                })
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            aux_strips.push((aux, fader, mute));
        }
        drop(state);

        let send_count = sends.len();
        self.publish(MonitorStateEvent::ClientState {
            endpoint,
            snapshot: ClientStateSnapshot {
                sends,
                input_names,
                aux_names,
                aux_strips,
            },
        });

        debug!(
            name = %client.name,
            send_count,
            "Published full state to monitor client"
        );
    }

    /// Forward a send parameter change to the console via GP OSC (or iPad fallback).
    ///
    /// Optimistically updates the state mirror *before* the network send so
    /// concurrent readers (notably the 20Hz poll-and-push) reflect the
    /// operator's intent immediately. The bool return indicates whether
    /// the send-to-console actually succeeded — used to gate the immediate
    /// echo to other clients (other clients still see the change via the
    /// poll-and-push within 50 ms, even if the immediate echo is skipped).
    async fn forward_send_change(
        &self,
        input_ch: u8,
        aux_ch: u8,
        param: SendParam,
        value: &ParameterValue,
    ) -> bool {
        let parameter = match param {
            SendParam::Level => ParameterPath::SendLevel(aux_ch),
            SendParam::Pan => ParameterPath::SendPan(aux_ch),
            SendParam::On => ParameterPath::SendEnabled(aux_ch),
        };
        let addr = ParameterAddress {
            channel: ChannelId::Input(input_ch),
            parameter,
        };

        // Optimistic mirror update — see contract comment above.
        self.state.write().await.update(addr.clone(), value.clone());

        // Try GP OSC first
        match encode::encode_parameter(&addr, value) {
            Some((path, args)) => {
                if let Err(e) = self.sender.send(&path, args).await {
                    warn!(%addr, "Monitor: failed to send to console: {e}");
                    return false;
                }
                true
            }
            None => {
                // Try iPad protocol fallback
                if let Some(ref ipad) = self.ipad_sender {
                    match ipad_encode::encode_ipad_parameter(&addr, value) {
                        Some((path, args)) => {
                            if let Err(e) = ipad.send(&path, args).await {
                                warn!(%addr, "Monitor: iPad send failed: {e}");
                                return false;
                            }
                            true
                        }
                        None => {
                            warn!(%addr, "Monitor: cannot encode send parameter");
                            false
                        }
                    }
                } else {
                    warn!(%addr, "Monitor: no sender available for parameter");
                    false
                }
            }
        }
    }

    /// PRD 5.7 step 5: Poll ConsoleState for send + aux parameter changes and push updates.
    /// Uses a generation counter to skip scanning when nothing changed.
    /// Per-parameter rate limited to 20Hz via `last_push_times`.
    pub async fn poll_and_push_state_changes(
        &self,
        last_send_state: &mut HashMap<(u8, u8), (f32, f32, bool)>,
        last_aux_state: &mut HashMap<u8, (f32, bool)>,
        last_generation: &mut u64,
        last_push_times: &mut HashMap<(u8, u8), std::time::Instant>,
        last_aux_push_times: &mut HashMap<u8, std::time::Instant>,
        manager: &MonitorManager,
    ) {
        let state = self.state.read().await;

        // Skip entirely if nothing changed since last poll
        let current_gen = state.generation();
        if current_gen == *last_generation {
            return;
        }
        *last_generation = current_gen;

        let now = std::time::Instant::now();
        let min_interval = std::time::Duration::from_millis(50); // 20Hz per parameter

        // Collect all auxes and inputs of interest from connected clients
        let mut auxes_of_interest = std::collections::HashSet::new();
        let mut inputs_of_interest = std::collections::HashSet::new();
        let mut has_all_inputs = false;

        for client in manager.clients.values() {
            if !client.is_connected() {
                continue;
            }
            for &aux in &client.permitted_auxes {
                auxes_of_interest.insert(aux);
            }
            if client.visible_inputs.is_empty() {
                has_all_inputs = true;
            } else {
                for &input in &client.visible_inputs {
                    inputs_of_interest.insert(input);
                }
            }
        }

        if auxes_of_interest.is_empty() {
            return;
        }

        let inputs: Vec<u8> = if has_all_inputs {
            (1..=60).collect()
        } else {
            inputs_of_interest.into_iter().collect()
        };

        for &input in &inputs {
            for &aux in &auxes_of_interest {
                let level = state
                    .get(&ParameterAddress {
                        channel: ChannelId::Input(input),
                        parameter: ParameterPath::SendLevel(aux),
                    })
                    .and_then(|v| v.as_float())
                    .unwrap_or(0.0);

                let pan = state
                    .get(&ParameterAddress {
                        channel: ChannelId::Input(input),
                        parameter: ParameterPath::SendPan(aux),
                    })
                    .and_then(|v| v.as_float())
                    .unwrap_or(0.0);

                let on = state
                    .get(&ParameterAddress {
                        channel: ChannelId::Input(input),
                        parameter: ParameterPath::SendEnabled(aux),
                    })
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let key = (input, aux);
                let new_state = (level, pan, on);

                if let Some(old) = last_send_state.get(&key) {
                    if *old == new_state {
                        continue; // No change
                    }
                }

                // Per-parameter 20Hz rate limit
                if let Some(last_push) = last_push_times.get(&key) {
                    if now.duration_since(*last_push) < min_interval {
                        continue;
                    }
                }

                // State changed — publish once; the transports fan it out to
                // each permitted+visible client.
                last_send_state.insert(key, new_state);
                last_push_times.insert(key, now);

                self.publish(MonitorStateEvent::SendState {
                    input,
                    aux,
                    level,
                    pan,
                    on,
                });
            }
        }

        // Poll aux fader/mute for permitted auxes
        for &aux in &auxes_of_interest {
            let fader_val = state.get(&ParameterAddress {
                channel: ChannelId::Aux(aux),
                parameter: ParameterPath::Fader,
            });
            let mute_val = state.get(&ParameterAddress {
                channel: ChannelId::Aux(aux),
                parameter: ParameterPath::Mute,
            });

            let fader = fader_val.and_then(|v| v.as_float()).unwrap_or(-150.0);
            let mute = mute_val.and_then(|v| v.as_bool()).unwrap_or(false);

            let new_aux_state = (fader, mute);
            if let Some(old) = last_aux_state.get(&aux) {
                if (old.0 - new_aux_state.0).abs() < 0.001 && old.1 == new_aux_state.1 {
                    continue;
                }
            }

            // Per-parameter 20Hz rate limit
            if let Some(last_push) = last_aux_push_times.get(&aux) {
                if now.duration_since(*last_push) < min_interval {
                    continue;
                }
            }

            last_aux_state.insert(aux, new_aux_state);
            last_aux_push_times.insert(aux, now);

            self.publish(MonitorStateEvent::AuxState { aux, fader, mute });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::ConsoleConfig;
    use crate::model::monitor::{ClientEndpoint, MonitorClient};
    use std::net::SocketAddr;
    use std::time::Instant;

    /// Create a test engine with a dummy sender that will fail to send
    /// (we don't have a real console, but we can test the logic).
    fn make_test_engine() -> MonitorEngine {
        let state = Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default())));
        // Create a sender that points to localhost — sends will fail but that's fine for tests
        let rt = tokio::runtime::Runtime::new().unwrap();
        let sender = rt.block_on(async {
            let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let console_addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
            OscSender::new(std::sync::Arc::new(socket), console_addr)
        });
        let (events, _events_rx) = broadcast::channel(16);
        MonitorEngine::new(state, sender, events)
    }

    fn make_manager_with_clients() -> MonitorManager {
        let mut mgr = MonitorManager::new();

        let mut drummer = MonitorClient::new("Drummer".into(), vec![1, 2], vec![]);
        drummer.endpoint = Some(ClientEndpoint::Udp("192.168.1.100:9000".parse().unwrap()));
        drummer.last_seen = Some(Instant::now());
        mgr.add_client(drummer);

        let keys = MonitorClient::new("Keys".into(), vec![3], vec![1, 2, 3]);
        mgr.add_client(keys);

        mgr
    }

    #[test]
    fn permission_denied_for_unpermitted_aux() {
        let mgr = make_manager_with_clients();
        // Keys has aux 3 only — aux 1 should be denied
        let keys = mgr.find_by_name("Keys").unwrap();
        assert!(!keys.is_permitted(1, 1));
    }

    #[test]
    fn permission_allowed_for_permitted() {
        let mgr = make_manager_with_clients();
        let drummer = mgr.find_by_name("Drummer").unwrap();
        // Drummer has aux 1,2 and all inputs
        assert!(drummer.is_permitted(5, 1));
        assert!(drummer.is_permitted(30, 2));
    }

    #[test]
    fn unknown_client_not_found() {
        let mgr = make_manager_with_clients();
        assert!(mgr.find_by_name("Unknown").is_none());
    }

    #[test]
    fn send_param_to_address_mapping() {
        // Verify our parameter path construction
        let addr = ParameterAddress {
            channel: ChannelId::Input(5),
            parameter: ParameterPath::SendLevel(2),
        };
        let value = ParameterValue::Float(-10.0);
        let encoded = encode::encode_parameter(&addr, &value);
        assert!(encoded.is_some());
        let (path, _) = encoded.unwrap();
        assert!(path.contains("send"));
    }

    #[test]
    fn echo_skips_source_and_disconnected() {
        let mgr = make_manager_with_clients();
        // Only Drummer is connected (Keys is not)
        // Echo from Drummer should not target Drummer or disconnected Keys
        let connected: Vec<_> = mgr
            .clients
            .values()
            .filter(|c| {
                c.is_connected()
                    && !c.name.eq_ignore_ascii_case("Drummer")
                    && c.permitted_auxes.contains(&1)
            })
            .collect();
        assert!(connected.is_empty());
    }

    /// Consume and discard every datagram queued on `sock` (used to flush the
    /// connect-time state snapshot before asserting on later traffic).
    async fn drain(sock: &tokio::net::UdpSocket) {
        let mut buf = [0u8; 4096];
        while tokio::time::timeout(
            std::time::Duration::from_millis(150),
            sock.recv_from(&mut buf),
        )
        .await
        .is_ok()
        {}
    }

    /// Receive one OSC message's address path, or `None` if nothing arrives in
    /// 200 ms (reliable on loopback as a "nothing was sent" check).
    async fn recv_path(sock: &tokio::net::UdpSocket) -> Option<String> {
        let mut buf = [0u8; 4096];
        match tokio::time::timeout(
            std::time::Duration::from_millis(200),
            sock.recv_from(&mut buf),
        )
        .await
        {
            Ok(Ok((n, _))) => match rosc::decoder::decode_udp(&buf[..n]) {
                Ok((_, rosc::OscPacket::Message(m))) => Some(m.addr),
                _ => None,
            },
            _ => None,
        }
    }

    /// End-to-end coexistence gate (no web server): two native clients sharing
    /// aux 1. A move on client A forwards to the console and echoes to client B
    /// but not back to A — the pre-bridge behaviour, now via the broadcast +
    /// UDP fan-out.
    #[tokio::test]
    async fn end_to_end_command_forwards_and_echoes_to_peer_not_source() {
        use crate::console::monitor_event::{MonitorStateEvent, run_udp_fanout};
        use crate::osc::monitor_server::MonitorServer;
        use std::time::Duration;
        use tokio::net::UdpSocket;

        // A real "console" socket so the GP-OSC forward succeeds — the echo
        // only fires when the forward succeeds.
        let console_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let console_addr = console_sock.local_addr().unwrap();
        let local_sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let console_sender = OscSender::new(local_sock, console_addr);

        let state = Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default())));
        let (events_tx, _) = broadcast::channel::<MonitorStateEvent>(256);
        let engine = MonitorEngine::new(state, console_sender, events_tx.clone());

        // Two native clients; both permit aux 1, all inputs.
        let client_a = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let client_b = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr_a = client_a.local_addr().unwrap();
        let addr_b = client_b.local_addr().unwrap();

        let manager = Arc::new(RwLock::new(MonitorManager::new()));
        {
            let mut mgr = manager.write().await;
            mgr.add_client(MonitorClient::new("A".into(), vec![1], vec![]));
            mgr.add_client(MonitorClient::new("B".into(), vec![1], vec![]));
        }

        let (monitor_sender, _rx) = MonitorServer::start("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        tokio::spawn(run_udp_fanout(
            events_tx.subscribe(),
            monitor_sender,
            manager.clone(),
        ));

        // Both clients connect (sets endpoint + triggers a full-state snapshot).
        {
            let mut mgr = manager.write().await;
            engine
                .handle_command(
                    MonitorCommand::Connect {
                        client_name: "A".into(),
                        endpoint: ClientEndpoint::Udp(addr_a),
                    },
                    &mut mgr,
                    true,
                )
                .await;
            engine
                .handle_command(
                    MonitorCommand::Connect {
                        client_name: "B".into(),
                        endpoint: ClientEndpoint::Udp(addr_b),
                    },
                    &mut mgr,
                    true,
                )
                .await;
        }

        // The snapshot reaches A, then flush both clients' snapshot bursts.
        let mut buf = [0u8; 4096];
        assert!(
            tokio::time::timeout(Duration::from_millis(300), client_a.recv_from(&mut buf))
                .await
                .is_ok(),
            "A should receive a state snapshot on connect"
        );
        drain(&client_a).await;
        drain(&client_b).await;

        // A moves a send on aux 1.
        {
            let mut mgr = manager.write().await;
            engine
                .handle_command(
                    MonitorCommand::SetSendLevel {
                        client_name: "A".into(),
                        input_ch: 5,
                        aux_ch: 1,
                        value: -6.0,
                        endpoint: ClientEndpoint::Udp(addr_a),
                    },
                    &mut mgr,
                    true,
                )
                .await;
        }

        // Console received the forwarded change.
        assert!(
            tokio::time::timeout(Duration::from_millis(300), console_sock.recv_from(&mut buf))
                .await
                .is_ok(),
            "console should receive the forwarded send change"
        );

        // B received the echo; A (the source) did not.
        let b_path = recv_path(&client_b)
            .await
            .expect("B should receive the echo");
        assert_eq!(b_path, "/monitor/state/send/5/1/level");
        assert!(
            recv_path(&client_a).await.is_none(),
            "source A must not receive its own echo"
        );
    }
}
