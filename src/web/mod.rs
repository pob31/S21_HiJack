//! Browser-based personal-monitoring surface (HTTP + WebSocket).
//!
//! Phase 1 (this module): a minimal `axum` HTTP server that serves a
//! placeholder page, gated by a source-IP CIDR allowlist, with graceful
//! shutdown driven by the shared [`CancellationToken`]. It mirrors the
//! lifecycle of [`crate::osc::monitor_server::MonitorServer::start_with_cancel`]
//! — bind first (so bind errors propagate to the caller), then spawn the serve
//! loop and return.
//!
//! Later phases widen this into the real web monitor: a `/ws` WebSocket
//! endpoint speaking a JSON protocol 1:1 with the OSC monitor contract, an
//! embedded Svelte UI, and the engine context (console state, monitor manager,
//! command channel, state-event broadcast). None of that is present yet — this
//! is pure scaffolding with no monitor-engine changes.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    extract::{ConnectInfo, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::model::cidr::{self, Ipv4Cidr};

/// Start the web monitor server: serves a placeholder page (Phase 1) gated by
/// a source-IP CIDR allowlist, with graceful shutdown on `cancel`. Binds the
/// TCP listener before returning so bind errors propagate to the caller
/// (mirrors [`crate::osc::monitor_server::MonitorServer::start_with_cancel`]).
///
/// An empty `allowlist` accepts all sources (current behaviour for the other
/// listeners); a non-empty list rejects any peer whose IP matches no CIDR with
/// `403 Forbidden`. See audit C2 for the closed-LAN deployment rationale.
pub async fn start_web_server(
    listen_addr: SocketAddr,
    cancel: CancellationToken,
    allowlist: Vec<Ipv4Cidr>,
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
        .layer(middleware::from_fn_with_state(
            Arc::new(allowlist),
            cidr_guard,
        ));

    tokio::spawn(async move {
        // `ConnectInfo` requires the connect-info make-service so the peer
        // `SocketAddr` is available to the CIDR middleware below.
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
/// expressed here as an axum middleware layer on the connection's peer IP.
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
/// phase; for now it just confirms the server is reachable.
async fn placeholder() -> impl IntoResponse {
    Html(
        "<!doctype html><meta charset=utf-8>\
         <title>S21 HiJack — Web Monitor</title>\
         <h1>S21 HiJack web monitor</h1>\
         <p>Server is running. The monitor mixer UI ships in a later phase.</p>",
    )
}
