use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;

use crate::console::gang_manager::GangManager;
use crate::model::channel::ChannelId;
use crate::model::gang::GangGroup;
use crate::model::parameter::ParameterSection;
use super::theme;

/// Channel type selector for the Add Gang form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelTypeSelection {
    Input,
    Aux,
    Group,
    Matrix,
    ControlGroup,
    Mixed,
}

impl ChannelTypeSelection {
    fn label(&self) -> &'static str {
        match self {
            Self::Input => "Input",
            Self::Aux => "Aux",
            Self::Group => "Group",
            Self::Matrix => "Matrix",
            Self::ControlGroup => "Control Group",
            Self::Mixed => "Mixed",
        }
    }

    const ALL: [Self; 6] = [
        Self::Input,
        Self::Aux,
        Self::Group,
        Self::Matrix,
        Self::ControlGroup,
        Self::Mixed,
    ];
}

/// Per-tab UI state for the Gangs tab.
pub struct GangsTabState {
    pub new_gang_name: String,
    pub new_gang_channel_type: ChannelTypeSelection,
    /// Range notation: "1-4,7,12" or for Mixed: "I1-4,A1-2,G5"
    pub new_gang_members: String,
    pub new_gang_sections: HashSet<ParameterSection>,
    pub editing_gang_id: Option<uuid::Uuid>,
    pub status_message: Option<String>,
}

impl Default for GangsTabState {
    fn default() -> Self {
        Self {
            new_gang_name: String::new(),
            new_gang_channel_type: ChannelTypeSelection::Input,
            new_gang_members: String::new(),
            new_gang_sections: HashSet::from([ParameterSection::FaderMutePan]),
            editing_gang_id: None,
            status_message: None,
        }
    }
}

