use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::channel::ChannelId;
use super::palette::ChannelPalette;
use super::parameter::{
    ParameterAddress, ParameterPath, ParameterSection, ParameterValue, PaletteKind,
    TimingCategory,
};

/// Reusable scope template — defines which channels and sections to capture/recall.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScopeTemplate {
    pub id: Uuid,
    pub name: String,
    pub channel_scopes: Vec<ChannelScope>,
}

impl ScopeTemplate {
    /// Create a new scope template with a generated ID.
    pub fn new(name: String, channel_scopes: Vec<ChannelScope>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            channel_scopes,
        }
    }

    /// Check if a parameter address is within this scope.
    ///
    /// Reads BOTH the new per-path `paths` field (the v8+ source of truth)
    /// AND the legacy per-section `sections` field (kept for v7 back-compat).
    /// New scopes write only `paths`; the migration helper expands legacy
    /// section selections into paths the first time the scope editor opens
    /// a v7 template.
    pub fn contains(&self, addr: &ParameterAddress) -> bool {
        let section = addr.parameter.section();
        self.channel_scopes.iter().any(|cs| {
            cs.channel == addr.channel
                && (cs.paths.contains(&addr.parameter) || cs.sections.contains(&section))
        })
    }

    /// True if any `ChannelScope` has per-category timing configured.
    /// Used to decide whether to use the new timed-recall path or the
    /// legacy single-fade-time path.
    pub fn has_any_category_timing(&self) -> bool {
        self.channel_scopes.iter().any(|cs| !cs.category_timings.is_empty())
    }

    /// Look up the timing for a specific channel + category. Returns default
    /// (0/0) if no timing is configured for that combination.
    pub fn timing_for(&self, channel: &ChannelId, cat: TimingCategory) -> CategoryTiming {
        self.channel_scopes
            .iter()
            .find(|cs| cs.channel == *channel)
            .map(|cs| cs.timing_for(cat))
            .unwrap_or_default()
    }
}

/// Per-category pre-wait and fade timing for snapshot recalls.
/// Stored per-channel per-category on `ChannelScope`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CategoryTiming {
    /// Delay before this category starts (seconds). Default 0.0.
    #[serde(default)]
    pub pre_wait_secs: f32,
    /// Fade duration for continuous params in this category (seconds).
    /// Ignored for `TimingCategory::Mute` (always instant after pre-wait).
    /// Default 0.0 (instant).
    #[serde(default)]
    pub fade_time_secs: f32,
}

/// Which parameters are in scope for a specific channel.
///
/// Carries both the new per-`ParameterPath` granularity (`paths`, v8+) and the
/// legacy per-`ParameterSection` field (`sections`, v7 and earlier). The two
/// fields are read additively by `ScopeTemplate::contains` so legacy show files
/// keep working without a load-time migration. New scopes write only to
/// `paths`; `migrate_sections_to_paths` expands the legacy field into the new
/// one when the scope editor first touches a legacy template.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelScope {
    pub channel: ChannelId,
    /// Per-path scope (new in v8). Maximum granularity — every distinct
    /// `ParameterPath` is independently selectable.
    #[serde(default)]
    pub paths: HashSet<ParameterPath>,
    /// Legacy per-section scope (v7 and earlier). Empty for new scopes; kept
    /// populated only for legacy templates that haven't yet been edited.
    #[serde(default)]
    pub sections: HashSet<ParameterSection>,
    /// Per-category recall timing (pre-wait + fade). Empty = all instant.
    #[serde(default)]
    pub category_timings: HashMap<TimingCategory, CategoryTiming>,
}

impl ChannelScope {
    /// Build a new path-granularity ChannelScope (v8+ style).
    pub fn new(channel: ChannelId, paths: HashSet<ParameterPath>) -> Self {
        Self {
            channel,
            paths,
            sections: HashSet::new(),
            category_timings: HashMap::new(),
        }
    }

    /// Build a legacy section-granularity ChannelScope. Used by tests and the
    /// section→path migration helper.
    pub fn from_sections(channel: ChannelId, sections: HashSet<ParameterSection>) -> Self {
        Self {
            channel,
            paths: HashSet::new(),
            sections,
            category_timings: HashMap::new(),
        }
    }

    /// Look up the recall timing for a category on this channel.
    /// Returns default (0/0) if no timing is configured.
    pub fn timing_for(&self, cat: TimingCategory) -> CategoryTiming {
        self.category_timings.get(&cat).cloned().unwrap_or_default()
    }

