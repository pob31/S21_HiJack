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
        self.groups.values().find(|g| g.name.to_lowercase() == lower)
    }

    /// All groups sorted by name for UI display.
    pub fn sorted_groups(&self) -> Vec<&GangGroup> {
        let mut sorted: Vec<_> = self.groups.values().collect();
        sorted.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
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
            .filter(|g| {
                g.enabled && g.contains_channel(channel) && g.links_section(section)
            })
            .collect()
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
        let found = mgr.find_gangs_for_channel_and_section(
            &ChannelId::Input(1),
            &ParameterSection::Sends,
        );
        assert!(found.is_empty());
    }

    #[test]
    fn disabled_gangs_excluded() {
        let mut mgr = GangManager::new();
        let mut gang = make_gang(
            "Drums",
            vec![ChannelId::Input(1), ChannelId::Input(2)],
        );
        gang.enabled = false;
        mgr.add_group(gang);

        let found = mgr.find_gangs_for_channel_and_section(
            &ChannelId::Input(1),
            &ParameterSection::FaderMutePan,
        );
        assert!(found.is_empty());
    }
}
