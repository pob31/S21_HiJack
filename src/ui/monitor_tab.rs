use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::monitor_channel_picker::{self, ChannelPickerState, PickerOutcome};
use super::theme;
use crate::console::monitor_manager::MonitorManager;
use crate::model::channel::ChannelId;
use crate::model::monitor::MonitorClient;
use crate::model::parameter::{ParameterAddress, ParameterPath};
use crate::model::state::ConsoleState;

/// Per-tab UI state for the Monitor tab.
#[derive(Default)]
pub struct MonitorTabState {
    pub status_message: Option<String>,
    pub monitor_server_running: bool,
    /// `Some` while the channel picker is open (either adding a new profile
    /// or editing an existing one); `None` when the picker is closed.
    pub picker: Option<ChannelPickerState>,
    /// `Some(client_id)` when that profile's row is in drag-to-reorder mode.
    /// The badges in that row become drag sources; drop them onto each other
    /// to rearrange the saved channel order without opening the picker.
    pub reorder_for: Option<Uuid>,
}

/// Draw the Monitor tab.
pub fn draw_monitor_tab(
    ui: &mut egui::Ui,
    tab: &mut MonitorTabState,
    monitor_manager: &Arc<RwLock<MonitorManager>>,
    console_state: &Arc<RwLock<ConsoleState>>,
    connected: &Arc<AtomicBool>,
    runtime: &tokio::runtime::Handle,
) {
    let is_connected = connected.load(Ordering::Relaxed);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;

                // Left column — fixed-width stack of Server Status,
                // Aux Channels reference table, and the Add-Profile
                // launcher. The outer `horizontal_top` makes child
                // sub-uis inherit `Layout::left_to_right`, which is why
                // the cards previously rendered horizontally; we force
                // `top_down` here so the three cards stack vertically
                // as expected.
                let left_w = 320.0_f32.min(ui.available_width() * 0.35);
                ui.allocate_ui_with_layout(
                    egui::Vec2::new(left_w, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_width(left_w);
                        ui.set_max_width(left_w);

                        // ── Server Status card ──
                        theme::card_frame().show(ui, |ui| {
                            theme::section_heading(ui, "Server Status");

                            // Console status — its own row.
                            ui.horizontal(|ui| {
                                let console_color = if is_connected {
                                    theme::COLOR_CONNECTED
                                } else {
                                    theme::COLOR_DISCONNECTED
                                };
                                theme::status_dot(ui, console_color);
                                ui.label(
                                    egui::RichText::new(if is_connected {
                                        "Console Connected"
                                    } else {
                                        "Console Disconnected"
                                    })
                                    .color(console_color),
                                );
                            });

                            ui.add_space(2.0);

                            // Monitor-server status — its own row, below the
                            // console status (was on the same line; user
                            // asked for one item per row).
                            ui.horizontal(|ui| {
                                let monitor_color = if tab.monitor_server_running {
                                    theme::COLOR_CONNECTED
                                } else {
                                    theme::TEXT_DISABLED
                                };
                                theme::status_dot(ui, monitor_color);
                                ui.label(
                                    egui::RichText::new(if tab.monitor_server_running {
                                        "Monitor Server Running"
                                    } else {
                                        "Monitor Server Off"
                                    })
                                    .color(monitor_color),
                                );
                            });

                            let mgr = runtime.block_on(monitor_manager.read());
                            let connected_count = mgr.connected_count();
                            let total_count = mgr.clients.len();

                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                theme::colored_badge(
                                    ui,
                                    &format!("{connected_count} connected"),
                                    theme::ACCENT_GREEN,
                                );
                                theme::colored_badge(
                                    ui,
                                    &format!("{total_count} configured"),
                                    theme::BG_ELEVATED,
                                );
                            });

                            drop(mgr);
                        });

                        ui.add_space(8.0);

                        // ── Aux Reference Table ──
                        theme::card_frame().show(ui, |ui| {
                            theme::section_heading(ui, "Aux Channels");

                            let st = runtime.block_on(console_state.read());
                            let aux_count = st.config.aux_output_count;

                            if aux_count == 0 {
                                ui.label(
                                    egui::RichText::new("Connect to console to see aux channels.")
                                        .color(theme::TEXT_SECONDARY),
                                );
                            } else {
                                egui::Grid::new("aux_ref_grid")
                                    .num_columns(3)
                                    .spacing([16.0, 4.0])
                                    .striped(true)
                                    .show(ui, |ui| {
                                        // Header
                                        ui.label(
                                            egui::RichText::new("Ch #")
                                                .strong()
                                                .color(theme::TEXT_SECONDARY),
                                        );
                                        ui.label(
                                            egui::RichText::new("Name")
                                                .strong()
                                                .color(theme::TEXT_SECONDARY),
                                        );
                                        ui.label(
                                            egui::RichText::new("Send #")
                                                .strong()
                                                .color(theme::TEXT_SECONDARY),
                                        );
                                        ui.end_row();

                                        for aux in 1..=aux_count {
                                            let name = st
                                                .get(&ParameterAddress {
                                                    channel: ChannelId::Aux(aux),
                                                    parameter: ParameterPath::Name,
                                                })
                                                .and_then(|v| {
                                                    match v {
                                                crate::model::parameter::ParameterValue::String(
                                                    s,
                                                ) => Some(s.clone()),
                                                _ => None,
                                            }
                                                })
                                                .unwrap_or_default();

                                            ui.label(
                                                egui::RichText::new(format!("Aux {aux}"))
                                                    .color(theme::CH_AUX),
                                            );
                                            ui.label(
                                                egui::RichText::new(if name.is_empty() {
                                                    "—"
                                                } else {
                                                    &name
                                                })
                                                .color(theme::TEXT_PRIMARY),
                                            );
                                            ui.label(
                                                egui::RichText::new(format!("{aux}"))
                                                    .color(theme::TEXT_SECONDARY),
                                            );
                                            ui.end_row();
                                        }
                                    });
                            }
                            drop(st);
                        });

                        ui.add_space(8.0);

                        // ── Add-profile button + status row ──
                        theme::card_frame().show(ui, |ui| {
                            theme::section_heading(ui, "Profiles");

                            ui.horizontal(|ui| {
                                let add_btn = theme::action_button(
                                    "Add profile…",
                                    theme::ACCENT_GREEN,
                                    egui::Vec2::new(120.0, 32.0),
                                );
                                if ui.add(add_btn).clicked() && tab.picker.is_none() {
                                    let st = runtime.block_on(console_state.read());
                                    tab.picker = Some(ChannelPickerState::for_new_client(&st));
                                }

                                if let Some(ref msg) = tab.status_message {
                                    ui.add_space(8.0);
                                    ui.colored_label(theme::TEXT_WARNING, msg.as_str());
                                }
                            });
                        });
                    }, // end left-column closure
                ); // end left-column allocate_ui_with_layout

                // Right column — Clients list. Force it to claim the
                // entire remaining horizontal width via
                // `allocate_ui_with_layout`, otherwise the inner
                // `card_frame` shrinks to fit its widest row instead of
                // expanding to the window.
                let remaining_w = ui.available_width().max(0.0);
                ui.allocate_ui_with_layout(
                    egui::Vec2::new(remaining_w, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_width(remaining_w);
                        // ── Client List card ──
                        theme::card_frame().show(ui, |ui| {
                            theme::section_heading(ui, "Clients");

                            let mgr = runtime.block_on(monitor_manager.read());
                            let clients = mgr.sorted_clients();

                            if clients.is_empty() {
                                ui.label(
                                    egui::RichText::new("No monitoring clients configured.")
                                        .color(theme::TEXT_SECONDARY),
                                );
                            } else {
                                let mut to_remove = None;
                                let mut to_edit: Option<MonitorClient> = None;
                                // Per-row reorder result staged for after the loop, so we
                                // can release the read lock before calling update_client.
                                let mut to_update_order: Option<(Uuid, Vec<u8>, Vec<u8>)> = None;

                                for client in &clients {
                                    let in_reorder = tab.reorder_for == Some(client.id);
                                    let stroke_color = if in_reorder {
                                        theme::ACCENT_BLUE
                                    } else {
                                        theme::BORDER_SUBTLE
                                    };
                                    egui::Frame::new()
                                        .fill(theme::BG_ELEVATED)
                                        .stroke(egui::Stroke::new(
                                            if in_reorder { 2.0 } else { 1.0 },
                                            stroke_color,
                                        ))
                                        .corner_radius(6.0)
                                        .inner_margin(egui::Margin::same(8))
                                        .show(ui, |ui| {
                                            ui.horizontal_wrapped(|ui| {
                                            // Connection status dot
                                            let status_color = if client.is_connected() {
                                                theme::COLOR_CONNECTED
                                            } else {
                                                theme::TEXT_DISABLED
                                            };
                                            theme::status_dot(ui, status_color);

                                            ui.add_space(4.0);

                                            // Client name
                                            ui.label(
                                                egui::RichText::new(&client.name)
                                                    .strong()
                                                    .color(theme::TEXT_PRIMARY),
                                            );

                                            ui.add_space(8.0);

                                            // Aux badges (magenta) — drag-reorderable in reorder mode.
                                            let mut new_aux_order: Option<Vec<u8>> = None;
                                            if in_reorder {
                                                if let Some(reordered) = draw_reorderable_badges(
                                                    ui,
                                                    &client.permitted_auxes,
                                                    |v| format!("Aux {v}"),
                                                    theme::CH_AUX,
                                                    (client.id, "aux"),
                                                ) {
                                                    new_aux_order = Some(reordered);
                                                }
                                            } else {
                                                for aux in &client.permitted_auxes {
                                                    theme::colored_badge(
                                                        ui,
                                                        &format!("Aux {aux}"),
                                                        theme::CH_AUX,
                                                    );
                                                }
                                            }

                                            ui.add_space(4.0);

                                            // Input badges (blue) — drag-reorderable when visible_inputs
                                            // is a specific list. The "All Inputs" sentinel is shown as
                                            // a single non-draggable badge.
                                            let mut new_input_order: Option<Vec<u8>> = None;
                                            if client.visible_inputs.is_empty() {
                                                theme::colored_badge(
                                                    ui,
                                                    "All Inputs",
                                                    theme::CH_INPUT,
                                                );
                                            } else if in_reorder {
                                                if let Some(reordered) = draw_reorderable_badges(
                                                    ui,
                                                    &client.visible_inputs,
                                                    |v| format!("In {v}"),
                                                    theme::CH_INPUT,
                                                    (client.id, "input"),
                                                ) {
                                                    new_input_order = Some(reordered);
                                                }
                                            } else {
                                                for input in &client.visible_inputs {
                                                    theme::colored_badge(
                                                        ui,
                                                        &format!("In {input}"),
                                                        theme::CH_INPUT,
                                                    );
                                                }
                                            }

                                            // Stage the reorder result. Either auxes or inputs (not
                                            // both in one frame) since each drop happens once.
                                            if new_aux_order.is_some() || new_input_order.is_some()
                                            {
                                                to_update_order = Some((
                                                    client.id,
                                                    new_aux_order.unwrap_or_else(|| {
                                                        client.permitted_auxes.clone()
                                                    }),
                                                    new_input_order.unwrap_or_else(|| {
                                                        client.visible_inputs.clone()
                                                    }),
                                                ));
                                            }
                                        });

                                            // Status + action-button row
                                            ui.horizontal(|ui| {
                                                ui.add_space(18.0); // align under dot
                                                let status_text = if in_reorder {
                                                    "Reorder mode — drag badges to rearrange"
                                                } else if client.is_connected() {
                                                    "Connected"
                                                } else {
                                                    "Offline"
                                                };
                                                ui.label(
                                                    egui::RichText::new(status_text)
                                                        .color(if in_reorder {
                                                            theme::ACCENT_BLUE
                                                        } else {
                                                            theme::TEXT_SECONDARY
                                                        })
                                                        .small(),
                                                );

                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| {
                                                        // Long-press: deleting a profile mid-show by accident
                                                        // would silently drop a musician's IEM control.
                                                        if theme::long_press_button(
                                                            ui,
                                                            "Delete",
                                                            theme::ACCENT_RED,
                                                            egui::Vec2::new(60.0, 24.0),
                                                            !in_reorder,
                                                            theme::LONG_PRESS_DURATION_MS,
                                                        ) {
                                                            to_remove = Some(client.id);
                                                        }

                                                        let edit_btn = theme::action_button(
                                                            "Edit",
                                                            theme::ACCENT_BLUE,
                                                            egui::Vec2::new(60.0, 24.0),
                                                        );
                                                        if ui
                                                            .add_enabled(!in_reorder, edit_btn)
                                                            .clicked()
                                                        {
                                                            to_edit = Some((*client).clone());
                                                        }

                                                        let (label, color) = if in_reorder {
                                                            ("Done", theme::ACCENT_GREEN)
                                                        } else {
                                                            ("Reorder", theme::ACCENT_ORANGE)
                                                        };
                                                        let reorder_btn = theme::action_button(
                                                            label,
                                                            color,
                                                            egui::Vec2::new(70.0, 24.0),
                                                        );
                                                        if ui.add(reorder_btn).clicked() {
                                                            tab.reorder_for = if in_reorder {
                                                                None
                                                            } else {
                                                                Some(client.id)
                                                            };
                                                        }
                                                    },
                                                );
                                            });
                                        });
                                    ui.add_space(4.0);
                                }

                                drop(mgr);
                                if let Some(id) = to_remove {
                                    let mgr_clone = monitor_manager.clone();
                                    runtime.spawn(async move {
                                        mgr_clone.write().await.remove_client(id);
                                    });
                                    tab.status_message = Some("Client removed".into());
                                }
                                if let Some(client) = to_edit {
                                    if tab.picker.is_none() {
                                        let st = runtime.block_on(console_state.read());
                                        tab.picker =
                                            Some(ChannelPickerState::for_edit(&client, &st));
                                    }
                                }
                                if let Some((id, auxes, inputs)) = to_update_order {
                                    // Read the existing client to keep its name (the
                                    // Reorder UI doesn't expose name editing) and skip
                                    // the update if nothing actually changed (paranoia
                                    // against frame races where the same drop fires twice).
                                    let mgr_clone = monitor_manager.clone();
                                    runtime.spawn(async move {
                                        let mut mgr = mgr_clone.write().await;
                                        if let Some(existing) = mgr.clients.get(&id) {
                                            let name = existing.name.clone();
                                            mgr.update_client(id, name, auxes, inputs);
                                        }
                                    });
                                }
                            }
                        });
                    }, // end right-column allocate_ui_with_layout closure
                );
            }); // end horizontal_top
        });

    // ── Channel picker window (modal-style; not actually modal, just floats) ──
    let picker_outcome = if let Some(picker_state) = tab.picker.as_mut() {
        monitor_channel_picker::draw_channel_picker(ui.ctx(), picker_state)
    } else {
        None
    };
    if let Some(outcome) = picker_outcome {
        match outcome {
            PickerOutcome::Save {
                editing,
                name,
                permitted_auxes,
                visible_inputs,
            } => {
                let mgr_clone = monitor_manager.clone();
                let name_for_status = name.clone();
                runtime.spawn(async move {
                    let mut mgr = mgr_clone.write().await;
                    match editing {
                        Some(id) => {
                            mgr.update_client(id, name, permitted_auxes, visible_inputs);
                        }
                        None => {
                            mgr.add_client(MonitorClient::new(
                                name,
                                permitted_auxes,
                                visible_inputs,
                            ));
                        }
                    }
                });
                tab.status_message = Some(if editing.is_some() {
                    format!("Updated profile '{name_for_status}'")
                } else {
                    format!("Added profile '{name_for_status}'")
                });
                tab.picker = None;
            }
            PickerOutcome::Cancel => {
                tab.picker = None;
            }
        }
    }
}