    /// Expand the legacy `sections` field into `paths` (every applicable path
    /// whose section is in `sections` is added). Called by the scope editor
    /// when it loads a legacy template so subsequent edits operate on paths.
    /// Idempotent — safe to call repeatedly. The aux/group/matrix counts come
    /// from the show config so the path enumeration respects the actual
    /// number of sends configured.
    pub fn migrate_sections_to_paths(&mut self, aux_count: u8, group_count: u8, matrix_count: u8) {
        if self.sections.is_empty() {
            return;
        }
        for path in
            ParameterPath::applicable_to(&self.channel, aux_count, group_count, matrix_count)
        {
            if self.sections.contains(&path.section()) {
                self.paths.insert(path);
            }
        }
        self.sections.clear();
    }
}

/// How the scope interacts with capture and recall.
///
/// Two modes mirroring the WFS-DIY snapshot system:
///
/// - **`ApplyOnSave`** (default, current behaviour): the scope filters at
///   CAPTURE time. Only in-scope parameters are stored in `SnapshotData`. The
///   stored data IS the scope — there's nothing outside it to recall later, so
///   "recall without scope" is a no-op.
///
/// - **`ApplyOnRecall`**: every live parameter is captured (the scope is
///   ignored at capture time). At RECALL time the scope is used as an
///   exclusion filter. The operator can also "Recall without scope" to
///   restore the entire saved state in one shot — useful for jumping into a
///   cue list mid-show without dragging accumulated partial changes along.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapshotKind {
    /// Scope is applied when capturing — only in-scope parameters are stored.
    /// (Current S21_HiJack behaviour, kept as the default for v7 back-compat.)
    #[default]
    ApplyOnSave,
    /// Every live parameter is captured; scope filters at recall time only.
    ApplyOnRecall,
}

/// A captured snapshot of console parameters.
///
/// `Deserialize` is implemented manually so v8 show files (which have a
/// `eq_palette_refs: HashMap<ChannelId, Uuid>` field) load into the new
/// `palette_refs: HashMap<(ChannelId, PaletteKind), Uuid>` map by remapping
/// each entry to `(channel, PaletteKind::Eq) → uuid`. New v9 files only
/// write the `palette_refs` field; legacy `eq_palette_refs` is ignored on
/// the way out.
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub id: Uuid,
    pub name: String,
    /// The scope used when this snapshot was captured.
    pub scope: ScopeTemplate,
    /// How the scope interacts with capture and recall. New in v8 — old v7
    /// snapshots load with the default `ApplyOnSave`, which preserves their
    /// captured-data semantics exactly.
    pub kind: SnapshotKind,
    /// The stored parameter values.
    pub data: SnapshotData,
    /// Per-section palette references: `(channel, kind) → palette UUID`.
    /// When set, recall uses palette values instead of the snapshot's stored
    /// values for that section on that channel. New in v9 — replaces the v8
    /// `eq_palette_refs` field. Lookup is done at recall time so changes to
    /// a palette ripple to every linked snapshot automatically.
    #[serde(with = "palette_refs_serde")]
    pub palette_refs: HashMap<(ChannelId, PaletteKind), Uuid>,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
}

impl Snapshot {
    /// Create a new snapshot with generated ID and current timestamps.
    pub fn new(
        name: String,
        scope: ScopeTemplate,
        data: SnapshotData,
        kind: SnapshotKind,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            scope,
            kind,
            data,
            palette_refs: HashMap::new(),
            created_at: now,
            modified_at: now,
        }
    }

    /// Look up the palette UUID (if any) for the parameter at `addr`.
    /// Returns None when the parameter's section has no `PaletteKind`, or
    /// when no palette is linked for that `(channel, kind)`. Used by the
    /// snapshot engine to substitute palette values during recall.
    pub fn palette_ref_for(&self, addr: &ParameterAddress) -> Option<Uuid> {
        let kind = addr.parameter.section().palette_kind()?;
        self.palette_refs.get(&(addr.channel.clone(), kind)).copied()
    }
}

