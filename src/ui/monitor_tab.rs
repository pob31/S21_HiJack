use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;

use crate::console::monitor_manager::MonitorManager;
use crate::model::monitor::MonitorClient;
use super::theme;

/// Per-tab UI state for the Monitor tab.
#[derive(Default)]
pub struct MonitorTabState {
    pub new_client_name: String,
    pub new_client_auxes: String,
    pub new_client_inputs: String,
    pub status_message: Option<String>,
    pub monitor_server_running: bool,
}

/// Draw the Monitor tab.
pub fn draw_monitor_tab(
    ui: &mut egui::Ui,
    tab: &mut MonitorTabState,
    monitor_manager: &Arc<RwLock<MonitorManager>>,
    connected: &Arc<AtomicBool>,
    runtime: &tokio::runtime::Handle,
) {
    let is_connected = connected.load(Ordering::Relaxed);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // ── Server Status card ──
            theme::card_frame().show(ui, |ui| {
                theme::section_heading(ui, "Server Status");

                ui.horizontal(|ui| {
                    // Console status
                    let console_color = if is_connected {
                        theme::COLOR_CONNECTED
                    } else {
                        theme::COLOR_DISCONNECTED
                    };
                    theme::status_dot(ui, console_color);
                    ui.label(
                        egui::RichText::new(if is_connected { "Console Connected" } else { "Console Disconnected" })
                            .color(console_color),
                    );

                    ui.add_space(20.0);

                    // Monitor server status
                    let monitor_color = if tab.monitor_server_running {
                        theme::COLOR_CONNECTED
                    } else {
                        theme::TEXT_DISABLED
                    };
                    theme::status_dot(ui, monitor_color);
                    ui.label(
                        egui::RichText::new(if tab.monitor_server_running { "Monitor Server Running" } else { "Monitor Server Off" })
                            .color(monitor_color),
                    );
                });

                let mgr = runtime.block_on(monitor_manager.read());
                let connected_count = mgr.connected_count();
                let total_count = mgr.clients.len();

                ui.add_space(4.0);
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

            // ── Add Client card ──
            theme::card_frame().show(ui, |ui| {
                theme::section_heading(ui, "Add Client");

                egui::Grid::new("add_client_grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Name:");
                        ui.add(egui::TextEdit::singleline(&mut tab.new_client_name).desired_width(200.0));
                        ui.end_row();

                        ui.label("Permitted Auxes:");
                        ui.add(
                            egui::TextEdit::singleline(&mut tab.new_client_auxes)
                                .desired_width(200.0)
                                .hint_text("1,2,3"),
                        );
                        ui.end_row();

                        ui.label("Visible Inputs:");
                        ui.add(
                            egui::TextEdit::singleline(&mut tab.new_client_inputs)
                                .desired_width(200.0)
                                .hint_text("empty = all"),
                        );
                        ui.end_row();
                    });

                ui.add_space(6.0);
                let add_btn = theme::action_button("Add Client", theme::ACCENT_GREEN, egui::Vec2::new(100.0, 32.0));
                if ui.add(add_btn).clicked() && !tab.new_client_name.trim().is_empty() {
                    let auxes: Vec<u8> = tab
                        .new_client_auxes
                        .split(',')
                        .filter_map(|s| s.trim().parse().ok())
                        .collect();
                    let inputs: Vec<u8> = tab
                        .new_client_inputs
                        .split(',')
                        .filter_map(|s| s.trim().parse().ok())
                        .collect();

                    if auxes.is_empty() {
                        tab.status_message = Some("At least one aux is required".into());
                    } else {
                        let name = tab.new_client_name.trim().to_string();
                        let client = MonitorClient::new(name.clone(), auxes, inputs);
                        let mgr_clone = monitor_manager.clone();
                        runtime.spawn(async move {
                            mgr_clone.write().await.add_client(client);
                        });
                        tab.new_client_name.clear();
                        tab.new_client_auxes.clear();
                        tab.new_client_inputs.clear();
                        tab.status_message = Some(format!("Added client '{name}'"));
                    }
                }

                // Status message
                if let Some(ref msg) = tab.status_message {
                    ui.add_space(4.0);
                    ui.colored_label(theme::TEXT_WARNING, msg.as_str());
                }
            });

            ui.add_space(8.0);

            // ── Client List card ──
            theme::card_frame().show(ui, |ui| {
                theme::section_heading(ui, "Clients");

                let mgr = runtime.block_on(monitor_manager.read());
                let clients = mgr.sorted_clients();

                if clients.is_empty() {
                    ui.label(egui::RichText::new("No monitoring clients configured.").color(theme::TEXT_SECONDARY));
                } else {
                    let mut to_remove = None;

                    for client in &clients {
                        egui::Frame::new()
                            .fill(theme::BG_ELEVATED)
                            .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE))
                            .corner_radius(6.0)
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
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

                                    // Aux badges (magenta)
                                    for aux in &client.permitted_auxes {
                                        theme::colored_badge(ui, &format!("Aux {aux}"), theme::CH_AUX);
                                    }

                                    ui.add_space(4.0);

                                    // Input badges (blue) — only show if restricted
                                    if client.visible_inputs.is_empty() {
                                        theme::colored_badge(ui, "All Inputs", theme::CH_INPUT);
                                    } else {
                                        for input in &client.visible_inputs {
                                            theme::colored_badge(ui, &format!("In {input}"), theme::CH_INPUT);
                                        }
                                    }
                                });

                                // Status + delete row
                                ui.horizontal(|ui| {
                                    ui.add_space(18.0); // align under dot
                                    let status_text = if client.is_connected() {
                                        "Connected"
                                    } else {
                                        "Offline"
                                    };
                                    ui.label(
                                        egui::RichText::new(status_text)
                                            .color(theme::TEXT_SECONDARY)
                                            .small(),
                                    );

                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        let del_btn = theme::action_button("Delete", theme::ACCENT_RED, egui::Vec2::new(60.0, 24.0));
                                        if ui.add(del_btn).clicked() {
                                            to_remove = Some(client.id);
                                        }
                                    });
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
                }
            });
        });
}
