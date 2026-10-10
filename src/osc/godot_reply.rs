//! Rendezvous for Go.dot's answers: `/godot/declared`, `/godot/described`
//! and `/godot/captured` (contract `docs/godot-authoring-protocol-0.1.md`
//! in the Go.dot repository, §3 and §4.5).
//!
//! Go.dot answers the device the app declared, at its declared port: the
//! **trigger listener's** port, not the ephemeral socket the request left
//! from. So the listener and the asker sit at opposite ends of this bus, as
//! they do for QLab's uniqueID replies ([`crate::osc::qlab_reply`]): the
//! asker [`register`](GoDotReplyBus::register)s a key before it sends, the
//! listener [`deliver`](GoDotReplyBus::deliver)s each answer under its key.
//!
//! A capture's answers are keyed by the cue's identifier, which the app drew
//! itself, so two captures in flight never take each other's answer. The
//! declare's two answers have fixed keys, [`DECLARED_KEY`] and
//! [`DESCRIBED_KEY`]: one declare is in flight at a time, and a fresh
//! registration replaces a stale one.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use rosc::OscType;
use tokio::sync::{Mutex, oneshot};

use super::{GODOT_CAPTURED_ADDR, GODOT_DECLARED_ADDR, GODOT_DESCRIBED_ADDR};

/// The bus key of a `/godot/declared` answer.
pub const DECLARED_KEY: &str = "/godot/declared";

/// The bus key of a `/godot/described` answer.
pub const DESCRIBED_KEY: &str = "/godot/described";

/// One answer from Go.dot.
#[derive(Debug, Clone, PartialEq)]
pub enum GoDotAck {
    /// `/godot/captured ,ssss id outcome number name`: `outcome` is
    /// `created`, `updated`, `appended` (a `more` chunk) or the refusal's
    /// reason; `number` and `name` are the cue's as they now stand, empty on
    /// a refusal.
    Captured {
        id: String,
        outcome: String,
        number: String,
        name: String,
    },
    /// `/godot/declared ,ss id outcome`: `created`, `updated` or the
    /// refusal's reason. `id` is the device's identifier in Go.dot.
    Declared { id: String, outcome: String },
    /// `/godot/described ,sis id nodeCount problem`: `problem` is empty when
    /// the description loaded.
    Described {
        id: String,
        node_count: i64,
        problem: String,
    },
}

impl GoDotAck {
    /// The key this answer is delivered under.
    pub fn key(&self) -> &str {
        match self {
            GoDotAck::Captured { id, .. } => id,
            GoDotAck::Declared { .. } => DECLARED_KEY,
            GoDotAck::Described { .. } => DESCRIBED_KEY,
        }
    }

    /// The refusal's reason, when this answer is one: a capture or declare
    /// outcome other than `created`, `updated` or `appended`, or a
    /// description with a problem.
    pub fn refusal(&self) -> Option<&str> {
        match self {
            GoDotAck::Captured { outcome, .. } | GoDotAck::Declared { outcome, .. } => {
                match outcome.as_str() {
                    "created" | "updated" | "appended" => None,
                    reason => Some(reason),
                }
            }
            GoDotAck::Described { problem, .. } => {
                (!problem.is_empty()).then_some(problem.as_str())
            }
        }
    }
}

/// Read an answer from its address and arguments; `None` for any other
/// address, or arguments that are not the answer's shape.
pub fn parse_answer(addr: &str, args: &[OscType]) -> Option<GoDotAck> {
    match addr {
        GODOT_CAPTURED_ADDR => parse_captured(args),
        GODOT_DECLARED_ADDR => parse_declared(args),
        GODOT_DESCRIBED_ADDR => parse_described(args),
        _ => None,
    }
}

/// `/godot/captured ,ssss id outcome number name`. A missing `number` or
/// `name` reads as empty, so a refusal sent short still reads.
pub fn parse_captured(args: &[OscType]) -> Option<GoDotAck> {
    Some(GoDotAck::Captured {
        id: string_at(args, 0)?,
        outcome: string_at(args, 1)?,
        number: string_at(args, 2).unwrap_or_default(),
        name: string_at(args, 3).unwrap_or_default(),
    })
}

/// `/godot/declared ,ss id outcome`.
pub fn parse_declared(args: &[OscType]) -> Option<GoDotAck> {
    Some(GoDotAck::Declared {
        id: string_at(args, 0)?,
        outcome: string_at(args, 1)?,
    })
}

/// `/godot/described ,sis id nodeCount problem`. The count is read from an
/// int, a long, a float or a string; the problem may be left out.
pub fn parse_described(args: &[OscType]) -> Option<GoDotAck> {
    let node_count = match args.get(1)? {
        OscType::Int(n) => i64::from(*n),
        OscType::Long(n) => *n,
        OscType::Float(f) if f.is_finite() => *f as i64,
        OscType::Double(d) if d.is_finite() => *d as i64,
        OscType::String(s) => s.trim().parse().ok()?,
        _ => return None,
    };
    Some(GoDotAck::Described {
        id: string_at(args, 0)?,
        node_count,
        problem: string_at(args, 2).unwrap_or_default(),
    })
}