/// Resolve the effective parameter values for a snapshot recall, applying
/// palette overrides where linked. Returns all `(address, value)` pairs that
/// should be sent — both snapshot-stored values (with palette substitution)
/// and palette-only values not present in the snapshot data.
///
/// When `ignore_scope` is true, every stored/palette parameter is included;
/// otherwise only those within `scope` are returned.
pub fn resolve_recall_values<'a>(
    snapshot: &'a Snapshot,
    scope: &ScopeTemplate,
    palettes: &'a HashMap<Uuid, ChannelPalette>,
    ignore_scope: bool,
) -> Vec<(ParameterAddress, &'a ParameterValue)> {
    use std::collections::HashSet;

    let mut out = Vec::new();
    let mut palette_params_seen: HashSet<(Uuid, ParameterAddress)> = HashSet::new();

    // 1. Walk snapshot data, substituting palette values where linked.
    for (addr, snap_value) in &snapshot.data.values {
        if !ignore_scope && !scope.contains(addr) {
            continue;
        }
        let effective_value = if let Some(palette_id) = snapshot.palette_ref_for(addr) {
            if let Some(palette) = palettes.get(&palette_id) {
                palette_params_seen.insert((palette_id, addr.clone()));
                palette.values.get(&addr.parameter).unwrap_or(snap_value)
            } else {
                snap_value
            }
        } else {
            snap_value
        };
        out.push((addr.clone(), effective_value));
    }

    // 2. Palette-only values: params in palettes but not in snapshot data.
    for ((channel, kind), palette_id) in &snapshot.palette_refs {
        let Some(palette) = palettes.get(palette_id) else {
            continue;
        };
        if palette.kind != *kind {
            continue;
        }
        for (param_path, value) in &palette.values {
            let addr = ParameterAddress {
                channel: channel.clone(),
                parameter: param_path.clone(),
            };
            if palette_params_seen.contains(&(*palette_id, addr.clone())) {
                continue;
            }
            if !ignore_scope && !scope.contains(&addr) {
                continue;
            }
            out.push((addr, value));
        }
    }

    out
}

// Manual Deserialize impl: accepts both v8 (`eq_palette_refs: HashMap<ChannelId, Uuid>`)
// and v9 (`palette_refs: HashMap<(ChannelId, PaletteKind), Uuid>`) shapes, and merges
// any legacy entries into the new map keyed by `(channel, PaletteKind::Eq)`.
impl<'de> serde::Deserialize<'de> for Snapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct LegacyEqRefEntry {
            channel: ChannelId,
            palette_id: Uuid,
        }

        #[derive(Deserialize)]
        struct ShadowSnapshot {
            id: Uuid,
            name: String,
            scope: ScopeTemplate,
            #[serde(default)]
            kind: SnapshotKind,
            data: SnapshotData,
            /// v9 field — new shape.
            #[serde(default, with = "palette_refs_serde")]
            palette_refs: HashMap<(ChannelId, PaletteKind), Uuid>,
            /// v8 legacy field — present on older show files.
            #[serde(default)]
            eq_palette_refs: Vec<LegacyEqRefEntry>,
            created_at: DateTime<Utc>,
            modified_at: DateTime<Utc>,
        }

        let mut shadow = ShadowSnapshot::deserialize(deserializer)?;
        // Merge legacy v8 entries into the new map keyed by Eq kind.
        for entry in shadow.eq_palette_refs.drain(..) {
            shadow
                .palette_refs
                .insert((entry.channel, PaletteKind::Eq), entry.palette_id);
        }
        Ok(Snapshot {
            id: shadow.id,
            name: shadow.name,
            scope: shadow.scope,
            kind: shadow.kind,
            data: shadow.data,
            palette_refs: shadow.palette_refs,
            created_at: shadow.created_at,
            modified_at: shadow.modified_at,
        })
    }
}

/// Parameter values captured within a scope.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotData {
    /// Serialized as a Vec of entries since ParameterAddress isn't a valid JSON key.
    #[serde(with = "parameter_map")]
    pub values: HashMap<ParameterAddress, ParameterValue>,
}

