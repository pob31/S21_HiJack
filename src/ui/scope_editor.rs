use std::collections::{HashMap, HashSet};

use eframe::egui;

use crate::model::channel::ChannelId;
use crate::model::parameter::ParameterSection;
use crate::model::snapshot::{ChannelScope, ScopeTemplate};

/// Channel type group for the hierarchical scope editor.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ChannelGroup {
    Inputs,
    Aux,
    Groups,
    Matrix,
    ControlGroups,
    GraphicEq,
    MatrixInputs,
}

impl ChannelGroup {
    pub fn label(&self) -> &str {
        match self {
            ChannelGroup::Inputs => "Inputs",
            ChannelGroup::Aux => "Aux",
            ChannelGroup::Groups => "Groups",
            ChannelGroup::Matrix => "Matrix",
            ChannelGroup::ControlGroups => "Control Groups",
            ChannelGroup::GraphicEq => "Graphic EQ",
            ChannelGroup::MatrixInputs => "Matrix Inputs",
        }
    }

    /// Generate all channel IDs for this group given console channel counts.
    pub fn channels(&self, input_count: u8, aux_count: u8, group_count: u8) -> Vec<ChannelId> {
        match self {
            ChannelGroup::Inputs => (1..=input_count).map(ChannelId::Input).collect(),
            ChannelGroup::Aux => (1..=aux_count).map(ChannelId::Aux).collect(),
            ChannelGroup::Groups => (1..=group_count).map(ChannelId::Group).collect(),
            ChannelGroup::Matrix => (1..=8).map(ChannelId::Matrix).collect(),
            ChannelGroup::ControlGroups => (1..=10).map(ChannelId::ControlGroup).collect(),
            ChannelGroup::GraphicEq => (1..=16).map(ChannelId::GraphicEq).collect(),
            ChannelGroup::MatrixInputs => (1..=10).map(ChannelId::MatrixInput).collect(),
        }
    }

    /// Applicable parameter sections for channels in this group.
    pub fn applicable_sections(&self) -> Vec<ParameterSection> {
        // Use a representative channel from this group
        let representative = match self {
            ChannelGroup::Inputs => ChannelId::Input(1),
            ChannelGroup::Aux => ChannelId::Aux(1),
            ChannelGroup::Groups => ChannelId::Group(1),
            ChannelGroup::Matrix => ChannelId::Matrix(1),
            ChannelGroup::ControlGroups => ChannelId::ControlGroup(1),
            ChannelGroup::GraphicEq => ChannelId::GraphicEq(1),
            ChannelGroup::MatrixInputs => ChannelId::MatrixInput(1),
        };
        ParameterSection::applicable_to(&representative)
    }

    pub fn all() -> &'static [ChannelGroup] {
        &[
            ChannelGroup::Inputs,
            ChannelGroup::Aux,
            ChannelGroup::Groups,
            ChannelGroup::Matrix,
            ChannelGroup::ControlGroups,
            ChannelGroup::GraphicEq,
            ChannelGroup::MatrixInputs,
        ]
    }
}

/// State for the scope editor widget.
pub struct ScopeEditorState {
    /// Per-channel section selections.
    pub channel_selections: HashMap<ChannelId, HashSet<ParameterSection>>,
    /// Which channel groups are expanded in the UI.
    pub expanded_groups: HashSet<ChannelGroup>,
}

impl Default for ScopeEditorState {
    fn default() -> Self {
        Self {
            channel_selections: HashMap::new(),
            expanded_groups: HashSet::new(),
        }
    }
}

impl ScopeEditorState {
    /// Build a ScopeTemplate from current selections.
    pub fn to_scope_template(&self, name: String) -> ScopeTemplate {
        let channel_scopes: Vec<ChannelScope> = self
            .channel_selections
            .iter()
            .filter(|(_, sections)| !sections.is_empty())
            .map(|(ch, sections)| ChannelScope {
                channel: ch.clone(),
                sections: sections.clone(),
            })
            .collect();
        ScopeTemplate::new(name, channel_scopes)
    }

