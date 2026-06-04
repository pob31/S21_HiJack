//! Phase D: OSC client for sending QLab cue-creation sequences.
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
//! addresses operate on it. To reliably nest a child cue *inside* a group we
//! cannot rely on QLab's insertion cursor — we must learn each new cue's
//! unique id and issue an explicit `/move/{uid} {position} {group_uid}`.
//!
//! QLab returns the id in reply to a `/cue/selected/uniqueID` query, as a
//! `/reply/.../uniqueID` message whose string arg is JSON
//! `{"status":"ok","data":"<uid>"}`. That reply does **not** come back on this
//! client's socket — the operator's QLab is configured to send replies to the
//! app on its trigger port, where the trigger listener catches it and hands
//! the uid over via [`crate::osc::qlab_reply`]. So `send_sequence` registers
//! on the reply bus, fires the query, and awaits the uid (with a timeout).
//!
//! Mirrors WFS-DIY's `OSCManager::sendToQLab` (group → query → children →
//! query → move). If a query goes unanswered the whole export aborts with a
//! [`QLabError::Timeout`] carrying operator-facing setup guidance.

use std::net::SocketAddr;
use std::time::Duration;

use rosc::{OscMessage, OscPacket, OscType};
use tokio::net::UdpSocket;
use tracing::{debug, info};

use super::qlab_cue_builder::QLabCueSequence;
use crate::osc::qlab_reply::QLabReplyBus;

/// QLab's query for the currently-selected cue's unique id.
const QUERY_UNIQUE_ID_ADDR: &str = "/cue/selected/uniqueID";

/// Pause between consecutive OSC messages so QLab finishes processing the
/// previous one before the next setter/query fires. WFS-DIY uses 30 ms; at
/// the old 5 ms a `/cue/selected/uniqueID` query could race QLab into
/// replying with the *previously* selected cue's id.
const INTER_MESSAGE_DELAY: Duration = Duration::from_millis(30);

/// How long to wait for QLab's `/cue/selected/uniqueID` reply before aborting.
/// Matches WFS-DIY's `queryTimeoutMs`.
const QUERY_TIMEOUT: Duration = Duration::from_millis(2000);

/// Operator-facing reminder appended to QLab reply timeouts. The two usual
/// causes of "no reply on the trigger port": QLab not knowing where to send
/// replies, or its OSC passcode rejecting our commands.
pub const QLAB_REPLY_HELP: &str = "Check QLab: (1) add the S21HiJack client as an OSC patch/destination so QLab sends replies back to this app, and (2) allow network actions without a password (disable the OSC passcode in QLab's settings).";

/// Errors from the QLab client.
#[derive(Debug)]
pub enum QLabError {
    /// Socket binding or connect failed.
    Io(std::io::Error),
    /// OSC encoding failed.
    Encode(String),
    /// A `/cue/selected/uniqueID` query went unanswered within the timeout.
    /// The payload names which cue was being created, for context.
    Timeout(String),
}