/// Draw the Gangs tab.
pub fn draw_gangs_tab(
    ui: &mut egui::Ui,
    tab: &mut GangsTabState,
    gang_manager: &Arc<RwLock<GangManager>>,
    connected: &Arc<AtomicBool>,
    runtime: &tokio::runtime::Handle,
) {
    let is_connected = connected.load(Ordering::Relaxed);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Header
            let mgr = runtime.block_on(gang_manager.read());
            let active_count = mgr.groups.values().filter(|g| g.enabled).count();
            let total_count = mgr.groups.len();

            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Smart Ganging")
                        .size(theme::FONT_SIZE_SECTION)
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );
                ui.add_space(12.0);
                theme::colored_badge(
                    ui,
                    &format!("{active_count} active / {total_count} total"),
                    theme::BG_ELEVATED,
                );
            });

            if !is_connected {
                ui.add_space(4.0);
                ui.colored_label(theme::TEXT_WARNING, "Connect to console for gang propagation to take effect");
            }

            ui.add_space(8.0);

            // ── Add / Edit gang form card ──
            let editing = tab.editing_gang_id.is_some();
            theme::card_frame().show(ui, |ui| {
                theme::section_heading(ui, if editing { "Edit Gang" } else { "New Gang" });

                egui::Grid::new("add_gang_grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Name:");
                        ui.add(egui::TextEdit::singleline(&mut tab.new_gang_name).desired_width(200.0));
                        ui.end_row();

                        ui.label("Channel Type:");
                        egui::ComboBox::from_id_salt("gang_channel_type")
                            .selected_text(tab.new_gang_channel_type.label())
                            .show_ui(ui, |ui| {
                                for ct in &ChannelTypeSelection::ALL {
                                    ui.selectable_value(
                                        &mut tab.new_gang_channel_type,
                                        *ct,
                                        ct.label(),
                                    );
                                }
                            });
                        ui.end_row();

                        ui.label("Members:");
                        let hint = if tab.new_gang_channel_type == ChannelTypeSelection::Mixed {
                            "I1-4,A1-2,G5"
                        } else {
                            "1-4,7,12"
                        };
                        ui.add(
                            egui::TextEdit::singleline(&mut tab.new_gang_members)
                                .hint_text(hint)
                                .desired_width(200.0),
                        );
                        ui.end_row();
                    });

                // Section toggle blocks (instead of checkboxes)
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Linked Sections").strong().color(theme::TEXT_PRIMARY));
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    for section in ParameterSection::all_variants() {
                        let active = tab.new_gang_sections.contains(section);
                        if theme::toggle_block(ui, &section.to_string(), active).clicked() {
                            if active {
                                tab.new_gang_sections.remove(section);
                            } else {
                                tab.new_gang_sections.insert(section.clone());
                            }
                        }
                    }
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let btn_text = if editing { "Save" } else { "Add Gang" };
                    let btn_color = if editing { theme::ACCENT_BLUE } else { theme::ACCENT_GREEN };
                    let save_btn = theme::action_button(btn_text, btn_color, egui::Vec2::new(100.0, 32.0));
                    if ui.add(save_btn).clicked() && !tab.new_gang_name.trim().is_empty() {
                        let members = parse_channel_members(tab.new_gang_channel_type, &tab.new_gang_members);

                        if members.is_empty() {
                            tab.status_message = Some("No valid members parsed".into());
                        } else if tab.new_gang_sections.is_empty() {
                            tab.status_message = Some("Select at least one section".into());
                        } else if members.len() < 2 {
                            tab.status_message = Some("A gang needs at least 2 members".into());
                        } else {
                            let name = tab.new_gang_name.trim().to_string();
                            let sections = tab.new_gang_sections.clone();
                            let mgr_clone = gang_manager.clone();

                            if let Some(edit_id) = tab.editing_gang_id.take() {
                                runtime.spawn(async move {
                                    let mut mgr = mgr_clone.write().await;
                                    if let Some(group) = mgr.groups.get_mut(&edit_id) {
                                        group.name = name;
                                        group.members = members;
                                        group.linked_sections = sections;
                                    }
                                });
                                tab.status_message = Some("Gang updated".into());
                            } else {
                                let group = GangGroup::new(name.clone(), members, sections);
                                runtime.spawn(async move {
                                    mgr_clone.write().await.add_group(group);
                                });
                                tab.status_message = Some(format!("Added gang '{name}'"));
                            }

                            tab.new_gang_name.clear();
                            tab.new_gang_members.clear();
                            tab.new_gang_sections = HashSet::from([ParameterSection::FaderMutePan]);
                        }
                    }

                    if editing {
                        let cancel_btn = theme::action_button("Cancel", theme::BG_ELEVATED, egui::Vec2::new(80.0, 32.0));
                        if ui.add(cancel_btn).clicked() {
                            tab.editing_gang_id = None;
                            tab.new_gang_name.clear();
                            tab.new_gang_members.clear();
                            tab.new_gang_sections = HashSet::from([ParameterSection::FaderMutePan]);
                            tab.status_message = None;
                        }
                    }
                });

                // Status message
                if let Some(ref msg) = tab.status_message {
                    ui.add_space(4.0);
                    ui.colored_label(theme::TEXT_WARNING, msg.as_str());
                }
            });

            ui.add_space(8.0);

            // ── Gang list card ──
            theme::card_frame().show(ui, |ui| {
                theme::section_heading(ui, "Gang Groups");

                let groups: Vec<GangGroup> = mgr.sorted_groups().into_iter().cloned().collect();
                drop(mgr);

                if groups.is_empty() {
                    ui.label(egui::RichText::new("No gang groups configured.").color(theme::TEXT_SECONDARY));
                } else {
                    let mut to_remove = None;
                    let mut to_edit = None;
                    let mut to_toggle = None;

                    for group in &groups {
                        let bg = if group.enabled { theme::BG_ELEVATED } else { theme::BG_PANEL };
                        egui::Frame::new()
                            .fill(bg)
                            .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE))
                            .corner_radius(6.0)
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    // Enable/disable toggle
                                    let toggle_color = if group.enabled {
                                        theme::ACCENT_GREEN
                                    } else {
                                        theme::BG_ELEVATED
                                    };
                                    let toggle_label = if group.enabled { "ON" } else { "OFF" };
                                    let toggle_btn = egui::Button::new(
                                        egui::RichText::new(toggle_label)
                                            .color(theme::TEXT_PRIMARY)
                                            .strong()
                                            .small(),
                                    )
                                    .fill(toggle_color)
                                    .corner_radius(4.0);
                                    if ui.add(toggle_btn).clicked() {
                                        to_toggle = Some((group.id, !group.enabled));
                                    }

                                    ui.add_space(8.0);

                                    // Gang name
                                    ui.label(
                                        egui::RichText::new(&group.name)
                                            .strong()
                                            .color(theme::TEXT_PRIMARY),
                                    );

                                    ui.add_space(8.0);

                                    // Member badge
                                    let member_text = format_members(&group.members);
                                    // Determine member type color
                                    let member_color = if !group.members.is_empty() {
                                        theme::channel_color(&group.members[0])
                                    } else {
                                        theme::BG_ELEVATED
                                    };
                                    theme::colored_badge(ui, &member_text, member_color);

                                    ui.add_space(4.0);

                                    // Section badges
                                    for section in &group.linked_sections {
                                        theme::colored_badge(ui, &section.to_string(), theme::SCOPE_ACTIVE);
                                    }
                                });

                                // Action buttons row
                                ui.horizontal(|ui| {
                                    ui.add_space(52.0); // align under content
                                    let edit_btn = theme::action_button("Edit", theme::ACCENT_ORANGE, egui::Vec2::new(60.0, 24.0));
                                    if ui.add(edit_btn).clicked() {
                                        to_edit = Some(group.clone());
                                    }
                                    let del_btn = theme::action_button("Delete", theme::ACCENT_RED, egui::Vec2::new(60.0, 24.0));
                                    if ui.add(del_btn).clicked() {
                                        to_remove = Some(group.id);
                                    }
                                });
                            });
                        ui.add_space(4.0);
                    }

                    if let Some(id) = to_remove {
                        let mgr_clone = gang_manager.clone();
                        runtime.spawn(async move {
                            mgr_clone.write().await.remove_group(id);
                        });
                        tab.status_message = Some("Gang removed".into());
                    }

                    if let Some((id, new_enabled)) = to_toggle {
                        let mgr_clone = gang_manager.clone();
                        runtime.spawn(async move {
                            let mut mgr = mgr_clone.write().await;
                            if let Some(group) = mgr.groups.get_mut(&id) {
                                group.enabled = new_enabled;
                            }
                        });
                    }

                    if let Some(group) = to_edit {
                        tab.editing_gang_id = Some(group.id);
                        tab.new_gang_name = group.name.clone();
                        tab.new_gang_members = format_members(&group.members);
                        tab.new_gang_sections = group.linked_sections.clone();
                        tab.status_message = None;
                    }
                }
            });
        });
}

