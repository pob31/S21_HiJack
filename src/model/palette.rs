//! Channel palettes — reusable per-channel templates for any combination of
//! EQ, Compressor (Dyn1), and Gate (Dyn2) values.
//!
//! A single palette can hold values across any of the three processes so an
//! operator who wants to lock the whole vocal chain of one actor doesn't have
//! to maintain three parallel palettes. The recall engine still looks up
//! palettes per `(channel, kind)` via `Snapshot::palette_refs`, so different
//! snapshots can pull just one process from the palette and leave the others
//! to a different palette.
//!
//! Modifying a palette "ripples" to all referencing snapshots automatically
//! on next recall — see [crate::console::snapshot_engine::SnapshotEngine].

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::channel::ChannelId;
use super::parameter::{PaletteKind, ParameterPath, ParameterSection, ParameterValue};

/// A reusable per-channel parameter template. May store values for any subset
/// of `{Eq, Dyn1, Dyn2}` — the kind set is derived at runtime from the
/// sections present in `values` (see [`kinds`](ChannelPalette::kinds)).
///
/// Snapshots tag each `(ChannelId, PaletteKind)` slot with a palette UUID via
/// `Snapshot::palette_refs`; at recall time the engine substitutes palette
/// values for the snapshot's stored values within the matching section. A
/// snapshot can link several different palettes for the same channel as long
/// as they each provide a different kind.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelPalette {
    pub id: Uuid,
    pub name: String,
    /// Which channel this palette stores values for.
    pub channel: ChannelId,
    /// Parameter values across any of EQ / Dyn1 / Dyn2 sections. The serde
    /// alias `eq_values` lets v8 show files load — they had a field of that
    /// name on the old `EqPalette`.
    #[serde(with = "palette_values_serde", alias = "eq_values")]
    pub values: HashMap<ParameterPath, ParameterValue>,
    /// Back-references: snapshot IDs that link to this palette on at least
    /// one `(channel, kind)` slot. Used by the UI for the ref count and to
    /// drive ripple-count status messages after re-capture.
    pub referencing_snapshots: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    /// Transient in-session adjustments that ripple to linked snapshots at
    /// recall time but are NOT persisted to the show file (`#[serde(skip)]`).
    /// Kept as a *diff overlay*: an entry exists only while the operator's live
    /// value differs from the permanent `values`. `store_changes` folds these
    /// into `values` and clears the overlay; an un-stored overlay is discarded
    /// on reload. Populated by the live palette-absorb loop
    /// (see [crate::console::palette_tracker]).
    #[serde(skip)]
    pub working_values: HashMap<ParameterPath, ParameterValue>,
    /// Transient, in-memory content revision: a monotonic counter bumped by
    /// every method that changes what recall would send (stored `values` via
    /// `touch`, and the live `working_values` overlay via `set_working` /
    /// `discard_working`). NOT persisted (`#[serde(skip)]`, resets to 0 on
    /// load) and NOT part of a palette's identity — it exists only so the
    /// recall engine can tell "same palette, same content as last sent" from
    /// "same palette, edited since" and skip re-sending unchanged palettes on
    /// recall (see `SnapshotEngine::last_sent_palettes`). Note `modified_at`
    /// alone can't serve this: the working overlay must move the revision, but
    /// the overlay is transient and must not dirty the persisted timestamp.
    #[serde(skip)]
    content_rev: u64,
}