/// Custom serde for HashMap<ParameterAddress, ParameterValue> — serializes as a Vec of entries.
mod parameter_map {
    use super::*;
    use serde::{Deserializer, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Entry {
        address: ParameterAddress,
        value: ParameterValue,
    }

    pub fn serialize<S>(
        map: &HashMap<ParameterAddress, ParameterValue>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let entries: Vec<Entry> = map
            .iter()
            .map(|(k, v)| Entry { address: k.clone(), value: v.clone() })
            .collect();
        entries.serialize(serializer)
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<HashMap<ParameterAddress, ParameterValue>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let entries: Vec<Entry> = Vec::deserialize(deserializer)?;
        Ok(entries.into_iter().map(|e| (e.address, e.value)).collect())
    }
}

/// Custom serde for `HashMap<(ChannelId, PaletteKind), Uuid>` — serialized as
/// a Vec of entries since neither `ChannelId` (tagged enum) nor a tuple key
/// is a valid JSON map key. New in v9; v8 used a simpler `(ChannelId, Uuid)`
/// shape under the field name `eq_palette_refs` — handled by `Snapshot`'s
/// custom `Deserialize` impl above.
mod palette_refs_serde {
    use super::*;
    use serde::{Deserializer, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Entry {
        channel: ChannelId,
        kind: PaletteKind,
        palette_id: Uuid,
    }

    pub fn serialize<S>(
        map: &HashMap<(ChannelId, PaletteKind), Uuid>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let entries: Vec<Entry> = map
            .iter()
            .map(|((ch, kind), v)| Entry {
                channel: ch.clone(),
                kind: *kind,
                palette_id: *v,
            })
            .collect();
        entries.serialize(serializer)
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<HashMap<(ChannelId, PaletteKind), Uuid>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let entries: Vec<Entry> = Vec::deserialize(deserializer)?;
        Ok(entries
            .into_iter()
            .map(|e| ((e.channel, e.kind), e.palette_id))
            .collect())
    }
}

impl SnapshotData {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }

    pub fn parameter_count(&self) -> usize {
        self.values.len()
    }
}

impl Default for SnapshotData {
    fn default() -> Self {
        Self::new()
    }
}

/// Ordered list of cues for a show.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CueList {
    pub id: Uuid,
    pub name: String,
    pub cues: Vec<Cue>,
}

impl CueList {
    pub fn new(name: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            cues: Vec::new(),
        }
    }
}

impl Default for CueList {
    fn default() -> Self {
        Self::new("Main".to_string())
    }
}

/// A single cue in the cue list.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cue {
    pub id: Uuid,
    /// Supports decimal cue numbers (e.g., 1.0, 1.5, 2.0).
    pub cue_number: f32,
    pub name: String,
    /// Reference to the snapshot to recall.
    pub snapshot_id: Uuid,
    /// If set, overrides the snapshot's built-in scope for this cue.
    pub scope_override: Option<ScopeTemplate>,
    /// Fade time in seconds (0 = instant).
    pub fade_time: f32,
    /// QLab cue identifier for trigger mapping.
    pub qlab_cue_id: Option<String>,
    /// Notes for the operator.
    pub notes: String,
}

impl Cue {
    pub fn new(cue_number: f32, name: String, snapshot_id: Uuid) -> Self {
        Self {
            id: Uuid::new_v4(),
            cue_number,
            name,
            snapshot_id,
            scope_override: None,
            fade_time: 0.0,
            qlab_cue_id: None,
            notes: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parameter::ParameterPath;

    #[test]
    fn scope_contains_matching_parameter() {
        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([
                    ParameterSection::FaderMutePan,
                    ParameterSection::Eq,
                ]),
            )],
        );

        let fader_addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        assert!(scope.contains(&fader_addr));