/// Format a list of channel members for display.
fn format_members(members: &[ChannelId]) -> String {
    if members.is_empty() {
        return String::new();
    }

    // Check if all members are the same type
    let all_same_type = members.windows(2).all(|w| {
        std::mem::discriminant(&w[0]) == std::mem::discriminant(&w[1])
    });

    if all_same_type {
        // Simple format: just the numbers with ranges
        let prefix = match members[0] {
            ChannelId::Input(_) => "Input",
            ChannelId::Aux(_) => "Aux",
            ChannelId::Group(_) => "Group",
            ChannelId::Matrix(_) => "Mtx",
            ChannelId::ControlGroup(_) => "CG",
            ChannelId::GraphicEq(_) => "GEQ",
            ChannelId::MatrixInput(_) => "MtxIn",
        };
        let numbers: Vec<u8> = members
            .iter()
            .map(|m| match m {
                ChannelId::Input(n)
                | ChannelId::Aux(n)
                | ChannelId::Group(n)
                | ChannelId::Matrix(n)
                | ChannelId::ControlGroup(n)
                | ChannelId::GraphicEq(n)
                | ChannelId::MatrixInput(n) => *n,
            })
            .collect();
        format!("{} {}", prefix, format_ranges(&numbers))
    } else {
        // Mixed: use prefix notation
        let mut parts = Vec::new();
        for m in members {
            let (prefix, n) = match m {
                ChannelId::Input(n) => ("I", *n),
                ChannelId::Aux(n) => ("A", *n),
                ChannelId::Group(n) => ("G", *n),
                ChannelId::Matrix(n) => ("M", *n),
                ChannelId::ControlGroup(n) => ("CG", *n),
                ChannelId::GraphicEq(n) => ("GEQ", *n),
                ChannelId::MatrixInput(n) => ("MI", *n),
            };
            parts.push(format!("{prefix}{n}"));
        }
        parts.join(",")
    }
}

/// Compress a sorted list of numbers into range notation: [1,2,3,7,12] -> "1-3,7,12"
fn format_ranges(numbers: &[u8]) -> String {
    if numbers.is_empty() {
        return String::new();
    }

    let mut sorted = numbers.to_vec();
    sorted.sort();
    sorted.dedup();

    let mut parts = Vec::new();
    let mut start = sorted[0];
    let mut end = sorted[0];

    for &n in &sorted[1..] {
        if n == end + 1 {
            end = n;
        } else {
            if start == end {
                parts.push(start.to_string());
            } else {
                parts.push(format!("{start}-{end}"));
            }
            start = n;
            end = n;
        }
    }
    if start == end {
        parts.push(start.to_string());
    } else {
        parts.push(format!("{start}-{end}"));
    }

    parts.join(",")
}

/// Parse channel members from text input.
///
/// For single-type modes: "1-4,7,12" -> vec of that type.
/// For Mixed mode: "I1-4,A1-2,G5" -> mixed vec.
pub fn parse_channel_members(
    channel_type: ChannelTypeSelection,
    input: &str,
) -> Vec<ChannelId> {
    let input = input.trim();
    if input.is_empty() {
        return Vec::new();
    }

    if channel_type == ChannelTypeSelection::Mixed {
        parse_mixed_members(input)
    } else {
        let numbers = parse_number_ranges(input);
        let constructor: fn(u8) -> ChannelId = match channel_type {
            ChannelTypeSelection::Input => ChannelId::Input,
            ChannelTypeSelection::Aux => ChannelId::Aux,
            ChannelTypeSelection::Group => ChannelId::Group,
            ChannelTypeSelection::Matrix => ChannelId::Matrix,
            ChannelTypeSelection::ControlGroup => ChannelId::ControlGroup,
            ChannelTypeSelection::Mixed => unreachable!(),
        };
        numbers.into_iter().map(constructor).collect()
    }
}

