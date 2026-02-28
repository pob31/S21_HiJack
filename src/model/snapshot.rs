use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::channel::ChannelId;
use super::parameter::{ParameterAddress, ParameterSection, ParameterValue};

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
    pub fn contains(&self, addr: &ParameterAddress) -> bool {
        let section = addr.parameter.section();
        self.channel_scopes.iter().any(|cs| {
            cs.channel == addr.channel && cs.sections.contains(&section)
        })
    }
}

/// Which sections are in scope for a specific channel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelScope {
    pub channel: ChannelId,
    pub sections: HashSet<ParameterSection>,
}

/// A captured snapshot of console parameters.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: Uuid,
    pub name: String,
    /// The scope used when this snapshot was captured.
    pub scope: ScopeTemplate,
    /// The stored parameter values.
    pub data: SnapshotData,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
}

impl Snapshot {
    /// Create a new snapshot with generated ID and current timestamps.
    pub fn new(name: String, scope: ScopeTemplate, data: SnapshotData) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            scope,
            data,
            created_at: now,
            modified_at: now,
        }
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
            vec![ChannelScope {
                channel: ChannelId::Input(1),
                sections: HashSet::from([
                    ParameterSection::FaderMutePan,
                    ParameterSection::Eq,
                ]),
            }],
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
            vec![ChannelScope {
                channel: ChannelId::Input(1),
                sections: HashSet::from([ParameterSection::FaderMutePan]),
            }],
        );

        // Wrong section
        let gain_addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Gain,
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
    fn cue_list_default() {
        let list = CueList::default();
        assert_eq!(list.name, "Main");
        assert!(list.cues.is_empty());
    }
}
