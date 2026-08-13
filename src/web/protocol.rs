//! Browser monitor WebSocket JSON protocol — 1:1 with the OSC monitor
//! contract, same units (level/fader = dB f32, pan −1..1, on/mute bool).
//!
//! Inbound [`ClientMsg`] maps to the existing [`MonitorCommand`] with an
//! `endpoint = Ws(id)`, so the engine treats web clients exactly like native
//! ones (same permission gate, console forward, echo). Outbound, each WS
//! connection filters the shared [`MonitorStateEvent`] broadcast by its
//! client's permissions and serializes to [`ServerMsg`].

use serde::{Deserialize, Serialize};

use crate::console::monitor_engine::SendParam;
use crate::console::monitor_event::{ClientStateSnapshot, MonitorStateEvent};
use crate::model::monitor::{ClientEndpoint, MonitorClient};
use crate::model::parameter::ParameterValue;
use crate::osc::monitor_server::MonitorCommand;

/// Which continuous send field is addressed (level or pan). `on` is carried by
/// its own message since it is a bool.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendField {
    Level,
    Pan,
}

/// Client → server messages (browser → daemon).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// First message: identify the profile and (if the profile requires one)
    /// present its PIN.
    Hello {
        name: String,
        #[serde(default)]
        pin: Option<String>,
    },
    /// Keep-alive; refreshes the connection's last-seen timestamp.
    Heartbeat,
    /// Ask for a fresh full-state snapshot (e.g. after a reconnect).
    RequestState,
    SetSend {
        input: u16,
        aux: u16,
        field: SendField,
        value: f32,
    },
    SetSendOn {
        input: u16,
        aux: u16,
        on: bool,
    },
    SetAuxFader {
        aux: u16,
        value: f32,
    },
    SetAuxMute {
        aux: u16,
        mute: bool,
    },
}

/// Why a `Hello` was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthErrorReason {
    /// No profile with that name exists.
    UnknownClient,
    /// The profile has a PIN but the client supplied none.
    PinRequired,
    /// The supplied PIN does not match.
    BadPin,
}

#[derive(Debug, Clone, Serialize)]
pub struct SendSnapshot {
    pub input: u16,
    pub aux: u16,
    pub level: f32,
    pub pan: f32,
    pub on: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NameEntry {
    pub channel: u16,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuxSnapshot {
    pub aux: u16,
    pub fader: f32,
    pub mute: bool,
}

/// Server → client messages (daemon → browser).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Sent right after a successful `Hello`.
    Welcome {
        client_name: String,
        permitted_auxes: Vec<u16>,
        visible_inputs: Vec<u16>,
        input_count: u16,
        console_connected: bool,
    },
    /// Full permitted-state snapshot.
    State {
        sends: Vec<SendSnapshot>,
        input_names: Vec<NameEntry>,
        aux_names: Vec<NameEntry>,
        aux_strips: Vec<AuxSnapshot>,
    },
    /// Incremental: a single continuous send field changed (peer echo).
    SendField {
        input: u16,
        aux: u16,
        field: SendField,
        value: f32,
    },
    /// Incremental: a send's on/off changed (peer echo).
    SendOn { input: u16, aux: u16, on: bool },
    /// Incremental: a send's full `(level, pan, on)` (poll push).
    Send {
        input: u16,
        aux: u16,
        level: f32,
        pan: f32,
        on: bool,
    },
    /// Incremental: an aux master's `(fader, mute)`.
    Aux { aux: u16, fader: f32, mute: bool },
    /// Console connection status changed / queried.
    ConsoleStatus { connected: bool },
    /// `Hello` was rejected.
    AuthError { reason: AuthErrorReason },
}

/// A connection's resolved identity + permissions, cached at `Hello` so the
/// outbound filter doesn't re-read the manager per event. `name` is the
/// profile's canonical name (case as stored), used for outbound commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientPerms {
    pub name: String,
    pub permitted_auxes: Vec<u16>,
    pub visible_inputs: Vec<u16>,
}

