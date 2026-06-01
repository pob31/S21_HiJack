//! Browser-based personal-monitoring surface (HTTP + WebSocket).
//!
//! Serves a (placeholder, for now) web app and a `/ws` WebSocket endpoint that
//! speaks the JSON protocol in [`protocol`]. Each WebSocket connection is just
//! another monitor client: inbound messages map to the existing
//! [`MonitorCommand`] stream (with a `Ws(id)` endpoint) and outbound state
//! comes from the shared [`MonitorStateEvent`](crate::console::monitor_event)
//! broadcast — so web and native clients echo to each other for free.
//!
//! Everything runs on spawned tasks; the egui thread never touches this.

pub mod protocol;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use axum::{
    Router,
    extract::{
        ConnectInfo, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use crate::console::monitor_event::MonitorStateEvent;
use crate::console::monitor_manager::MonitorManager;
use crate::model::cidr::{self, Ipv4Cidr};
use crate::model::monitor::ClientEndpoint;
use crate::model::state::ConsoleState;
use crate::osc::monitor_server::MonitorCommand;

/// Shared handles the web server needs to bridge browsers to the monitor
/// engine. Cheap to clone (all `Arc`/channel handles); used as axum router state.
#[derive(Clone)]
pub struct WebContext {
    /// Console mirror — read for the `Welcome` (input count, console status).
    pub state: Arc<RwLock<ConsoleState>>,
    /// Monitor profiles — read at login for auth + permissions.
    pub manager: Arc<RwLock<MonitorManager>>,
    /// Inbound command stream shared with the UDP server (the engine drains it).
    pub commands: mpsc::Sender<MonitorCommand>,
    /// Server→client state events; each connection subscribes.
    pub events: broadcast::Sender<MonitorStateEvent>,
    /// Console connection flag (for `Welcome.console_connected`).
    pub offline_mode: Arc<AtomicBool>,
    /// Monotonic source of per-connection ids (`ClientEndpoint::Ws`).
    pub conn_counter: Arc<AtomicU64>,
}

/// Start the web monitor server: static page + `/ws`, gated by a source-IP CIDR
/// allowlist, graceful shutdown on `cancel`. Binds the TCP listener before
/// returning so bind errors propagate to the caller.
pub async fn start_web_server(
    listen_addr: SocketAddr,
    cancel: CancellationToken,
    allowlist: Vec<Ipv4Cidr>,
    ctx: WebContext,
) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    if allowlist.is_empty() {
        info!(%listen_addr, "Web server started (no source allowlist)");
    } else {
        info!(
            %listen_addr,
            allowlist_size = allowlist.len(),
            "Web server started with CIDR allowlist"
        );
    }

    let app = Router::new()
        .route("/", get(placeholder))
        .route("/ws", get(ws_handler))
        .layer(middleware::from_fn_with_state(
            Arc::new(allowlist),
            cidr_guard,
        ))
        .with_state(ctx);

    tokio::spawn(async move {
        // ConnectInfo requires the connect-info make-service so the peer
        // SocketAddr is available to the CIDR middleware.
        let svc = app.into_make_service_with_connect_info::<SocketAddr>();
        if let Err(e) = axum::serve(listener, svc)
            .with_graceful_shutdown(async move { cancel.cancelled().await })
            .await
        {
            error!("Web server error: {e}");
        }
        info!("Web server stopped");
    });

    Ok(())
}

/// Reject any peer whose IP is not in the allowlist (empty allowlist = allow
/// all). Same semantics as the UDP servers' `cidr::ip_allowed` drop check,
/// applied to both the static routes and the WebSocket upgrade.
async fn cidr_guard(
    State(allowlist): State<Arc<Vec<Ipv4Cidr>>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    if cidr::ip_allowed(peer.ip(), &allowlist) {
        next.run(req).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

/// Placeholder landing page. Replaced by the embedded Svelte app in a later
/// phase; the monitor surface itself is driven over `/ws`.
async fn placeholder() -> impl IntoResponse {
    Html(
        "<!doctype html><meta charset=utf-8>\
         <title>S21 HiJack — Web Monitor</title>\
         <h1>S21 HiJack web monitor</h1>\
         <p>Server is running. The monitor mixer UI ships in a later phase.</p>",
    )
}

async fn ws_handler(ws: WebSocketUpgrade, State(ctx): State<WebContext>) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, ctx))
}

/// Serialize a server message to a WebSocket text frame.
fn text(msg: &protocol::ServerMsg) -> Message {
    Message::Text(serde_json::to_string(msg).unwrap_or_default().into())
}

