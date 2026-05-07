//! Pan Link tab — bind input channel main pan to stereo aux send pans.
//!
//! Layout: channel selector at the top, existing-bindings strip below it,
//! then a grid of aux tiles for the selected input. Mono auxes are greyed
//! out; stereo auxes are blue (not sending) or green (sending), with a
//! brighter tint when the link is active for that input→aux node.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;

use super::theme;
use crate::model::channel::ChannelId;
use crate::model::config::ChannelMode;
use crate::model::pan_link::PanLinkBindings;
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::encode;

/// Per-frame state for the Pan Link tab.
pub struct PanLinkTabState {
    /// 1-based input channel currently in focus.
    pub selected_input: u8,
}

impl Default for PanLinkTabState {
    fn default() -> Self {
        Self { selected_input: 1 }
    }
}

pub fn draw_pan_link_tab(
    ui: &mut egui::Ui,
    tab: &mut PanLinkTabState,
    bindings: &Arc<RwLock<PanLinkBindings>>,
    console_state: &Arc<RwLock<ConsoleState>>,
    sender: &Option<OscSender>,
    _connected: &Arc<AtomicBool>,
    runtime: &tokio::runtime::Handle,
) {
    // Snapshot live state for this frame. try_read so we don't deadlock
    // if a write is in flight; the next frame will redraw.
    let Ok(state_guard) = console_state.try_read() else {
        ui.label("Loading console state…");
        return;
    };
    let input_count = state_guard.config.input_channel_count;
    let total_buses = state_guard.config.total_bus_count();
    let mix_types = state_guard.config.mix_output_types.clone();
    let mix_modes = state_guard.config.mix_output_modes.clone();

    if tab.selected_input == 0 {
        tab.selected_input = 1;
    }
    if input_count > 0 && tab.selected_input > input_count {
        tab.selected_input = input_count;
    }

    let bindings_snapshot: PanLinkBindings = match bindings.try_read() {
        Ok(b) => b.clone(),
        Err(_) => {
            ui.label("Loading pan link bindings…");
            return;
        }
    };

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // ── Channel selector + name + linked-inputs chips ──
            // The selector card folds in the previously-separate "Existing
            // bindings" navigation: a compact chip row right under the
            // selector lists inputs that have pan links, clickable to jump
            // to them. No more standalone card duplicating the navigation.
            theme::card_frame().show(ui, |ui| {
                theme::section_heading(ui, "Pan Link");
                ui.horizontal(|ui| {
                    ui.label("Input channel:");
                    if ui
                        .add_enabled(
                            tab.selected_input > 1,
                            egui::Button::new("◀").min_size(egui::Vec2::new(28.0, 22.0)),
                        )
                        .clicked()
                    {
                        tab.selected_input = tab.selected_input.saturating_sub(1).max(1);
                    }

                    // DragValue allows both dragging and click-to-type for the
                    // channel number — replaces the previous read-only label.
                    let max_input = input_count.max(1) as u32;
                    let mut input_val = tab.selected_input as u32;
                    let drag = ui.add(
                        egui::DragValue::new(&mut input_val)
                            .range(1..=max_input)
                            .speed(0.2),
                    );
                    if drag.changed() {
                        tab.selected_input = (input_val as u8).clamp(1, input_count.max(1));
                    }

                    if ui
                        .add_enabled(
                            tab.selected_input < input_count,
                            egui::Button::new("▶").min_size(egui::Vec2::new(28.0, 22.0)),
                        )
                        .clicked()
                    {
                        tab.selected_input = (tab.selected_input + 1).min(input_count);
                    }
                    ui.label(
                        egui::RichText::new(format!("/ {input_count}"))
                            .color(theme::TEXT_SECONDARY)
                            .small(),
                    );

                    // Channel name from live state — always rendered, with a
                    // dimmed "(unnamed)" placeholder when the console hasn't
                    // assigned a name. Helps confirm the right channel is
                    // selected without the user having to scrub through.
                    ui.add_space(10.0);
                    let name = state_guard
                        .get(&ParameterAddress {
                            channel: ChannelId::Input(tab.selected_input),
                            parameter: ParameterPath::Name,
                        })
                        .and_then(|v| match v {
                            ParameterValue::String(s) => Some(s.as_str()),
                            _ => None,
                        })
                        .unwrap_or("");
                    if name.is_empty() {
                        ui.label(
                            egui::RichText::new("(unnamed)")
                                .color(theme::TEXT_DISABLED)
                                .italics(),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new(format!("\u{201C}{}\u{201D}", name))
                                .color(theme::TEXT_PRIMARY)
                                .strong(),
                        );
                    }
                });

                // Compact chip strip — only renders when there's at least one
                // pan link to point to. Each chip jumps the selector to that
                // input. Replaces the standalone "Existing bindings" card.
                let linked_inputs = bindings_snapshot.linked_inputs();
                if !linked_inputs.is_empty() {
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new("Has pan links:")
                                .color(theme::TEXT_SECONDARY)
                                .small(),
                        );
                        for input in linked_inputs {
                            let aux_count = bindings_snapshot.auxes_for(input).len();
                            let label = format!("Ch {input} ({aux_count})");
                            let is_selected = input == tab.selected_input;
                            let fill = if is_selected {
                                theme::ACCENT_BLUE
                            } else {
                                theme::BG_ELEVATED
                            };
                            let btn = egui::Button::new(
                                egui::RichText::new(label)
                                    .color(theme::TEXT_PRIMARY)
                                    .small(),
                            )
                            .fill(fill)
                            .corner_radius(4.0);
                            if ui.add(btn).clicked() {
                                tab.selected_input = input;
                            }
                        }
                    });
                }
            });

            ui.add_space(8.0);

            // ── Aux tile grid ──
            theme::card_frame().show(ui, |ui| {
                theme::section_heading(ui, &format!("Aux sends for input {}", tab.selected_input));
                ui.label(
                    egui::RichText::new(
                        "Mono auxes are disabled. Blue = not sending. Green = sending. \
                     Brighter = pan link active.",
                    )
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );
                ui.add_space(6.0);

                // Iterate the unified bus space; show only buses configured as auxes.
                let mut clicked_bus: Option<u8> = None;
                ui.horizontal_wrapped(|ui| {
                    for bus in 1..=total_buses {
                        let idx0 = (bus - 1) as usize;
                        let is_aux = mix_types.get(idx0).copied().unwrap_or(true);
                        if !is_aux {
                            continue;
                        }
                        let is_stereo = matches!(mix_modes.get(idx0), Some(ChannelMode::Stereo));
                        let label = state_guard.config.bus_label(bus);

                        // Sending? Read live SendEnabled state for this input → bus.
                        let sending = match state_guard.get(&ParameterAddress {
                            channel: ChannelId::Input(tab.selected_input),
                            parameter: ParameterPath::SendEnabled(bus),
                        }) {
                            Some(ParameterValue::Bool(b)) => *b,
                            Some(ParameterValue::Int(i)) => *i != 0,
                            Some(ParameterValue::Float(f)) => f.abs() > f32::EPSILON,
                            _ => false,
                        };
                        let active = bindings_snapshot.is_active(tab.selected_input, bus);

                        if draw_aux_tile(ui, &label, is_stereo, sending, active) {
                            clicked_bus = Some(bus);
                        }
                    }
                });

                // Apply click outside the borrow of `mix_modes` etc.
                if let Some(bus) = clicked_bus {
                    let was_active = bindings_snapshot.is_active(tab.selected_input, bus);
                    let new_active = !was_active;
                    let bindings_arc = bindings.clone();
                    let input = tab.selected_input;
                    runtime.spawn(async move {
                        let mut b = bindings_arc.write().await;
                        b.set_active(input, bus, new_active);
                    });

                    // On enabling: push the input's current main pan to that aux's
                    // send pan immediately so the linked aux snaps to the live pan
                    // (matches the absolute semantics).
                    if new_active {
                        let current_pan = state_guard
                            .get(&ParameterAddress {
                                channel: ChannelId::Input(tab.selected_input),
                                parameter: ParameterPath::Pan,
                            })
                            .cloned();
                        if let (Some(pan), Some(snd)) = (current_pan, sender.clone()) {
                            let target = ParameterAddress {
                                channel: ChannelId::Input(tab.selected_input),
                                parameter: ParameterPath::SendPan(bus),
                            };
                            if let Some((path, args)) = encode::encode_parameter(&target, &pan) {
                                runtime.spawn(async move {
                                    let _ = snd.send(&path, args).await;
                                });
                            }
                        }
                    }
                }
            });
        });

    // Suppress unused-variable warning when sender is None and no tile clicked.
    let _ = sender;
    let _ = _connected.load(Ordering::Relaxed);
    drop(state_guard);
}

