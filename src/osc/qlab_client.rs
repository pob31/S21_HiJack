//! Phase D: thin OSC client for sending QLab cue creation sequences.
//!
//! Wraps a `tokio::net::UdpSocket` bound on an ephemeral local port,
//! pointing at QLab's OSC listen address (configured in the Setup tab via
//! `qlab_ip` / `qlab_port`).
//!
//! Pairs with [`crate::osc::qlab_cue_builder`], the pure builder that
//! produces the OSC sequences this client fires. The builder owns "what to
//! send" and the client owns "how to deliver it".
//!
//! ## QLab cue-creation protocol
//!
//! QLab's OSC API is positional: after `/new <type>`, the most-recently-
//! created cue becomes the "currently selected cue", and `/cue/selected/*`
//! addresses operate on it. The `send_sequence` helper just fires the
//! messages in order — there's no need to query unique ids or issue
//! moveInto commands when each child is created immediately after the
//! parent in a single sequence, because QLab inserts the new cue at the
//! current insertion point (which is inside the group right after the
//! group is selected).
//!
//! That's a simpler model than the WFS-DIY implementation, which queried
//! ids and issued moveInto for each child — useful when cues are created
//! out of order or interleaved with user actions, but unnecessary for our
//! one-shot snapshot exports.

use std::net::SocketAddr;
use std::time::Duration;

use rosc::{OscMessage, OscPacket};
use tokio::net::UdpSocket;
use tracing::{debug, info};

use super::qlab_cue_builder::QLabCueSequence;

/// Errors from the QLab client.
#[derive(Debug)]
pub enum QLabError {
    /// Socket binding or connect failed.
    Io(std::io::Error),
    /// OSC encoding failed.
    Encode(String),
}

impl std::fmt::Display for QLabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "QLab I/O error: {e}"),
            Self::Encode(e) => write!(f, "QLab encode error: {e}"),
        }
    }
}

impl std::error::Error for QLabError {}

impl From<std::io::Error> for QLabError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// QLab OSC client. Holds an ephemeral UDP socket and the QLab destination
/// address. Cheap to construct; intended to be created on-demand for a
/// single export operation rather than held for the lifetime of the app.
pub struct QLabClient {
    socket: UdpSocket,
    dest: SocketAddr,
}

impl QLabClient {
    /// Bind a fresh ephemeral local socket and target the given QLab
    /// address. Defaults to `127.0.0.1:53000` semantics — pass the live
    /// values from `ConnectionSettings::qlab_ip` / `qlab_port`.
    pub async fn new(qlab_ip: &str, qlab_port: u16) -> Result<Self, QLabError> {
        let dest_str = format!("{qlab_ip}:{qlab_port}");
        let dest: SocketAddr = dest_str.parse().map_err(|e| {
            QLabError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid QLab address '{dest_str}': {e}"),
            ))
        })?;
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        Ok(Self { socket, dest })
    }

    /// Send a single OSC message and return when it's been written to the
    /// kernel buffer.
    pub async fn send_message(&self, msg: OscMessage) -> Result<(), QLabError> {
        let packet = OscPacket::Message(msg);
        let buf = rosc::encoder::encode(&packet).map_err(|e| QLabError::Encode(format!("{e}")))?;
        self.socket.send_to(&buf, self.dest).await?;
        Ok(())
    }

    /// Fire a complete cue sequence: group messages first (creating and
    /// configuring the group cue), then each network cue's messages in
    /// order. A small delay is inserted between messages so QLab has time
    /// to process the previous cue before we issue `/cue/selected/*` calls
    /// on the next one — without this, fast-fire sequences can race the
    /// "selected cue" pointer and apply settings to the wrong cue.
    ///
    /// Returns the total number of messages sent. The caller can use this
    /// for status reporting ("Sent 47 OSC messages to QLab").
    pub async fn send_sequence(&self, sequence: &QLabCueSequence) -> Result<usize, QLabError> {
        let mut sent = 0usize;

        // Group cue first.
        for msg in &sequence.group_messages {
            self.send_message(msg.clone()).await?;
            sent += 1;
            tokio::time::sleep(INTER_MESSAGE_DELAY).await;
        }

        // Then each child cue.
        for child in &sequence.network_cues {
            for msg in &child.messages {
                self.send_message(msg.clone()).await?;
                sent += 1;
                tokio::time::sleep(INTER_MESSAGE_DELAY).await;
            }
            // (move_position is recorded for completeness but unused in
            // this simple implementation — children are inserted at the
            // group's current cursor in creation order, which already
            // gives us the right ordering for snapshot exports.)
        }

        info!(sent, dest = %self.dest, "Sent QLab cue sequence");
        debug!(
            "QLab sequence had {} group messages, {} children",
            sequence.group_messages.len(),
            sequence.network_cues.len()
        );
        Ok(sent)
    }
}

/// Pause between consecutive OSC messages so QLab's "currently selected cue"
/// pointer settles before the next setter fires. 5ms is plenty in practice
/// and keeps a 100-cue export under one second.
const INTER_MESSAGE_DELAY: Duration = Duration::from_millis(5);

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn new_binds_local_socket() {
        // Use 127.0.0.1:53000 — no QLab actually has to be listening; we
        // only need bind to succeed.
        let client = QLabClient::new("127.0.0.1", 53000).await.unwrap();
        assert_eq!(client.dest.to_string(), "127.0.0.1:53000");
    }

    #[tokio::test]
    async fn new_rejects_invalid_address() {
        let result = QLabClient::new("not.an.ip", 53000).await;
        assert!(result.is_err());
    }
}
