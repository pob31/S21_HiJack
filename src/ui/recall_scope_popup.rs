//! Popup windows for editing the console's recall scope (global) and
//! per-channel recall safe settings. These are visual references only —
//! the app cannot read or write these settings on the console via OSC.

use eframe::egui;

use crate::model::channel::ChannelId;
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
use crate::model::recall_scope::{ConsoleRecallConfig, RecallBlock};
use crate::model::state::ConsoleState;
use super::theme;

/// UI state for the recall scope/safe popups.
#[derive(Default)]
pub struct RecallScopePopupState {
    /// Which popup is currently open.
    pub open: Option<RecallPopupKind>,
    /// For per-channel safe: which channel index is selected (1-based).
    pub selected_channel: u8,
}

/// Which popup to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecallPopupKind {
    SessionScope,
    InputSafe,
    AuxSafe,
    GroupSafe,
    MatrixSafe,
    CgSafe,
}

impl RecallPopupKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::SessionScope => "Session Recall Scope",
            Self::InputSafe => "Input Recall Safe",
            Self::AuxSafe => "Aux Recall Safe",
            Self::GroupSafe => "Group Recall Safe",
            Self::MatrixSafe => "Matrix Recall Safe",
            Self::CgSafe => "CG Recall Safe",
        }
    }

    pub fn is_scope(&self) -> bool {
        matches!(self, Self::SessionScope)
    }

    pub fn channel_id(&self, index: u8) -> Option<ChannelId> {
        match self {
            Self::SessionScope => None,
            Self::InputSafe => Some(ChannelId::Input(index)),
            Self::AuxSafe => Some(ChannelId::Aux(index)),
            Self::GroupSafe => Some(ChannelId::Group(index)),
            Self::MatrixSafe => Some(ChannelId::Matrix(index)),
            Self::CgSafe => Some(ChannelId::ControlGroup(index)),
        }
    }

    pub fn max_channels(&self, ic: u8, ac: u8, gc: u8, mc: u8, cc: u8) -> u8 {
        match self {
            Self::SessionScope => 0,
            Self::InputSafe => ic,
            Self::AuxSafe => ac,
            Self::GroupSafe => gc,
            Self::MatrixSafe => mc,
            Self::CgSafe => cc,
        }
    }

    pub fn reference_channel(&self) -> ChannelId {
        match self {
            Self::SessionScope | Self::InputSafe => ChannelId::Input(1),
            Self::AuxSafe => ChannelId::Aux(1),
            Self::GroupSafe => ChannelId::Group(1),
            Self::MatrixSafe => ChannelId::Matrix(1),
            Self::CgSafe => ChannelId::ControlGroup(1),
        }
    }

    pub fn type_label(&self) -> &'static str {
        match self {
            Self::SessionScope => "",
            Self::InputSafe => "Inputs",
            Self::AuxSafe => "Auxes",
            Self::GroupSafe => "Groups",
            Self::MatrixSafe => "Matrices",
            Self::CgSafe => "CGs",
        }
    }
}

