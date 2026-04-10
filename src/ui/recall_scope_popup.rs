//! Popup windows for editing the console's recall scope (global) and
//! per-channel recall safe settings. These are visual references only —
//! the app cannot read or write these settings on the console via OSC.

use eframe::egui;

use crate::model::channel::ChannelId;
use crate::model::recall_scope::{ConsoleRecallConfig, RecallBlock};
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

    /// Build a ChannelId for the selected channel index.
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

    /// Max channel count for this type.
    pub fn max_channels(&self, input_count: u8, aux_count: u8, group_count: u8, matrix_count: u8, cg_count: u8) -> u8 {
        match self {
            Self::SessionScope => 0,
            Self::InputSafe => input_count,
            Self::AuxSafe => aux_count,
            Self::GroupSafe => group_count,
            Self::MatrixSafe => matrix_count,
            Self::CgSafe => cg_count,
        }
    }

    /// Reference ChannelId for block availability checks.
    pub fn reference_channel(&self) -> ChannelId {
        match self {
            Self::SessionScope | Self::InputSafe => ChannelId::Input(1),
            Self::AuxSafe => ChannelId::Aux(1),
            Self::GroupSafe => ChannelId::Group(1),
            Self::MatrixSafe => ChannelId::Matrix(1),
            Self::CgSafe => ChannelId::ControlGroup(1),
        }
    }
}