fn string_at(args: &[OscType], i: usize) -> Option<String> {
    match args.get(i)? {
        OscType::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// Shared, cloneable handle to the waiting askers, one per key.
#[derive(Clone, Default)]
pub struct GoDotReplyBus {
    slots: Arc<Mutex<HashMap<String, oneshot::Sender<GoDotAck>>>>,
}

impl GoDotReplyBus {
    /// Wait for the next answer under `key`. Call before sending, so a fast
    /// answer cannot be missed. Replaces (and so cancels) an earlier waiter
    /// under the same key.
    pub async fn register(&self, key: &str) -> oneshot::Receiver<GoDotAck> {
        let (tx, rx) = oneshot::channel();
        self.slots.lock().await.insert(key.to_string(), tx);
        rx
    }

    /// Hand an answer to whoever waits under its key. Returns false when
    /// nobody did (a stray or late answer), which is not an error.
    pub async fn deliver(&self, ack: GoDotAck) -> bool {
        let waiter = self.slots.lock().await.remove(ack.key());
        match waiter {
            Some(tx) => tx.send(ack).is_ok(),
            None => false,
        }
    }

    /// Stop waiting under `key` (after a timeout), so a late answer is not
    /// held for nobody.
    pub async fn forget(&self, key: &str) {
        self.slots.lock().await.remove(key);
    }
}

/// Process-global bus shared by the trigger listener and the Go.dot client.
pub fn global() -> GoDotReplyBus {
    static GLOBAL: OnceLock<GoDotReplyBus> = OnceLock::new();
    GLOBAL.get_or_init(GoDotReplyBus::default).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> OscType {
        OscType::String(v.into())
    }

    #[test]
    fn captured_reads_four_strings() {
        let ack = parse_answer(
            "/godot/captured",
            &[s("0123ABCD"), s("created"), s("12"), s("Recall 'A'")],
        )
        .unwrap();
        assert_eq!(
            ack,
            GoDotAck::Captured {
                id: "0123ABCD".into(),
                outcome: "created".into(),
                number: "12".into(),
                name: "Recall 'A'".into(),
            }
        );
        assert_eq!(ack.key(), "0123ABCD");
        assert_eq!(ack.refusal(), None);

        let refused = parse_captured(&[s("0123ABCD"), s("locked")]).unwrap();
        assert_eq!(refused.refusal(), Some("locked"));
    }

    #[test]
    fn declared_and_described_read_their_shapes() {
        let declared = parse_answer("/godot/declared", &[s("DEV00001"), s("updated")]).unwrap();
        assert_eq!(declared.key(), DECLARED_KEY);
        assert_eq!(declared.refusal(), None);
        assert_eq!(
            parse_declared(&[s(""), s("bad-value")]).unwrap().refusal(),
            Some("bad-value")
        );

        let described =
            parse_answer("/godot/described", &[s("DEV00001"), OscType::Int(7), s("")]).unwrap();
        assert_eq!(
            described,
            GoDotAck::Described {
                id: "DEV00001".into(),
                node_count: 7,
                problem: String::new(),
            }
        );
        assert_eq!(described.key(), DESCRIBED_KEY);
        let failed =
            parse_described(&[s("DEV00001"), OscType::Int(0), s("connection refused")]).unwrap();
        assert_eq!(failed.refusal(), Some("connection refused"));
    }

    #[test]
    fn malformed_answers_are_not_read() {
        assert_eq!(parse_answer("/godot/captured", &[s("only one")]), None);
        assert_eq!(
            parse_answer("/godot/declared", &[OscType::Int(1), s("x")]),
            None
        );
        assert_eq!(parse_answer("/godot/described", &[s("id")]), None);
        assert_eq!(parse_answer("/godot/other", &[s("a"), s("b")]), None);
    }

    #[tokio::test]
    async fn answers_reach_the_waiter_of_their_key() {
        let bus = GoDotReplyBus::default();
        let a = bus.register("AAAAAAAA").await;
        let b = bus.register("BBBBBBBB").await;
        let ack = |id: &str| GoDotAck::Captured {
            id: id.into(),
            outcome: "created".into(),
            number: String::new(),
            name: String::new(),
        };
        assert!(bus.deliver(ack("BBBBBBBB")).await);
        assert!(bus.deliver(ack("AAAAAAAA")).await);
        assert_eq!(a.await.unwrap().key(), "AAAAAAAA");
        assert_eq!(b.await.unwrap().key(), "BBBBBBBB");
        // Nobody waits any more: a late answer is dropped.
        assert!(!bus.deliver(ack("AAAAAAAA")).await);
    }

    #[tokio::test]
    async fn a_fresh_register_cancels_the_previous_one() {
        let bus = GoDotReplyBus::default();
        let first = bus.register(DECLARED_KEY).await;
        let second = bus.register(DECLARED_KEY).await;
        bus.deliver(GoDotAck::Declared {
            id: "D".into(),
            outcome: "created".into(),
        })
        .await;
        assert!(first.await.is_err());
        assert!(second.await.is_ok());
    }
}
