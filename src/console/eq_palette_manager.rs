use std::collections::HashMap;

use tracing::info;
use uuid::Uuid;

use crate::model::channel::ChannelId;
use crate::model::eq_palette::EqPalette;

/// Manages the collection of EQ palettes, linking to snapshots, and ripple tracking.
pub struct EqPaletteManager {
    /// All palettes indexed by UUID.
    pub palettes: HashMap<Uuid, EqPalette>,
}

impl EqPaletteManager {
    pub fn new() -> Self {
        Self {
            palettes: HashMap::new(),
        }
    }

    // ─── CRUD ──────────────────────────────────────────────────────

    pub fn add_palette(&mut self, palette: EqPalette) {
        info!(name = %palette.name, id = %palette.id, channel = %palette.channel, "Added EQ palette");
        self.palettes.insert(palette.id, palette);
    }

    pub fn remove_palette(&mut self, id: Uuid) -> bool {
        let removed = self.palettes.remove(&id).is_some();
        if removed {
            info!(%id, "Removed EQ palette");
        }
        removed
    }

    pub fn get_palette(&self, id: &Uuid) -> Option<&EqPalette> {
        self.palettes.get(id)
    }

    pub fn get_palette_mut(&mut self, id: &Uuid) -> Option<&mut EqPalette> {
        self.palettes.get_mut(id)
    }

    /// Return all palettes sorted by name (for UI display).
    pub fn sorted_palettes(&self) -> Vec<&EqPalette> {
        let mut palettes: Vec<_> = self.palettes.values().collect();
        palettes.sort_by(|a, b| a.name.cmp(&b.name));
        palettes
    }

    /// Return palettes that store EQ for a specific channel.
    pub fn palettes_for_channel(&self, channel: &ChannelId) -> Vec<&EqPalette> {
        self.palettes
            .values()
            .filter(|p| &p.channel == channel)
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
            palette.referencing_snapshots.retain(|id| *id != snapshot_id);
            info!(palette = %palette.name, %snapshot_id, "Unlinked palette from snapshot");
        }
    }

    /// Remove all back-references to a snapshot across all palettes.
    /// Called when a snapshot is deleted.
    pub fn unlink_all_from_snapshot(&mut self, snapshot_id: Uuid) {
        for palette in self.palettes.values_mut() {
            palette.referencing_snapshots.retain(|id| *id != snapshot_id);
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
    use crate::model::parameter::{ParameterPath, ParameterValue};
    use std::collections::HashMap;

    fn make_palette(name: &str, channel: ChannelId) -> EqPalette {
        let mut eq_values = HashMap::new();
        eq_values.insert(ParameterPath::EqEnabled, ParameterValue::Bool(true));
        eq_values.insert(ParameterPath::EqBandFrequency(1), ParameterValue::Float(1000.0));
        EqPalette::new(name.to_string(), channel, eq_values)
    }

    #[test]
    fn crud_lifecycle() {
        let mut mgr = EqPaletteManager::new();
        let palette = make_palette("Vocal EQ", ChannelId::Input(1));
        let id = palette.id;

        mgr.add_palette(palette);
        assert!(mgr.get_palette(&id).is_some());
        assert_eq!(mgr.get_palette(&id).unwrap().name, "Vocal EQ");

        assert!(mgr.remove_palette(id));
        assert!(mgr.get_palette(&id).is_none());
        assert!(!mgr.remove_palette(id)); // already removed
    }

    #[test]
    fn palettes_for_channel() {
        let mut mgr = EqPaletteManager::new();
        mgr.add_palette(make_palette("Input 1 EQ", ChannelId::Input(1)));
        mgr.add_palette(make_palette("Input 1 Alt", ChannelId::Input(1)));
        mgr.add_palette(make_palette("Aux 1 EQ", ChannelId::Aux(1)));

        let input1 = mgr.palettes_for_channel(&ChannelId::Input(1));
        assert_eq!(input1.len(), 2);

        let aux1 = mgr.palettes_for_channel(&ChannelId::Aux(1));
        assert_eq!(aux1.len(), 1);

        let input2 = mgr.palettes_for_channel(&ChannelId::Input(2));
        assert!(input2.is_empty());
    }

    #[test]
    fn link_and_unlink() {
        let mut mgr = EqPaletteManager::new();
        let palette = make_palette("Test", ChannelId::Input(1));
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
        let mut mgr = EqPaletteManager::new();
        let p1 = make_palette("P1", ChannelId::Input(1));
        let p2 = make_palette("P2", ChannelId::Input(2));
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
        let mgr = EqPaletteManager::new();
        assert!(mgr.affected_snapshots(&Uuid::new_v4()).is_empty());
    }

    #[test]
    fn sorted_palettes() {
        let mut mgr = EqPaletteManager::new();
        mgr.add_palette(make_palette("Zebra", ChannelId::Input(1)));
        mgr.add_palette(make_palette("Alpha", ChannelId::Input(2)));
        mgr.add_palette(make_palette("Middle", ChannelId::Aux(1)));

        let sorted = mgr.sorted_palettes();
        assert_eq!(sorted[0].name, "Alpha");
        assert_eq!(sorted[1].name, "Middle");
        assert_eq!(sorted[2].name, "Zebra");
    }
}