impl std::fmt::Display for QLabError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "QLab I/O error: {e}"),
            Self::Encode(e) => write!(f, "QLab encode error: {e}"),
            Self::Timeout(ctx) => write!(
                f,
                "QLab did not reply to the {ctx} uniqueID query within {} ms. {QLAB_REPLY_HELP}",
                QUERY_TIMEOUT.as_millis()
            ),
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
    /// Where QLab `/reply/.../uniqueID` responses are delivered from (the
    /// trigger listener routes them here). Shared process-global instance in
    /// production; tests inject a local bus.
    reply_bus: QLabReplyBus,
    /// How long to wait for each uniqueID reply.
    query_timeout: Duration,
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
        Ok(Self {
            socket,
            dest,
            reply_bus: crate::osc::qlab_reply::global(),
            query_timeout: QUERY_TIMEOUT,
        })
    }

    /// Send a single OSC message and return when it's been written to the
    /// kernel buffer.
    pub async fn send_message(&self, msg: OscMessage) -> Result<(), QLabError> {
        let packet = OscPacket::Message(msg);
        let buf = rosc::encoder::encode(&packet).map_err(|e| QLabError::Encode(format!("{e}")))?;
        self.socket.send_to(&buf, self.dest).await?;
        Ok(())
    }

    /// Query QLab for the currently-selected cue's unique id and await the
    /// reply on the bus. `context` ("group" / "network") is folded into the
    /// timeout error so the operator knows which step failed.
    async fn query_unique_id(&self, context: &str) -> Result<String, QLabError> {
        // Arm the bus *before* sending so a fast reply can't be missed.
        let rx = self.reply_bus.register().await;
        self.send_message(OscMessage {
            addr: QUERY_UNIQUE_ID_ADDR.into(),
            args: vec![],
        })
        .await?;
        match tokio::time::timeout(self.query_timeout, rx).await {
            Ok(Ok(uid)) => Ok(uid),
            // Either the wait elapsed, or the sender was dropped because a
            // newer query replaced it — both mean we never got this id.
            Ok(Err(_)) | Err(_) => Err(QLabError::Timeout(context.to_string())),
        }
    }

    /// Fire a complete cue sequence: create the group, learn its id, then for
    /// each child create it, learn its id, and `/move` it into the group at
    /// its recorded position. Mirrors WFS-DIY's `sendToQLab`.
    ///
    /// Aborts with [`QLabError::Timeout`] if any uniqueID query goes
    /// unanswered (e.g. QLab isn't set up to reply on the trigger port, or
    /// its OSC passcode is rejecting us — see [`QLAB_REPLY_HELP`]).
    ///
    /// Returns the total number of messages sent, for status reporting.
    pub async fn send_sequence(&self, sequence: &QLabCueSequence) -> Result<usize, QLabError> {
        let mut sent = 0usize;

        // ── Group cue ──
        for msg in &sequence.group_messages {
            self.send_message(msg.clone()).await?;
            sent += 1;
            tokio::time::sleep(INTER_MESSAGE_DELAY).await;
        }

        // Learn the group's id so children can be moved into it. A sequence
        // with no group messages (e.g. a standalone cue) skips this.
        let group_uid = if sequence.group_messages.is_empty() {
            None
        } else {
            let uid = self.query_unique_id("group").await?;
            sent += 1;
            tokio::time::sleep(INTER_MESSAGE_DELAY).await;
            Some(uid)
        };

        // ── Child cues ──
        for child in &sequence.network_cues {
            for msg in &child.messages {
                self.send_message(msg.clone()).await?;
                sent += 1;
                tokio::time::sleep(INTER_MESSAGE_DELAY).await;
            }

            // Nest into the group at the recorded 1-based position. Needs a
            // group to move into and a real slot (`move_position == 0` means
            // "standalone cue").
            if let Some(group_uid) = &group_uid {
                if child.move_position > 0 {
                    let cue_uid = self.query_unique_id("network").await?;
                    sent += 1;
                    tokio::time::sleep(INTER_MESSAGE_DELAY).await;

                    self.send_message(build_move_message(&cue_uid, child.move_position, group_uid))
                        .await?;
                    sent += 1;
                    tokio::time::sleep(INTER_MESSAGE_DELAY).await;
                }
            }
        }

        info!(sent, dest = %self.dest, "Sent QLab cue sequence");
        debug!(
            "QLab sequence had {} group messages, {} children",
            sequence.group_messages.len(),
            sequence.network_cues.len()
        );
        Ok(sent)
    }

    /// Test-only: route replies through a caller-supplied bus instead of the
    /// process-global one, so tests don't share global state.
    #[cfg(test)]
    fn with_reply_bus(mut self, bus: QLabReplyBus) -> Self {
        self.reply_bus = bus;
        self
    }

    /// Test-only: shorten the per-query timeout so the abort path is fast.
    #[cfg(test)]
    fn with_query_timeout(mut self, timeout: Duration) -> Self {
        self.query_timeout = timeout;
        self
    }
}