/// Parse "I1-4,A1-2,G5" into mixed channel IDs.
fn parse_mixed_members(input: &str) -> Vec<ChannelId> {
    let mut result = Vec::new();

    for token in input.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }

        // Determine prefix and rest
        let (constructor, rest): (fn(u8) -> ChannelId, &str) =
            if let Some(r) = token.strip_prefix("CG") {
                (ChannelId::ControlGroup, r)
            } else if let Some(r) = token.strip_prefix("GEQ") {
                (ChannelId::GraphicEq, r)
            } else if let Some(r) = token.strip_prefix("MI") {
                (ChannelId::MatrixInput, r)
            } else if let Some(r) = token.strip_prefix('I') {
                (ChannelId::Input, r)
            } else if let Some(r) = token.strip_prefix('A') {
                (ChannelId::Aux, r)
            } else if let Some(r) = token.strip_prefix('G') {
                (ChannelId::Group, r)
            } else if let Some(r) = token.strip_prefix('M') {
                (ChannelId::Matrix, r)
            } else {
                continue; // Unknown prefix, skip
            };

        let numbers = parse_number_ranges(rest);
        result.extend(numbers.into_iter().map(constructor));
    }

    result
}

/// Parse "1-4,7,12" into a vec of numbers.
fn parse_number_ranges(input: &str) -> Vec<u8> {
    let mut result = Vec::new();

    for part in input.split(|c: char| c == ',' || c == ' ') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        if let Some((start_str, end_str)) = part.split_once('-') {
            if let (Ok(start), Ok(end)) = (
                start_str.trim().parse::<u8>(),
                end_str.trim().parse::<u8>(),
            ) {
                for n in start..=end {
                    result.push(n);
                }
            }
        } else if let Ok(n) = part.parse::<u8>() {
            result.push(n);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_range() {
        let result = parse_channel_members(ChannelTypeSelection::Input, "1-4,7,12");
        assert_eq!(result.len(), 6);
        assert_eq!(result[0], ChannelId::Input(1));
        assert_eq!(result[3], ChannelId::Input(4));
        assert_eq!(result[4], ChannelId::Input(7));
        assert_eq!(result[5], ChannelId::Input(12));
    }

    #[test]
    fn parse_aux_single() {
        let result = parse_channel_members(ChannelTypeSelection::Aux, "3");
        assert_eq!(result, vec![ChannelId::Aux(3)]);
    }

    #[test]
    fn parse_mixed_members_notation() {
        let result = parse_channel_members(ChannelTypeSelection::Mixed, "I1-3,A1,G5");
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], ChannelId::Input(1));
        assert_eq!(result[1], ChannelId::Input(2));
        assert_eq!(result[2], ChannelId::Input(3));
        assert_eq!(result[3], ChannelId::Aux(1));
        assert_eq!(result[4], ChannelId::Group(5));
    }

    #[test]
    fn parse_empty_returns_empty() {
        let result = parse_channel_members(ChannelTypeSelection::Input, "");
        assert!(result.is_empty());
    }

    #[test]
    fn format_ranges_compresses() {
        assert_eq!(format_ranges(&[1, 2, 3, 7, 12]), "1-3,7,12");
        assert_eq!(format_ranges(&[5]), "5");
        assert_eq!(format_ranges(&[1, 3, 5]), "1,3,5");
    }

    #[test]
    fn format_members_same_type() {
        let members = vec![
            ChannelId::Input(1),
            ChannelId::Input(2),
            ChannelId::Input(3),
        ];
        assert_eq!(format_members(&members), "Input 1-3");
    }

    #[test]
    fn format_members_mixed() {
        let members = vec![
            ChannelId::Input(1),
            ChannelId::Aux(2),
            ChannelId::Group(5),
        ];
        assert_eq!(format_members(&members), "I1,A2,G5");
    }

    #[test]
    fn parse_mixed_control_group() {
        let result = parse_channel_members(ChannelTypeSelection::Mixed, "CG1-3,I5");
        assert_eq!(result.len(), 4);
        assert_eq!(result[0], ChannelId::ControlGroup(1));
        assert_eq!(result[1], ChannelId::ControlGroup(2));
        assert_eq!(result[2], ChannelId::ControlGroup(3));
        assert_eq!(result[3], ChannelId::Input(5));
    }
}
