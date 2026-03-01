use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

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
        } else {
            debug!(
                name = %client.name,
                send_count = sends.len(),
                "Sent full state to monitor client"
            );
        }
    }

    /// Forward a send parameter change to the console via GP OSC (or iPad fallback).
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

        // Try GP OSC first
        match encode::encode_parameter(&addr, value) {
            Some((path, args)) => {
                if let Err(e) = self.sender.send(&path, args).await {
                    warn!(%addr, "Monitor: failed to send to console: {e}");
                    return false;
                }
                // Also update local state mirror
                self.state.write().await.update(addr, value.clone());
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
                            self.state.write().await.update(addr, value.clone());
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
            ParameterValue::Bool(b) => vec![rosc::OscType::Int(if *b { 1 } else { 0 })],
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

    /// PRD 5.7 step 5: Poll ConsoleState for send parameter changes and push updates.
    /// `last_send_state` tracks the previous values to detect changes.
    pub async fn poll_and_push_state_changes(
        &self,
        last_send_state: &mut HashMap<(u8, u8), (f32, f32, bool)>,
        manager: &MonitorManager,
        monitor_sender: &MonitorSender,
    ) {
        let state = self.state.read().await;

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

                // State changed — push to affected clients
                last_send_state.insert(key, new_state);

                for client in manager.clients.values() {
                    if !client.is_connected() || !client.permitted_auxes.contains(&aux) {
                        continue;
                    }
                    let input_visible = client.visible_inputs.is_empty()
                        || client.visible_inputs.contains(&input);
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
                                    rosc::OscType::Int(if on { 1 } else { 0 }),
                                ],
                            )
                            .await;
                    }
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
