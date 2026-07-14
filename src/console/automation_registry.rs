//! Live operator-override registry for timed cue recalls.
//!
//! When a timed cue recall pre-waits or fades a parameter, the operator (hands
//! on the console) must always be able to take it back: moving that fader/knob
//! should stop the automation for THAT exact `(channel, parameter)` while the
//! rest of the cue keeps running. This registry is how the inbound GP OSC path
//! tells the running fade/pre-wait tasks "the operator grabbed this one — drop
//! it."
//!
//! Design (verified):
//! - One entry per automated `ParameterAddress`, registered up front at the top
//!   of `recall_cue_timed`. Each entry holds the last two values the recall/fade
//!   pushed (for echo suppression) and the group's deadline (for the progress
//!   countdown).
//! - The fade loop and the pre-wait path consult [`is_active`] each tick / after
//!   the wait: a removed entry means "operator took it" → stop sending it.
//! - The inbound dispatcher calls [`maybe_override`]: an operator-originated
//!   value (distinguished from the recall's own console echoes by value +
//!   time window, like the gang engine) removes the entry.
//! - Lock discipline: a `std::sync::Mutex` LEAF lock, held only for an O(1) map
//!   op and never across an `.await` — so the inbound hot path can never
//!   reintroduce the reader-starvation that froze App→Console.
//! - Per-recall `generation` stamping: a slow task from a previous cue that
//!   calls `note_sent`/`is_active` after a fresh `begin_recall` is a no-op, so
//!   overrides never leak across cues.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use tokio::time::Duration;

use crate::model::parameter::{ParameterAddress, ParameterValue};

/// Shared handle, threaded like `ConsoleLoadSuppression`: built once, put into
/// `DaemonState` (read by the inbound dispatcher) and set on `SnapshotEngine`
/// (written by the timed recall + fade tasks).
pub type AutomationOverride = Arc<AutomationRegistry>;

/// Float equality tolerance for echo matching (mirrors the gang engine).
const FLOAT_TOLERANCE: f32 = 0.01;
/// How long after the recall/fade sent a value an inbound match still counts as
/// our own echo rather than an operator move (mirrors the gang engine's window;
/// comfortably covers the desk's echo round-trip and 1-2 fade ticks of lag).
const ECHO_WINDOW: Duration = Duration::from_millis(300);

/// One automated address's bookkeeping.
struct AutoEntry {
    /// Most recent value the recall/fade pushed for this address.
    last_sent: Option<ParameterValue>,
    /// The value before that — covers echo lag of a step or two.
    prev_sent: Option<ParameterValue>,
    /// When `last_sent` was pushed.
    last_sent_at: Instant,
    /// When this group's automation finishes (registered_at + pre_wait + fade).
    /// Read by the progress countdown to compute the remaining max span.
    deadline: Instant,
    /// Recall generation that registered this entry.
    generation: u64,
}

/// Classification of an inbound parameter change for an automated address.
#[derive(Debug, PartialEq, Eq)]
pub enum Override {
    /// Address isn't under automation — handle normally.
    NotUnderAutomation,
    /// Inbound value is the console's echo of our own recall/fade send — ignore.
    Echo,
    /// The operator changed it — automation for this address has been cancelled.
    Operator,
}

/// Shared registry of addresses currently under timed-recall automation.
pub struct AutomationRegistry {
    generation: AtomicU64,
    inner: Mutex<HashMap<ParameterAddress, AutoEntry>>,
}

impl Default for AutomationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AutomationRegistry {
    pub fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Begin a fresh recall: clear all prior entries and bump the generation.
    /// Returns the new generation, which the recall stamps onto every register /
    /// note_sent so stale tasks from a previous cue can't write into fresh
    /// entries. Call once at the top of `recall_cue_timed`, after cancelling
    /// any prior fades.
    pub fn begin_recall(&self) -> u64 {
        let gen_id = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.inner.lock().unwrap().clear();
        gen_id
    }

    /// Register an address as under automation for `gen_id`, finishing at
    /// `now + span` (span = the group's pre_wait + fade_time).
    pub fn register(&self, addr: &ParameterAddress, gen_id: u64, span: Duration) {
        let now = Instant::now();
        self.inner.lock().unwrap().insert(
            addr.clone(),
            AutoEntry {
                last_sent: None,
                prev_sent: None,
                last_sent_at: now,
                deadline: now + span,
                generation: gen_id,
            },
        );
    }