impl ChannelPalette {
    /// Create a new palette covering the given `kinds`. Filters `values` to
    /// only those whose section maps to one of the requested kinds — defensive
    /// against the caller passing in a capture result that includes unrelated
    /// params.
    pub fn new(
        name: String,
        channel: ChannelId,
        kinds: &[PaletteKind],
        values: HashMap<ParameterPath, ParameterValue>,
    ) -> Self {
        let allowed: Vec<ParameterSection> = kinds.iter().map(|k| k.section()).collect();
        let values: HashMap<_, _> = values
            .into_iter()
            .filter(|(p, _)| allowed.contains(&p.section()))
            .collect();
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            channel,
            values,
            referencing_snapshots: Vec::new(),
            created_at: now,
            modified_at: now,
            working_values: HashMap::new(),
            content_rev: 0,
        }
    }

    /// Update the modified timestamp. Call after editing values. Also bumps the
    /// transient content revision so recall knows the stored values changed.
    pub fn touch(&mut self) {
        self.modified_at = Utc::now();
        self.content_rev = self.content_rev.wrapping_add(1);
    }

    /// The transient in-memory content revision — bumps whenever the effective
    /// recall output changes (stored values via [`touch`](Self::touch), or the
    /// live overlay via [`set_working`](Self::set_working) /
    /// [`discard_working`](Self::discard_working)). See the field docs.
    pub fn content_rev(&self) -> u64 {
        self.content_rev
    }

    // ─── In-session working overlay (live ripple) ──────────────────

    /// Record a live operator adjustment to `path`. The overlay is a *diff*:
    /// if `value` equals the permanent stored value, the entry is removed
    /// (the adjustment is back at baseline); otherwise it is stored. This keeps
    /// `has_working_changes` honest after a [`store_changes`](Self::store_changes)
    /// — the absorb loop re-sees the same live value, finds it equal to the now-
    /// stored value, and leaves the overlay empty.
    pub fn set_working(&mut self, path: ParameterPath, value: ParameterValue) {
        let changed = if self.values.get(&path) == Some(&value) {
            self.working_values.remove(&path).is_some()
        } else {
            self.working_values.insert(path, value.clone()) != Some(value)
        };
        // Only bump the revision when the overlay actually moved, so a no-op
        // re-absorb of an unchanged value (the 150 ms absorb loop) doesn't
        // needlessly defeat the recall skip.
        if changed {
            self.content_rev = self.content_rev.wrapping_add(1);
        }
    }

    /// True if there are un-stored in-session adjustments.
    pub fn has_working_changes(&self) -> bool {
        !self.working_values.is_empty()
    }

    /// Number of un-stored adjusted parameters.
    pub fn working_count(&self) -> usize {
        self.working_values.len()
    }

    /// The value recall should send for `path`: the live overlay value if the
    /// operator has adjusted it this session, otherwise the permanent value.
    pub fn effective_value(&self, path: &ParameterPath) -> Option<&ParameterValue> {
        self.working_values
            .get(path)
            .or_else(|| self.values.get(path))
    }

    /// Iterate every stored parameter with its *effective* value (overlay wins),
    /// plus any overlay-only parameters not present in `values`. Used by recall
    /// so all linked snapshots ripple the live adjustments.
    pub fn iter_effective(&self) -> impl Iterator<Item = (&ParameterPath, &ParameterValue)> {
        self.values
            .iter()
            .map(move |(k, v)| (k, self.working_values.get(k).unwrap_or(v)))
            .chain(
                self.working_values
                    .iter()
                    .filter(move |(k, _)| !self.values.contains_key(*k)),
            )
    }

    /// Commit the in-session overlay into the permanent `values` (making the
    /// rippled changes storable in the show file) and clear the overlay.
    /// Returns the number of parameters committed. Touches `modified_at`.
    pub fn store_changes(&mut self) -> usize {
        let n = self.working_values.len();
        if n > 0 {
            for (path, value) in self.working_values.drain() {
                self.values.insert(path, value);
            }
            self.touch();
        }
        n
    }

    /// Discard the in-session overlay without committing (revert to stored).
    pub fn discard_working(&mut self) {
        if !self.working_values.is_empty() {
            self.working_values.clear();
            self.content_rev = self.content_rev.wrapping_add(1);
        }
    }

    /// Total number of stored parameters across every kind.
    pub fn parameter_count(&self) -> usize {
        self.values.len()
    }

    /// Number of stored parameters for one kind.
    pub fn parameter_count_for(&self, kind: PaletteKind) -> usize {
        let section = kind.section();
        self.values
            .keys()
            .filter(|p| p.section() == section)
            .count()
    }

    /// True if the palette stores at least one parameter for `kind`.
    pub fn has_kind(&self, kind: PaletteKind) -> bool {
        let section = kind.section();
        self.values.keys().any(|p| p.section() == section)
    }

    /// Kinds present in this palette, in canonical Eq → Dyn1 → Dyn2 order
    /// so UI column layout stays stable as values are added or removed.
    pub fn kinds(&self) -> Vec<PaletteKind> {
        PaletteKind::all()
            .iter()
            .copied()
            .filter(|k| self.has_kind(*k))
            .collect()
    }
}

