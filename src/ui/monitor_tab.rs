use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;

use crate::console::monitor_manager::MonitorManager;
use crate::model::monitor::MonitorClient;

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

    // Connection health indicators
    ui.horizontal(|ui| {
        let console_color = if is_connected {
            egui::Color32::from_rgb(100, 200, 100)
        } else {
            egui::Color32::from_rgb(200, 100, 100)
        };
        ui.colored_label(console_color, if is_connected { "Console: Connected" } else { "Console: Disconnected" });
        ui.separator();

        let monitor_color = if tab.monitor_server_running {
            egui::Color32::from_rgb(100, 200, 100)
        } else {
            egui::Color32::from_rgb(150, 150, 150)
        };
        ui.colored_label(monitor_color, if tab.monitor_server_running { "Monitor Server: Running" } else { "Monitor Server: Off" });
    });

    ui.separator();

    // Client count
    let mgr = runtime.block_on(monitor_manager.read());
    let connected_count = mgr.connected_count();
    let total_count = mgr.clients.len();
    ui.label(format!("Monitoring clients: {connected_count} connected / {total_count} configured"));

    ui.separator();

    // Add client form
    ui.heading("Add Client");
    egui::Grid::new("add_client_grid")
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.label("Name:");
            ui.text_edit_singleline(&mut tab.new_client_name);
            ui.end_row();

            ui.label("Permitted Auxes:");
            ui.text_edit_singleline(&mut tab.new_client_auxes);
            ui.end_row();

            ui.label("Visible Inputs:");
            ui.add(egui::TextEdit::singleline(&mut tab.new_client_inputs)
                .hint_text("empty = all"));
            ui.end_row();
        });

    if ui.button("Add Client").clicked() && !tab.new_client_name.trim().is_empty() {
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

    ui.separator();

    // Status message
    if let Some(ref msg) = tab.status_message {
        ui.label(msg.as_str());
        ui.separator();
    }

    // Client list
    ui.heading("Clients");

    let clients = mgr.sorted_clients();
    if clients.is_empty() {
        ui.label("No monitoring clients configured.");
    } else {
        let mut to_remove = None;

        egui_extras::TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .column(egui_extras::Column::auto().at_least(100.0))
            .column(egui_extras::Column::auto().at_least(100.0))
            .column(egui_extras::Column::auto().at_least(100.0))
            .column(egui_extras::Column::auto().at_least(80.0))
            .column(egui_extras::Column::auto().at_least(60.0))
            .header(20.0, |mut header| {
                header.col(|ui| { ui.strong("Name"); });
                header.col(|ui| { ui.strong("Auxes"); });
                header.col(|ui| { ui.strong("Inputs"); });
                header.col(|ui| { ui.strong("Status"); });
                header.col(|ui| { ui.strong(""); });
            })
            .body(|mut body| {
                for client in &clients {
                    body.row(20.0, |mut row| {
                        row.col(|ui| { ui.label(&client.name); });
                        row.col(|ui| {
                            let auxes: Vec<String> = client.permitted_auxes.iter().map(|a| a.to_string()).collect();
                            ui.label(auxes.join(", "));
                        });
                        row.col(|ui| {
                            if client.visible_inputs.is_empty() {
                                ui.label("All");
                            } else {
                                let inputs: Vec<String> = client.visible_inputs.iter().map(|i| i.to_string()).collect();
                                ui.label(inputs.join(", "));
                            }
                        });
                        row.col(|ui| {
                            if client.is_connected() {
                                ui.colored_label(
                                    egui::Color32::from_rgb(100, 200, 100),
                                    "Connected",
                                );
                            } else {
                                ui.colored_label(
                                    egui::Color32::from_rgb(150, 150, 150),
                                    "Offline",
                                );
                            }
                        });
                        row.col(|ui| {
                            if ui.button("Delete").clicked() {
                                to_remove = Some(client.id);
                            }
                        });
                    });
                }
            });

        // Can't mutably borrow inside the table body closure, so handle deletion after
        drop(mgr);
        if let Some(id) = to_remove {
            let mgr_clone = monitor_manager.clone();
            runtime.spawn(async move {
                mgr_clone.write().await.remove_client(id);
            });
            tab.status_message = Some("Client removed".into());
        }
    }
}
