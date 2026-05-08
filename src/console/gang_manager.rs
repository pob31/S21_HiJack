use std::collections::HashMap;

use tracing::info;
use uuid::Uuid;

use crate::model::channel::ChannelId;
use crate::model::gang::GangGroup;
use crate::model::parameter::ParameterSection;

/// Manages gang groups: CRUD operations and channel-to-gang lookups.
pub struct GangManager {
    pub groups: HashMap<Uuid, GangGroup>,
}

impl GangManager {
    pub fn new() -> Self {
        Self {
            groups: HashMap::new(),
        }
    }

    pub fn add_group(&mut self, group: GangGroup) {
        info!(name = %group.name, id = %group.id, members = group.members.len(), "Gang group added");
        self.groups.insert(group.id, group);
    }

    pub fn remove_group(&mut self, id: Uuid) -> bool {
        if let Some(group) = self.groups.remove(&id) {
            info!(name = %group.name, "Gang group removed");
            true
        } else {
            false
        }
    }

    /// Find a gang group by name (case-insensitive).
    pub fn find_by_name(&self, name: &str) -> Option<&GangGroup> {
        let lower = name.to_lowercase();
        self.groups
            .values()
            .find(|g| g.name.to_lowercase() == lower)
    }

    /// All groups sorted by name for UI display.
    pub fn sorted_groups(&self) -> Vec<&GangGroup> {
        let mut sorted: Vec<_> = self.groups.values().collect();
        sorted.sort_by_key(|g| g.name.to_lowercase());
        sorted
    }

    /// Find all enabled gangs that contain this channel AND link this section.
    /// This is the hot-path lookup called on every parameter update.
    pub fn find_gangs_for_channel_and_section(
        &self,
        channel: &ChannelId,
        section: &ParameterSection,
    ) -> Vec<&GangGroup> {
        self.groups
            .values()
            .filter(|g| g.enabled && g.contains_channel(channel) && g.links_section(section))
            .collect()
    }

    /// True if any pair of *active* gangs (enabled + not paused) shares a
    /// channel AND at least one linked section. Configurations like that
    /// produce duplicate / fighting propagation: a parameter change on
    /// the shared channel triggers writes via both gangs in the same
    /// dispatch cycle, and any difference between the two propagation
    /// paths becomes a race. The Gangs tab and the app-level warning
    /// banner both consult this to nag the operator into untangling
    /// the configuration.
    pub fn has_overlap_conflicts(&self) -> bool {
        let active: Vec<&GangGroup> = self
            .groups
            .values()
            .filter(|g| g.enabled && !g.paused)
            .collect();
        for i in 0..active.len() {
            for j in (i + 1)..active.len() {
                let a = active[i];
                let b = active[j];
                let shared_channel = a.members.iter().any(|c| b.members.contains(c));
                if !shared_channel {
                    continue;
                }
                let shared_section = a
                    .linked_sections
                    .iter()
                    .any(|s| b.linked_sections.contains(s));
                if shared_section {
                    return true;
                }
            }
        }
        false
    }

