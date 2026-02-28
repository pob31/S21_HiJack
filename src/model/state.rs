use chrono::{DateTime, Utc};
use std::collections::HashMap;

use super::channel::ChannelId;
use super::config::ConsoleConfig;
use super::parameter::{ParameterAddress, ParameterPath, ParameterSection, ParameterValue};
use super::snapshot::{ScopeTemplate, SnapshotData};

/// Live mirror of the full console state.
pub struct ConsoleState {
    pub config: ConsoleConfig,
    /// All parameter values indexed by address.
    parameters: HashMap<ParameterAddress, ParameterValue>,
    /// Timestamp of last update per parameter.
    last_updated: HashMap<ParameterAddress, DateTime<Utc>>,
}

impl ConsoleState {
    pub fn new(config: ConsoleConfig) -> Self {
        Self {
            config,
            parameters: HashMap::new(),
            last_updated: HashMap::new(),
        }
    }

    /// Apply a parameter change (from incoming OSC).
    pub fn update(&mut self, addr: ParameterAddress, value: ParameterValue) {
        self.last_updated.insert(addr.clone(), Utc::now());
        self.parameters.insert(addr, value);
    }

    /// Get current value of a parameter.
    pub fn get(&self, addr: &ParameterAddress) -> Option<&ParameterValue> {
        self.parameters.get(addr)
    }

    /// Get the timestamp of the last update for a parameter.
    pub fn last_updated(&self, addr: &ParameterAddress) -> Option<&DateTime<Utc>> {
        self.last_updated.get(addr)
    }

    /// Total number of tracked parameters.
    pub fn parameter_count(&self) -> usize {
        self.parameters.len()
    }

    /// Capture parameters within scope from the live state mirror (PRD §5.2).
    pub fn capture(&self, scope: &ScopeTemplate) -> SnapshotData {
        let mut data = SnapshotData::new();
        for (addr, value) in &self.parameters {
            if scope.contains(addr) {
                data.values.insert(addr.clone(), value.clone());
            }
        }
        data
    }

    /// Iterate over all parameters in the state mirror.
    pub fn iter_parameters(&self) -> impl Iterator<Item = (&ParameterAddress, &ParameterValue)> {
        self.parameters.iter()
    }

    /// Capture only EQ-section parameters for a specific channel.
    /// Returns a map of ParameterPath → ParameterValue (channel stored separately in palette).
    pub fn capture_eq(&self, channel: &ChannelId) -> HashMap<ParameterPath, ParameterValue> {
        self.parameters
            .iter()
            .filter(|(addr, _)| {
                &addr.channel == channel
                    && addr.parameter.section() == ParameterSection::Eq
            })
            .map(|(addr, value)| (addr.parameter.clone(), value.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use crate::model::channel::ChannelId;
    use crate::model::parameter::{ParameterPath, ParameterSection};
    use crate::model::snapshot::{ChannelScope, ScopeTemplate};

    #[test]
    fn update_and_get() {
        let mut state = ConsoleState::new(ConsoleConfig::default());
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };

        assert!(state.get(&addr).is_none());

        state.update(addr.clone(), ParameterValue::Float(-10.0));
        assert_eq!(state.get(&addr), Some(&ParameterValue::Float(-10.0)));
        assert!(state.last_updated(&addr).is_some());
        assert_eq!(state.parameter_count(), 1);

        // Update overwrites
        state.update(addr.clone(), ParameterValue::Float(0.0));
        assert_eq!(state.get(&addr), Some(&ParameterValue::Float(0.0)));
    }

    #[test]
    fn capture_eq_for_channel() {
        let mut state = ConsoleState::new(ConsoleConfig::default());

        // Add EQ and non-EQ params for Input 1
        state.update(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::EqEnabled },
            ParameterValue::Bool(true),
        );
        state.update(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::EqBandFrequency(1) },
            ParameterValue::Float(1000.0),
        );
        state.update(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::EqBandGain(1) },
            ParameterValue::Float(3.0),
        );
        state.update(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Fader },
            ParameterValue::Float(-10.0),
        );
        // EQ for Input 2 — should not appear
        state.update(
            ParameterAddress { channel: ChannelId::Input(2), parameter: ParameterPath::EqEnabled },
            ParameterValue::Bool(false),
        );

        let eq = state.capture_eq(&ChannelId::Input(1));
        assert_eq!(eq.len(), 3);
        assert_eq!(eq.get(&ParameterPath::EqEnabled), Some(&ParameterValue::Bool(true)));
        assert_eq!(eq.get(&ParameterPath::EqBandFrequency(1)), Some(&ParameterValue::Float(1000.0)));
        assert!(!eq.contains_key(&ParameterPath::Fader));
    }

    #[test]
    fn capture_eq_empty_for_no_data() {
        let state = ConsoleState::new(ConsoleConfig::default());
        let eq = state.capture_eq(&ChannelId::Input(99));
        assert!(eq.is_empty());
    }

    #[test]
    fn capture_within_scope() {
        let mut state = ConsoleState::new(ConsoleConfig::default());

        // Add some parameters
        state.update(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Fader },
            ParameterValue::Float(-10.0),
        );
        state.update(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Mute },
            ParameterValue::Bool(false),
        );
        state.update(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::EqEnabled },
            ParameterValue::Bool(true),
        );
        state.update(
            ParameterAddress { channel: ChannelId::Input(2), parameter: ParameterPath::Fader },
            ParameterValue::Float(-5.0),
        );

        // Scope: only FaderMutePan for Input 1
        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope {
                channel: ChannelId::Input(1),
                sections: HashSet::from([ParameterSection::FaderMutePan]),
            }],
        );

        let captured = state.capture(&scope);

        // Should capture fader and mute for Input 1, but not EQ or Input 2
        assert_eq!(captured.parameter_count(), 2);
        assert!(captured.values.contains_key(&ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        }));
        assert!(captured.values.contains_key(&ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Mute,
        }));
        // EQ not in scope
        assert!(!captured.values.contains_key(&ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::EqEnabled,
        }));
        // Input 2 not in scope
        assert!(!captured.values.contains_key(&ParameterAddress {
            channel: ChannelId::Input(2),
            parameter: ParameterPath::Fader,
        }));
    }
}