    /// Record that the recall/fade just pushed `value` for `addr` (so the
    /// console's echo of it is recognised as ours, not an operator move).
    /// No-op if the address isn't registered or the generation is stale.
    pub fn note_sent(&self, addr: &ParameterAddress, value: &ParameterValue, gen_id: u64) {
        let mut map = self.inner.lock().unwrap();
        if let Some(e) = map.get_mut(addr) {
            if e.generation != gen_id {
                return;
            }
            e.prev_sent = e.last_sent.take();
            e.last_sent = Some(value.clone());
            e.last_sent_at = Instant::now();
        }
    }

    /// Whether the address is still under automation (i.e. not overridden and
    /// not yet deregistered). The fade loop / pre-wait path skip an address that
    /// is no longer active.
    pub fn is_active(&self, addr: &ParameterAddress) -> bool {
        self.inner.lock().unwrap().contains_key(addr)
    }

    /// Like [`is_active`](Self::is_active), but only true when the entry
    /// belongs to `gen_id`. Recall/fade tasks must use this, not `is_active`:
    /// a zombie fade from a superseded recall would otherwise resume sending
    /// its stale trajectory as soon as the NEW recall re-registers the same
    /// address (`begin_recall` clears the map, so a bare `contains_key` flips
    /// back to true the moment the successor registers).
    pub fn is_active_for(&self, addr: &ParameterAddress, gen_id: u64) -> bool {
        self.inner
            .lock()
            .unwrap()
            .get(addr)
            .is_some_and(|e| e.generation == gen_id)
    }

    /// Remove an address (a group/fade finished normally). Keeps the registry
    /// from shadowing the operator after automation ends.
    pub fn deregister(&self, addr: &ParameterAddress) {
        self.inner.lock().unwrap().remove(addr);
    }

    /// Largest remaining time across all active entries — the countdown's live
    /// max(pre_wait + fade). `None` when nothing is active.
    pub fn max_remaining(&self) -> Option<Duration> {
        let now = Instant::now();
        self.inner
            .lock()
            .unwrap()
            .values()
            .map(|e| e.deadline.saturating_duration_since(now))
            .max()
    }

    /// Classify an inbound change for `addr`. On a confirmed operator move the
    /// entry is removed (and the caller's running fade/pre-wait will stop
    /// sending it). Called from the inbound dispatcher.
    pub fn maybe_override(&self, addr: &ParameterAddress, incoming: &ParameterValue) -> Override {
        let mut map = self.inner.lock().unwrap();
        let Some(e) = map.get(addr) else {
            return Override::NotUnderAutomation;
        };
        // Pre-wait phase: nothing sent yet, so any inbound change is the operator.
        let within_window = e.last_sent_at.elapsed() < ECHO_WINDOW;
        let is_echo = within_window
            && (matches_value(e.last_sent.as_ref(), incoming)
                || matches_value(e.prev_sent.as_ref(), incoming));
        if e.last_sent.is_some() && is_echo {
            return Override::Echo;
        }
        // Operator move (or pre-wait change): drop the entry so the fade/pre-wait
        // stops sending this address and its later echoes are no longer matched.
        map.remove(addr);
        Override::Operator
    }
}

