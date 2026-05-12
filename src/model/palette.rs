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
        }
    }

    /// Update the modified timestamp. Call after editing values.
    pub fn touch(&mut self) {
        self.modified_at = Utc::now();
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
