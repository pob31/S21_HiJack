//! Phase C: tracks parameter-level "dirty" state since the last snapshot
//! recall or capture.
//!
//! The scope editor uses this to power three operator workflows:
//!
//! - **"Select modified"** — one-shot button that copies the current dirty
//!   set into the editor's selections. Lets you snapshot exactly the
//!   parameters you've been twiddling since the last cue.
//! - **"Auto-preselect modified"** — toggle that does the same thing
//!   continuously. As you change parameters on the console, they appear in
//!   the matrix as selected cells.
//! - **"Clear changes"** — wipes the dirty set without sending anything to
//!   the console. Useful when the engineer wants a fresh baseline mid-show.
//!
//! Marks come from the OSC dispatcher whenever an inbound parameter update
//! actually changes the live state value. Echoes from our own writes
//! (snapshot recalls, cue fires, monitor sends) would otherwise pollute the
//! dirty set, so the snapshot/recall paths hold a [`SuppressionGuard`] from
//! [`DirtyTracker::suppress`] while they write. Marks made while suppression
//! is active are discarded.
//!
//! Granularity is per-`ParameterPath` per-`ChannelId`, matching the scope
//! editor's matrix cells.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use super::channel::ChannelId;
use super::parameter::{ParameterAddress, ParameterPath, ParameterSection};

/// Per-channel dirty parameter set, with suppression and a monotonic
/// generation counter the UI uses to decide when to refresh.
#[derive(Debug, Default)]
pub struct DirtyTracker {
    dirty: HashMap<ChannelId, HashSet<ParameterPath>>,
    /// While non-zero, `mark()` is a no-op. Suppression brackets any
    /// daemon-initiated writes (snapshot recall, cue fire, macro playback, …)
    /// so console echoes don't pollute the dirty set. A depth counter — not a
    /// bool — so overlapping brackets (e.g. a macro running while a cue
    /// recalls) can nest safely: suppression only lifts when the LAST bracket
    /// closes. Shared with each [`SuppressionGuard`] so the guard can close
    /// its bracket on drop without the tracker's lock.
    suppress_depth: Arc<AtomicU32>,
    /// Bumps on every state change (mark, clear, suppression toggle that
    /// affects content). The scope editor caches the last-seen generation so
    /// it knows when to re-pull the dirty set into its selections in
    /// auto-preselect mode.
    generation: u64,
}