/// Draw the recall scope/safe popup if open.
pub fn draw_recall_popup(
    ctx: &egui::Context,
    state: &mut RecallScopePopupState,
    config: &mut ConsoleRecallConfig,
    console_state: &ConsoleState,
) {
    let input_count = console_state.config.input_channel_count;
    let aux_count = console_state.config.aux_output_count;
    let group_count = console_state.config.group_output_count;
    let matrix_count = console_state.config.matrix_output_count;
    let cg_count = console_state.config.control_group_count;
    let Some(kind) = state.open.clone() else { return };

    let title = kind.label();
    let mut open = true;

    egui::Window::new(title)
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .show(ctx, |ui| {
            // Warning banner
            ui.label(
                egui::RichText::new("Visual reference only — not sent to the console")
                    .color(theme::TEXT_WARNING)
                    .small()
                    .italics(),
            );
            ui.add_space(4.0);

            // Channel selector (for safe popups): number + arrows
            if !kind.is_scope() {
                let max = kind.max_channels(input_count, aux_count, group_count, matrix_count, cg_count);
                if max > 0 {
                    ui.horizontal(|ui| {
                        ui.label("Channel:");
                        if ui.add_enabled(
                            state.selected_channel > 1,
                            egui::Button::new("◀").min_size(egui::Vec2::new(28.0, 22.0)),
                        ).clicked() {
                            state.selected_channel = state.selected_channel.saturating_sub(1).max(1);
                        }
                        ui.label(
                            egui::RichText::new(format!("{}", state.selected_channel))
                                .strong()
                                .size(14.0),
                        );
                        if ui.add_enabled(
                            state.selected_channel < max,
                            egui::Button::new("▶").min_size(egui::Vec2::new(28.0, 22.0)),
                        ).clicked() {
                            state.selected_channel = (state.selected_channel + 1).min(max);
                        }
                        ui.label(
                            egui::RichText::new(format!("/ {max}"))
                                .color(theme::TEXT_SECONDARY)
                                .small(),
                        );
                        // Show channel name
                        if let Some(ch_id) = kind.channel_id(state.selected_channel) {
                            let name = console_state
                                .get(&ParameterAddress {
                                    channel: ch_id,
                                    parameter: ParameterPath::Name,
                                })
                                .and_then(|v| match v {
                                    ParameterValue::String(s) => Some(s.as_str()),
                                    _ => None,
                                })
                                .unwrap_or("");
                            if !name.is_empty() {
                                ui.add_space(8.0);
                                ui.label(
                                    egui::RichText::new(name)
                                        .color(theme::TEXT_PRIMARY)
                                        .strong(),
                                );
                            }
                        }
                    });
                    ui.add_space(4.0);
                }
            }

            let is_scope = kind.is_scope();
            let channel_id = kind.channel_id(state.selected_channel);
            let ref_channel = kind.reference_channel();

            // Global blocks row (Session Scope only)
            if is_scope {
                ui.label(egui::RichText::new("Global").color(theme::TEXT_SECONDARY).small());
                ui.horizontal_wrapped(|ui| {
                    for &block in RecallBlock::global_blocks() {
                        draw_block_button(ui, block, config.session_scope.active_blocks.contains(&block), true, is_scope, |toggled| {
                            if toggled { config.session_scope.active_blocks.insert(block); }
                            else { config.session_scope.active_blocks.remove(&block); }
                        });
                    }
                });
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
            }

            // Signal-flow sections — compact vertical layout
            let sections: &[(&str, &[RecallBlock])] = &[
                ("Sources", RecallBlock::sources_column()),
                ("Input Processing", RecallBlock::input_processing_column()),
                ("Insert (A)", RecallBlock::insert_a_column()),
                ("Channel Processing", RecallBlock::channel_processing_column()),
                ("Insert (B)", RecallBlock::insert_b_column()),
                ("Outputs", RecallBlock::outputs_column()),
            ];

            for &(section_name, blocks) in sections {
                // Skip sections with no applicable blocks for this channel type
                let any_available = blocks.iter().any(|b| RecallBlock::available_for_channel(*b, &ref_channel));
                if !any_available { continue; }

                ui.label(egui::RichText::new(section_name).color(theme::TEXT_SECONDARY).small());
                ui.horizontal_wrapped(|ui| {
                    for &block in blocks {
                        let available = RecallBlock::available_for_channel(block, &ref_channel);
                        let active = if is_scope {
                            config.session_scope.active_blocks.contains(&block)
                        } else if let Some(ch) = &channel_id {
                            config.channel_safes.get(ch).is_some_and(|s| s.safe_blocks.contains(&block))
                        } else {
                            false
                        };

                        draw_block_button(ui, block, active, available, is_scope, |toggled| {
                            if is_scope {
                                if toggled { config.session_scope.active_blocks.insert(block); }
                                else { config.session_scope.active_blocks.remove(&block); }
                            } else if let Some(ch) = &channel_id {
                                let safe = config.channel_safes.entry(ch.clone()).or_default();
                                if toggled { safe.safe_blocks.insert(block); }
                                else { safe.safe_blocks.remove(&block); }
                            }
                        });
                    }
                });
                ui.add_space(2.0);
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);

            // Footer: Copy to all + Close
            ui.horizontal(|ui| {
                if !is_scope {
                    if let Some(ref ch_id) = channel_id {
                        let copy_label = format!("Copy to all {}", kind.type_label());
                        let copy_btn = theme::action_button(
                            &copy_label,
                            theme::ACCENT_ORANGE,
                            egui::Vec2::new(150.0, 26.0),
                        );
                        if ui.add(copy_btn).clicked() {
                            let max = kind.max_channels(input_count, aux_count, group_count, matrix_count, cg_count);
                            if let Some(source_safe) = config.channel_safes.get(ch_id).cloned() {
                                for i in 1..=max {
                                    if let Some(target) = kind.channel_id(i) {
                                        config.channel_safes.insert(target, source_safe.clone());
                                    }
                                }
                            }
                        }
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let close_btn = theme::action_button("Close", theme::ACCENT_GREEN, egui::Vec2::new(70.0, 26.0));
                    if ui.add(close_btn).clicked() {
                        state.open = None;
                    }
                });
            });
        });

    if !open {
        state.open = None;
    }
}

/// Draw a single toggle block button (compact).
fn draw_block_button(
    ui: &mut egui::Ui,
    block: RecallBlock,
    active: bool,
    available: bool,
    is_scope: bool,
    mut on_toggle: impl FnMut(bool),
) {
    // Single-line label (replace newlines with spaces)
    let label_text = block.label().replace('\n', " ");

    let fill = if !available {
        theme::BG_PANEL
    } else if active {
        if is_scope {
            egui::Color32::from_rgb(0, 120, 0)
        } else {
            egui::Color32::from_rgb(180, 30, 30)
        }
    } else {
        theme::BG_ELEVATED
    };

    let text_color = if !available {
        theme::TEXT_DISABLED
    } else if active {
        egui::Color32::WHITE
    } else {
        theme::TEXT_SECONDARY
    };

    let btn = egui::Button::new(
        egui::RichText::new(label_text).color(text_color).size(10.0),
    )
    .fill(fill)
    .corner_radius(4.0)
    .min_size(egui::Vec2::new(0.0, 26.0));

    let resp = ui.add_enabled(available, btn);
    if resp.clicked() {
        on_toggle(!active);
    }
}