    /// Count distinct channels that appear in two or more active gangs
    /// where those gangs share at least one linked section. Used by the
    /// warning banner to summarise the size of the problem ("⚠ N
    /// channels…").
    pub fn count_overlap_conflict_channels(&self) -> usize {
        use std::collections::HashSet;
        let active: Vec<&GangGroup> = self
            .groups
            .values()
            .filter(|g| g.enabled && !g.paused)
            .collect();
        let mut conflicting: HashSet<ChannelId> = HashSet::new();
        for i in 0..active.len() {
            for j in (i + 1)..active.len() {
                let a = active[i];
                let b = active[j];
                let shared_section = a
                    .linked_sections
                    .iter()
                    .any(|s| b.linked_sections.contains(s));
                if !shared_section {
                    continue;
                }
                for ch in &a.members {
                    if b.members.contains(ch) {
                        conflicting.insert(ch.clone());
                    }
                }
            }
        }
        conflicting.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn make_gang(name: &str, members: Vec<ChannelId>) -> GangGroup {
        GangGroup::new(
            name.into(),
            members,
            HashSet::from([ParameterSection::FaderMutePan, ParameterSection::Eq]),
        )
    }

    #[test]
    fn add_and_remove() {
        let mut mgr = GangManager::new();
        let gang = make_gang("Drums", vec![ChannelId::Input(1), ChannelId::Input(2)]);
        let id = gang.id;
        mgr.add_group(gang);
        assert_eq!(mgr.groups.len(), 1);

        assert!(mgr.remove_group(id));
        assert!(mgr.groups.is_empty());
        assert!(!mgr.remove_group(id)); // already removed
    }

    #[test]
    fn find_by_name_case_insensitive() {
        let mut mgr = GangManager::new();
        mgr.add_group(make_gang("Drums", vec![ChannelId::Input(1)]));

        assert!(mgr.find_by_name("drums").is_some());
        assert!(mgr.find_by_name("DRUMS").is_some());
        assert!(mgr.find_by_name("Drums").is_some());
        assert!(mgr.find_by_name("guitars").is_none());
    }

    #[test]
    fn sorted_groups() {
        let mut mgr = GangManager::new();
        mgr.add_group(make_gang("Vocals", vec![ChannelId::Input(5)]));
        mgr.add_group(make_gang("Drums", vec![ChannelId::Input(1)]));
        mgr.add_group(make_gang("Bass", vec![ChannelId::Input(3)]));

        let sorted = mgr.sorted_groups();
        assert_eq!(sorted[0].name, "Bass");
        assert_eq!(sorted[1].name, "Drums");
        assert_eq!(sorted[2].name, "Vocals");
    }

    #[test]
    fn find_gangs_for_channel_and_section_match() {
        let mut mgr = GangManager::new();
        mgr.add_group(make_gang(
            "Drums",
            vec![ChannelId::Input(1), ChannelId::Input(2)],
        ));

        let found = mgr.find_gangs_for_channel_and_section(
            &ChannelId::Input(1),
            &ParameterSection::FaderMutePan,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Drums");
    }

    #[test]
    fn find_gangs_for_channel_and_section_mismatch() {
        let mut mgr = GangManager::new();
        mgr.add_group(make_gang(
            "Drums",
            vec![ChannelId::Input(1), ChannelId::Input(2)],
        ));

        // Channel not in gang
        let found = mgr.find_gangs_for_channel_and_section(
            &ChannelId::Input(5),
            &ParameterSection::FaderMutePan,
        );
        assert!(found.is_empty());

        // Section not linked
        let found =
            mgr.find_gangs_for_channel_and_section(&ChannelId::Input(1), &ParameterSection::Sends);
        assert!(found.is_empty());
    }

    #[test]
    fn disabled_gangs_excluded() {
        let mut mgr = GangManager::new();
        let mut gang = make_gang("Drums", vec![ChannelId::Input(1), ChannelId::Input(2)]);
        gang.enabled = false;
        mgr.add_group(gang);

        let found = mgr.find_gangs_for_channel_and_section(
            &ChannelId::Input(1),
            &ParameterSection::FaderMutePan,
        );
        assert!(found.is_empty());
    }

    fn gang_with_sections(
        name: &str,
        members: Vec<ChannelId>,
        sections: HashSet<ParameterSection>,
    ) -> GangGroup {
        GangGroup::new(name.into(), members, sections)
    }

    #[test]
    fn no_overlap_when_gangs_dont_share_channels() {
        let mut mgr = GangManager::new();
        mgr.add_group(gang_with_sections(
            "A",
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        ));
        mgr.add_group(gang_with_sections(
            "B",
            vec![ChannelId::Input(3), ChannelId::Input(4)],
            HashSet::from([ParameterSection::FaderMutePan]),
        ));
        assert!(!mgr.has_overlap_conflicts());
        assert_eq!(mgr.count_overlap_conflict_channels(), 0);
    }

    #[test]
    fn no_overlap_when_gangs_share_channel_but_different_sections() {
        let mut mgr = GangManager::new();
        mgr.add_group(gang_with_sections(
            "A",
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        ));
        mgr.add_group(gang_with_sections(
            "B",
            vec![ChannelId::Input(2), ChannelId::Input(3)],
            HashSet::from([ParameterSection::Eq]),
        ));
        assert!(!mgr.has_overlap_conflicts());
        assert_eq!(mgr.count_overlap_conflict_channels(), 0);
    }

    #[test]
    fn overlap_when_shared_channel_and_shared_section() {
        let mut mgr = GangManager::new();
        mgr.add_group(gang_with_sections(
            "A",
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan, ParameterSection::Eq]),
        ));
        mgr.add_group(gang_with_sections(
            "B",
            vec![ChannelId::Input(2), ChannelId::Input(3)],
            HashSet::from([ParameterSection::FaderMutePan]),
        ));
        assert!(mgr.has_overlap_conflicts());
        // Only Input(2) is in both gangs.
        assert_eq!(mgr.count_overlap_conflict_channels(), 1);
    }

    #[test]
    fn paused_or_disabled_gang_does_not_count_as_overlap() {
        let mut mgr = GangManager::new();
        mgr.add_group(gang_with_sections(
            "A",
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        ));
        let mut b = gang_with_sections(
            "B",
            vec![ChannelId::Input(2), ChannelId::Input(3)],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        b.paused = true;
        mgr.add_group(b);
        assert!(!mgr.has_overlap_conflicts());

        // Same with disabled.
        let mut mgr2 = GangManager::new();
        mgr2.add_group(gang_with_sections(
            "A",
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        ));
        let mut c = gang_with_sections(
            "C",
            vec![ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        c.enabled = false;
        mgr2.add_group(c);
        assert!(!mgr2.has_overlap_conflicts());
    }

    #[test]
    fn overlap_count_dedupes_channels_across_pairs() {
        let mut mgr = GangManager::new();
        mgr.add_group(gang_with_sections(
            "A",
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        ));
        mgr.add_group(gang_with_sections(
            "B",
            vec![ChannelId::Input(2), ChannelId::Input(3)],
            HashSet::from([ParameterSection::FaderMutePan]),
        ));
        mgr.add_group(gang_with_sections(
            "C",
            vec![ChannelId::Input(2), ChannelId::Input(4)],
            HashSet::from([ParameterSection::FaderMutePan]),
        ));
        // Input(2) is in all three; should be counted once.
        assert_eq!(mgr.count_overlap_conflict_channels(), 1);
    }
}