/// Build QLab's `/move/{cue_uid} {position} {group_uid}` command — places a
/// freshly-created cue at a 1-based slot inside a group. Arg order/types
/// mirror WFS-DIY (`OSCManager.cpp`): position as int, group uid as string.
fn build_move_message(cue_uid: &str, position: u32, group_uid: &str) -> OscMessage {
    OscMessage {
        addr: format!("/move/{cue_uid}"),
        args: vec![
            OscType::Int(position as i32),
            OscType::String(group_uid.to_string()),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc::qlab_cue_builder::NetworkCue;

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

    #[test]
    fn build_move_message_addr_and_args() {
        let msg = build_move_message("ABC-123", 2, "GRP-9");
        assert_eq!(msg.addr, "/move/ABC-123");
        assert_eq!(
            msg.args,
            vec![OscType::Int(2), OscType::String("GRP-9".into())]
        );
    }

    fn new_cue(addr: &str, ty: &str) -> OscMessage {
        OscMessage {
            addr: addr.into(),
            args: vec![OscType::String(ty.into())],
        }
    }

    /// End-to-end (in-process): a stand-in QLab answers each uniqueID query
    /// via the bus, and we assert `send_sequence` issues the right `/move`.
    #[tokio::test]
    async fn send_sequence_moves_child_into_group() {
        let fake_qlab = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let fake_port = fake_qlab.local_addr().unwrap().port();

        let bus = QLabReplyBus::default();
        let client = QLabClient::new("127.0.0.1", fake_port)
            .await
            .unwrap()
            .with_reply_bus(bus.clone());

        // Drive the stand-in QLab: answer uniqueID queries, capture moves.
        let pump = tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            let mut counter = 0;
            loop {
                let Ok(Ok((size, _src))) =
                    tokio::time::timeout(Duration::from_secs(2), fake_qlab.recv_from(&mut buf))
                        .await
                else {
                    return None;
                };
                if let Ok((_, OscPacket::Message(msg))) = rosc::decoder::decode_udp(&buf[..size]) {
                    if msg.addr == QUERY_UNIQUE_ID_ADDR {
                        counter += 1;
                        bus.deliver(format!("UID-{counter}")).await;
                    } else if msg.addr.starts_with("/move/") {
                        return Some((msg.addr.clone(), msg.args.clone()));
                    }
                }
            }
        });

        let seq = QLabCueSequence {
            group_messages: vec![new_cue("/new", "group")],
            network_cues: vec![NetworkCue {
                messages: vec![new_cue("/new", "network")],
                move_position: 1,
            }],
        };

        let sent = client.send_sequence(&seq).await.unwrap();
        assert!(sent > 0);

        let mv = pump.await.unwrap().expect("expected a /move message");
        // Group is queried first (UID-1), then the child (UID-2). The move
        // targets the child's id and references the group's id.
        assert_eq!(mv.0, "/move/UID-2");
        assert_eq!(mv.1, vec![OscType::Int(1), OscType::String("UID-1".into())]);
    }

    /// When QLab never replies, the export aborts with a Timeout error.
    #[tokio::test]
    async fn send_sequence_times_out_without_reply() {
        let fake_qlab = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let fake_port = fake_qlab.local_addr().unwrap().port();
        // Drain incoming packets so sends succeed, but never reply.
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            while fake_qlab.recv_from(&mut buf).await.is_ok() {}
        });

        let client = QLabClient::new("127.0.0.1", fake_port)
            .await
            .unwrap()
            .with_reply_bus(QLabReplyBus::default())
            .with_query_timeout(Duration::from_millis(50));

        let seq = QLabCueSequence {
            group_messages: vec![new_cue("/new", "group")],
            network_cues: vec![],
        };
        let result = client.send_sequence(&seq).await;
        assert!(matches!(result, Err(QLabError::Timeout(_))));
    }
}