/// Draw the recall scope/safe popup if open.
pub fn draw_recall_popup(
    ctx: &egui::Context,
    state: &mut RecallScopePopupState,
    config: &mut ConsoleRecallConfig,
    input_count: u8,
    aux_count: u8,
    group_count: u8,
    matrix_count: u8,
    cg_count: u8,
) {
    let Some(kind) = state.open.clone() else { return };

    let title = kind.label();
    let mut open = true;

    egui::Window::new(title)
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([700.0, 450.0])
        .show(ctx, |ui| {
            // Warning banner
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("ℹ Visual reference only — not sent to the console")
                        .color(theme::TEXT_WARNING)
                        .small(),
                );
            });
            ui.add_space(4.0);

            // Channel selector (for safe popups)
            if !kind.is_scope() {
                let max = kind.max_channels(input_count, aux_count, group_count, matrix_count, cg_count);
                if max > 0 {
                    ui.horizontal(|ui| {
                        ui.label("Channel:");
                        for ch in 1..=max.min(48) {
                            let selected = state.selected_channel == ch;
                            let btn = egui::Button::new(
                                egui::RichText::new(format!("{ch}")).small(),
                            )
                            .selected(selected)
                            .min_size(egui::Vec2::new(24.0, 22.0));
                            if ui.add(btn).clicked() {
                                state.selected_channel = ch;
                            }
                        }
                    });
                    ui.add_space(4.0);
                }
            }

            // Get the active set to edit
            let is_scope = kind.is_scope();
            let channel_id = kind.channel_id(state.selected_channel);
            let ref_channel = kind.reference_channel();

            // Global blocks row (Session Scope only)
            if is_scope {
                ui.horizontal(|ui| {
                    for &block in RecallBlock::global_blocks() {
                        let active = config.session_scope.active_blocks.contains(&block);
                        draw_block_button(ui, block, active, true, is_scope, |toggled| {
                            if toggled {
                                config.session_scope.active_blocks.insert(block);
                            } else {
                                config.session_scope.active_blocks.remove(&block);
                            }
                        });
                        ui.add_space(4.0);
                    }
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);
            }

            // Signal-flow columns
            ui.horizontal(|ui| {
                // Sources
                draw_column(ui, "Sources", RecallBlock::sources_column(), &ref_channel, is_scope, &channel_id, config);
                ui.add_space(4.0);
                ui.label(egui::RichText::new("►").color(theme::TEXT_SECONDARY));
                ui.add_space(4.0);

                // Input Processing
                draw_column(ui, "Input\nProcessing", RecallBlock::input_processing_column(), &ref_channel, is_scope, &channel_id, config);
                ui.add_space(4.0);
                ui.label(egui::RichText::new("►").color(theme::TEXT_SECONDARY));
                ui.add_space(4.0);

                // Insert A
                draw_column(ui, "Insert\n(A)", RecallBlock::insert_a_column(), &ref_channel, is_scope, &channel_id, config);
                ui.add_space(4.0);
                ui.label(egui::RichText::new("►").color(theme::TEXT_SECONDARY));
                ui.add_space(4.0);

                // Channel Processing
                draw_column(ui, "Channel\nProcessing", RecallBlock::channel_processing_column(), &ref_channel, is_scope, &channel_id, config);
                ui.add_space(4.0);
                ui.label(egui::RichText::new("►").color(theme::TEXT_SECONDARY));
                ui.add_space(4.0);

                // Insert B
                draw_column(ui, "Insert\n(B)", RecallBlock::insert_b_column(), &ref_channel, is_scope, &channel_id, config);
                ui.add_space(4.0);
                ui.label(egui::RichText::new("►").color(theme::TEXT_SECONDARY));
                ui.add_space(4.0);

                // Outputs
                draw_column(ui, "Outputs", RecallBlock::outputs_column(), &ref_channel, is_scope, &channel_id, config);
            });

            ui.add_space(12.0);

            // Copy to all + Apply buttons
            ui.horizontal(|ui| {
                if !is_scope {
                    if let Some(ref ch_id) = channel_id {
                        let type_label = match &kind {
                            RecallPopupKind::InputSafe => "Inputs",
                            RecallPopupKind::AuxSafe => "Auxes",
                            RecallPopupKind::GroupSafe => "Groups",
                            RecallPopupKind::MatrixSafe => "Matrices",
                            RecallPopupKind::CgSafe => "CGs",
                            _ => "",
                        };
                        let copy_label = format!("Copy to all {type_label}");
                        let copy_btn = theme::action_button(
                            &copy_label,
                            theme::ACCENT_ORANGE,
                            egui::Vec2::new(160.0, 28.0),
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
                    let close_btn = theme::action_button("Close", theme::ACCENT_GREEN, egui::Vec2::new(80.0, 28.0));
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

/// Draw a column of blocks with a header.
fn draw_column(
    ui: &mut egui::Ui,
    header: &str,
    blocks: &[RecallBlock],
    ref_channel: &ChannelId,
    is_scope: bool,
    channel_id: &Option<ChannelId>,
    config: &mut ConsoleRecallConfig,
) {
    ui.vertical(|ui| {
        ui.label(
            egui::RichText::new(header)
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        ui.add_space(4.0);

        for &block in blocks {
            let available = RecallBlock::available_for_channel(block, ref_channel);

            let active = if is_scope {
                config.session_scope.active_blocks.contains(&block)
            } else if let Some(ch) = channel_id {
                config
                    .channel_safes
                    .get(ch)
                    .is_some_and(|s| s.safe_blocks.contains(&block))
            } else {
                false
            };

            draw_block_button(ui, block, active, available, is_scope, |toggled| {
                if is_scope {
                    if toggled {
                        config.session_scope.active_blocks.insert(block);
                    } else {
                        config.session_scope.active_blocks.remove(&block);
                    }
                } else if let Some(ch) = channel_id {
                    let safe = config
                        .channel_safes
                        .entry(ch.clone())
                        .or_default();
                    if toggled {
                        safe.safe_blocks.insert(block);
                    } else {
                        safe.safe_blocks.remove(&block);
                    }
                }
            });
            ui.add_space(2.0);
        }
    });
}

/// Draw a single toggle block button.
fn draw_block_button(
    ui: &mut egui::Ui,
    block: RecallBlock,
    active: bool,
    available: bool,
    is_scope: bool,
    mut on_toggle: impl FnMut(bool),
) {
    let size = egui::Vec2::new(80.0, 36.0);

    let fill = if !available {
        theme::BG_PANEL
    } else if active {
        if is_scope {
            egui::Color32::from_rgb(0, 120, 0) // green for scope
        } else {
            egui::Color32::from_rgb(180, 30, 30) // red for safe
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
        egui::RichText::new(block.label())
            .color(text_color)
            .size(9.0),
    )
    .fill(fill)
    .corner_radius(4.0)
    .min_size(size);

    let resp = ui.add_enabled(available, btn);
    if resp.clicked() {
        on_toggle(!active);
    }
}
