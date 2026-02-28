use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::channel::ChannelId;
use super::parameter::{ParameterPath, ParameterSection, ParameterValue};

/// A reusable EQ parameter palette — stores EQ values for a single channel.
///
/// When linked to snapshots, palette values override the snapshot's stored EQ
/// values on recall.  Modifying a palette "ripples" to all referencing snapshots
/// automatically on next recall.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EqPalette {
    pub id: Uuid,
    pub name: String,
    /// Which channel's EQ this palette stores.
    pub channel: ChannelId,
    /// EQ-section parameter values (ParameterPath only — channel stored separately).
    #[serde(with = "eq_values_serde")]
    pub eq_values: HashMap<ParameterPath, ParameterValue>,
    /// Back-references: snapshot IDs that link to this palette.
    pub referencing_snapshots: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
}

impl EqPalette {
    /// Create a new EQ palette from captured EQ values.
    pub fn new(
        name: String,
        channel: ChannelId,
        eq_values: HashMap<ParameterPath, ParameterValue>,
    ) -> Self {
        // Only keep EQ-section parameters
        let eq_values = eq_values
            .into_iter()
            .filter(|(p, _)| p.section() == ParameterSection::Eq)
            .collect();

        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            channel,
            eq_values,
            referencing_snapshots: Vec::new(),
            created_at: now,
            modified_at: now,
        }
    }

    /// Update the modified timestamp.
    pub fn touch(&mut self) {
        self.modified_at = Utc::now();
    }

    /// Number of stored EQ parameters.
    pub fn parameter_count(&self) -> usize {
        self.eq_values.len()
    }
}

/// Custom serde for HashMap<ParameterPath, ParameterValue> — serializes as Vec of entries.
mod eq_values_serde {
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
            .map(|(k, v)| Entry { path: k.clone(), value: v.clone() })
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
        m.insert(ParameterPath::EqBandFrequency(1), ParameterValue::Float(1000.0));
        m.insert(ParameterPath::EqBandGain(1), ParameterValue::Float(3.0));
        m.insert(ParameterPath::EqBandQ(1), ParameterValue::Float(2.0));
        m.insert(ParameterPath::HighpassEnabled, ParameterValue::Bool(true));
        m.insert(ParameterPath::HighpassFrequency, ParameterValue::Float(80.0));
        m
    }

    #[test]
    fn creation_and_parameter_count() {
        let palette = EqPalette::new(
            "Vocal EQ".into(),
            ChannelId::Input(1),
            sample_eq_values(),
        );
        assert_eq!(palette.name, "Vocal EQ");
        assert_eq!(palette.channel, ChannelId::Input(1));
        assert_eq!(palette.parameter_count(), 6);
        assert!(palette.referencing_snapshots.is_empty());
    }

    #[test]
    fn filters_non_eq_parameters() {
        let mut values = sample_eq_values();
        // Sneak in a non-EQ parameter
        values.insert(ParameterPath::Fader, ParameterValue::Float(-10.0));
        values.insert(ParameterPath::Gain, ParameterValue::Float(20.0));

        let palette = EqPalette::new("Test".into(), ChannelId::Input(1), values);
        // Should have only the 6 EQ params, not the fader or gain
        assert_eq!(palette.parameter_count(), 6);
        assert!(!palette.eq_values.contains_key(&ParameterPath::Fader));
        assert!(!palette.eq_values.contains_key(&ParameterPath::Gain));
    }

    #[test]
    fn touch_updates_modified_at() {
        let mut palette = EqPalette::new(
            "Test".into(),
            ChannelId::Input(1),
            sample_eq_values(),
        );
        let before = palette.modified_at;
        std::thread::sleep(std::time::Duration::from_millis(10));
        palette.touch();
        assert!(palette.modified_at > before);
    }

    #[test]
    fn serde_round_trip() {
        let palette = EqPalette::new(
            "Vocal EQ".into(),
            ChannelId::Input(1),
            sample_eq_values(),
        );

        let json = serde_json::to_string_pretty(&palette).unwrap();
        let loaded: EqPalette = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.name, palette.name);
        assert_eq!(loaded.channel, palette.channel);
        assert_eq!(loaded.parameter_count(), palette.parameter_count());
        assert_eq!(
            loaded.eq_values.get(&ParameterPath::EqBandFrequency(1)),
            Some(&ParameterValue::Float(1000.0)),
        );
        assert_eq!(
            loaded.eq_values.get(&ParameterPath::EqEnabled),
            Some(&ParameterValue::Bool(true)),
        );
    }
}
