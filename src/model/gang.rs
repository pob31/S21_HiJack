use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::channel::ChannelId;
use super::parameter::ParameterSection;

fn default_true() -> bool {
    true
}

/// How continuous parameters are propagated across gang members.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GangMode {
    /// Apply the delta (difference) from the source change to each target's
    /// current value. Members can have different absolute values but move
    /// together.
    #[default]
    Relative,
    /// Copy the source's exact value to all targets. Members snap to the
    /// same absolute value.
    Absolute,
}

/// How the main channel pan propagates across gang members. Independent of
/// the `Fader/Mute/Pan` section toggle and of [`GangMode`] — pan has its own
/// three-state control in the Gangs tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GangPanMode {
    /// Pan is not linked: a pan change on one member is left alone.
    Off,
    /// Pan is linked and follows the gang's [`GangMode`] (relative or absolute).
    #[default]
    On,
    /// Pan is mirrored around centre so a pair pans away from the middle
    /// symmetrically (`target = -source`). Only meaningful on 2-member gangs.
    Reversed,
}

/// A gang group: a set of channels whose selected parameter sections
/// are linked with bidirectional propagation.
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
    /// How continuous parameters propagate: relative delta or absolute copy.
    #[serde(default)]
    pub mode: GangMode,
    /// How the main channel pan propagates, independently of the
    /// `Fader/Mute/Pan` section. `None` in legacy show files written before
    /// pan was decoupled from that section — resolved by
    /// [`GangGroup::effective_pan_mode`] so those gangs keep linking pan.
    /// New gangs always store `Some(..)`.
    #[serde(default)]
    pub pan_mode: Option<GangPanMode>,
    /// Temporarily suspend propagation without breaking the gang.
    /// Quick toggle for mid-show individual channel edits.
    #[serde(default)]
    pub paused: bool,
}

impl GangGroup {
    pub fn new(
        name: String,
        members: Vec<ChannelId>,
        linked_sections: HashSet<ParameterSection>,
    ) -> Self {
        // Preserve the historical default: a gang that links Fader/Mute/Pan
        // also links pan. Pan is now its own control, so capture that intent
        // explicitly at construction.
        let pan_mode = if linked_sections.contains(&ParameterSection::FaderMutePan) {
            GangPanMode::On
        } else {
            GangPanMode::Off
        };
        Self {
            id: Uuid::new_v4(),
            name,
            members,
            linked_sections,
            enabled: true,
            mode: GangMode::Relative,
            pan_mode: Some(pan_mode),
            paused: false,
        }
    }

    /// Resolve the effective pan mode, mapping a legacy `None` (show files
    /// written before pan was decoupled from the Fader/Mute/Pan section) to
    /// the behaviour those files implied: pan linked iff that section is.
    pub fn effective_pan_mode(&self) -> GangPanMode {
        self.pan_mode.unwrap_or_else(|| {
            if self
                .linked_sections
                .contains(&ParameterSection::FaderMutePan)
            {
                GangPanMode::On
            } else {
                GangPanMode::Off
            }
        })
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
    fn new_sets_pan_mode_from_sections() {
        // FaderMutePan present → pan linked (On).
        let g = make_gang();
        assert_eq!(g.effective_pan_mode(), GangPanMode::On);

        // No FaderMutePan → pan off.
        let g2 = GangGroup::new(
            "Eq only".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::Eq]),
        );
        assert_eq!(g2.effective_pan_mode(), GangPanMode::Off);
    }

    #[test]
    fn effective_pan_mode_resolves_legacy_none() {
        // Legacy gang (pan_mode = None) that linked FaderMutePan → On.
        let mut g = make_gang();
        g.pan_mode = None;
        assert_eq!(g.effective_pan_mode(), GangPanMode::On);

        // Legacy gang without FaderMutePan → Off.
        let mut g2 = GangGroup::new(
            "Eq only".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::Eq]),
        );
        g2.pan_mode = None;
        assert_eq!(g2.effective_pan_mode(), GangPanMode::Off);

        // Explicit Some is honoured regardless of sections.
        g.pan_mode = Some(GangPanMode::Reversed);
        assert_eq!(g.effective_pan_mode(), GangPanMode::Reversed);
    }

    #[test]
    fn pan_mode_serde_round_trip_and_legacy_default() {
        // Explicit pan_mode round-trips.
        let mut g = make_gang();
        g.pan_mode = Some(GangPanMode::Reversed);
        let json = serde_json::to_string(&g).unwrap();
        let loaded: GangGroup = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.pan_mode, Some(GangPanMode::Reversed));

        // Legacy JSON with no pan_mode field deserializes to None and
        // resolves to On via the FaderMutePan section.
        let legacy = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "Test",
            "members": [{"Input": 1}, {"Input": 2}],
            "linked_sections": ["FaderMutePan"]
        }"#;
        let gang: GangGroup = serde_json::from_str(legacy).unwrap();
        assert_eq!(gang.pan_mode, None);
        assert_eq!(gang.effective_pan_mode(), GangPanMode::On);
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
