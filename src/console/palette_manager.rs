//! Manages the collection of channel palettes — EQ, Compressor (Dyn1), and
//! Gate (Dyn2). Generalized in Phase 5/6 from the original `EqPaletteManager`.
//!
//! Palettes of all kinds live in a single `HashMap<Uuid, ChannelPalette>`;
//! filtering by kind / channel happens at lookup time.

use std::collections::HashMap;
use std::sync::Arc;

use tracing::info;
use uuid::Uuid;

use crate::console::recall_cache::RecallCache;
use crate::model::channel::ChannelId;
use crate::model::palette::ChannelPalette;
use crate::model::parameter::PaletteKind;

/// Manages the collection of palettes, linking to snapshots, and ripple
/// tracking. All palette kinds live in one map keyed by UUID.
pub struct PaletteManager {
    /// All palettes indexed by UUID, regardless of kind.
    pub palettes: HashMap<Uuid, ChannelPalette>,
    /// Optional look-ahead recall cache handle. Every palette mutation —
    /// including handing out `&mut` access, which covers the absorb loop's
    /// working-overlay writes and all UI edit paths — bumps its model
    /// generation so pre-resolved recall data can't go stale. A spurious
    /// bump only costs a background recompute. `None` in tests and before
    /// the first connect.
    model_gen: Option<Arc<RecallCache>>,
}

impl PaletteManager {
    pub fn new() -> Self {
        Self {
            palettes: HashMap::new(),
            model_gen: None,
        }
    }

    /// Attach the look-ahead recall cache so mutations invalidate it.
    pub fn set_model_gen(&mut self, cache: Arc<RecallCache>) {
        self.model_gen = Some(cache);
    }

    /// Notify the look-ahead recall cache that palette data changed. Also
    /// used by callers that mutate the pub `palettes` field directly (show
    /// load, new show). Harmless no-op when no cache is attached.
    pub fn bump_model_gen(&self) {
        if let Some(cache) = &self.model_gen {
            cache.bump();
        }
    }

    // ─── CRUD ──────────────────────────────────────────────────────

    pub fn add_palette(&mut self, palette: ChannelPalette) {
        info!(
            name = %palette.name,
            id = %palette.id,
            kinds = ?palette.kinds(),
            channel = %palette.channel,
            "Added palette"
        );
        self.palettes.insert(palette.id, palette);
        self.bump_model_gen();
    }

    pub fn remove_palette(&mut self, id: Uuid) -> bool {
        let removed = self.palettes.remove(&id).is_some();
        if removed {
            info!(%id, "Removed palette");
            self.bump_model_gen();
        }
        removed
    }

    pub fn get_palette(&self, id: &Uuid) -> Option<&ChannelPalette> {
        self.palettes.get(id)
    }

    /// Mutable palette access. Conservatively bumps the look-ahead cache's
    /// model generation — the caller may be about to change values the
    /// cache resolved against (this is the choke point for the absorb
    /// loop's `set_working` and the palette editor).
    pub fn get_palette_mut(&mut self, id: &Uuid) -> Option<&mut ChannelPalette> {
        self.bump_model_gen();
        self.palettes.get_mut(id)
    }

    /// Return all palettes sorted by name for UI display. Palettes can cover
    /// any subset of `{Eq, Dyn1, Dyn2}` so there's no single kind to group by.
    pub fn sorted_palettes(&self) -> Vec<&ChannelPalette> {
        let mut palettes: Vec<_> = self.palettes.values().collect();
        palettes.sort_by(|a, b| a.name.cmp(&b.name));
        palettes
    }

    /// Return palettes containing at least one parameter for `kind`, sorted
    /// by name.
    pub fn sorted_palettes_of_kind(&self, kind: PaletteKind) -> Vec<&ChannelPalette> {
        let mut palettes: Vec<_> = self
            .palettes
            .values()
            .filter(|p| p.has_kind(kind))
            .collect();
        palettes.sort_by(|a, b| a.name.cmp(&b.name));
        palettes
    }

