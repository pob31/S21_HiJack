use std::collections::{HashMap, HashSet};

use eframe::egui;

use crate::model::channel::ChannelId;
use crate::model::parameter::ParameterSection;
use crate::model::snapshot::{ChannelScope, ScopeTemplate};
use super::theme;

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

    /// Color for this channel group (DiGiCo style).
    pub fn color(&self) -> egui::Color32 {
        match self {
            ChannelGroup::Inputs => theme::CH_INPUT,
            ChannelGroup::Aux => theme::CH_AUX,
            ChannelGroup::Groups => theme::CH_GROUP,
            ChannelGroup::Matrix | ChannelGroup::GraphicEq | ChannelGroup::MatrixInputs => {
                theme::CH_MATRIX
            }
            ChannelGroup::ControlGroups => theme::CH_CG,
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

/// Signal-flow column definition for the block layout.
struct FlowColumn {
    heading: &'static str,
    sections: Vec<(ParameterSection, &'static str)>,
}

/// Build the signal-flow columns for a given set of applicable sections.
fn build_flow_columns(applicable: &[ParameterSection]) -> Vec<FlowColumn> {
    let mut columns = Vec::new();

    // Sources
    let mut sources = Vec::new();
    for s in applicable {
        match s {
            ParameterSection::Name => sources.push((s.clone(), "Channel\nName")),
            ParameterSection::InputGain => sources.push((s.clone(), "Input Gain")),
            _ => {}
        }
    }
    if !sources.is_empty() {
        columns.push(FlowColumn {
            heading: "Sources",
            sections: sources,
        });
    }

    // Input Processing
    let mut input_proc = Vec::new();
    for s in applicable {
        match s {
            ParameterSection::Digitube => input_proc.push((s.clone(), "DiGiTube")),
            ParameterSection::Delay => input_proc.push((s.clone(), "Delay")),
            _ => {}
        }
    }
    if !input_proc.is_empty() {
        columns.push(FlowColumn {
            heading: "Input\nProcessing",
            sections: input_proc,
        });
    }

    // Insert
    let mut insert = Vec::new();
    for s in applicable {
        if matches!(s, ParameterSection::Inserts) {
            insert.push((s.clone(), "Send &\nReturn"));
        }
    }
    if !insert.is_empty() {
        columns.push(FlowColumn {
            heading: "Insert",
            sections: insert,
        });
    }

    // Channel Processing
    let mut chan_proc = Vec::new();
    for s in applicable {
        match s {
            ParameterSection::Eq => chan_proc.push((s.clone(), "Equaliser")),
            ParameterSection::Dyn1 => chan_proc.push((s.clone(), "Dynamics 1")),
            ParameterSection::Dyn2 => chan_proc.push((s.clone(), "Dynamics 2")),
            ParameterSection::GraphicEq => chan_proc.push((s.clone(), "Graphic EQ")),
            _ => {}
        }
    }
    if !chan_proc.is_empty() {
        columns.push(FlowColumn {
            heading: "Channel\nProcessing",
            sections: chan_proc,
        });
    }

    // Outputs
    let mut outputs = Vec::new();
    for s in applicable {
        match s {
            ParameterSection::Sends => outputs.push((s.clone(), "Aux Sends")),
            ParameterSection::GroupRouting => outputs.push((s.clone(), "Group\nAssigns")),
            ParameterSection::MatrixSends => outputs.push((s.clone(), "Matrix\nSends")),
            ParameterSection::FaderMutePan => outputs.push((s.clone(), "Fader /\nMute / Pan")),
            ParameterSection::CgMembership => outputs.push((s.clone(), "CG\nMembers")),
            _ => {}
        }
    }
    if !outputs.is_empty() {
        columns.push(FlowColumn {
            heading: "Outputs",
            sections: outputs,
        });
    }

    columns
}

/// State for the scope editor widget.
pub struct ScopeEditorState {
    /// Per-channel section selections.
    pub channel_selections: HashMap<ChannelId, HashSet<ParameterSection>>,
    /// Which channel groups are expanded in the UI.
    pub expanded_groups: HashSet<ChannelGroup>,
    /// Currently active channel group in the signal-flow view.
    pub active_group: Option<ChannelGroup>,
}

impl Default for ScopeEditorState {
    fn default() -> Self {
        Self {
            channel_selections: HashMap::new(),
            expanded_groups: HashSet::new(),
            active_group: Some(ChannelGroup::Inputs),
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
            active_group: Some(ChannelGroup::Inputs),
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

/// Draw the scope editor widget with DiGiCo-style signal-flow blocks.
pub fn draw_scope_editor(
    ui: &mut egui::Ui,
    state: &mut ScopeEditorState,
    input_count: u8,
    aux_count: u8,
    group_count: u8,
) {
    theme::section_heading(ui, "Scope Editor");

    // Controls bar
    ui.horizontal(|ui| {
        let clear_btn = theme::action_button(
            "Clear All",
            theme::BG_ELEVATED,
            egui::Vec2::new(80.0, 30.0),
        );
        if ui.add(clear_btn).clicked() {
            state.clear();
        }
        ui.add_space(8.0);
        theme::colored_badge(
            ui,
            &format!("{} selections", state.selection_count()),
            theme::ACCENT_BLUE,
        );
    });

    ui.add_space(8.0);

    // Channel group selector tabs (colored blocks)
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new("Channel Group:")
                .color(theme::TEXT_SECONDARY)
                .size(theme::FONT_SIZE_BADGE),
        );
        for group in ChannelGroup::all() {
            let channels = group.channels(input_count, aux_count, group_count);
            if channels.is_empty() {
                continue;
            }
            let is_active = state.active_group.as_ref() == Some(group);
            let base_color = group.color();
            let fill = if is_active {
                base_color
            } else {
                theme::BG_ELEVATED
            };
            let text_color = if is_active {
                theme::TEXT_PRIMARY
            } else {
                theme::TEXT_SECONDARY
            };

            let label = format!("{} ({})", group.label(), channels.len());
            let btn = egui::Button::new(
                egui::RichText::new(&label)
                    .color(text_color)
                    .size(theme::FONT_SIZE_BADGE),
            )
            .fill(fill)
            .corner_radius(4.0);

            if ui.add(btn).clicked() {
                state.active_group = Some(group.clone());
            }
        }
    });

    ui.add_space(8.0);

    // Signal-flow block grid for the active group
    if let Some(active_group) = &state.active_group.clone() {
        let applicable = active_group.applicable_sections();
        let columns = build_flow_columns(&applicable);

        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (col_idx, column) in columns.iter().enumerate() {
                        // Arrow separator between columns
                        if col_idx > 0 {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new("\u{25B6}")
                                    .color(theme::TEXT_SECONDARY)
                                    .size(theme::FONT_SIZE_BODY),
                            );
                            ui.add_space(4.0);
                        }

                        // Column
                        ui.vertical(|ui| {
                            ui.set_min_width(100.0);

                            // Column heading
                            ui.label(
                                egui::RichText::new(column.heading)
                                    .strong()
                                    .color(theme::TEXT_PRIMARY)
                                    .size(theme::FONT_SIZE_BADGE),
                            );
                            ui.add_space(4.0);

                            // Section toggle blocks
                            for (section, display_name) in &column.sections {
                                let all_sel = state.is_group_section_all_selected(
                                    active_group,
                                    section,
                                    input_count,
                                    aux_count,
                                    group_count,
                                );
                                let any_sel = state.is_group_section_any_selected(
                                    active_group,
                                    section,
                                    input_count,
                                    aux_count,
                                    group_count,
                                );

                                let response = theme::toggle_block_tristate(
                                    ui,
                                    display_name,
                                    all_sel,
                                    any_sel,
                                );

                                if response.clicked() {
                                    let enable = !all_sel;
                                    state.toggle_group_section(
                                        active_group,
                                        section,
                                        enable,
                                        input_count,
                                        aux_count,
                                        group_count,
                                    );
                                }
                            }
                        });
                    }
                });
            });
    }
}
