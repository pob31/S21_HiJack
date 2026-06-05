//! Lock-free shared progress handle for long parameter transfers.
//!
//! Two producers drive it: the daemon's console-dump loop (on connect / recovery
//! resend — an unknown total, estimated from the channel config) and the recall
//! engines (snapshot / cue / macro — an exact known total). The egui UI thread
//! polls [`RecallProgress::snapshot`] each frame and renders the thin progress
//! line under the tab bar. All fields are atomics so neither side ever blocks the
//! other — mirrors the existing `Arc<AtomicU64>` send-pacing pattern.

use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Process-wide monotonic epoch so a producer thread (a recall task) and the
/// consumer (the egui UI thread) can express the same instant as a `u64` of
/// microseconds in shared atomics. `Instant` itself can't live in an atomic.
static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Microseconds elapsed since the process epoch. Cheap, monotonic, lock-free.
fn now_us() -> u64 {
    EPOCH.elapsed().as_micros() as u64
}

/// Whether the bar fills by counting parameters or by elapsing a wall-clock
/// span. A timed cue recall (pre-wait + fade) has no meaningful "params done"
/// pulse to count — it fills over `max(pre_wait + fade)` instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressMode {
    /// Fill toward `done / total` (the console dump, the discrete recall path).
    Count,
    /// Fill toward `elapsed / (span_end - span_start)` (the timed cue recall).
    Time,
}

impl ProgressMode {
    fn to_u8(self) -> u8 {
        match self {
            ProgressMode::Count => 0,
            ProgressMode::Time => 1,
        }
    }
    fn from_u8(v: u8) -> Self {
        match v {
            1 => ProgressMode::Time,
            _ => ProgressMode::Count,
        }
    }
}

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
    /// `Count` vs `Time` fill semantics (see [`ProgressMode`]).
    mode: AtomicU8,
    /// 0 => unknown total (indeterminate); the UI shows a sweep until `finish`.
    total: AtomicUsize,
    done: AtomicUsize,
    /// Time-mode window, in microseconds since [`EPOCH`]. `span_start` is when
    /// the timed recall began; `span_end` is when its longest pre-wait+fade
    /// finishes. The UI fills toward `(now - start) / (end - start)`. `span_end`
    /// can be moved earlier mid-flight (operator override shrinks the live
    /// max(pre_wait+fade)).
    span_start_us: AtomicU64,
    span_end_us: AtomicU64,
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
    pub mode: ProgressMode,
    pub total: usize,
    pub done: usize,
    pub span_start_us: u64,
    pub span_end_us: u64,
    pub generation: u64,
}

impl RecallProgressView {
    /// Fraction (0.0..=1.0) of the timed window elapsed *right now*. Recomputed
    /// each frame against the live `span_end`, so an operator override that
    /// shrinks the window makes the bar advance to meet the new, nearer finish.
    /// Returns 1.0 for a degenerate (zero-length) window.
    pub fn time_fraction(&self) -> f32 {
        let start = self.span_start_us;
        let end = self.span_end_us;
        if end <= start {
            return 1.0;
        }
        let now = now_us();
        let elapsed = now.saturating_sub(start) as f32;
        let total = (end - start) as f32;
        (elapsed / total).clamp(0.0, 1.0)
    }
}

impl RecallProgress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a new count-based operation. `total == 0` means the total is
    /// unknown (indeterminate); otherwise the bar fills toward `done / total`.
    pub fn begin(&self, kind: RecallKind, total: usize) {
        self.kind.store(kind.to_u8(), Ordering::Relaxed);
        self.mode
            .store(ProgressMode::Count.to_u8(), Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.active.store(true, Ordering::Release);
    }

    /// Start a new time-based operation: the bar fills over `total_span`
    /// (the recall's `max(pre_wait + fade)`). Returns the new generation so the
    /// caller's progress-driver task can tell when a later recall has superseded
    /// it. Pair with [`set_span_end`] (to follow a shrinking window) and
    /// [`finish`].
    pub fn begin_timed(&self, total_span: Duration) -> u64 {
        let start = now_us();
        self.kind
            .store(RecallKind::Recall.to_u8(), Ordering::Relaxed);
        self.mode
            .store(ProgressMode::Time.to_u8(), Ordering::Relaxed);
        self.total.store(0, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.span_start_us.store(start, Ordering::Relaxed);
        self.span_end_us
            .store(start + total_span.as_micros() as u64, Ordering::Relaxed);
        let g = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.active.store(true, Ordering::Release);
        g
    }

    /// Move the time-mode window's end to `remaining` from now. Used by the
    /// progress driver to track the live `max(pre_wait + fade)` as operator
    /// overrides shrink it. No-op effect on count-mode bars (they ignore the
    /// span), so it's safe to call unconditionally on the outbound handle.
    pub fn set_span_end(&self, remaining: Duration) {
        self.span_end_us
            .store(now_us() + remaining.as_micros() as u64, Ordering::Relaxed);
    }

    /// Current generation (bumped by every `begin` / `begin_timed`).
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
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
            mode: ProgressMode::from_u8(self.mode.load(Ordering::Relaxed)),
            total: self.total.load(Ordering::Relaxed),
            done: self.done.load(Ordering::Relaxed),
            span_start_us: self.span_start_us.load(Ordering::Relaxed),
            span_end_us: self.span_end_us.load(Ordering::Relaxed),
            generation: self.generation.load(Ordering::Relaxed),
        }
    }
}

/// The two independent progress bars rendered under the tab bar, following
/// the OSC-log color convention: **inbound** (the console dump flood, green)
/// and **outbound** (snapshot/cue/macro recall sends, blue). Splitting them
/// lets both animate at once — e.g. firing a cue while a reconnect dump is
/// still streaming. Cheap to clone; both fields are `Arc`.
#[derive(Clone, Debug, Default)]
pub struct ProgressBars {
    /// Inbound: driven by the daemon's console-dump / resend loop.
    pub inbound: Arc<RecallProgress>,
    /// Outbound: driven by the snapshot / cue / macro recall engines.
    pub outbound: Arc<RecallProgress>,
}

impl ProgressBars {
    pub fn new() -> Self {
        Self::default()
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
    fn begin_timed_sets_time_mode_and_fills_over_span() {
        let p = RecallProgress::new();
        let g = p.begin_timed(Duration::from_secs(10));
        let v = p.snapshot();
        assert_eq!(v.mode, ProgressMode::Time);
        assert!(v.active);
        assert_eq!(v.generation, g);
        // Just started → near 0, and certainly well below full.
        assert!(v.time_fraction() < 0.5);
        assert!(v.span_end_us > v.span_start_us);
    }

    #[test]
    fn set_span_end_pulls_the_window_in() {
        let p = RecallProgress::new();
        p.begin_timed(Duration::from_secs(100));
        let before = p.snapshot().time_fraction();
        // Operator override drops the long fade → only 0s remain. The bar should
        // now read effectively complete against the new, nearer finish.
        p.set_span_end(Duration::ZERO);
        let after = p.snapshot().time_fraction();
        assert!(after >= before);
        assert!(after >= 0.99);
    }

    #[test]
    fn begin_count_after_timed_restores_count_mode() {
        let p = RecallProgress::new();
        p.begin_timed(Duration::from_secs(5));
        assert_eq!(p.snapshot().mode, ProgressMode::Time);
        p.begin(RecallKind::Recall, 8);
        assert_eq!(p.snapshot().mode, ProgressMode::Count);
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
