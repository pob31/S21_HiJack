use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::help::{HelpHover, HelpKey, help};
use super::monitor_channel_picker::{self, ChannelPickerState, PickerOutcome};
use super::status::StatusMessage;
use super::theme;
use crate::console::monitor_manager::MonitorManager;
use crate::model::channel::ChannelId;
use crate::model::monitor::MonitorClient;
use crate::model::parameter::{ParameterAddress, ParameterPath};
use crate::model::state::{ConnectionHealth, ConsoleState};

/// Per-tab UI state for the Monitor tab.
#[derive(Default)]
pub struct MonitorTabState {
    pub status_message: Option<StatusMessage>,
    pub monitor_server_running: bool,
    /// Whether the web (HTTP/WebSocket) monitor server is currently running.
    pub web_server_running: bool,
    /// `Some` while the channel picker is open (either adding a new profile
    /// or editing an existing one); `None` when the picker is closed.
    pub picker: Option<ChannelPickerState>,
    /// `Some(client_id)` when that profile's row is in drag-to-reorder mode.
    /// The badges in that row become drag sources; drop them onto each other
    /// to rearrange the saved channel order without opening the picker.
    pub reorder_for: Option<Uuid>,
    /// Profile whose web-login QR is currently displayed (`None` = none). Only
    /// one QR shows at a time, by design — avoids a wall of codes.
    pub qr_for: Option<Uuid>,
    /// Cached QR texture for `qr_for`; regenerated when the shown profile
    /// changes or a profile is edited (the picker-save handler clears it).
    pub qr_tex: Option<(Uuid, egui::TextureHandle)>,
    /// Last-good console health, shown on a frame where the heavily-written
    /// `state` lock is momentarily busy — so the Monitor tab's per-frame read
    /// never blocks the UI thread (which would freeze during a recall flood).
    pub last_health: ConnectionHealth,
    /// Last-good aux reference rows `(aux_number, name)`. Same rationale: the
    /// table renders from this snapshot, refreshed only when the lock is free.
    pub aux_ref_cache: Vec<(u8, String)>,
}