/// Drive one browser connection: authenticate, then bridge it to the monitor
/// engine over the shared command channel + event broadcast.
async fn handle_ws(socket: WebSocket, ctx: WebContext) {
    let id = ctx.conn_counter.fetch_add(1, Ordering::Relaxed);
    let endpoint = ClientEndpoint::Ws(id);
    let (mut ws_tx, mut ws_rx) = socket.split();

    // 1. First frame must be a Hello.
    let (name, pin) = loop {
        match ws_rx.next().await {
            Some(Ok(Message::Text(t))) => match serde_json::from_str::<protocol::ClientMsg>(&t) {
                Ok(protocol::ClientMsg::Hello { name, pin }) => break (name, pin),
                _ => {
                    debug!(conn = id, "WS: first message was not a Hello; closing");
                    return;
                }
            },
            Some(Ok(Message::Close(_))) | None => return,
            Some(Ok(_)) => continue, // ignore ping/pong/binary before Hello
            Some(Err(_)) => return,
        }
    };

    // 2. Authenticate against the profile (name + optional PIN).
    let auth = {
        let mgr = ctx.manager.read().await;
        protocol::authenticate(mgr.find_by_name(&name), pin.as_deref())
    };
    let perms = match auth {
        Ok(p) => p,
        Err(reason) => {
            debug!(conn = id, name = %name, ?reason, "WS: auth rejected");
            let _ = ws_tx
                .send(text(&protocol::ServerMsg::AuthError { reason }))
                .await;
            return;
        }
    };
    info!(conn = id, name = %perms.name, "WS: monitor client connected");

    // 3. Welcome.
    let (input_count, console_connected) = {
        let st = ctx.state.read().await;
        (
            st.config.input_channel_count,
            !ctx.offline_mode.load(Ordering::Relaxed),
        )
    };
    let welcome = protocol::ServerMsg::Welcome {
        client_name: perms.name.clone(),
        permitted_auxes: perms.permitted_auxes.clone(),
        visible_inputs: perms.visible_inputs.clone(),
        input_count,
        console_connected,
    };
    if ws_tx.send(text(&welcome)).await.is_err() {
        return;
    }

    // 4. Subscribe BEFORE registering, so the snapshot the engine publishes in
    //    response to Connect is captured by this connection.
    let mut events_rx = ctx.events.subscribe();
    let _ = ctx
        .commands
        .send(MonitorCommand::Connect {
            client_name: perms.name.clone(),
            endpoint,
        })
        .await;

    // 5. Write task: broadcast events → filtered JSON → socket.
    let perms_w = perms.clone();
    let commands_w = ctx.commands.clone();
    let name_w = perms.name.clone();
    let mut write = tokio::spawn(async move {
        loop {
            match events_rx.recv().await {
                Ok(ev) => {
                    if let Some(msg) = protocol::event_to_server_msg(&ev, endpoint, &perms_w)
                        && ws_tx.send(text(&msg)).await.is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    // Fell behind: request a fresh snapshot to resync, never
                    // block peers or the desk.
                    debug!(conn = id, skipped = n, "WS: lagged, requesting resync");
                    let _ = commands_w
                        .send(MonitorCommand::RequestState {
                            client_name: name_w.clone(),
                            endpoint,
                        })
                        .await;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // 6. Read loop: socket → commands. Runs until the client goes away.
    loop {
        tokio::select! {
            // If the write task ends (socket send failed), tear down.
            _ = &mut write => break,
            frame = ws_rx.next() => {
                match frame {
                    Some(Ok(Message::Text(t))) => {
                        match serde_json::from_str::<protocol::ClientMsg>(&t) {
                            Ok(protocol::ClientMsg::Heartbeat)
                            | Ok(protocol::ClientMsg::Hello { .. }) => {
                                // Keep-alive: refresh liveness so the 20 Hz poll
                                // keeps scanning this client's auxes.
                                ctx.manager.write().await.update_last_seen(&perms.name, endpoint);
                            }
                            Ok(msg) => {
                                if let Some(cmd) = protocol::to_command(&msg, &perms.name, endpoint) {
                                    let _ = ctx.commands.send(cmd).await;
                                }
                            }
                            Err(_) => debug!(conn = id, "WS: ignoring malformed message"),
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // ping/pong/binary
                    Some(Err(_)) => break,
                }
            }
        }
    }

    write.abort();
    info!(conn = id, name = %perms.name, "WS: monitor client disconnected");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::monitor_event::ClientStateSnapshot;
    use crate::model::config::ConsoleConfig;
    use crate::model::monitor::MonitorClient;
    use std::time::Duration;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as TMsg;

    /// Read the next text frame as JSON, failing the test on timeout/non-text.
    async fn recv_json<S>(ws: &mut S) -> serde_json::Value
    where
        S: futures_util::Stream<Item = Result<TMsg, tokio_tungstenite::tungstenite::Error>> + Unpin,
    {
        loop {
            match tokio::time::timeout(Duration::from_secs(2), ws.next()).await {
                Ok(Some(Ok(TMsg::Text(t)))) => return serde_json::from_str(t.as_str()).unwrap(),
                Ok(Some(Ok(_))) => continue, // skip ping/pong
                other => panic!("expected a text frame, got {other:?}"),
            }
        }
    }

    /// End-to-end WebSocket gate: Hello → Welcome → State, a fader move flows
    /// through as a command, and the optional PIN is enforced.
    #[tokio::test]
    async fn ws_handshake_state_command_and_pin_auth() {
        // Two profiles: Drummer (no PIN), Keys (PIN "1234"); both permit aux 1.
        let mut mgr = MonitorManager::new();
        mgr.add_client(MonitorClient::new("Drummer".into(), vec![1], vec![]));
        let mut keys = MonitorClient::new("Keys".into(), vec![1], vec![]);
        keys.pin = Some("1234".into());
        mgr.add_client(keys);

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<MonitorCommand>(64);
        let (events_tx, _) = broadcast::channel::<MonitorStateEvent>(64);
        let ctx = WebContext {
            state: Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default()))),
            manager: Arc::new(RwLock::new(mgr)),
            commands: cmd_tx,
            events: events_tx.clone(),
            offline_mode: Arc::new(AtomicBool::new(false)),
            conn_counter: Arc::new(AtomicU64::new(0)),
        };

        // Grab a free port, then start the server on it.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        start_web_server(addr, CancellationToken::new(), Vec::new(), ctx)
            .await
            .unwrap();
        let url = format!("ws://127.0.0.1:{port}/ws");

        // Happy path: Drummer logs in (no PIN) → Welcome.
        let (mut ws, _) = connect_async(&url).await.unwrap();
        ws.send(TMsg::Text(r#"{"type":"hello","name":"Drummer"}"#.into()))
            .await
            .unwrap();
        let welcome = recv_json(&mut ws).await;
        assert_eq!(welcome["type"].as_str(), Some("welcome"));
        assert_eq!(welcome["client_name"].as_str(), Some("Drummer"));

        // The handler registers via a Connect command; play the engine and
        // publish a snapshot back to that endpoint.
        let endpoint = match cmd_rx.recv().await.unwrap() {
            MonitorCommand::Connect {
                client_name,
                endpoint,
            } => {
                assert_eq!(client_name, "Drummer");
                endpoint
            }
            other => panic!("expected Connect, got {other:?}"),
        };
        events_tx
            .send(MonitorStateEvent::ClientState {
                endpoint,
                snapshot: ClientStateSnapshot {
                    sends: vec![(5, 1, -6.0, 0.0, true)],
                    input_names: vec![],
                    aux_names: vec![],
                    aux_strips: vec![],
                },
            })
            .unwrap();
        let state = recv_json(&mut ws).await;
        assert_eq!(state["type"].as_str(), Some("state"));
        assert_eq!(state["sends"][0]["input"].as_u64(), Some(5));

        // A fader move flows through as a SetSendLevel command.
        ws.send(TMsg::Text(
            r#"{"type":"set_send","input":5,"aux":1,"field":"level","value":-3.0}"#.into(),
        ))
        .await
        .unwrap();
        match cmd_rx.recv().await.unwrap() {
            MonitorCommand::SetSendLevel {
                input_ch, aux_ch, ..
            } => assert_eq!((input_ch, aux_ch), (5, 1)),
            other => panic!("expected SetSendLevel, got {other:?}"),
        }

        // PIN enforced: rejected without it, accepted with the right one.
        let (mut ws_nopin, _) = connect_async(&url).await.unwrap();
        ws_nopin
            .send(TMsg::Text(r#"{"type":"hello","name":"Keys"}"#.into()))
            .await
            .unwrap();
        let err = recv_json(&mut ws_nopin).await;
        assert_eq!(err["type"].as_str(), Some("auth_error"));
        assert_eq!(err["reason"].as_str(), Some("pin_required"));

        let (mut ws_pin, _) = connect_async(&url).await.unwrap();
        ws_pin
            .send(TMsg::Text(
                r#"{"type":"hello","name":"Keys","pin":"1234"}"#.into(),
            ))
            .await
            .unwrap();
        let w = recv_json(&mut ws_pin).await;
        assert_eq!(w["type"].as_str(), Some("welcome"));
    }
}