        let eq_addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::EqBandGain(1),
        };
        assert!(scope.contains(&eq_addr));
    }

    #[test]
    fn scope_rejects_out_of_scope() {
        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::FaderMutePan]),
            )],
        );

        // Wrong section
        let gain_addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::AnalogGain,
        };
        assert!(!scope.contains(&gain_addr));

        // Wrong channel
        let fader_addr = ParameterAddress {
            channel: ChannelId::Input(2),
            parameter: ParameterPath::Fader,
        };
        assert!(!scope.contains(&fader_addr));
    }

    #[test]
    fn snapshot_data_serialization_round_trip() {
        let mut data = SnapshotData::new();
        data.values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(-10.0),
        );
        data.values.insert(
            ParameterAddress {
                channel: ChannelId::Aux(1),
                parameter: ParameterPath::Mute,
            },
            ParameterValue::Bool(true),
        );

        let json = serde_json::to_string(&data).unwrap();
        let loaded: SnapshotData = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.parameter_count(), 2);
        assert_eq!(
            loaded.values.get(&ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            }),
            Some(&ParameterValue::Float(-10.0))
        );
    }

    #[test]
    fn snapshot_palette_refs_serde_round_trip() {
        let scope = ScopeTemplate::new("Test".into(), vec![]);
        let mut snapshot = Snapshot::new(
            "Test Snap".into(),
            scope,
            SnapshotData::new(),
            SnapshotKind::ApplyOnSave,
        );

        let eq_palette_id = Uuid::new_v4();
        let dyn1_palette_id = Uuid::new_v4();
        let dyn2_palette_id = Uuid::new_v4();
        snapshot.palette_refs.insert((ChannelId::Input(1), PaletteKind::Eq), eq_palette_id);
        snapshot.palette_refs.insert((ChannelId::Input(1), PaletteKind::Dyn1), dyn1_palette_id);
        snapshot.palette_refs.insert((ChannelId::Aux(3), PaletteKind::Dyn2), dyn2_palette_id);

        let json = serde_json::to_string(&snapshot).unwrap();
        let loaded: Snapshot = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.palette_refs.len(), 3);
        assert_eq!(
            loaded.palette_refs.get(&(ChannelId::Input(1), PaletteKind::Eq)),
            Some(&eq_palette_id),
        );
        assert_eq!(
            loaded.palette_refs.get(&(ChannelId::Input(1), PaletteKind::Dyn1)),
            Some(&dyn1_palette_id),
        );
        assert_eq!(
            loaded.palette_refs.get(&(ChannelId::Aux(3), PaletteKind::Dyn2)),
            Some(&dyn2_palette_id),
        );
    }

    #[test]
    fn snapshot_backward_compat_no_palette_refs() {
        // Simulate an early-version snapshot JSON without any palette refs field.
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "Old Snap",
            "scope": {"id": "00000000-0000-0000-0000-000000000002", "name": "S", "channel_scopes": []},
            "data": {"values": []},
            "created_at": "2025-01-01T00:00:00Z",
            "modified_at": "2025-01-01T00:00:00Z"
        }"#;

        let loaded: Snapshot = serde_json::from_str(json).unwrap();
        assert!(loaded.palette_refs.is_empty());
    }

    #[test]
    fn v8_snapshot_eq_palette_refs_loads_into_palette_refs() {
        // V8 shape: snapshots had `eq_palette_refs: HashMap<ChannelId, Uuid>`
        // serialized as a Vec of {channel, palette_id}. The new `Snapshot`
        // Deserialize impl should remap legacy entries to the new
        // `(channel, PaletteKind::Eq) -> uuid` shape.
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "Legacy Snap",
            "scope": {"id": "00000000-0000-0000-0000-000000000002", "name": "S", "channel_scopes": []},
            "kind": "ApplyOnSave",
            "data": {"values": []},
            "eq_palette_refs": [
                {"channel": {"Input": 1}, "palette_id": "11111111-1111-1111-1111-111111111111"},
                {"channel": {"Aux": 2}, "palette_id": "22222222-2222-2222-2222-222222222222"}
            ],
            "created_at": "2025-01-01T00:00:00Z",
            "modified_at": "2025-01-01T00:00:00Z"
        }"#;

        let loaded: Snapshot = serde_json::from_str(json).unwrap();
        // Both legacy entries should be in palette_refs keyed by Eq.
        assert_eq!(loaded.palette_refs.len(), 2);
        let input1 = loaded.palette_refs.get(&(ChannelId::Input(1), PaletteKind::Eq)).copied();
        let aux2 = loaded.palette_refs.get(&(ChannelId::Aux(2), PaletteKind::Eq)).copied();
        assert_eq!(
            input1,
            Some(Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()),
        );
        assert_eq!(
            aux2,
            Some(Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap()),
        );
    }

    #[test]
    fn v8_snapshot_with_both_legacy_and_new_palette_refs_merges() {
        // Edge case: a snapshot file written by some version that has both
        // legacy and new fields populated. Both should end up in the
        // unified map; legacy entries fill in the Eq slot for any channels
        // not already covered by the new map.
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "Mixed Snap",
            "scope": {"id": "00000000-0000-0000-0000-000000000002", "name": "S", "channel_scopes": []},
            "kind": "ApplyOnSave",
            "data": {"values": []},
            "palette_refs": [
                {"channel": {"Input": 1}, "kind": "Dyn1", "palette_id": "33333333-3333-3333-3333-333333333333"}
            ],
            "eq_palette_refs": [
                {"channel": {"Input": 1}, "palette_id": "11111111-1111-1111-1111-111111111111"}
            ],
            "created_at": "2025-01-01T00:00:00Z",
            "modified_at": "2025-01-01T00:00:00Z"
        }"#;

        let loaded: Snapshot = serde_json::from_str(json).unwrap();
        // Two entries: the new Dyn1 + the migrated Eq.
        assert_eq!(loaded.palette_refs.len(), 2);
        assert!(loaded.palette_refs.contains_key(&(ChannelId::Input(1), PaletteKind::Eq)));
        assert!(loaded.palette_refs.contains_key(&(ChannelId::Input(1), PaletteKind::Dyn1)));
    }

    #[test]
    fn palette_ref_for_returns_correct_uuid() {
        let scope = ScopeTemplate::new("Test".into(), vec![]);
        let mut snapshot = Snapshot::new(
            "Test".into(),
            scope,
            SnapshotData::new(),
            SnapshotKind::ApplyOnSave,
        );
        let eq_id = Uuid::new_v4();
        let dyn1_id = Uuid::new_v4();
        snapshot.palette_refs.insert((ChannelId::Input(1), PaletteKind::Eq), eq_id);
        snapshot.palette_refs.insert((ChannelId::Input(1), PaletteKind::Dyn1), dyn1_id);

        // EQ-section parameter on Input(1) → eq palette.
        let eq_addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::EqBandFrequency(1),
        };
        assert_eq!(snapshot.palette_ref_for(&eq_addr), Some(eq_id));

        // Dyn1-section parameter on Input(1) → dyn1 palette.
        let dyn1_addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Dyn1Threshold(1),
        };
        assert_eq!(snapshot.palette_ref_for(&dyn1_addr), Some(dyn1_id));

        // Fader on Input(1) → no palette kind for FaderMutePan section.
        let fader_addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        assert_eq!(snapshot.palette_ref_for(&fader_addr), None);

        // EQ on Input(2) → no link for that channel.
        let other_eq = ParameterAddress {
            channel: ChannelId::Input(2),
            parameter: ParameterPath::EqBandFrequency(1),
        };
        assert_eq!(snapshot.palette_ref_for(&other_eq), None);
    }

    #[test]
    fn cue_list_default() {
        let list = CueList::default();
        assert_eq!(list.name, "Main");
        assert!(list.cues.is_empty());
    }

    // ─── Phase 0: per-path scope granularity ────────────────────────────

    #[test]
    fn legacy_section_scope_still_contains_in_section_param() {
        // Legacy v7 ChannelScope: only `sections` is populated, no `paths`.
        let scope = ScopeTemplate::new(
            "Legacy".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::Eq]),
            )],
        );

        // EqBandFrequency(2) belongs to ParameterSection::Eq → still in scope.
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::EqBandFrequency(2),
        };
        assert!(scope.contains(&addr));

        // FaderMutePan is NOT in {Eq} → out of scope.
        let fader_addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        assert!(!scope.contains(&fader_addr));
    }

    #[test]
    fn new_path_scope_contains_only_listed_paths() {
        let scope = ScopeTemplate::new(
            "New".into(),
            vec![ChannelScope::new(
                ChannelId::Input(1),
                HashSet::from([ParameterPath::EqBandGain(1)]),
            )],
        );

        // Exact match.
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::EqBandGain(1),
        };
        assert!(scope.contains(&addr));

        // Same section but different path → out of scope (per-path granularity).
        let addr2 = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::EqBandGain(2),
        };
        assert!(!scope.contains(&addr2));
    }

    #[test]
    fn migrate_sections_to_paths_expands_correctly() {
        let mut cs = ChannelScope::from_sections(
            ChannelId::Input(1),
            HashSet::from([ParameterSection::Eq]),
        );
        assert!(cs.paths.is_empty());
        assert!(!cs.sections.is_empty());

        cs.migrate_sections_to_paths(8, 8, 8);

        assert!(cs.sections.is_empty(), "sections should be drained");
        // Every applicable EQ-section path should now be present.
        assert!(cs.paths.contains(&ParameterPath::EqEnabled));
        assert!(cs.paths.contains(&ParameterPath::EqBandFrequency(1)));
        assert!(cs.paths.contains(&ParameterPath::EqBandFrequency(4)));
        assert!(cs.paths.contains(&ParameterPath::EqBandDynRelease(2)));
        // Non-EQ paths must NOT be present.
        assert!(!cs.paths.contains(&ParameterPath::Fader));
        assert!(!cs.paths.contains(&ParameterPath::Dyn1Threshold(1)));
    }

    #[test]
    fn migrate_sections_to_paths_is_idempotent() {
        let mut cs = ChannelScope::from_sections(
            ChannelId::Input(1),
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        cs.migrate_sections_to_paths(8, 8, 8);
        let snapshot = cs.paths.clone();
        // Second call is a no-op (sections already drained).
        cs.migrate_sections_to_paths(8, 8, 8);
        assert_eq!(cs.paths, snapshot);
    }

    #[test]
    fn migrate_sections_to_paths_skips_paths_not_applicable_to_channel() {
        // Aux channel has no Pan / no Send paths even if `Sends` is in scope.
        let mut cs = ChannelScope::from_sections(
            ChannelId::Aux(1),
            HashSet::from([ParameterSection::Sends, ParameterSection::FaderMutePan]),
        );
        cs.migrate_sections_to_paths(8, 8, 8);
        // Sends section produced 0 paths for Aux (sends are input-only).
        assert!(!cs.paths.iter().any(|p| matches!(p, ParameterPath::SendLevel(_))));
        // FaderMutePan paths are present, but not Pan (input-only on S21 GP OSC).
        assert!(cs.paths.contains(&ParameterPath::Fader));
        assert!(cs.paths.contains(&ParameterPath::Mute));
        assert!(cs.paths.contains(&ParameterPath::Solo));
        assert!(!cs.paths.contains(&ParameterPath::Pan));
    }

    #[test]
    fn channel_scope_deserializes_v7_shape_with_only_sections_field() {
        // v7-shaped JSON: ChannelScope had only the `sections` field.
        let json = r#"{
            "channel": {"Input": 1},
            "sections": ["Eq", "FaderMutePan"]
        }"#;
        let cs: ChannelScope = serde_json::from_str(json).unwrap();
        assert!(cs.paths.is_empty());
        assert_eq!(cs.sections.len(), 2);
        assert!(cs.sections.contains(&ParameterSection::Eq));
        assert!(cs.sections.contains(&ParameterSection::FaderMutePan));
    }

    // ─── Phase B: snapshot kinds ───────────────────────────────────────

    #[test]
    fn snapshot_kind_default_is_apply_on_save() {
        // Mirrors v7 behaviour: capture is scope-filtered.
        assert_eq!(SnapshotKind::default(), SnapshotKind::ApplyOnSave);
    }

    #[test]
    fn snapshot_new_carries_kind() {
        let scope = ScopeTemplate::new("S".into(), vec![]);
        let snap = Snapshot::new(
            "Test".into(),
            scope,
            SnapshotData::new(),
            SnapshotKind::ApplyOnRecall,
        );
        assert_eq!(snap.kind, SnapshotKind::ApplyOnRecall);
    }

    #[test]
    fn v7_snapshot_loads_as_apply_on_save() {
        // v7 JSON had no `kind` field. Loading it must default to ApplyOnSave
        // so existing show files keep their captured-data semantics intact.
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "Old Snap",
            "scope": {"id": "00000000-0000-0000-0000-000000000002", "name": "S", "channel_scopes": []},
            "data": {"values": []},
            "created_at": "2025-01-01T00:00:00Z",
            "modified_at": "2025-01-01T00:00:00Z"
        }"#;
        let loaded: Snapshot = serde_json::from_str(json).unwrap();
        assert_eq!(loaded.kind, SnapshotKind::ApplyOnSave);
    }

    #[test]
    fn snapshot_kind_round_trips_through_serde() {
        let scope = ScopeTemplate::new("S".into(), vec![]);
        let snap = Snapshot::new(
            "Round trip".into(),
            scope,
            SnapshotData::new(),
            SnapshotKind::ApplyOnRecall,
        );
        let json = serde_json::to_string(&snap).unwrap();
        let loaded: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.kind, SnapshotKind::ApplyOnRecall);
    }

    #[test]
    fn channel_scope_deserializes_v8_shape_with_paths_only() {
        // v8 shape: only `paths`, no `sections`.
        let json = r#"{
            "channel": {"Input": 1},
            "paths": [{"EqBandGain": 2}]
        }"#;
        let cs: ChannelScope = serde_json::from_str(json).unwrap();
        assert!(cs.sections.is_empty());
        assert_eq!(cs.paths.len(), 1);
        assert!(cs.paths.contains(&ParameterPath::EqBandGain(2)));
    }
}