/// Draw the Monitor tab.
pub fn draw_monitor_tab(
    ui: &mut egui::Ui,
    tab: &mut MonitorTabState,
    monitor_manager: &Arc<RwLock<MonitorManager>>,
    console_state: &Arc<RwLock<ConsoleState>>,
    connected: &Arc<AtomicBool>,
    runtime: &tokio::runtime::Handle,
    web_port: u16,
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

                            // Console status — its own row. Reflects link
                            // *health* (ping/pong), not just whether a session
                            // was started, so a failed heartbeat (Stale/Lost)
                            // no longer reads as connected.
                            ui.horizontal(|ui| {
                                // Non-blocking: never freeze the UI on the
                                // (heavily-written) state lock during a recall
                                // flood; reuse the last-good health on a miss.
                                let health = match console_state.try_read() {
                                    Ok(s) => {
                                        tab.last_health = s.health;
                                        s.health
                                    }
                                    Err(_) => tab.last_health,
                                };
                                let (console_color, base) =
                                    theme::console_status(is_connected, health);
                                theme::status_dot(ui, console_color);
                                ui.label(
                                    egui::RichText::new(format!("Console {base}"))
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
                                    theme::btn_neutral(),
                                );
                            });

                            // ── Web monitor: status, LAN URLs, and the
                            //    selected profile's QR (one at a time). ──
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                let web_color = if tab.web_server_running {
                                    theme::COLOR_CONNECTED
                                } else {
                                    theme::TEXT_DISABLED
                                };
                                theme::status_dot(ui, web_color);
                                ui.label(
                                    egui::RichText::new(if tab.web_server_running {
                                        "Web Server Running"
                                    } else {
                                        "Web Server Off"
                                    })
                                    .color(web_color),
                                );
                            });

                            // URLs + QR are shown whenever the web port is
                            // configured — not gated on the server running, so
                            // the operator can prepare QR codes offline / before
                            // connecting. The deep-link only depends on the NIC
                            // IP + port + profile, all known without a console.
                            if web_port > 0 {
                                let ifaces = crate::ui::net_interfaces::list_interfaces();
                                ui.add_space(4.0);
                                if !tab.web_server_running {
                                    ui.label(
                                        egui::RichText::new(
                                            "Starts when you Connect — you can prepare QR codes now.",
                                        )
                                        .color(theme::label_weak())
                                        .small(),
                                    )
                                    .on_hover_help_inline(HelpKey::MonitorInfoWebPrepare);
                                }
                                if ifaces.is_empty() {
                                    ui.label(
                                        egui::RichText::new(
                                            "No LAN interface detected — join a network to get a URL.",
                                        )
                                        .color(theme::label_weak())
                                        .small(),
                                    )
                                    .on_hover_help_inline(HelpKey::MonitorWarnNoLan);
                                } else {
                                    ui.label(
                                        egui::RichText::new("Open in a phone browser (LAN only):")
                                            .color(theme::label_weak())
                                            .small(),
                                    )
                                    .on_hover_help_inline(HelpKey::MonitorInfoBrowserHint);
                                    for iface in &ifaces {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "http://{}:{}",
                                                iface.ip, web_port
                                            ))
                                            .monospace(),
                                        );
                                    }
                                    // The per-profile QR opens in a centered
                                    // popup (see the "QR" button on each client
                                    // row) so it never shifts this card around.
                                }
                            }

                            drop(mgr);
                        });

                        ui.add_space(8.0);

                        // ── Aux Reference Table ──
                        theme::card_frame().show(ui, |ui| {
                            theme::section_heading(ui, "Aux Channels");

                            // Refresh the aux reference snapshot when the state
                            // lock is free; otherwise render from the cached
                            // snapshot so this table never blocks the UI thread
                            // on the heavily-written state lock.
                            if let Ok(st) = console_state.try_read() {
                                let aux_count = st.config.aux_output_count;
                                tab.aux_ref_cache = (1..=aux_count)
                                    .map(|aux| {
                                        let name = st
                                            .get(&ParameterAddress {
                                                channel: ChannelId::Aux(aux),
                                                parameter: ParameterPath::Name,
                                            })
                                            .and_then(|v| match v {
                                                crate::model::parameter::ParameterValue::String(
                                                    s,
                                                ) => Some(s.clone()),
                                                _ => None,
                                            })
                                            .unwrap_or_default();
                                        (aux, name)
                                    })
                                    .collect();
                            }

                            if tab.aux_ref_cache.is_empty() {
                                ui.label(
                                    egui::RichText::new("Connect to console to see aux channels.")
                                        .color(theme::label_weak()),
                                )
                                .on_hover_help_inline(HelpKey::MonitorInfoNoAux);
                            } else {
                                // Force the Ch # and Send # columns to
                                // fixed widths so the Name column can
                                // absorb whatever's left and push the
                                // Send # column to the card's right
                                // edge — matching the left padding.
                                const CH_W: f32 = 60.0;
                                const SEND_W: f32 = 50.0;
                                const COL_SPACING: f32 = 16.0;
                                let avail = ui.available_width();
                                let name_w = (avail - CH_W - SEND_W - 2.0 * COL_SPACING).max(40.0);
                                let row_h = 16.0;

                                egui::Grid::new("aux_ref_grid")
                                    .num_columns(3)
                                    .spacing([COL_SPACING, 4.0])
                                    .striped(true)
                                    .show(ui, |ui| {
                                        // Header
                                        ui.add_sized(
                                            [CH_W, row_h],
                                            egui::Label::new(
                                                egui::RichText::new("Ch #")
                                                    .strong()
                                                    .color(theme::label_weak()),
                                            ),
                                        );
                                        ui.add_sized(
                                            [name_w, row_h],
                                            egui::Label::new(
                                                egui::RichText::new("Name")
                                                    .strong()
                                                    .color(theme::label_weak()),
                                            ),
                                        );
                                        ui.add_sized(
                                            [SEND_W, row_h],
                                            egui::Label::new(
                                                egui::RichText::new("Send #")
                                                    .strong()
                                                    .color(theme::label_weak()),
                                            ),
                                        );
                                        ui.end_row();

                                        for (aux, name) in &tab.aux_ref_cache {
                                            let aux = *aux;
                                            ui.add_sized(
                                                [CH_W, row_h],
                                                egui::Label::new(
                                                    egui::RichText::new(format!("Aux {aux}"))
                                                        .color(theme::CH_AUX),
                                                ),
                                            );
                                            ui.add_sized(
                                                [name_w, row_h],
                                                egui::Label::new(
                                                    egui::RichText::new(if name.is_empty() {
                                                        "—"
                                                    } else {
                                                        name
                                                    })
                                                    .color(theme::label_color()),
                                                ),
                                            );
                                            ui.add_sized(
                                                [SEND_W, row_h],
                                                egui::Label::new(
                                                    egui::RichText::new(format!("{aux}"))
                                                        .color(theme::label_weak()),
                                                ),
                                            );
                                            ui.end_row();
                                        }
                                    });
                            }
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
                                if ui
                                    .add(add_btn)
                                    .on_hover_text(help(HelpKey::MonitorAddProfile))
                                    .clicked()
                                    && tab.picker.is_none()
                                {
                                    // Non-blocking: if the state lock is busy
                                    // this frame, skip opening rather than block
                                    // the UI; the operator can click again.
                                    if let Ok(st) = console_state.try_read() {
                                        tab.picker = Some(ChannelPickerState::for_new_client(&st));
                                    }
                                }

                                if let Some(ref msg) = tab.status_message {
                                    ui.add_space(8.0);
                                    let resp = ui.colored_label(theme::TEXT_WARNING, &msg.text);
                                    if let Some(key) = msg.help {
                                        resp.on_hover_help_inline(key);
                                    }
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
                                        .color(theme::label_weak()),
                                )
                                .on_hover_help_inline(HelpKey::MonitorInfoEmpty);
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
                                        theme::border_subtle()
                                    };
                                    egui::Frame::new()
                                        .fill(theme::bg_elevated())
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
                                                    .color(theme::label_color()),
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
                                                            theme::label_weak()
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
                                                            .on_hover_text(help(
                                                                HelpKey::MonitorEditProfile,
                                                            ))
                                                            .clicked()
                                                        {
                                                            to_edit = Some((*client).clone());
                                                        }

                                                        // Per-profile web-login QR toggle (one
                                                        // shown at a time, in the Server Status card).
                                                        let qr_on = tab.qr_for == Some(client.id);
                                                        let qr_btn = theme::action_button(
                                                            if qr_on { "Hide QR" } else { "QR" },
                                                            theme::ACCENT_GREEN,
                                                            egui::Vec2::new(64.0, 24.0),
                                                        );
                                                        if ui
                                                            .add_enabled(!in_reorder, qr_btn)
                                                            .on_hover_text(
                                                                "Show this profile's web-login QR code",
                                                            )
                                                            .clicked()
                                                        {
                                                            tab.qr_for =
                                                                if qr_on { None } else { Some(client.id) };
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
                                                        if ui
                                                            .add(reorder_btn)
                                                            .on_hover_text(help(if in_reorder {
                                                                HelpKey::MonitorReorderDone
                                                            } else {
                                                                HelpKey::MonitorReorder
                                                            }))
                                                            .clicked()
                                                        {
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
                                        // Non-blocking (see Add-profile above).
                                        if let Ok(st) = console_state.try_read() {
                                            tab.picker =
                                                Some(ChannelPickerState::for_edit(&client, &st));
                                        }
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
                                            let pin = existing.pin.clone();
                                            mgr.update_client(id, name, auxes, inputs, pin);
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
                pin,
                permitted_auxes,
                visible_inputs,
            } => {
                let mgr_clone = monitor_manager.clone();
                let name_for_status = name.clone();
                runtime.spawn(async move {
                    let mut mgr = mgr_clone.write().await;
                    match editing {
                        Some(id) => {
                            mgr.update_client(id, name, permitted_auxes, visible_inputs, pin);
                        }
                        None => {
                            let mut client =
                                MonitorClient::new(name, permitted_auxes, visible_inputs);
                            client.pin = pin;
                            mgr.add_client(client);
                        }
                    }
                });
                tab.status_message = Some(
                    if editing.is_some() {
                        format!("Updated profile '{name_for_status}'")
                    } else {
                        format!("Added profile '{name_for_status}'")
                    }
                    .into(),
                );
                tab.qr_tex = None; // a name/PIN edit changes the QR
                tab.picker = None;
            }
            PickerOutcome::Cancel => {
                tab.picker = None;
            }
        }
    }

    // ── Per-profile web-login QR — a centered popup window so it never shifts
    //    the Monitor-tab layout. Opened by the "QR" button on a client row. ──
    if let Some(id) = tab.qr_for {
        let info = {
            let mgr = runtime.block_on(monitor_manager.read());
            mgr.clients
                .get(&id)
                .map(|c| (c.name.clone(), c.pin.clone()))
        };
        match info {
            Some((name, pin)) => {
                let ifaces = crate::ui::net_interfaces::list_interfaces();
                let mut open = true;
                egui::Window::new(format!("Web login · {name}"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                    .open(&mut open)
                    .show(ui.ctx(), |ui| {
                        if web_port == 0 {
                            ui.label("The web monitor is off — set a web port in Setup.");
                            return;
                        }
                        let Some(iface) = ifaces.first() else {
                            ui.label("No LAN interface detected — join a network to get a URL.");
                            return;
                        };
                        let url = profile_url(iface.ip, web_port, &name, &pin);
                        let stale = tab.qr_tex.as_ref().map(|(cid, _)| *cid) != Some(id);
                        if stale && let Some(img) = build_qr_image(&url) {
                            let tex = ui.ctx().load_texture(
                                format!("qr-{id}"),
                                img,
                                egui::TextureOptions::NEAREST,
                            );
                            tab.qr_tex = Some((id, tex));
                        }
                        ui.vertical_centered(|ui| {
                            if let Some((cid, tex)) = &tab.qr_tex
                                && *cid == id
                            {
                                ui.image(egui::load::SizedTexture::new(
                                    tex.id(),
                                    egui::vec2(240.0, 240.0),
                                ));
                            }
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new(&url).monospace().small());
                            ui.label(
                                egui::RichText::new(if pin.is_some() {
                                    "Scan to log in (name + PIN embedded)."
                                } else {
                                    "Scan to log in (name embedded)."
                                })
                                .color(theme::label_weak())
                                .small(),
                            );
                            if !tab.web_server_running {
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(
                                        "⚠ Web server not running — Connect to start it.",
                                    )
                                    .color(theme::ACCENT_AMBER),
                                );
                            }
                        });
                    });
                if !open {
                    tab.qr_for = None;
                }
            }
            // Profile was deleted while its QR was open — close the popup.
            None => tab.qr_for = None,
        }
    }
}