impl ClientPerms {
    fn permits_aux(&self, aux: u16) -> bool {
        self.permitted_auxes.contains(&aux)
    }
    fn input_visible(&self, input: u16) -> bool {
        self.visible_inputs.is_empty() || self.visible_inputs.contains(&input)
    }
}

/// Validate a `Hello` against the matched profile. `client` is the result of a
/// case-insensitive name lookup (`None` = no such profile); `pin` is what the
/// client supplied. Returns the resolved permissions on success.
pub fn authenticate(
    client: Option<&MonitorClient>,
    pin: Option<&str>,
) -> Result<ClientPerms, AuthErrorReason> {
    let client = client.ok_or(AuthErrorReason::UnknownClient)?;
    if let Some(required) = client.pin.as_deref() {
        match pin {
            None => return Err(AuthErrorReason::PinRequired),
            Some(p) if p == required => {}
            Some(_) => return Err(AuthErrorReason::BadPin),
        }
    }
    Ok(ClientPerms {
        name: client.name.clone(),
        permitted_auxes: client.permitted_auxes.clone(),
        visible_inputs: client.visible_inputs.clone(),
    })
}

/// Map an inbound message to a [`MonitorCommand`] for the engine. `Hello` and
/// `Heartbeat` carry no command (handled directly by the connection) → `None`.
pub fn to_command(msg: &ClientMsg, name: &str, endpoint: ClientEndpoint) -> Option<MonitorCommand> {
    let client_name = name.to_string();
    match msg {
        ClientMsg::RequestState => Some(MonitorCommand::RequestState {
            client_name,
            endpoint,
        }),
        ClientMsg::SetSend {
            input,
            aux,
            field,
            value,
        } => Some(match field {
            SendField::Level => MonitorCommand::SetSendLevel {
                client_name,
                input_ch: *input,
                aux_ch: *aux,
                value: *value,
                endpoint,
            },
            SendField::Pan => MonitorCommand::SetSendPan {
                client_name,
                input_ch: *input,
                aux_ch: *aux,
                value: *value,
                endpoint,
            },
        }),
        ClientMsg::SetSendOn { input, aux, on } => Some(MonitorCommand::SetSendOn {
            client_name,
            input_ch: *input,
            aux_ch: *aux,
            on: *on,
            endpoint,
        }),
        ClientMsg::SetAuxFader { aux, value } => Some(MonitorCommand::SetAuxFader {
            client_name,
            aux_ch: *aux,
            value: *value,
            endpoint,
        }),
        ClientMsg::SetAuxMute { aux, mute } => Some(MonitorCommand::SetAuxMute {
            client_name,
            aux_ch: *aux,
            mute: *mute,
            endpoint,
        }),
        ClientMsg::Hello { .. } | ClientMsg::Heartbeat => None,
    }
}

/// Build the full-state `ServerMsg::State` from an engine snapshot.
pub fn state_from_snapshot(snapshot: &ClientStateSnapshot) -> ServerMsg {
    ServerMsg::State {
        sends: snapshot
            .sends
            .iter()
            .map(|&(input, aux, level, pan, on)| SendSnapshot {
                input,
                aux,
                level,
                pan,
                on,
            })
            .collect(),
        input_names: snapshot
            .input_names
            .iter()
            .map(|(channel, name)| NameEntry {
                channel: *channel,
                name: name.clone(),
            })
            .collect(),
        aux_names: snapshot
            .aux_names
            .iter()
            .map(|(channel, name)| NameEntry {
                channel: *channel,
                name: name.clone(),
            })
            .collect(),
        aux_strips: snapshot
            .aux_strips
            .iter()
            .map(|&(aux, fader, mute)| AuxSnapshot { aux, fader, mute })
            .collect(),
    }
}