    /// Return palettes that store values for a specific channel (any kind).
    pub fn palettes_for_channel(&self, channel: &ChannelId) -> Vec<&ChannelPalette> {
        self.palettes
            .values()
            .filter(|p| &p.channel == channel)
            .collect()
    }

    /// Return palettes on `channel` whose stored values include `kind`.
    pub fn palettes_for_channel_kind(
        &self,
        channel: &ChannelId,
        kind: PaletteKind,
    ) -> Vec<&ChannelPalette> {
        self.palettes
            .values()
            .filter(|p| &p.channel == channel && p.has_kind(kind))
            .collect()
    }

    // ─── Linking ───────────────────────────────────────────────────

    /// Add a snapshot back-reference to a palette.
    pub fn link_to_snapshot(&mut self, palette_id: Uuid, snapshot_id: Uuid) {
        if let Some(palette) = self.palettes.get_mut(&palette_id) {
            if !palette.referencing_snapshots.contains(&snapshot_id) {
                palette.referencing_snapshots.push(snapshot_id);
                info!(palette = %palette.name, %snapshot_id, "Linked palette to snapshot");
            }
        }
        self.bump_model_gen();
    }

    /// Remove a snapshot back-reference from a palette.
    pub fn unlink_from_snapshot(&mut self, palette_id: Uuid, snapshot_id: Uuid) {
        if let Some(palette) = self.palettes.get_mut(&palette_id) {
            palette
                .referencing_snapshots
                .retain(|id| *id != snapshot_id);
            info!(palette = %palette.name, %snapshot_id, "Unlinked palette from snapshot");
        }
        self.bump_model_gen();
    }

    /// Remove all back-references to a snapshot across all palettes.
    /// Called when a snapshot is deleted so the "Linked Snapshots" UI count
    /// stays accurate.
    pub fn unlink_all_from_snapshot(&mut self, snapshot_id: Uuid) {
        for palette in self.palettes.values_mut() {
            palette
                .referencing_snapshots
                .retain(|id| *id != snapshot_id);
        }
        self.bump_model_gen();
    }

    /// Return snapshot IDs that reference a given palette (for ripple tracking).
    pub fn affected_snapshots(&self, palette_id: &Uuid) -> &[Uuid] {
        self.palettes
            .get(palette_id)
            .map(|p| p.referencing_snapshots.as_slice())
            .unwrap_or(&[])
    }

    // ─── In-session working overlay ────────────────────────────────

    /// Commit every palette's in-session overlay. Returns the number of
    /// palettes that had changes committed (for a status message).
    pub fn store_all_changes(&mut self) -> usize {
        let mut changed = 0;
        for palette in self.palettes.values_mut() {
            if palette.store_changes() > 0 {
                changed += 1;
            }
        }
        if changed > 0 {
            self.bump_model_gen();
        }
        changed
    }

