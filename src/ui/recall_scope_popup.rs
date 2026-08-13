//! Popup windows for editing the console's recall scope (global) and
//! per-channel recall safe settings. These are visual references only —
//! the app cannot read or write these settings on the console via OSC.

use eframe::egui;

use super::help::{HelpKey, help};
use super::theme;
use crate::model::channel::ChannelId;
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
use crate::model::recall_scope::{ConsoleRecallConfig, RecallBlock};
use crate::model::state::ConsoleState;

/// UI state for the recall scope/safe popups.
#[derive(Default)]
pub struct RecallScopePopupState {
    /// Which popup is currently open.
    pub open: Option<RecallPopupKind>,
    /// For per-channel safe: which channel index is selected (1-based).
    pub selected_channel: u16,
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

    pub fn channel_id(&self, index: u16) -> Option<ChannelId> {
        match self {
            Self::SessionScope => None,
            Self::InputSafe => Some(ChannelId::Input(index)),
            Self::AuxSafe => Some(ChannelId::Aux(index)),
            Self::GroupSafe => Some(ChannelId::Group(index)),
            Self::MatrixSafe => Some(ChannelId::Matrix(index)),
            Self::CgSafe => Some(ChannelId::ControlGroup(index)),
        }
    }

    pub fn max_channels(&self, ic: u16, ac: u16, gc: u16, mc: u16, cc: u16) -> u16 {
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
    let Some(kind) = state.open.clone() else {
        return;
    };

    // Defence in depth: the buttons that open this popup are already hidden on
    // families without the S-series recall-scope screen, but a stale `open`
    // flag (e.g. a show loaded while the popup was up) must not render it.
    if !console_state
        .config
        .profile()
        .supports(crate::model::family::AppFeature::RecallScopeUi)
    {
        state.open = None;
        return;
    }

    let input_count = console_state.config.input_channel_count;
    let aux_count = console_state.config.aux_output_count;
    let group_count = console_state.config.group_output_count;
    let matrix_count = console_state.config.matrix_output_count;
    let cg_count = console_state.config.control_group_count;

    let title = kind.label();
    let mut open = true;

    egui::Window::new(title)
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([850.0, 500.0])
        .show(ctx, |ui| {
            // Warning banner
            ui.label(
                egui::RichText::new("Visual reference only — not sent to the console")
                    .color(theme::TEXT_WARNING)
                    .small()
                    .italics(),
            );
            ui.add_space(4.0);

            // Channel selector: ◀ N ▶ / max  + channel name
            if !kind.is_scope() {
                let max =
                    kind.max_channels(input_count, aux_count, group_count, matrix_count, cg_count);
                if max > 0 {
                    ui.horizontal(|ui| {
                        ui.label("Channel:");
                        if ui
                            .add_enabled(
                                state.selected_channel > 1,
                                egui::Button::new("◀").min_size(egui::Vec2::new(28.0, 22.0)),
                            )
                            .on_hover_text(help(HelpKey::RecallScopePrevChannel))
                            .clicked()
                        {
                            state.selected_channel =
                                state.selected_channel.saturating_sub(1).max(1);
                        }
                        ui.label(
                            egui::RichText::new(format!("{}", state.selected_channel))
                                .strong()
                                .size(theme::FONT_SIZE_SMALL),
                        );
                        if ui
                            .add_enabled(
                                state.selected_channel < max,
                                egui::Button::new("▶").min_size(egui::Vec2::new(28.0, 22.0)),
                            )
                            .on_hover_text(help(HelpKey::RecallScopeNextChannel))
                            .clicked()
                        {
                            state.selected_channel = (state.selected_channel + 1).min(max);
                        }
                        ui.label(
                            egui::RichText::new(format!("/ {max}"))
                                .color(theme::label_weak())
                                .small(),
                        );
                        // Channel name
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
                                        .color(theme::label_color())
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

            // Signal-flow columns (horizontal layout matching the console)
            ui.horizontal(|ui| {
                // Sources
                draw_column(
                    ui,
                    "Sources",
                    RecallBlock::sources_column(),
                    &ref_channel,
                    is_scope,
                    &channel_id,
                    config,
                );
                ui.add_space(4.0);
                ui.label(egui::RichText::new("►").color(theme::label_weak()));
                ui.add_space(4.0);

                // Input Processing
                draw_column(
                    ui,
                    "Input\nProcessing",
                    RecallBlock::input_processing_column(),
                    &ref_channel,
                    is_scope,
                    &channel_id,
                    config,
                );
                ui.add_space(4.0);
                ui.label(egui::RichText::new("►").color(theme::label_weak()));
                ui.add_space(4.0);

                // Insert A
                draw_column(
                    ui,
                    "Insert\n(A)",
                    RecallBlock::insert_a_column(),
                    &ref_channel,
                    is_scope,
                    &channel_id,
                    config,
                );
                ui.add_space(4.0);
                ui.label(egui::RichText::new("►").color(theme::label_weak()));
                ui.add_space(4.0);

                // Channel Processing
                draw_column(
                    ui,
                    "Channel\nProcessing",
                    RecallBlock::channel_processing_column(),
                    &ref_channel,
                    is_scope,
                    &channel_id,
                    config,
                );
                ui.add_space(4.0);
                ui.label(egui::RichText::new("►").color(theme::label_weak()));
                ui.add_space(4.0);

                // Insert B
                draw_column(
                    ui,
                    "Insert\n(B)",
                    RecallBlock::insert_b_column(),
                    &ref_channel,
                    is_scope,
                    &channel_id,
                    config,
                );
                ui.add_space(4.0);
                ui.label(egui::RichText::new("►").color(theme::label_weak()));
                ui.add_space(4.0);

                // Outputs
                draw_column(
                    ui,
                    "Outputs",
                    RecallBlock::outputs_column(),
                    &ref_channel,
                    is_scope,
                    &channel_id,
                    config,
                );
            });

            ui.add_space(12.0);
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
                            let max = kind.max_channels(
                                input_count,
                                aux_count,
                                group_count,
                                matrix_count,
                                cg_count,
                            );
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
                    let close_btn = theme::action_button(
                        "Close",
                        theme::ACCENT_GREEN,
                        egui::Vec2::new(70.0, 26.0),
                    );
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
                .color(theme::label_weak())
                .size(theme::FONT_SIZE_TINY),
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
                    let safe = config.channel_safes.entry(ch.clone()).or_default();
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
    let size = egui::Vec2::new(95.0, 40.0);

    // Toggle block: kept theme-independent so it looks identical in both
    // themes (matching the scope `toggle_block` helper). Off/unavailable
    // states stay dark; active stays the green/red accent.
    let fill = if !available {
        theme::SCOPE_UNAVAILABLE
    } else if active {
        if is_scope {
            egui::Color32::from_rgb(0, 120, 0)
        } else {
            egui::Color32::from_rgb(180, 30, 30)
        }
    } else {
        theme::btn_neutral()
    };

    let text_color = if !available {
        theme::TEXT_DISABLED
    } else if active {
        egui::Color32::WHITE
    } else {
        theme::neutral_inactive_text()
    };

    // Manual paint to avoid egui Button adding checkbox-like decorations
    let (rect, resp) = ui.allocate_exact_size(
        size,
        if available {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let bg = if available && resp.hovered() {
        theme::lighten(fill, 20)
    } else {
        fill
    };
    ui.painter().rect_filled(rect, 4.0, bg);
    ui.painter().rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, theme::border_subtle()),
        egui::StrokeKind::Outside,
    );

    let galley = ui.painter().layout(
        block.label().to_string(),
        egui::FontId::proportional(11.0),
        text_color,
        rect.width() - 6.0,
    );
    let text_pos = egui::pos2(
        rect.center().x - galley.size().x / 2.0,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(text_pos, galley, text_color);

    if resp.clicked() && available {
        on_toggle(!active);
    }
}