/// Translate a broadcast event into the `ServerMsg` this connection should
/// send, applying the same recipient rules the UDP fan-out uses. Returns
/// `None` when the event is not for this client.
pub fn event_to_server_msg(
    event: &MonitorStateEvent,
    me: ClientEndpoint,
    perms: &ClientPerms,
) -> Option<ServerMsg> {
    match event {
        MonitorStateEvent::ClientState { endpoint, snapshot } => {
            (*endpoint == me).then(|| state_from_snapshot(snapshot))
        }
        MonitorStateEvent::SendEcho {
            source,
            input,
            aux,
            param,
            value,
        } => {
            // Echo: skip the source; aux permission only (no visible filter),
            // matching the legacy echo semantics.
            if *source == me || !perms.permits_aux(*aux) {
                return None;
            }
            Some(match param {
                SendParam::Level => ServerMsg::SendField {
                    input: *input,
                    aux: *aux,
                    field: SendField::Level,
                    value: value_f32(value),
                },
                SendParam::Pan => ServerMsg::SendField {
                    input: *input,
                    aux: *aux,
                    field: SendField::Pan,
                    value: value_f32(value),
                },
                SendParam::On => ServerMsg::SendOn {
                    input: *input,
                    aux: *aux,
                    on: value_bool(value),
                },
            })
        }
        MonitorStateEvent::SendState {
            input,
            aux,
            level,
            pan,
            on,
        } => {
            if !perms.permits_aux(*aux) || !perms.input_visible(*input) {
                return None;
            }
            Some(ServerMsg::Send {
                input: *input,
                aux: *aux,
                level: *level,
                pan: *pan,
                on: *on,
            })
        }
        MonitorStateEvent::AuxState { aux, fader, mute } => {
            perms.permits_aux(*aux).then_some(ServerMsg::Aux {
                aux: *aux,
                fader: *fader,
                mute: *mute,
            })
        }
        MonitorStateEvent::ConsoleStatus {
            endpoint,
            connected,
        } => (*endpoint == me).then_some(ServerMsg::ConsoleStatus {
            connected: *connected,
        }),
        // UDP-native; browsers don't use discovery or the client-count query.
        MonitorStateEvent::Discovered { .. } | MonitorStateEvent::ClientCount { .. } => None,
    }
}

fn value_f32(v: &ParameterValue) -> f32 {
    match v {
        ParameterValue::Float(f) => *f,
        ParameterValue::Int(i) => *i as f32,
        _ => 0.0,
    }
}

