//! Channel palettes — reusable per-channel templates for one parameter section.
//!
//! Generalizes the original EQ-only `EqPalette` (Phase 5/6 work, replaces
//! [src/model/eq_palette.rs] from earlier phases) into a kind-discriminated
//! `ChannelPalette` that can store EQ, Compressor (Dyn1), or Gate (Dyn2)
//! values for one channel. The recall engine substitutes palette values for
//! the snapshot's stored values when a snapshot has a palette ref for a
//! given (channel, kind) pair.
//!
//! Modifying a palette "ripples" to all referencing snapshots automatically
//! on next recall — see [crate::console::snapshot_engine::SnapshotEngine].

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::channel::ChannelId;
use super::parameter::{PaletteKind, ParameterPath, ParameterValue};

/// A reusable per-channel parameter template, scoped to one parameter section.
///
/// Snapshots tag a `(ChannelId, PaletteKind)` pair with a palette UUID via
/// `Snapshot::palette_refs`; at recall time the engine substitutes palette
/// values for the snapshot's stored values within the matching section.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelPalette {
    pub id: Uuid,
    pub name: String,
    /// Which section this palette stores. Determines which `ParameterPath`
    /// variants are accepted; `ChannelPalette::new` filters incoming values
    /// to only those matching this kind.
    ///
    /// Defaults to `Eq` for back-compat with v8 show files where the field
    /// did not exist (every legacy palette WAS an EQ palette).
    #[serde(default)]
    pub kind: PaletteKind,
    /// Which channel this palette stores values for.
    pub channel: ChannelId,
    /// Section-specific parameter values. The serde alias `eq_values` lets
    /// v8 show files load — they had a field of that name on `EqPalette`.
    #[serde(with = "palette_values_serde", alias = "eq_values")]
    pub values: HashMap<ParameterPath, ParameterValue>,
    /// Back-references: snapshot IDs that link to this palette. Used by the
    /// UI for the "Linked Snapshots" display and the "ripple count" status
    /// message after re-capture.
    pub referencing_snapshots: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
}

impl ChannelPalette {
    /// Create a new palette of the given kind. Filters `values` to only
    /// those whose section matches the kind — defensive against the
    /// caller passing in a `capture_*` result that includes other params.
    pub fn new(
        name: String,
        kind: PaletteKind,
        channel: ChannelId,
        values: HashMap<ParameterPath, ParameterValue>,
    ) -> Self {
        let target_section = kind.section();
        let values = values
            .into_iter()
            .filter(|(p, _)| p.section() == target_section)
            .collect();
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            kind,
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

    /// Number of stored parameters.
    pub fn parameter_count(&self) -> usize {
        self.values.len()
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
    fn creation_and_parameter_count() {
        let palette = ChannelPalette::new(
            "Vocal EQ".into(),
            PaletteKind::Eq,
            ChannelId::Input(1),
            sample_eq_values(),
        );
        assert_eq!(palette.name, "Vocal EQ");
        assert_eq!(palette.kind, PaletteKind::Eq);
        assert_eq!(palette.channel, ChannelId::Input(1));
        assert_eq!(palette.parameter_count(), 6);
        assert!(palette.referencing_snapshots.is_empty());
    }

    #[test]
    fn channel_palette_filters_values_to_kind_section() {
        // Mixed values: some EQ, some Dyn1, some Fader/Mute.
        let mut mixed = sample_eq_values();
        mixed.extend(sample_dyn1_values());
        mixed.insert(ParameterPath::Fader, ParameterValue::Float(-10.0));
        mixed.insert(ParameterPath::AnalogGain, ParameterValue::Float(20.0));

        // EQ palette only keeps the EQ values.
        let eq_palette = ChannelPalette::new(
            "EQ".into(),
            PaletteKind::Eq,
            ChannelId::Input(1),
            mixed.clone(),
        );
        assert_eq!(eq_palette.parameter_count(), 6);
        assert!(!eq_palette.values.contains_key(&ParameterPath::Fader));
        assert!(!eq_palette.values.contains_key(&ParameterPath::Dyn1Enabled));

        // Dyn1 palette only keeps the Dyn1 values.
        let dyn1_palette =
            ChannelPalette::new("Comp".into(), PaletteKind::Dyn1, ChannelId::Input(1), mixed);
        assert_eq!(dyn1_palette.parameter_count(), 5);
        assert!(
            dyn1_palette
                .values
                .contains_key(&ParameterPath::Dyn1Threshold(1))
        );
        assert!(!dyn1_palette.values.contains_key(&ParameterPath::EqEnabled));
    }

    #[test]
    fn touch_updates_modified_at() {
        let mut palette = ChannelPalette::new(
            "Test".into(),
            PaletteKind::Eq,
            ChannelId::Input(1),
            sample_eq_values(),
        );
        let before = palette.modified_at;
        std::thread::sleep(std::time::Duration::from_millis(10));
        palette.touch();
        assert!(palette.modified_at > before);
    }

    #[test]
    fn channel_palette_serde_round_trip_eq() {
        let palette = ChannelPalette::new(
            "Vocal EQ".into(),
            PaletteKind::Eq,
            ChannelId::Input(1),
            sample_eq_values(),
        );

        let json = serde_json::to_string_pretty(&palette).unwrap();
        let loaded: ChannelPalette = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.name, palette.name);
        assert_eq!(loaded.kind, PaletteKind::Eq);
        assert_eq!(loaded.channel, palette.channel);
        assert_eq!(loaded.parameter_count(), palette.parameter_count());
        assert_eq!(
            loaded.values.get(&ParameterPath::EqBandFrequency(1)),
            Some(&ParameterValue::Float(1000.0)),
        );
    }

    #[test]
    fn channel_palette_serde_round_trip_dyn1() {
        let palette = ChannelPalette::new(
            "Snare Comp".into(),
            PaletteKind::Dyn1,
            ChannelId::Input(2),
            sample_dyn1_values(),
        );
        let json = serde_json::to_string(&palette).unwrap();
        let loaded: ChannelPalette = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.kind, PaletteKind::Dyn1);
        assert_eq!(loaded.parameter_count(), 5);
        assert_eq!(
            loaded.values.get(&ParameterPath::Dyn1Threshold(1)),
            Some(&ParameterValue::Float(-12.0)),
        );
    }

    #[test]
    fn channel_palette_serde_round_trip_dyn2() {
        let palette = ChannelPalette::new(
            "Tom Gate".into(),
            PaletteKind::Dyn2,
            ChannelId::Input(3),
            sample_dyn2_values(),
        );
        let json = serde_json::to_string(&palette).unwrap();
        let loaded: ChannelPalette = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.kind, PaletteKind::Dyn2);
        assert_eq!(loaded.parameter_count(), 5);
    }

    #[test]
    fn v8_eq_palette_loads_as_eq_kind_channel_palette() {
        // V8 JSON shape: no `kind` field, values stored under `eq_values`.
        // The serde alias on `values` and the default on `kind` should let
        // this load as a fully-functional ChannelPalette with PaletteKind::Eq.
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
        assert_eq!(loaded.kind, PaletteKind::Eq); // serde default
        assert_eq!(loaded.parameter_count(), 2);
        assert_eq!(
            loaded.values.get(&ParameterPath::EqEnabled),
            Some(&ParameterValue::Bool(true)),
        );
        assert_eq!(
            loaded.values.get(&ParameterPath::EqBandFrequency(1)),
            Some(&ParameterValue::Float(1200.0)),
        );
    }
}
