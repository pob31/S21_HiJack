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
//! dirty set, so the snapshot/recall paths bracket their writes with
//! `begin_suppression()` / `end_suppression()` calls. Marks made while
//! suppression is active are discarded.
//!
//! Granularity is per-`ParameterPath` per-`ChannelId`, matching the scope
//! editor's matrix cells.

use std::collections::{HashMap, HashSet};

use super::channel::ChannelId;
use super::parameter::{ParameterAddress, ParameterPath};

/// Per-channel dirty parameter set, with suppression and a monotonic
/// generation counter the UI uses to decide when to refresh.
#[derive(Debug, Default)]
pub struct DirtyTracker {
    dirty: HashMap<ChannelId, HashSet<ParameterPath>>,
    /// While true, `mark()` is a no-op. Bracket suppression around any
    /// daemon-initiated writes (snapshot recall, cue fire, …) so console
    /// echoes don't pollute the dirty set.
    suppressed: bool,
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
        if self.suppressed {
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

    /// Begin suppression — every subsequent `mark` is a no-op until
    /// `end_suppression` is called. Calls nest by simply staying suppressed
    /// (no counter), so the engine should never call `begin_suppression`
    /// reentrantly without a matching `end_suppression`.
    pub fn begin_suppression(&mut self) {
        self.suppressed = true;
    }

    /// End suppression. Subsequent `mark` calls take effect again.
    pub fn end_suppression(&mut self) {
        self.suppressed = false;
    }

    /// True while the tracker is suppressing marks. The pan link engine
    /// uses this as a "recall in progress" guard so it doesn't fight
    /// snapshot/cue/macro recalls.
    pub fn is_suppressed(&self) -> bool {
        self.suppressed
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

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(ch: u8, path: ParameterPath) -> ParameterAddress {
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
        t.begin_suppression();
        t.mark(&addr(1, ParameterPath::Fader));
        assert!(!t.has_any());
        assert_eq!(t.generation(), 0);

        t.end_suppression();
        t.mark(&addr(1, ParameterPath::Fader));
        assert!(t.has_any());
        assert_eq!(t.generation(), 1);
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