/// Draw a single aux tile and return true if it was clicked (and is enabled).
fn draw_aux_tile(
    ui: &mut egui::Ui,
    label: &str,
    is_stereo: bool,
    sending: bool,
    active: bool,
) -> bool {
    let size = egui::Vec2::new(110.0, 50.0);

    // Mono = disabled grey. Stereo: blue (not sending) / green (sending),
    // dark when link inactive, bright when link active.
    let fill = if !is_stereo {
        theme::BG_PANEL
    } else if sending {
        if active {
            egui::Color32::from_rgb(0x00, 0xC0, 0x40)
        } else {
            egui::Color32::from_rgb(0x00, 0x55, 0x1E)
        }
    } else if active {
        egui::Color32::from_rgb(0x2D, 0x8B, 0xC9)
    } else {
        egui::Color32::from_rgb(0x14, 0x3D, 0x5A)
    };

    let text_color = if !is_stereo {
        theme::TEXT_DISABLED
    } else {
        egui::Color32::WHITE
    };

    let sense = if is_stereo {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let bg = if is_stereo && resp.hovered() {
        theme::lighten(fill, 20)
    } else {
        fill
    };
    ui.painter().rect_filled(rect, 4.0, bg);
    ui.painter().rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, theme::BORDER_SUBTLE),
        egui::StrokeKind::Outside,
    );

    // Top line: aux label. Bottom line: link state.
    let title = ui.painter().layout(
        label.to_string(),
        egui::FontId::proportional(13.0),
        text_color,
        rect.width() - 8.0,
    );
    let title_pos = egui::pos2(rect.center().x - title.size().x / 2.0, rect.top() + 6.0);
    ui.painter().galley(title_pos, title, text_color);

    let sub = if !is_stereo {
        "mono"
    } else if active {
        "linked"
    } else {
        "—"
    };
    let sub_galley = ui.painter().layout(
        sub.to_string(),
        egui::FontId::proportional(11.0),
        text_color,
        rect.width() - 8.0,
    );
    let sub_pos = egui::pos2(
        rect.center().x - sub_galley.size().x / 2.0,
        rect.bottom() - 18.0,
    );
    ui.painter().galley(sub_pos, sub_galley, text_color);

    ui.add_space(4.0);
    is_stereo && resp.clicked()
}
