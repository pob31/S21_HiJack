use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use rosc::OscType;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::model::channel::ChannelId;
use crate::model::monitor::MonitorClient;
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::encode;
use crate::osc::ipad_client::IpadSender;
use crate::osc::ipad_encode;
use crate::osc::monitor_server::{MonitorCommand, MonitorSender};

use super::monitor_manager::MonitorManager;

/// Which send parameter is being changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendParam {
    Level,
    Pan,
    On,
}

/// Processes monitoring client commands: validates permissions, forwards to console,
/// echoes to other clients.
pub struct MonitorEngine {
    state: Arc<RwLock<ConsoleState>>,
    sender: OscSender,
    ipad_sender: Option<IpadSender>,
}

impl MonitorEngine {
    pub fn new(state: Arc<RwLock<ConsoleState>>, sender: OscSender) -> Self {
        Self {
            state,
            sender,
            ipad_sender: None,
        }
    }

    pub fn set_ipad_sender(&mut self, sender: Option<IpadSender>) {
        self.ipad_sender = sender;
    }

    /// Handle a single monitor command.
    pub async fn handle_command(
        &self,
        cmd: MonitorCommand,
        manager: &mut MonitorManager,
        monitor_sender: &MonitorSender,
        console_connected: bool,
    ) {
        match cmd {
            MonitorCommand::Connect {
                client_name,
                reply_addr,
            } => {
                if manager.find_by_name(&client_name).is_none() {
                    warn!(name = %client_name, "Monitor connect: unknown client");
                    return;
                }
                manager.update_last_seen(&client_name, reply_addr);
                info!(name = %client_name, %reply_addr, "Monitor client connected");

                // Send full permitted state
                if let Some(client) = manager.find_by_name(&client_name) {
                    let client = client.clone();
                    self.send_client_state(&client, monitor_sender).await;
                }
            }
            MonitorCommand::RequestState {
                client_name,
                reply_addr,
            } => {
                manager.update_last_seen(&client_name, reply_addr);
                if let Some(client) = manager.find_by_name(&client_name) {
                    let client = client.clone();
                    self.send_client_state(&client, monitor_sender).await;
                } else {
                    warn!(name = %client_name, "Monitor state: unknown client");
                }
            }
            MonitorCommand::SetSendLevel {
                client_name,
                input_ch,
                aux_ch,
                value,
                reply_addr,
            } => {
                manager.update_last_seen(&client_name, reply_addr);
                self.handle_send_change(
                    &client_name,
                    input_ch,
                    aux_ch,
                    SendParam::Level,
                    ParameterValue::Float(value),
                    manager,
                    monitor_sender,
                )
                .await;
            }
            MonitorCommand::SetSendPan {
                client_name,
                input_ch,
                aux_ch,
                value,
                reply_addr,
            } => {
                manager.update_last_seen(&client_name, reply_addr);
                self.handle_send_change(
                    &client_name,
                    input_ch,
                    aux_ch,
                    SendParam::Pan,
                    ParameterValue::Float(value),
                    manager,
                    monitor_sender,
                )
                .await;
            }
            MonitorCommand::SetSendOn {
                client_name,
                input_ch,
                aux_ch,
                on,
                reply_addr,
            } => {
                manager.update_last_seen(&client_name, reply_addr);
                self.handle_send_change(
                    &client_name,
                    input_ch,
                    aux_ch,
                    SendParam::On,
                    ParameterValue::Bool(on),
                    manager,
                    monitor_sender,
                )
                .await;
            }
            MonitorCommand::SetAuxFader {
                client_name,
                aux_ch,
                value,
                reply_addr,
            } => {
                manager.update_last_seen(&client_name, reply_addr);
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
                reply_addr,
            } => {
                manager.update_last_seen(&client_name, reply_addr);
                self.handle_aux_change(
                    &client_name,
                    aux_ch,
                    "mute",
                    ParameterValue::Bool(mute),
                    manager,
                )
                .await;
            }
            MonitorCommand::Discover { reply_addr } => {
                let state = self.state.read().await;
                let name = if state.config.console_name.is_empty() {
                    "S21_HiJack".to_string()
                } else {
                    state.config.console_name.clone()
                };
                drop(state);
                let _ = monitor_sender
                    .send_to(
                        reply_addr,
                        "/monitor/discovered",
                        vec![OscType::String(name)],
                    )
                    .await;
                info!(%reply_addr, "Monitor discovery reply sent");
            }
            MonitorCommand::QueryConsoleStatus { reply_addr } => {
                self.handle_status_console(reply_addr, console_connected, monitor_sender)
                    .await;
            }
            MonitorCommand::QueryClientCount { reply_addr } => {
                self.handle_status_clients(reply_addr, manager, monitor_sender)
                    .await;
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
        if !client.is_permitted(aux_ch, 1) {
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
        monitor_sender: &MonitorSender,
    ) {
        let client = match manager.find_by_name(client_name) {
            Some(c) => c,
            None => {
                warn!(name = %client_name, "Monitor send change: unknown client");
                return;
            }
        };

        if !client.is_permitted(input_ch, aux_ch) {
            warn!(
                name = %client_name, input_ch, aux_ch,
                "Monitor send change: permission denied"
            );
            return;
        }

        // Forward to console
        let forwarded = self
            .forward_send_change(input_ch, aux_ch, param, &value)
            .await;

        if forwarded {
            debug!(
                name = %client_name, input_ch, aux_ch, ?param,
                "Monitor: forwarded send change to console"
            );

            // Echo to other connected clients with overlapping aux permissions
            self.echo_to_other_clients(
                client_name,
                input_ch,
                aux_ch,
                param,
                &value,
                manager,
                monitor_sender,
            )
            .await;
        }
    }

    /// Send current state of all permitted sends to a client.
    async fn send_client_state(&self, client: &MonitorClient, monitor_sender: &MonitorSender) {
        let Some(addr) = client.connected_addr else {
            return;
        };

        let state = self.state.read().await;
        let mut sends = Vec::new();

        // Determine input range
        let inputs: Vec<u8> = if client.visible_inputs.is_empty() {
            (1..=60).collect() // All inputs
        } else {
            client.visible_inputs.clone()
        };

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

        if let Err(e) = monitor_sender.send_client_state(addr, &sends).await {
            warn!(name = %client.name, "Failed to send state: {e}");
        }

        // Send channel names for inputs and permitted auxes
        for &input in &inputs {
            if let Some(ParameterValue::String(s)) = state.get(&ParameterAddress {
                channel: ChannelId::Input(input),
                parameter: ParameterPath::Name,
            }) {
                let _ = monitor_sender
                    .send_to(
                        addr,
                        &format!("/monitor/state/name/input/{input}"),
                        vec![OscType::String(s.clone())],
                    )
                    .await;
            }
        }
        for &aux in &client.permitted_auxes {
            if let Some(ParameterValue::String(s)) = state.get(&ParameterAddress {
                channel: ChannelId::Aux(aux),
                parameter: ParameterPath::Name,
            }) {
                let _ = monitor_sender
                    .send_to(
                        addr,
                        &format!("/monitor/state/name/aux/{aux}"),
                        vec![OscType::String(s.clone())],
                    )
                    .await;
            }
        }

        // Send aux fader/mute for each permitted aux
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
            let _ = monitor_sender
                .send_to(
                    addr,
                    &format!("/monitor/state/aux/{aux}"),
                    vec![OscType::Float(fader), OscType::Bool(mute)],
                )
                .await;
        }

        debug!(
            name = %client.name,
            send_count = sends.len(),
            "Sent full state to monitor client"
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

    /// Echo a send change to all OTHER connected clients with overlapping aux permissions.
    async fn echo_to_other_clients(
        &self,
        source_name: &str,
        input_ch: u8,
        aux_ch: u8,
        param: SendParam,
        value: &ParameterValue,
        manager: &MonitorManager,
        monitor_sender: &MonitorSender,
    ) {
        let param_name = match param {
            SendParam::Level => "level",
            SendParam::Pan => "pan",
            SendParam::On => "on",
        };
        let path = format!("/monitor/state/send/{input_ch}/{aux_ch}/{param_name}");
        let args = match value {
            ParameterValue::Float(f) => vec![rosc::OscType::Float(*f)],
            ParameterValue::Bool(b) => vec![rosc::OscType::Bool(*b)],
            ParameterValue::Int(i) => vec![rosc::OscType::Int(*i)],
            ParameterValue::String(s) => vec![rosc::OscType::String(s.clone())],
        };

        for client in manager.clients.values() {
            // Skip the source client
            if client.name.eq_ignore_ascii_case(source_name) {
                continue;
            }
            // Only echo to connected clients with matching aux permission
            if !client.is_connected() || !client.permitted_auxes.contains(&aux_ch) {
                continue;
            }
            if let Some(addr) = client.connected_addr {
                let _ = monitor_sender.send_to(addr, &path, args.clone()).await;
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
        monitor_sender: &MonitorSender,
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

                // State changed — push to affected clients
                last_send_state.insert(key, new_state);
                last_push_times.insert(key, now);

                for client in manager.clients.values() {
                    if !client.is_connected() || !client.permitted_auxes.contains(&aux) {
                        continue;
                    }
                    let input_visible =
                        client.visible_inputs.is_empty() || client.visible_inputs.contains(&input);
                    if !input_visible {
                        continue;
                    }
                    if let Some(addr) = client.connected_addr {
                        let _ = monitor_sender
                            .send_to(
                                addr,
                                &format!("/monitor/state/send/{input}/{aux}"),
                                vec![
                                    rosc::OscType::Float(level),
                                    rosc::OscType::Float(pan),
                                    rosc::OscType::Bool(on),
                                ],
                            )
                            .await;
                    }
                }
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

            for client in manager.clients.values() {
                if !client.is_connected() || !client.permitted_auxes.contains(&aux) {
                    continue;
                }
                if let Some(addr) = client.connected_addr {
                    let _ = monitor_sender
                        .send_to(
                            addr,
                            &format!("/monitor/state/aux/{aux}"),
                            vec![rosc::OscType::Float(fader), rosc::OscType::Bool(mute)],
                        )
                        .await;
                }
            }
        }
    }

    /// PRD 6.4: Reply to `/status/console`.
    async fn handle_status_console(
        &self,
        reply_addr: SocketAddr,
        connected: bool,
        monitor_sender: &MonitorSender,
    ) {
        let _ = monitor_sender
            .send_to(
                reply_addr,
                "/status/console",
                vec![rosc::OscType::Int(if connected { 1 } else { 0 })],
            )
            .await;
    }

    /// PRD 6.4: Reply to `/status/clients`.
    async fn handle_status_clients(
        &self,
        reply_addr: SocketAddr,
        manager: &MonitorManager,
        monitor_sender: &MonitorSender,
    ) {
        let count = manager.connected_count() as i32;
        let _ = monitor_sender
            .send_to(
                reply_addr,
                "/status/clients",
                vec![rosc::OscType::Int(count)],
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::ConsoleConfig;
    use crate::model::monitor::MonitorClient;
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
        MonitorEngine::new(state, sender)
    }

    fn make_manager_with_clients() -> MonitorManager {
        let mut mgr = MonitorManager::new();

        let mut drummer = MonitorClient::new("Drummer".into(), vec![1, 2], vec![]);
        drummer.connected_addr = Some("192.168.1.100:9000".parse().unwrap());
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
}
