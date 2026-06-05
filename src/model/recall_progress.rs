//! Lock-free shared progress handle for long parameter transfers.
//!
//! Two producers drive it: the daemon's console-dump loop (on connect / recovery
//! resend — an unknown total, estimated from the channel config) and the recall
//! engines (snapshot / cue / macro — an exact known total). The egui UI thread
//! polls [`RecallProgress::snapshot`] each frame and renders the thin progress
//! line under the tab bar. All fields are atomics so neither side ever blocks the
//! other — mirrors the existing `Arc<AtomicU64>` send-pacing pattern.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};

/// What is currently filling the bar. Disambiguates the two `done`-counting
/// sources so the dump loop only counts inbound params for a dump (a recall's
/// inbound echoes are counted by the engine on the *send* side instead).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecallKind {
    /// The console dump on connect (or a recovery resend). Unknown total.
    Dump,
    /// A snapshot / cue / macro recall with an exact known total.
    Recall,
}

impl RecallKind {
    fn to_u8(self) -> u8 {
        match self {
            RecallKind::Dump => 0,
            RecallKind::Recall => 1,
        }
    }
    fn from_u8(v: u8) -> Self {
        match v {
            1 => RecallKind::Recall,
            _ => RecallKind::Dump,
        }
    }
}

/// Shared progress state. Construct one `Arc<RecallProgress>` and clone it into
/// the daemon and the recall engines (producers) and the UI app (consumer).
#[derive(Debug, Default)]
pub struct RecallProgress {
    active: AtomicBool,
    kind: AtomicU8,
    /// 0 => unknown total (indeterminate); the UI shows a sweep until `finish`.
    total: AtomicUsize,
    done: AtomicUsize,
    /// Bumped on every `begin`; the UI watches it to reset its easing/fade when
    /// a fresh operation starts.
    generation: AtomicU64,
}

/// A consistent-ish snapshot of [`RecallProgress`] for one UI frame. The atomics
/// are read independently (not a single lock), which is fine: the UI only eases
/// toward these values, so a one-frame skew is invisible.
#[derive(Clone, Copy, Debug)]
pub struct RecallProgressView {
    pub active: bool,
    pub kind: RecallKind,
    pub total: usize,
    pub done: usize,
    pub generation: u64,
}

impl RecallProgress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a new operation. `total == 0` means the total is unknown
    /// (indeterminate); otherwise the bar fills toward `done / total`.
    pub fn begin(&self, kind: RecallKind, total: usize) {
        self.kind.store(kind.to_u8(), Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.active.store(true, Ordering::Release);
    }

    /// Advance the done counter by one (one parameter sent / received).
    pub fn bump(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }

    /// Advance the done counter by `n`.
    pub fn add(&self, n: usize) {
        self.done.fetch_add(n, Ordering::Relaxed);
    }

    /// Current done count — how many parameters have been received/sent so far.
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    /// Mark the current operation complete. Idempotent.
    pub fn finish(&self) {
        self.active.store(false, Ordering::Release);
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub fn kind(&self) -> RecallKind {
        RecallKind::from_u8(self.kind.load(Ordering::Relaxed))
    }

    /// Snapshot all fields for one UI frame.
    pub fn snapshot(&self) -> RecallProgressView {
        RecallProgressView {
            active: self.active.load(Ordering::Acquire),
            kind: RecallKind::from_u8(self.kind.load(Ordering::Relaxed)),
            total: self.total.load(Ordering::Relaxed),
            done: self.done.load(Ordering::Relaxed),
            generation: self.generation.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_inactive() {
        let p = RecallProgress::new();
        let v = p.snapshot();
        assert!(!v.active);
        assert_eq!(v.done, 0);
        assert_eq!(v.generation, 0);
    }

    #[test]
    fn begin_bump_finish_lifecycle() {
        let p = RecallProgress::new();
        p.begin(RecallKind::Recall, 10);
        let v = p.snapshot();
        assert!(v.active);
        assert_eq!(v.kind, RecallKind::Recall);
        assert_eq!(v.total, 10);
        assert_eq!(v.done, 0);
        assert_eq!(v.generation, 1);

        p.bump();
        p.add(3);
        assert_eq!(p.snapshot().done, 4);

        p.finish();
        assert!(!p.snapshot().active);
    }

    #[test]
    fn begin_resets_done_and_bumps_generation() {
        let p = RecallProgress::new();
        p.begin(RecallKind::Dump, 0);
        p.add(50);
        assert_eq!(p.snapshot().done, 50);
        assert_eq!(p.snapshot().generation, 1);

        p.begin(RecallKind::Recall, 5);
        let v = p.snapshot();
        assert_eq!(v.done, 0); // reset
        assert_eq!(v.total, 5);
        assert_eq!(v.generation, 2); // bumped
        assert_eq!(v.kind, RecallKind::Recall);
    }
}
