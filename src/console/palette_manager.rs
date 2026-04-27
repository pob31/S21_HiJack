//! Manages the collection of channel palettes — EQ, Compressor (Dyn1), and
//! Gate (Dyn2). Generalized in Phase 5/6 from the original `EqPaletteManager`.
//!
//! Palettes of all kinds live in a single `HashMap<Uuid, ChannelPalette>`;
//! filtering by kind / channel happens at lookup time.

use std::collections::HashMap;

use tracing::info;
use uuid::Uuid;

use crate::model::channel::ChannelId;
use crate::model::palette::ChannelPalette;
use crate::model::parameter::PaletteKind;

/// Manages the collection of palettes, linking to snapshots, and ripple
/// tracking. All palette kinds live in one map keyed by UUID.
pub struct PaletteManager {
    /// All palettes indexed by UUID, regardless of kind.
    pub palettes: HashMap<Uuid, ChannelPalette>,
}

impl PaletteManager {
    pub fn new() -> Self {
        Self {
            palettes: HashMap::new(),
        }
    }

    // ─── CRUD ──────────────────────────────────────────────────────

    pub fn add_palette(&mut self, palette: ChannelPalette) {
        info!(
            name = %palette.name,
            id = %palette.id,
            kind = ?palette.kind,
            channel = %palette.channel,
            "Added palette"
        );
        self.palettes.insert(palette.id, palette);
    }

    pub fn remove_palette(&mut self, id: Uuid) -> bool {
        let removed = self.palettes.remove(&id).is_some();
        if removed {
            info!(%id, "Removed palette");
        }
        removed
    }

    pub fn get_palette(&self, id: &Uuid) -> Option<&ChannelPalette> {
        self.palettes.get(id)
    }

    pub fn get_palette_mut(&mut self, id: &Uuid) -> Option<&mut ChannelPalette> {
        self.palettes.get_mut(id)
    }

    /// Return all palettes sorted by (kind, name) for UI display.
    pub fn sorted_palettes(&self) -> Vec<&ChannelPalette> {
        let mut palettes: Vec<_> = self.palettes.values().collect();
        palettes.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
        palettes
    }

    /// Return only the palettes of a specific kind, sorted by name.
    pub fn sorted_palettes_of_kind(&self, kind: PaletteKind) -> Vec<&ChannelPalette> {
        let mut palettes: Vec<_> = self.palettes.values().filter(|p| p.kind == kind).collect();
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

    /// Return palettes for a specific (channel, kind) pair.
    pub fn palettes_for_channel_kind(
        &self,
        channel: &ChannelId,
        kind: PaletteKind,
    ) -> Vec<&ChannelPalette> {
        self.palettes
            .values()
            .filter(|p| &p.channel == channel && p.kind == kind)
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
    }

    /// Remove a snapshot back-reference from a palette.
    pub fn unlink_from_snapshot(&mut self, palette_id: Uuid, snapshot_id: Uuid) {
        if let Some(palette) = self.palettes.get_mut(&palette_id) {
            palette
                .referencing_snapshots
                .retain(|id| *id != snapshot_id);
            info!(palette = %palette.name, %snapshot_id, "Unlinked palette from snapshot");
        }
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
    }

    /// Return snapshot IDs that reference a given palette (for ripple tracking).
    pub fn affected_snapshots(&self, palette_id: &Uuid) -> &[Uuid] {
        self.palettes
            .get(palette_id)
            .map(|p| p.referencing_snapshots.as_slice())
            .unwrap_or(&[])
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
        ChannelPalette::new(name.into(), PaletteKind::Eq, channel, values)
    }

    fn make_dyn1_palette(name: &str, channel: ChannelId) -> ChannelPalette {
        let mut values = HashMap::new();
        values.insert(ParameterPath::Dyn1Enabled, ParameterValue::Bool(true));
        values.insert(
            ParameterPath::Dyn1Threshold(1),
            ParameterValue::Float(-12.0),
        );
        ChannelPalette::new(name.into(), PaletteKind::Dyn1, channel, values)
    }

    fn make_dyn2_palette(name: &str, channel: ChannelId) -> ChannelPalette {
        let mut values = HashMap::new();
        values.insert(ParameterPath::Dyn2Enabled, ParameterValue::Bool(true));
        values.insert(ParameterPath::Dyn2Threshold, ParameterValue::Float(-30.0));
        ChannelPalette::new(name.into(), PaletteKind::Dyn2, channel, values)
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
    fn sorted_palettes_orders_by_kind_then_name() {
        let mut mgr = PaletteManager::new();
        // Mix kinds and names; sorted_palettes() should group by kind first.
        mgr.add_palette(make_dyn1_palette("Drum Comp", ChannelId::Input(1)));
        mgr.add_palette(make_eq_palette("Zebra EQ", ChannelId::Input(2)));
        mgr.add_palette(make_eq_palette("Alpha EQ", ChannelId::Aux(1)));

        let sorted = mgr.sorted_palettes();
        // PaletteKind enum order is Eq, Dyn1, Dyn2 — Eq palettes come first,
        // then Dyn1.
        assert_eq!(sorted[0].name, "Alpha EQ");
        assert_eq!(sorted[1].name, "Zebra EQ");
        assert_eq!(sorted[2].name, "Drum Comp");
    }
}