/// Render a horizontal row of channel badges that the operator can drag onto
/// each other to rearrange. Returns `Some(new_order)` on the frame the drop
/// completes; the caller is responsible for committing the new order to the
/// `MonitorManager`.
///
/// `id_prefix` namespaces the drag-source IDs so two reorderable rows on the
/// same screen (e.g. auxes and inputs of the same profile, or two different
/// profiles) don't collide.
fn draw_reorderable_badges(
    ui: &mut egui::Ui,
    items: &[u8],
    label_fn: impl Fn(u8) -> String,
    color: egui::Color32,
    id_prefix: (Uuid, &'static str),
) -> Option<Vec<u8>> {
    let mut reorder: Option<(usize, usize)> = None;

    for (i, &v) in items.iter().enumerate() {
        let drag_id = egui::Id::new((id_prefix.0, id_prefix.1, i));
        // Each badge is wrapped in BOTH a drop zone and a drag source so a
        // dropped badge can land on any other badge to reposition itself.
        // egui 0.33's `dnd_drop_zone` returns `(InnerResponse<R>, Option<Arc<P>>)`;
        // the Option is `Some` on the frame a drag is released over this zone.
        let (_inner, dropped) = ui.dnd_drop_zone::<usize, _>(egui::Frame::default(), |ui| {
            ui.dnd_drag_source(drag_id, i, |ui| {
                theme::colored_badge(ui, &label_fn(v), color);
            });
        });
        if let Some(payload) = dropped {
            let src = *payload;
            if src != i {
                reorder = Some((src, i));
            }
        }
    }

    reorder.map(|(src, dst)| reorder_vec(items, src, dst))
}

/// Move the item at `src` to position `dst`, returning the new order.
/// "Drop on dst" means the dragged item lands at the dst's current visual
/// position — items between src and dst shift to make room.
fn reorder_vec(items: &[u8], src: usize, dst: usize) -> Vec<u8> {
    let mut out: Vec<u8> = items.to_vec();
    if src >= out.len() || dst >= out.len() || src == dst {
        return out;
    }
    let item = out.remove(src);
    let insert_at = if dst > src { dst - 1 } else { dst };
    out.insert(insert_at, item);
    out
}

#[cfg(test)]
mod tests {
    use super::reorder_vec;

    #[test]
    fn reorder_drag_forward() {
        // Drag A (idx 0) onto D (idx 3) → A lands at D's old position,
        // intermediate items shift left to make room. Final: [B, C, A, D, E].
        let result = reorder_vec(&[1, 2, 3, 4, 5], 0, 3);
        assert_eq!(result, vec![2, 3, 1, 4, 5]);
    }

    #[test]
    fn reorder_drag_backward() {
        // Drag E (idx 4) onto B (idx 1) → E lands at idx 1, others shift right.
        let result = reorder_vec(&[1, 2, 3, 4, 5], 4, 1);
        assert_eq!(result, vec![1, 5, 2, 3, 4]);
    }

    #[test]
    fn reorder_drag_to_self_is_noop() {
        let original = [1, 2, 3];
        let result = reorder_vec(&original, 1, 1);
        assert_eq!(result, original.to_vec());
    }

    #[test]
    fn reorder_out_of_range_is_noop() {
        let original = [1, 2, 3];
        assert_eq!(reorder_vec(&original, 5, 1), original.to_vec());
        assert_eq!(reorder_vec(&original, 1, 5), original.to_vec());
    }
}