/// Percent-encode a string for use as a URL fragment value.
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build the deep-link a musician's browser opens — name (and PIN, when set) in
/// the URL fragment so the web app auto-connects as that profile.
fn profile_url(ip: std::net::Ipv4Addr, port: u16, name: &str, pin: &Option<String>) -> String {
    let mut url = format!("http://{ip}:{port}/#name={}", enc(name));
    if let Some(p) = pin.as_deref().filter(|p| !p.is_empty()) {
        url.push_str(&format!("&pin={}", enc(p)));
    }
    url
}

/// Rasterize `data` as a QR code into an egui image (black on white, with a
/// quiet zone). `None` only if the data is too large to encode as a QR.
fn build_qr_image(data: &str) -> Option<egui::ColorImage> {
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    let w = code.width();
    let colors = code.to_colors();
    let quiet = 2usize;
    let scale = 6usize;
    let dim = (w + 2 * quiet) * scale;
    let mut px = vec![255u8; dim * dim * 4];
    for y in 0..w {
        for x in 0..w {
            if colors[y * w + x] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        let cx = (x + quiet) * scale + dx;
                        let cy = (y + quiet) * scale + dy;
                        let i = (cy * dim + cx) * 4;
                        px[i] = 0;
                        px[i + 1] = 0;
                        px[i + 2] = 0;
                    }
                }
            }
        }
    }
    Some(egui::ColorImage::from_rgba_unmultiplied([dim, dim], &px))
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
    use super::{build_qr_image, profile_url, reorder_vec};

    #[test]
    fn profile_url_encodes_name_and_optional_pin() {
        let ip: std::net::Ipv4Addr = "192.168.1.50".parse().unwrap();
        assert_eq!(
            profile_url(ip, 8080, "Lead Vox", &Some("12 34".into())),
            "http://192.168.1.50:8080/#name=Lead%20Vox&pin=12%2034"
        );
        // No PIN → name only; blank PIN treated as no PIN.
        assert_eq!(
            profile_url(ip, 8080, "Drums", &None),
            "http://192.168.1.50:8080/#name=Drums"
        );
        assert_eq!(
            profile_url(ip, 8080, "Drums", &Some(String::new())),
            "http://192.168.1.50:8080/#name=Drums"
        );
    }

    #[test]
    fn build_qr_image_produces_a_square_bitmap() {
        let img = build_qr_image("http://192.168.1.50:8080/#name=Drummer&pin=1234")
            .expect("a short URL always encodes");
        assert!(img.size[0] > 0);
        assert_eq!(img.size[0], img.size[1]);
    }

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
