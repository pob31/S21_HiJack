use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::channel::ChannelId;
use super::parameter::ParameterSection;

fn default_true() -> bool {
    true
}

/// A gang group: a set of channels whose selected parameter sections
/// are linked with bidirectional relative propagation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GangGroup {
    pub id: Uuid,
    pub name: String,
    /// Channels in this gang (any type, possibly mixed).
    pub members: Vec<ChannelId>,
    /// Which parameter sections propagate across members.
    pub linked_sections: HashSet<ParameterSection>,
    /// Whether this gang is currently active.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl GangGroup {
    pub fn new(
        name: String,
        members: Vec<ChannelId>,
        linked_sections: HashSet<ParameterSection>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            members,
            linked_sections,
            enabled: true,
        }
    }

    /// Check if a channel is a member of this gang.
    pub fn contains_channel(&self, channel: &ChannelId) -> bool {
        self.members.contains(channel)
    }

    /// Check if this gang links a given parameter section.
    pub fn links_section(&self, section: &ParameterSection) -> bool {
        self.linked_sections.contains(section)
    }

    /// Return all members except the given source channel.
    pub fn other_members(&self, source: &ChannelId) -> Vec<&ChannelId> {
        self.members.iter().filter(|m| *m != source).collect()
    }
}

impl std::fmt::Display for GangGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_gang() -> GangGroup {
        GangGroup::new(
            "Drums".into(),
            vec![
                ChannelId::Input(1),
                ChannelId::Input(2),
                ChannelId::Input(3),
            ],
            HashSet::from([ParameterSection::FaderMutePan, ParameterSection::Eq]),
        )
    }

    #[test]
    fn creation() {
        let gang = make_gang();
        assert_eq!(gang.name, "Drums");
        assert_eq!(gang.members.len(), 3);
        assert_eq!(gang.linked_sections.len(), 2);
        assert!(gang.enabled);
    }

    #[test]
    fn contains_channel() {
        let gang = make_gang();
        assert!(gang.contains_channel(&ChannelId::Input(1)));
        assert!(gang.contains_channel(&ChannelId::Input(3)));
        assert!(!gang.contains_channel(&ChannelId::Input(4)));
        assert!(!gang.contains_channel(&ChannelId::Aux(1)));
    }

    #[test]
    fn links_section() {
        let gang = make_gang();
        assert!(gang.links_section(&ParameterSection::FaderMutePan));
        assert!(gang.links_section(&ParameterSection::Eq));
        assert!(!gang.links_section(&ParameterSection::Sends));
        assert!(!gang.links_section(&ParameterSection::Dyn1));
    }

    #[test]
    fn other_members_excludes_source() {
        let gang = make_gang();
        let others = gang.other_members(&ChannelId::Input(2));
        assert_eq!(others.len(), 2);
        assert!(others.contains(&&ChannelId::Input(1)));
        assert!(others.contains(&&ChannelId::Input(3)));
        assert!(!others.contains(&&ChannelId::Input(2)));
    }

    #[test]
    fn serde_round_trip() {
        let gang = make_gang();
        let json = serde_json::to_string(&gang).unwrap();
        let loaded: GangGroup = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.name, gang.name);
        assert_eq!(loaded.members, gang.members);
        assert_eq!(loaded.linked_sections, gang.linked_sections);
        assert_eq!(loaded.enabled, gang.enabled);
    }

    #[test]
    fn enabled_defaults_to_true() {
        // JSON without the enabled field should deserialize with enabled = true
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "Test",
            "members": [{"Input": 1}],
            "linked_sections": ["FaderMutePan"]
        }"#;
        let gang: GangGroup = serde_json::from_str(json).unwrap();
        assert!(gang.enabled);
    }
}
