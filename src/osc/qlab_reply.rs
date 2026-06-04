//! Rendezvous for QLab `/reply/.../uniqueID` responses.
//!
//! When the daemon exports cues to QLab it must learn each new cue's unique
//! id to nest it into a group (see [`crate::osc::qlab_client::QLabClient::send_sequence`]).
//! QLab sends that id back as an OSC `/reply/.../uniqueID` message whose single
//! string argument is JSON `{"status":"ok","data":"<uid>"}`.
//!
//! Those replies arrive on the **trigger listener's** socket (the app's
//! QLab-facing port, default 53001), *not* on the export client's socket,
//! because the operator's QLab is configured to send replies to the app
//! there — the same setup that makes WFS-DIY work. So the listener and the
//! exporter sit on opposite ends of this bus: the listener calls
//! [`QLabReplyBus::deliver`] when a reply lands, and the exporter calls
//! [`QLabReplyBus::register`] just before each query and awaits the result.
//!
//! Exports are operator-initiated (one at a time) and issue their queries
//! sequentially, so a single in-flight slot is enough — matching WFS-DIY's
//! single `replyUID`/`WaitableEvent`. A fresh `register` replaces any stale
//! sender, so a late reply to a previous query can't satisfy the next one.

use std::sync::{Arc, OnceLock};

use tokio::sync::{Mutex, oneshot};

/// Shared, cloneable handle to the single QLab reply slot.
#[derive(Clone, Default)]
pub struct QLabReplyBus {
    slot: Arc<Mutex<Option<oneshot::Sender<String>>>>,
}

impl QLabReplyBus {
    /// Arm the bus for the next reply. Returns a receiver that resolves with
    /// the uid string once [`Self::deliver`] is called. Replaces (and thereby
    /// cancels) any previously-registered sender, so a late reply to an
    /// earlier query cannot satisfy this one.
    pub async fn register(&self) -> oneshot::Receiver<String> {
        let (tx, rx) = oneshot::channel();
        *self.slot.lock().await = Some(tx);
        rx
    }

    /// Deliver a uid to whoever is currently awaiting. No-op when nobody is
    /// registered (a stray/late reply) or the awaiter has already gone away.
    pub async fn deliver(&self, uid: String) {
        if let Some(tx) = self.slot.lock().await.take() {
            let _ = tx.send(uid);
        }
    }
}

/// Process-global bus shared by the trigger listener and the export client.
/// A single instance is correct because there is at most one trigger
/// listener and one export in flight at a time.
pub fn global() -> QLabReplyBus {
    static GLOBAL: OnceLock<QLabReplyBus> = OnceLock::new();
    GLOBAL.get_or_init(QLabReplyBus::default).clone()
}

/// Parse QLab's `/reply/.../uniqueID` JSON argument, returning the `data`
/// uid string when `status == "ok"`. Returns `None` for an error status, a
/// missing/non-string `data`, or malformed JSON. Mirrors WFS-DIY's
/// `OSCManager.cpp` reply handling.
pub fn parse_unique_id_reply(json_str: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json_str).ok()?;
    if value.get("status")?.as_str()? != "ok" {
        return None;
    }
    Some(value.get("data")?.as_str()?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ok_reply_returns_uid() {
        let uid = parse_unique_id_reply(r#"{"status":"ok","data":"ABC-123"}"#);
        assert_eq!(uid.as_deref(), Some("ABC-123"));
    }

    #[test]
    fn parse_error_status_returns_none() {
        assert_eq!(
            parse_unique_id_reply(r#"{"status":"error","data":"ABC-123"}"#),
            None
        );
    }

    #[test]
    fn parse_missing_data_returns_none() {
        assert_eq!(parse_unique_id_reply(r#"{"status":"ok"}"#), None);
    }

    #[test]
    fn parse_non_string_data_returns_none() {
        assert_eq!(parse_unique_id_reply(r#"{"status":"ok","data":42}"#), None);
    }

    #[test]
    fn parse_malformed_json_returns_none() {
        assert_eq!(parse_unique_id_reply("not json at all"), None);
    }

    #[tokio::test]
    async fn deliver_wakes_registered_receiver() {
        let bus = QLabReplyBus::default();
        let rx = bus.register().await;
        bus.deliver("UID-9".into()).await;
        assert_eq!(rx.await.ok().as_deref(), Some("UID-9"));
    }

    #[tokio::test]
    async fn deliver_without_register_is_noop() {
        let bus = QLabReplyBus::default();
        // Must not panic when nobody is waiting.
        bus.deliver("orphan".into()).await;
    }

    #[tokio::test]
    async fn fresh_register_cancels_previous_sender() {
        let bus = QLabReplyBus::default();
        let first = bus.register().await;
        // A second query arms the bus again before the first reply arrives.
        let second = bus.register().await;
        bus.deliver("for-second".into()).await;
        // The first receiver was cancelled (sender dropped) — resolves to Err.
        assert!(first.await.is_err());
        assert_eq!(second.await.ok().as_deref(), Some("for-second"));
    }
}