fn value_bool(v: &ParameterValue) -> bool {
    match v {
        ParameterValue::Bool(b) => *b,
        ParameterValue::Int(i) => *i != 0,
        ParameterValue::Float(f) => *f != 0.0,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perms(name: &str, auxes: Vec<u16>, inputs: Vec<u16>) -> ClientPerms {
        ClientPerms {
            name: name.into(),
            permitted_auxes: auxes,
            visible_inputs: inputs,
        }
    }

    #[test]
    fn parses_hello_with_and_without_pin() {
        let with: ClientMsg =
            serde_json::from_str(r#"{"type":"hello","name":"Drummer","pin":"1234"}"#).unwrap();
        match with {
            ClientMsg::Hello { name, pin } => {
                assert_eq!(name, "Drummer");
                assert_eq!(pin.as_deref(), Some("1234"));
            }
            _ => panic!("expected Hello"),
        }
        let without: ClientMsg = serde_json::from_str(r#"{"type":"hello","name":"Keys"}"#).unwrap();
        assert!(matches!(without, ClientMsg::Hello { pin: None, .. }));
    }

    #[test]
    fn parses_and_maps_set_send() {
        let msg: ClientMsg = serde_json::from_str(
            r#"{"type":"set_send","input":5,"aux":1,"field":"level","value":-6.0}"#,
        )
        .unwrap();
        let cmd = to_command(&msg, "Drummer", ClientEndpoint::Ws(7)).unwrap();
        match cmd {
            MonitorCommand::SetSendLevel {
                client_name,
                input_ch,
                aux_ch,
                value,
                endpoint,
            } => {
                assert_eq!(client_name, "Drummer");
                assert_eq!((input_ch, aux_ch), (5, 1));
                assert!((value - -6.0).abs() < 1e-6);
                assert_eq!(endpoint, ClientEndpoint::Ws(7));
            }
            _ => panic!("expected SetSendLevel"),
        }
    }

    #[test]
    fn hello_and_heartbeat_map_to_no_command() {
        assert!(to_command(&ClientMsg::Heartbeat, "X", ClientEndpoint::Ws(1)).is_none());
        let hello = ClientMsg::Hello {
            name: "X".into(),
            pin: None,
        };
        assert!(to_command(&hello, "X", ClientEndpoint::Ws(1)).is_none());
    }

    #[test]
    fn authenticate_enforces_pin_only_when_set() {
        let no_pin = MonitorClient::new("Keys".into(), vec![1], vec![]);
        assert!(authenticate(Some(&no_pin), None).is_ok());
        assert!(authenticate(Some(&no_pin), Some("whatever")).is_ok());

        let mut with_pin = MonitorClient::new("Drummer".into(), vec![1], vec![]);
        with_pin.pin = Some("1234".into());
        assert_eq!(
            authenticate(Some(&with_pin), None),
            Err(AuthErrorReason::PinRequired)
        );
        assert_eq!(
            authenticate(Some(&with_pin), Some("0000")),
            Err(AuthErrorReason::BadPin)
        );
        assert!(authenticate(Some(&with_pin), Some("1234")).is_ok());

        assert_eq!(
            authenticate(None, None),
            Err(AuthErrorReason::UnknownClient)
        );
    }

    #[test]
    fn event_filter_respects_endpoint_and_permissions() {
        let me = ClientEndpoint::Ws(1);
        let other = ClientEndpoint::Ws(2);
        let p = perms("A", vec![1], vec![]);

        // Directed snapshot: delivered only to its endpoint.
        let snap = MonitorStateEvent::ClientState {
            endpoint: me,
            snapshot: ClientStateSnapshot {
                sends: vec![],
                input_names: vec![],
                aux_names: vec![],
                aux_strips: vec![],
            },
        };
        assert!(matches!(
            event_to_server_msg(&snap, me, &p),
            Some(ServerMsg::State { .. })
        ));
        let snap_other = MonitorStateEvent::ClientState {
            endpoint: other,
            snapshot: ClientStateSnapshot {
                sends: vec![],
                input_names: vec![],
                aux_names: vec![],
                aux_strips: vec![],
            },
        };
        assert!(event_to_server_msg(&snap_other, me, &p).is_none());

        // Poll push: permitted aux delivered, unpermitted aux dropped.
        let permitted = MonitorStateEvent::SendState {
            input: 5,
            aux: 1,
            level: -3.0,
            pan: 0.0,
            on: true,
        };
        assert!(matches!(
            event_to_server_msg(&permitted, me, &p),
            Some(ServerMsg::Send { aux: 1, .. })
        ));
        let unpermitted = MonitorStateEvent::SendState {
            input: 5,
            aux: 2,
            level: -3.0,
            pan: 0.0,
            on: true,
        };
        assert!(event_to_server_msg(&unpermitted, me, &p).is_none());

        // Echo: not delivered back to the source.
        let echo_from_me = MonitorStateEvent::SendEcho {
            source: me,
            input: 5,
            aux: 1,
            param: SendParam::Level,
            value: ParameterValue::Float(-6.0),
        };
        assert!(event_to_server_msg(&echo_from_me, me, &p).is_none());
        let echo_from_other = MonitorStateEvent::SendEcho {
            source: other,
            input: 5,
            aux: 1,
            param: SendParam::Level,
            value: ParameterValue::Float(-6.0),
        };
        assert!(matches!(
            event_to_server_msg(&echo_from_other, me, &p),
            Some(ServerMsg::SendField {
                field: SendField::Level,
                ..
            })
        ));
    }
}