impl DirtyTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a single (channel, parameter) cell dirty. No-op when
    /// suppression is active.
    pub fn mark(&mut self, addr: &ParameterAddress) {
        if self.is_suppressed() {
            return;
        }
        let inserted = self
            .dirty
            .entry(addr.channel.clone())
            .or_default()
            .insert(addr.parameter.clone());
        if inserted {
            self.generation = self.generation.wrapping_add(1);
        }
    }

    /// Clear every dirty cell. Bumps the generation if the set was non-empty.
    pub fn clear(&mut self) {
        if self.dirty.is_empty() {
            return;
        }
        self.dirty.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    /// Clear dirty cells for one channel whose parameter belongs to `section`.
    /// Used by the palette "Revert changes" action so the reloaded params stop
    /// showing as changed and the live palette-absorb loop doesn't re-capture
    /// them. Bumps the generation if anything was removed.
    pub fn clear_channel_section(&mut self, channel: &ChannelId, section: ParameterSection) {
        let mut changed = false;
        if let Some(paths) = self.dirty.get_mut(channel) {
            let before = paths.len();
            paths.retain(|p| p.section() != section);
            changed = paths.len() != before;
            if paths.is_empty() {
                self.dirty.remove(channel);
            }
        }
        if changed {
            self.generation = self.generation.wrapping_add(1);
        }
    }

    /// True if the (channel, path) cell is currently dirty.
    pub fn is_dirty(&self, channel: &ChannelId, path: &ParameterPath) -> bool {
        self.dirty
            .get(channel)
            .is_some_and(|set| set.contains(path))
    }

    /// Borrow the dirty set as a per-channel map. Used by the scope editor
    /// to compute "any cell in this section dirty" highlights.
    pub fn dirty_set(&self) -> &HashMap<ChannelId, HashSet<ParameterPath>> {
        &self.dirty
    }

    /// Begin suppression. Every `mark` is a no-op until the returned guard
    /// is dropped. Guards nest, so overlapping suppressors (cue recall +
    /// macro playback) can't turn marks back on while the other is still
    /// writing.
    ///
    /// The bracket closes on drop, so a recall that panics or whose future
    /// is cancelled still ends it. With a separate begin/end pair, a panic
    /// in between left suppression on for good, which silently disabled pan
    /// link, the palette absorb loop and cue auto-save (audit H3).
    #[must_use = "suppression ends when the guard is dropped"]
    pub fn suppress(&mut self) -> SuppressionGuard {
        self.suppress_depth.fetch_add(1, Ordering::SeqCst);
        SuppressionGuard {
            depth: Arc::clone(&self.suppress_depth),
        }
    }

    /// True while the tracker is suppressing marks. The pan link engine
    /// uses this as a "recall in progress" guard so it doesn't fight
    /// snapshot/cue/macro recalls.
    pub fn is_suppressed(&self) -> bool {
        self.suppress_depth.load(Ordering::SeqCst) > 0
    }

    /// True if any cell is currently dirty.
    pub fn has_any(&self) -> bool {
        self.dirty.values().any(|s| !s.is_empty())
    }

    /// Monotonic counter the UI watches to decide when to refresh.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// One open suppression bracket on a [`DirtyTracker`]. See
/// [`DirtyTracker::suppress`].
#[derive(Debug)]
pub struct SuppressionGuard {
    depth: Arc<AtomicU32>,
}

impl Drop for SuppressionGuard {
    fn drop(&mut self) {
        self.depth.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(ch: u16, path: ParameterPath) -> ParameterAddress {
        ParameterAddress {
            channel: ChannelId::Input(ch),
            parameter: path,
        }
    }

    #[test]
    fn new_is_empty_and_unsuppressed() {
        let t = DirtyTracker::new();
        assert!(!t.has_any());
        assert_eq!(t.generation(), 0);
        assert!(t.dirty_set().is_empty());
    }

    #[test]
    fn mark_records_path_for_channel_and_bumps_generation() {
        let mut t = DirtyTracker::new();
        t.mark(&addr(1, ParameterPath::Fader));
        assert!(t.is_dirty(&ChannelId::Input(1), &ParameterPath::Fader));
        assert!(t.has_any());
        assert_eq!(t.generation(), 1);
    }

    #[test]
    fn clear_channel_section_removes_only_that_section() {
        let mut t = DirtyTracker::new();
        t.mark(&addr(1, ParameterPath::EqBandGain(2))); // Eq
        t.mark(&addr(1, ParameterPath::Fader)); // FaderMutePan
        t.mark(&addr(2, ParameterPath::EqBandGain(1))); // other channel, Eq
        let gen_before = t.generation();

        t.clear_channel_section(&ChannelId::Input(1), ParameterSection::Eq);

        // The Eq cell on channel 1 is gone; its Fader stays; channel 2 untouched.
        assert!(!t.is_dirty(&ChannelId::Input(1), &ParameterPath::EqBandGain(2)));
        assert!(t.is_dirty(&ChannelId::Input(1), &ParameterPath::Fader));
        assert!(t.is_dirty(&ChannelId::Input(2), &ParameterPath::EqBandGain(1)));
        assert!(t.generation() > gen_before);

        // No-op clear (nothing in that section now) doesn't bump generation.
        let gen2 = t.generation();
        t.clear_channel_section(&ChannelId::Input(1), ParameterSection::Eq);
        assert_eq!(t.generation(), gen2);
    }

    #[test]
    fn marking_same_cell_twice_only_bumps_generation_once() {
        let mut t = DirtyTracker::new();
        t.mark(&addr(1, ParameterPath::Fader));
        t.mark(&addr(1, ParameterPath::Fader));
        assert_eq!(t.generation(), 1);
    }

    #[test]
    fn marking_different_cells_bumps_generation_each_time() {
        let mut t = DirtyTracker::new();
        t.mark(&addr(1, ParameterPath::Fader));
        t.mark(&addr(1, ParameterPath::Mute));
        t.mark(&addr(2, ParameterPath::Fader));
        assert_eq!(t.generation(), 3);
    }

    #[test]
    fn suppression_blocks_mark() {
        let mut t = DirtyTracker::new();
        let guard = t.suppress();
        t.mark(&addr(1, ParameterPath::Fader));
        assert!(!t.has_any());
        assert_eq!(t.generation(), 0);

        drop(guard);
        t.mark(&addr(1, ParameterPath::Fader));
        assert!(t.has_any());
        assert_eq!(t.generation(), 1);
    }

    #[test]
    fn suppression_nests_and_lifts_only_on_last_end() {
        let mut t = DirtyTracker::new();
        // Two overlapping brackets (e.g. macro playback + cue recall).
        let first = t.suppress();
        let last = t.suppress();
        drop(first); // first bracket closes — still suppressed
        assert!(t.is_suppressed());
        t.mark(&addr(1, ParameterPath::Fader));
        assert!(!t.has_any());

        drop(last); // last bracket closes — marks work again
        assert!(!t.is_suppressed());
        t.mark(&addr(1, ParameterPath::Fader));
        assert!(t.has_any());
    }

    #[test]
    fn a_panic_while_suppressed_still_ends_the_bracket() {
        let mut t = DirtyTracker::new();
        let guard = t.suppress();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = guard;
            panic!("recall blew up mid-bracket");
        }));
        assert!(result.is_err());
        assert!(!t.is_suppressed());
        t.mark(&addr(1, ParameterPath::Fader));
        assert!(t.has_any());
    }

    #[test]
    fn clear_empties_and_bumps_generation_when_nonempty() {
        let mut t = DirtyTracker::new();
        t.mark(&addr(1, ParameterPath::Fader));
        let g_before = t.generation();
        t.clear();
        assert!(!t.has_any());
        assert!(t.generation() > g_before);
    }

    #[test]
    fn clear_is_a_noop_when_already_empty() {
        let mut t = DirtyTracker::new();
        let g = t.generation();
        t.clear();
        assert_eq!(t.generation(), g);
    }

    #[test]
    fn dirty_set_groups_by_channel() {
        let mut t = DirtyTracker::new();
        t.mark(&addr(1, ParameterPath::Fader));
        t.mark(&addr(1, ParameterPath::Mute));
        t.mark(&addr(2, ParameterPath::Fader));

        let set = t.dirty_set();
        assert_eq!(set.len(), 2);
        assert_eq!(set.get(&ChannelId::Input(1)).unwrap().len(), 2);
        assert_eq!(set.get(&ChannelId::Input(2)).unwrap().len(), 1);
    }

    #[test]
    fn is_dirty_false_for_unmarked_cells() {
        let mut t = DirtyTracker::new();
        t.mark(&addr(1, ParameterPath::Fader));
        assert!(!t.is_dirty(&ChannelId::Input(1), &ParameterPath::Mute));
        assert!(!t.is_dirty(&ChannelId::Input(2), &ParameterPath::Fader));
    }
}