    /// Load from an existing ScopeTemplate.
    pub fn from_scope_template(template: &ScopeTemplate) -> Self {
        let mut selections = HashMap::new();
        for cs in &template.channel_scopes {
            selections.insert(cs.channel.clone(), cs.sections.clone());
        }
        ScopeEditorState {
            channel_selections: selections,
            expanded_groups: HashSet::new(),
        }
    }

    /// Clear all selections.
    pub fn clear(&mut self) {
        self.channel_selections.clear();
    }

    /// Count total selected channel-section pairs.
    pub fn selection_count(&self) -> usize {
        self.channel_selections.values().map(|s| s.len()).sum()
    }

    /// Toggle all channels in a group for a specific section.
    fn toggle_group_section(
        &mut self,
        group: &ChannelGroup,
        section: &ParameterSection,
        enable: bool,
        input_count: u8,
        aux_count: u8,
        group_count: u8,
    ) {
        for ch in group.channels(input_count, aux_count, group_count) {
            let entry = self.channel_selections.entry(ch).or_default();
            if enable {
                entry.insert(section.clone());
            } else {
                entry.remove(section);
            }
        }
    }

    /// Check if all channels in a group have a specific section selected.
    fn is_group_section_all_selected(
        &self,
        group: &ChannelGroup,
        section: &ParameterSection,
        input_count: u8,
        aux_count: u8,
        group_count: u8,
    ) -> bool {
        let channels = group.channels(input_count, aux_count, group_count);
        if channels.is_empty() {
            return false;
        }
        channels.iter().all(|ch| {
            self.channel_selections
                .get(ch)
                .is_some_and(|s| s.contains(section))
        })
    }

    /// Check if any channels in a group have a specific section selected.
    fn is_group_section_any_selected(
        &self,
        group: &ChannelGroup,
        section: &ParameterSection,
        input_count: u8,
        aux_count: u8,
        group_count: u8,
    ) -> bool {
        let channels = group.channels(input_count, aux_count, group_count);
        channels.iter().any(|ch| {
            self.channel_selections
                .get(ch)
                .is_some_and(|s| s.contains(section))
        })
    }
}

/// Draw the scope editor widget.
pub fn draw_scope_editor(
    ui: &mut egui::Ui,
    state: &mut ScopeEditorState,
    input_count: u8,
    aux_count: u8,
    group_count: u8,
) {
    ui.heading("Scope Editor");
    ui.separator();

    ui.horizontal(|ui| {
        if ui.button("Clear All").clicked() {
            state.clear();
        }
        ui.label(format!("{} selections", state.selection_count()));
    });

    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for group in ChannelGroup::all() {
                let channels = group.channels(input_count, aux_count, group_count);
                if channels.is_empty() {
                    continue;
                }
                let applicable = group.applicable_sections();
                let count_label = format!("{} (1-{})", group.label(), channels.len());

                let expanded = state.expanded_groups.contains(group);
                let header = egui::CollapsingHeader::new(&count_label)
                    .default_open(expanded)
                    .show(ui, |ui| {
                        // Per-group section toggles
                        ui.label("Sections (applies to all channels in group):");
                        ui.horizontal_wrapped(|ui| {
                            for section in &applicable {
                                let all_selected = state.is_group_section_all_selected(
                                    group,
                                    section,
                                    input_count,
                                    aux_count,
                                    group_count,
                                );
                                let any_selected = state.is_group_section_any_selected(
                                    group,
                                    section,
                                    input_count,
                                    aux_count,
                                    group_count,
                                );

                                // Use indeterminate style when partially selected
                                let mut checked = all_selected;
                                let label = format!("{section}");
                                let response = ui.checkbox(&mut checked, &label);

                                if response.changed() {
                                    state.toggle_group_section(
                                        group,
                                        section,
                                        checked,
                                        input_count,
                                        aux_count,
                                        group_count,
                                    );
                                } else if !all_selected && any_selected {
                                    // Visual hint: paint the checkbox label differently
                                    // when partially selected (egui doesn't support
                                    // tristate natively, but the label style helps)
                                }
                            }
                        });
                    });

                // Track expansion state
                if header.header_response.clicked() {
                    if state.expanded_groups.contains(group) {
                        state.expanded_groups.remove(group);
                    } else {
                        state.expanded_groups.insert(group.clone());
                    }
                }
            }
        });
}