/// Custom serde for `HashMap<ParameterPath, ParameterValue>` — serializes as
/// a Vec of entries since `ParameterPath` (a tagged enum with parameterised
/// variants) can't be a JSON map key. Same approach as the existing
/// `parameter_map` / `palette_refs_serde` modules elsewhere in the crate.
mod palette_values_serde {
    use super::*;
    use serde::{Deserializer, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Entry {
        path: ParameterPath,
        value: ParameterValue,
    }

    pub fn serialize<S>(
        map: &HashMap<ParameterPath, ParameterValue>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let entries: Vec<Entry> = map
            .iter()
            .map(|(k, v)| Entry {
                path: k.clone(),
                value: v.clone(),
            })
            .collect();
        entries.serialize(serializer)
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<HashMap<ParameterPath, ParameterValue>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let entries: Vec<Entry> = Vec::deserialize(deserializer)?;
        Ok(entries.into_iter().map(|e| (e.path, e.value)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::parameter::{ParameterPath, ParameterValue};

    fn sample_eq_values() -> HashMap<ParameterPath, ParameterValue> {
        let mut m = HashMap::new();
        m.insert(ParameterPath::EqEnabled, ParameterValue::Bool(true));
        m.insert(
            ParameterPath::EqBandFrequency(1),
            ParameterValue::Float(1000.0),
        );
        m.insert(ParameterPath::EqBandGain(1), ParameterValue::Float(3.0));
        m.insert(ParameterPath::EqBandQ(1), ParameterValue::Float(2.0));
        m.insert(ParameterPath::HighpassEnabled, ParameterValue::Bool(true));
        m.insert(
            ParameterPath::HighpassFrequency,
            ParameterValue::Float(80.0),
        );
        m
    }

    fn sample_dyn1_values() -> HashMap<ParameterPath, ParameterValue> {
        let mut m = HashMap::new();
        m.insert(ParameterPath::Dyn1Enabled, ParameterValue::Bool(true));
        m.insert(
            ParameterPath::Dyn1Threshold(1),
            ParameterValue::Float(-12.0),
        );
        m.insert(ParameterPath::Dyn1Ratio(1), ParameterValue::Float(4.0));
        m.insert(ParameterPath::Dyn1Attack(1), ParameterValue::Float(0.005));
        m.insert(ParameterPath::Dyn1Release(1), ParameterValue::Float(0.15));
        m
    }

    fn sample_dyn2_values() -> HashMap<ParameterPath, ParameterValue> {
        let mut m = HashMap::new();
        m.insert(ParameterPath::Dyn2Enabled, ParameterValue::Bool(true));
        m.insert(ParameterPath::Dyn2Threshold, ParameterValue::Float(-30.0));
        m.insert(ParameterPath::Dyn2Range, ParameterValue::Float(-40.0));
        m.insert(ParameterPath::Dyn2Attack, ParameterValue::Float(0.001));
        m.insert(ParameterPath::Dyn2Release, ParameterValue::Float(0.1));
        m
    }

    #[test]
    fn single_kind_palette() {
        let palette = ChannelPalette::new(
            "Vocal EQ".into(),
            ChannelId::Input(1),
            &[PaletteKind::Eq],
            sample_eq_values(),
        );
        assert_eq!(palette.name, "Vocal EQ");
        assert_eq!(palette.channel, ChannelId::Input(1));
        assert_eq!(palette.parameter_count(), 6);
        assert_eq!(palette.kinds(), vec![PaletteKind::Eq]);
        assert!(palette.has_kind(PaletteKind::Eq));
        assert!(!palette.has_kind(PaletteKind::Dyn1));
        assert_eq!(palette.parameter_count_for(PaletteKind::Eq), 6);
        assert_eq!(palette.parameter_count_for(PaletteKind::Dyn1), 0);
        assert!(palette.referencing_snapshots.is_empty());
    }

    #[test]
    fn multi_kind_palette_stores_all_sections() {
        let mut all = sample_eq_values();
        all.extend(sample_dyn1_values());
        all.extend(sample_dyn2_values());
        let palette = ChannelPalette::new(
            "Vocal full chain".into(),
            ChannelId::Input(7),
            &[PaletteKind::Eq, PaletteKind::Dyn1, PaletteKind::Dyn2],
            all,
        );
        assert_eq!(palette.parameter_count(), 16);
        assert_eq!(
            palette.kinds(),
            vec![PaletteKind::Eq, PaletteKind::Dyn1, PaletteKind::Dyn2]
        );
        assert_eq!(palette.parameter_count_for(PaletteKind::Eq), 6);
        assert_eq!(palette.parameter_count_for(PaletteKind::Dyn1), 5);
        assert_eq!(palette.parameter_count_for(PaletteKind::Dyn2), 5);
    }

    #[test]
    fn new_filters_to_requested_kinds_only() {
        // Mixed values: EQ + Dyn1 + Fader. Asking for only EQ keeps just EQ.
        let mut mixed = sample_eq_values();
        mixed.extend(sample_dyn1_values());
        mixed.insert(ParameterPath::Fader, ParameterValue::Float(-10.0));
        mixed.insert(ParameterPath::AnalogGain, ParameterValue::Float(20.0));

        let eq_palette = ChannelPalette::new(
            "EQ".into(),
            ChannelId::Input(1),
            &[PaletteKind::Eq],
            mixed.clone(),
        );
        assert_eq!(eq_palette.parameter_count(), 6);
        assert!(!eq_palette.values.contains_key(&ParameterPath::Fader));
        assert!(!eq_palette.values.contains_key(&ParameterPath::Dyn1Enabled));
        assert_eq!(eq_palette.kinds(), vec![PaletteKind::Eq]);

        // EQ + Dyn1 (but not Dyn2 or Fader) keeps both ducked sections.
        let chain = ChannelPalette::new(
            "EQ+Comp".into(),
            ChannelId::Input(1),
            &[PaletteKind::Eq, PaletteKind::Dyn1],
            mixed,
        );
        assert_eq!(chain.parameter_count(), 11);
        assert!(!chain.values.contains_key(&ParameterPath::Fader));
        assert_eq!(chain.kinds(), vec![PaletteKind::Eq, PaletteKind::Dyn1]);
    }

    #[test]
    fn kinds_returns_canonical_order() {
        // Build a palette covering only Dyn2 and Eq — kinds() must report
        // them in Eq → Dyn2 order, not insertion order.
        let mut vals = sample_dyn2_values();
        vals.extend(sample_eq_values());
        let palette = ChannelPalette::new(
            "Mixed".into(),
            ChannelId::Input(1),
            &[PaletteKind::Eq, PaletteKind::Dyn2],
            vals,
        );
        assert_eq!(palette.kinds(), vec![PaletteKind::Eq, PaletteKind::Dyn2]);
    }

    #[test]
    fn touch_updates_modified_at() {
        let mut palette = ChannelPalette::new(
            "Test".into(),
            ChannelId::Input(1),
            &[PaletteKind::Eq],
            sample_eq_values(),
        );
        let before = palette.modified_at;
        std::thread::sleep(std::time::Duration::from_millis(10));
        palette.touch();
        assert!(palette.modified_at > before);
    }

    #[test]
    fn working_overlay_diff_and_store() {
        let mut p = ChannelPalette::new(
            "Vocal EQ".into(),
            ChannelId::Input(1),
            &[PaletteKind::Eq],
            sample_eq_values(),
        );
        // sample has EqBandGain(1) = 3.0; clean to start.
        assert!(!p.has_working_changes());
        assert_eq!(
            p.effective_value(&ParameterPath::EqBandGain(1)),
            Some(&ParameterValue::Float(3.0))
        );

        // Adjust to a new value → overlay holds it and wins in effective_value.
        p.set_working(ParameterPath::EqBandGain(1), ParameterValue::Float(-6.0));
        assert!(p.has_working_changes());
        assert_eq!(p.working_count(), 1);
        assert_eq!(
            p.effective_value(&ParameterPath::EqBandGain(1)),
            Some(&ParameterValue::Float(-6.0))
        );

        // Setting it back to the stored value clears the diff entry.
        p.set_working(ParameterPath::EqBandGain(1), ParameterValue::Float(3.0));
        assert!(!p.has_working_changes());

        // Re-adjust and store → folds into `values`, clears overlay, touches.
        p.set_working(ParameterPath::EqBandGain(1), ParameterValue::Float(-6.0));
        let before = p.modified_at;
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert_eq!(p.store_changes(), 1);
        assert!(!p.has_working_changes());
        assert_eq!(
            p.values.get(&ParameterPath::EqBandGain(1)),
            Some(&ParameterValue::Float(-6.0))
        );
        assert!(p.modified_at > before);

        // Re-absorbing the now-stored value is a no-op (stays clean) — this is
        // what keeps "Store changes" from immediately re-flagging as modified.
        p.set_working(ParameterPath::EqBandGain(1), ParameterValue::Float(-6.0));
        assert!(!p.has_working_changes());
    }

    #[test]
    fn content_rev_bumps_on_effective_content_changes() {
        let mut p = ChannelPalette::new(
            "Vocal EQ".into(),
            ChannelId::Input(1),
            &[PaletteKind::Eq],
            sample_eq_values(),
        );
        let r0 = p.content_rev();

        // touch (stored-value edit / rename / recapture) bumps.
        p.touch();
        let r1 = p.content_rev();
        assert!(r1 > r0, "touch must bump content_rev");

        // A real overlay change bumps.
        p.set_working(ParameterPath::EqBandGain(1), ParameterValue::Float(-6.0));
        let r2 = p.content_rev();
        assert!(
            r2 > r1,
            "set_working with a new value must bump content_rev"
        );

        // Re-absorbing the SAME overlay value is a no-op (must NOT bump, else
        // the 150 ms absorb loop would defeat the recall skip every tick).
        p.set_working(ParameterPath::EqBandGain(1), ParameterValue::Float(-6.0));
        assert_eq!(
            p.content_rev(),
            r2,
            "re-absorbing an unchanged overlay value must not bump content_rev"
        );

        // Setting the overlay back to the stored value clears the diff → bump.
        p.set_working(ParameterPath::EqBandGain(1), ParameterValue::Float(3.0));
        let r3 = p.content_rev();
        assert!(r3 > r2, "clearing a diff entry must bump content_rev");

        // store_changes (folds overlay into values, via touch) bumps.
        p.set_working(ParameterPath::EqBandGain(1), ParameterValue::Float(-6.0));
        let r4 = p.content_rev();
        assert_eq!(p.store_changes(), 1);
        assert!(p.content_rev() > r4, "store_changes must bump content_rev");

        // discard_working on a non-empty overlay bumps; on an empty one, no-op.
        p.set_working(ParameterPath::EqBandGain(2), ParameterValue::Float(4.0));
        let r5 = p.content_rev();
        p.discard_working();
        assert!(p.content_rev() > r5, "discard of a non-empty overlay bumps");
        let r6 = p.content_rev();
        p.discard_working();
        assert_eq!(
            p.content_rev(),
            r6,
            "discard of an empty overlay is a no-op"
        );
    }

    #[test]
    fn iter_effective_overrides_and_includes_overlay_only() {
        let mut p = ChannelPalette::new(
            "EQ".into(),
            ChannelId::Input(1),
            &[PaletteKind::Eq],
            sample_eq_values(),
        );
        // Override an existing stored param, and add an overlay-only param
        // (EqBandGain(2) is not in sample_eq_values).
        p.set_working(ParameterPath::EqBandGain(1), ParameterValue::Float(-6.0));
        p.set_working(ParameterPath::EqBandGain(2), ParameterValue::Float(4.0));

        let eff: HashMap<_, _> = p.iter_effective().collect();
        assert_eq!(
            eff.get(&ParameterPath::EqBandGain(1)),
            Some(&&ParameterValue::Float(-6.0)),
            "overlay value wins over stored"
        );
        assert_eq!(
            eff.get(&ParameterPath::EqBandGain(2)),
            Some(&&ParameterValue::Float(4.0)),
            "overlay-only param is included"
        );
        assert_eq!(
            eff.get(&ParameterPath::EqBandFrequency(1)),
            Some(&&ParameterValue::Float(1000.0)),
            "untouched stored param keeps its stored value"
        );

        // Discard reverts to stored values.
        p.discard_working();
        assert!(!p.has_working_changes());
        assert_eq!(
            p.effective_value(&ParameterPath::EqBandGain(1)),
            Some(&ParameterValue::Float(3.0))
        );
    }

    #[test]
    fn working_overlay_is_not_serialized() {
        let mut p = ChannelPalette::new(
            "EQ".into(),
            ChannelId::Input(1),
            &[PaletteKind::Eq],
            sample_eq_values(),
        );
        p.set_working(ParameterPath::EqBandGain(1), ParameterValue::Float(-6.0));
        let json = serde_json::to_string(&p).unwrap();
        assert!(
            !json.contains("working_values"),
            "working overlay must not be persisted to the show file"
        );
        let loaded: ChannelPalette = serde_json::from_str(&json).unwrap();
        assert!(
            !loaded.has_working_changes(),
            "reloaded palette starts with an empty overlay"
        );
        // The un-stored adjustment is gone; the stored baseline remains.
        assert_eq!(
            loaded.values.get(&ParameterPath::EqBandGain(1)),
            Some(&ParameterValue::Float(3.0))
        );
    }

    #[test]
    fn multi_kind_serde_round_trip() {
        let mut all = sample_eq_values();
        all.extend(sample_dyn2_values());
        let palette = ChannelPalette::new(
            "EQ+Gate".into(),
            ChannelId::Input(3),
            &[PaletteKind::Eq, PaletteKind::Dyn2],
            all,
        );

        let json = serde_json::to_string(&palette).unwrap();
        let loaded: ChannelPalette = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.parameter_count(), palette.parameter_count());
        assert_eq!(loaded.kinds(), vec![PaletteKind::Eq, PaletteKind::Dyn2]);
        assert_eq!(
            loaded.values.get(&ParameterPath::Dyn2Threshold),
            Some(&ParameterValue::Float(-30.0)),
        );
    }

    #[test]
    fn legacy_single_kind_field_is_ignored_on_load() {
        // V8/early-v9 JSON shape: a `kind` field used to live on the struct;
        // it's been removed but legacy show files still write it. Serde should
        // ignore the unknown field and derive kinds from values.
        let v8_json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "Legacy Comp",
            "kind": "Dyn1",
            "channel": {"Input": 2},
            "values": [
                {"path": "Dyn1Enabled", "value": {"Bool": true}},
                {"path": {"Dyn1Threshold": 1}, "value": {"Float": -8.0}}
            ],
            "referencing_snapshots": [],
            "created_at": "2025-01-01T00:00:00Z",
            "modified_at": "2025-01-01T00:00:00Z"
        }"#;
        let loaded: ChannelPalette = serde_json::from_str(v8_json).unwrap();
        assert_eq!(loaded.name, "Legacy Comp");
        assert_eq!(loaded.kinds(), vec![PaletteKind::Dyn1]);
        assert_eq!(loaded.parameter_count(), 2);
    }

    #[test]
    fn legacy_eq_values_alias_still_loads() {
        // V8 EqPalette stored values under `eq_values`; the alias keeps these
        // files loadable.
        let v8_json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "Legacy Vocal",
            "channel": {"Input": 1},
            "eq_values": [
                {"path": "EqEnabled", "value": {"Bool": true}},
                {"path": {"EqBandFrequency": 1}, "value": {"Float": 1200.0}}
            ],
            "referencing_snapshots": [],
            "created_at": "2025-01-01T00:00:00Z",
            "modified_at": "2025-01-01T00:00:00Z"
        }"#;
        let loaded: ChannelPalette = serde_json::from_str(v8_json).unwrap();
        assert_eq!(loaded.name, "Legacy Vocal");
        assert_eq!(loaded.kinds(), vec![PaletteKind::Eq]);
        assert_eq!(loaded.parameter_count(), 2);
    }
}
