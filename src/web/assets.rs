//! Embedded static assets for the browser monitor UI.
//!
//! The contents of `web/dist` are embedded at compile time via `rust-embed`,
//! so the binary is self-contained — no filesystem dependency, no Node. The
//! handler serves the requested path, falling back to `index.html` (so the
//! single-page app and deep-links work), with the MIME type guessed from the
//! extension.

use axum::{
    http::{Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist"]
struct Assets;

/// Serve an embedded asset for `uri`, falling back to `index.html`.
pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    serve(path)
        .or_else(|| serve("index.html"))
        .unwrap_or_else(|| (axum::http::StatusCode::NOT_FOUND, "not found").into_response())
}

fn serve(path: &str) -> Option<Response> {
    let file = Assets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Some(
        (
            [(header::CONTENT_TYPE, mime.to_string())],
            file.data.into_owned(),
        )
            .into_response(),
    )
}