/// Value match with float tolerance (mirrors the gang engine's `values_match`).
fn matches_value(a: Option<&ParameterValue>, b: &ParameterValue) -> bool {
    let Some(a) = a else { return false };
    match (a, b) {
        (ParameterValue::Float(x), ParameterValue::Float(y)) => (x - y).abs() <= FLOAT_TOLERANCE,
        (ParameterValue::Int(x), ParameterValue::Int(y)) => x == y,
        (ParameterValue::Bool(x), ParameterValue::Bool(y)) => x == y,
        (ParameterValue::String(x), ParameterValue::String(y)) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::parameter::ParameterPath;

    fn addr() -> ParameterAddress {
        ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        }
    }

    #[test]
    fn prewait_any_change_is_operator() {
        let r = AutomationRegistry::new();
        let g = r.begin_recall();
        r.register(&addr(), g, Duration::from_secs(3));
        // Nothing sent yet → any inbound change is the operator.
        assert_eq!(
            r.maybe_override(&addr(), &ParameterValue::Float(-10.0)),
            Override::Operator
        );
        assert!(!r.is_active(&addr()));
    }

    #[test]
    fn echo_of_recent_send_is_suppressed() {
        let r = AutomationRegistry::new();
        let g = r.begin_recall();
        r.register(&addr(), g, Duration::from_secs(3));
        r.note_sent(&addr(), &ParameterValue::Float(-20.0), g);
        // Console echoes back the value we just sent → Echo, still active.
        assert_eq!(
            r.maybe_override(&addr(), &ParameterValue::Float(-20.0)),
            Override::Echo
        );
        assert!(r.is_active(&addr()));
    }

    #[test]
    fn lagging_echo_of_previous_step_is_suppressed() {
        let r = AutomationRegistry::new();
        let g = r.begin_recall();
        r.register(&addr(), g, Duration::from_secs(3));
        r.note_sent(&addr(), &ParameterValue::Float(-20.0), g);
        r.note_sent(&addr(), &ParameterValue::Float(-19.5), g);
        // A late echo of the previous step (-20.0) must still be suppressed.
        assert_eq!(
            r.maybe_override(&addr(), &ParameterValue::Float(-20.0)),
            Override::Echo
        );
    }

    #[test]
    fn operator_move_away_from_trajectory_cancels() {
        let r = AutomationRegistry::new();
        let g = r.begin_recall();
        r.register(&addr(), g, Duration::from_secs(3));
        r.note_sent(&addr(), &ParameterValue::Float(-20.0), g);
        // Operator jumps far from the faded value → Operator, deregistered.
        assert_eq!(
            r.maybe_override(&addr(), &ParameterValue::Float(0.0)),
            Override::Operator
        );
        assert!(!r.is_active(&addr()));
    }

    #[test]
    fn unregistered_is_not_under_automation() {
        let r = AutomationRegistry::new();
        r.begin_recall();
        assert_eq!(
            r.maybe_override(&addr(), &ParameterValue::Float(0.0)),
            Override::NotUnderAutomation
        );
    }

    #[test]
    fn stale_generation_note_sent_is_noop() {
        let r = AutomationRegistry::new();
        let g1 = r.begin_recall();
        r.register(&addr(), g1, Duration::from_secs(3));
        let _g2 = r.begin_recall(); // clears + bumps generation
        // A stale task from g1 trying to record a send must not resurrect state.
        r.note_sent(&addr(), &ParameterValue::Float(-20.0), g1);
        assert!(!r.is_active(&addr()));
    }

    #[test]
    fn is_active_for_rejects_stale_generation_after_reregister() {
        // The zombie-fade case: cue N registers an address, cue N+1 begins a
        // new recall and re-registers the SAME address. `is_active` (bare
        // contains_key) flips back to true — which used to revive N's fade —
        // but `is_active_for(N's gen)` must stay false.
        let r = AutomationRegistry::new();
        let g1 = r.begin_recall();
        r.register(&addr(), g1, Duration::from_secs(3));
        assert!(r.is_active_for(&addr(), g1));

        let g2 = r.begin_recall();
        r.register(&addr(), g2, Duration::from_secs(3));
        assert!(r.is_active(&addr()), "address is under (new) automation");
        assert!(
            !r.is_active_for(&addr(), g1),
            "the superseded recall must not see the re-registered address as its own"
        );
        assert!(r.is_active_for(&addr(), g2));
    }

    #[test]
    fn max_remaining_shrinks_after_override() {
        let r = AutomationRegistry::new();
        let g = r.begin_recall();
        let a1 = addr();
        let a2 = ParameterAddress {
            channel: ChannelId::Input(2),
            parameter: ParameterPath::Fader,
        };
        r.register(&a1, g, Duration::from_secs(5));
        r.register(&a2, g, Duration::from_secs(1));
        let before = r.max_remaining().unwrap();
        assert!(before > Duration::from_millis(4500));
        // Override the long one → remaining max drops to ~the short one.
        r.maybe_override(&a1, &ParameterValue::Float(0.0));
        let after = r.max_remaining().unwrap();
        assert!(after < Duration::from_millis(1100));
    }
}