    /// Number of palettes carrying un-stored in-session changes.
    pub fn palettes_with_working_changes(&self) -> usize {
        self.palettes
            .values()
            .filter(|p| p.has_working_changes())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::parameter::{PaletteKind, ParameterPath, ParameterValue};
    use std::collections::HashMap;

    fn make_eq_palette(name: &str, channel: ChannelId) -> ChannelPalette {
        let mut values = HashMap::new();
        values.insert(ParameterPath::EqEnabled, ParameterValue::Bool(true));
        values.insert(
            ParameterPath::EqBandFrequency(1),
            ParameterValue::Float(1000.0),
        );
        ChannelPalette::new(name.into(), channel, &[PaletteKind::Eq], values)
    }

    fn make_dyn1_palette(name: &str, channel: ChannelId) -> ChannelPalette {
        let mut values = HashMap::new();
        values.insert(ParameterPath::Dyn1Enabled, ParameterValue::Bool(true));
        values.insert(
            ParameterPath::Dyn1Threshold(1),
            ParameterValue::Float(-12.0),
        );
        ChannelPalette::new(name.into(), channel, &[PaletteKind::Dyn1], values)
    }

    fn make_dyn2_palette(name: &str, channel: ChannelId) -> ChannelPalette {
        let mut values = HashMap::new();
        values.insert(ParameterPath::Dyn2Enabled, ParameterValue::Bool(true));
        values.insert(ParameterPath::Dyn2Threshold, ParameterValue::Float(-30.0));
        ChannelPalette::new(name.into(), channel, &[PaletteKind::Dyn2], values)
    }

    #[test]
    fn crud_lifecycle() {
        let mut mgr = PaletteManager::new();
        let palette = make_eq_palette("Vocal EQ", ChannelId::Input(1));
        let id = palette.id;

        mgr.add_palette(palette);
        assert!(mgr.get_palette(&id).is_some());
        assert_eq!(mgr.get_palette(&id).unwrap().name, "Vocal EQ");

        assert!(mgr.remove_palette(id));
        assert!(mgr.get_palette(&id).is_none());
        assert!(!mgr.remove_palette(id));
    }

    #[test]
    fn palettes_for_channel_returns_all_kinds() {
        let mut mgr = PaletteManager::new();
        mgr.add_palette(make_eq_palette("Vocal EQ", ChannelId::Input(1)));
        mgr.add_palette(make_dyn1_palette("Vocal Comp", ChannelId::Input(1)));
        mgr.add_palette(make_dyn2_palette("Vocal Gate", ChannelId::Input(1)));
        mgr.add_palette(make_eq_palette("Drum EQ", ChannelId::Input(2)));

        // All three kinds for Input 1.
        assert_eq!(mgr.palettes_for_channel(&ChannelId::Input(1)).len(), 3);
        // Just Input 2.
        assert_eq!(mgr.palettes_for_channel(&ChannelId::Input(2)).len(), 1);
    }

    #[test]
    fn palettes_for_channel_kind_filters() {
        let mut mgr = PaletteManager::new();
        mgr.add_palette(make_eq_palette("Vocal EQ", ChannelId::Input(1)));
        mgr.add_palette(make_dyn1_palette("Vocal Comp", ChannelId::Input(1)));
        mgr.add_palette(make_dyn2_palette("Vocal Gate", ChannelId::Input(1)));

        let eq = mgr.palettes_for_channel_kind(&ChannelId::Input(1), PaletteKind::Eq);
        assert_eq!(eq.len(), 1);
        assert_eq!(eq[0].name, "Vocal EQ");

        let dyn1 = mgr.palettes_for_channel_kind(&ChannelId::Input(1), PaletteKind::Dyn1);
        assert_eq!(dyn1.len(), 1);
        assert_eq!(dyn1[0].name, "Vocal Comp");

        let dyn2 = mgr.palettes_for_channel_kind(&ChannelId::Input(1), PaletteKind::Dyn2);
        assert_eq!(dyn2.len(), 1);
        assert_eq!(dyn2[0].name, "Vocal Gate");
    }

    #[test]
    fn sorted_palettes_of_kind_filters() {
        let mut mgr = PaletteManager::new();
        mgr.add_palette(make_eq_palette("Zebra EQ", ChannelId::Input(1)));
        mgr.add_palette(make_dyn1_palette("Drum Comp", ChannelId::Input(1)));
        mgr.add_palette(make_eq_palette("Alpha EQ", ChannelId::Input(1)));

        let eq = mgr.sorted_palettes_of_kind(PaletteKind::Eq);
        assert_eq!(eq.len(), 2);
        assert_eq!(eq[0].name, "Alpha EQ");
        assert_eq!(eq[1].name, "Zebra EQ");

        let dyn1 = mgr.sorted_palettes_of_kind(PaletteKind::Dyn1);
        assert_eq!(dyn1.len(), 1);
    }

    #[test]
    fn link_and_unlink() {
        let mut mgr = PaletteManager::new();
        let palette = make_eq_palette("Test", ChannelId::Input(1));
        let pid = palette.id;
        mgr.add_palette(palette);

        let snap1 = Uuid::new_v4();
        let snap2 = Uuid::new_v4();

        mgr.link_to_snapshot(pid, snap1);
        mgr.link_to_snapshot(pid, snap2);
        // Duplicate link is a no-op
        mgr.link_to_snapshot(pid, snap1);

        assert_eq!(mgr.affected_snapshots(&pid).len(), 2);
        assert!(mgr.affected_snapshots(&pid).contains(&snap1));
        assert!(mgr.affected_snapshots(&pid).contains(&snap2));

        mgr.unlink_from_snapshot(pid, snap1);
        assert_eq!(mgr.affected_snapshots(&pid).len(), 1);
        assert!(!mgr.affected_snapshots(&pid).contains(&snap1));
    }

    #[test]
    fn unlink_all_from_snapshot() {
        let mut mgr = PaletteManager::new();
        let p1 = make_eq_palette("P1", ChannelId::Input(1));
        let p2 = make_dyn1_palette("P2", ChannelId::Input(2));
        let pid1 = p1.id;
        let pid2 = p2.id;
        mgr.add_palette(p1);
        mgr.add_palette(p2);

        let snap = Uuid::new_v4();
        mgr.link_to_snapshot(pid1, snap);
        mgr.link_to_snapshot(pid2, snap);

        mgr.unlink_all_from_snapshot(snap);
        assert!(mgr.affected_snapshots(&pid1).is_empty());
        assert!(mgr.affected_snapshots(&pid2).is_empty());
    }

    #[test]
    fn affected_snapshots_unknown_palette() {
        let mgr = PaletteManager::new();
        assert!(mgr.affected_snapshots(&Uuid::new_v4()).is_empty());
    }

    #[test]
    fn sorted_palettes_orders_by_name() {
        let mut mgr = PaletteManager::new();
        mgr.add_palette(make_dyn1_palette("Drum Comp", ChannelId::Input(1)));
        mgr.add_palette(make_eq_palette("Zebra EQ", ChannelId::Input(2)));
        mgr.add_palette(make_eq_palette("Alpha EQ", ChannelId::Aux(1)));

        let sorted = mgr.sorted_palettes();
        assert_eq!(sorted[0].name, "Alpha EQ");
        assert_eq!(sorted[1].name, "Drum Comp");
        assert_eq!(sorted[2].name, "Zebra EQ");
    }

    #[test]
    fn multi_kind_palette_appears_in_each_kind_filter() {
        let mut mgr = PaletteManager::new();
        let mut values = HashMap::new();
        values.insert(ParameterPath::EqEnabled, ParameterValue::Bool(true));
        values.insert(ParameterPath::Dyn1Enabled, ParameterValue::Bool(true));
        let full = ChannelPalette::new(
            "Full chain".into(),
            ChannelId::Input(7),
            &[PaletteKind::Eq, PaletteKind::Dyn1],
            values,
        );
        mgr.add_palette(full);
        mgr.add_palette(make_dyn2_palette("Gate only", ChannelId::Input(7)));

        // "Full chain" covers Eq AND Dyn1 — should appear in both filters.
        let eq = mgr.sorted_palettes_of_kind(PaletteKind::Eq);
        assert_eq!(eq.len(), 1);
        assert_eq!(eq[0].name, "Full chain");

        let dyn1 = mgr.sorted_palettes_of_kind(PaletteKind::Dyn1);
        assert_eq!(dyn1.len(), 1);
        assert_eq!(dyn1[0].name, "Full chain");

        // Dyn2 filter sees only the gate.
        let dyn2 = mgr.sorted_palettes_of_kind(PaletteKind::Dyn2);
        assert_eq!(dyn2.len(), 1);
        assert_eq!(dyn2[0].name, "Gate only");
    }
}
