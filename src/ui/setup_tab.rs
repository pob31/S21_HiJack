use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{info, error};

use crate::console::connection::ConnectionManager;
use crate::console::cue_manager::CueManager;
use crate::console::eq_palette_manager::EqPaletteManager;
use crate::console::gang_engine::GangEngine;
use crate::console::gang_manager::GangManager;
use crate::console::ipad_connection;
use crate::console::macro_engine::MacroEngine;
use crate::console::macro_manager::MacroManager;
use crate::console::monitor_engine::MonitorEngine;
use crate::console::monitor_manager::MonitorManager;
use crate::console::snapshot_engine::SnapshotEngine;
use crate::model::config::ChannelOption;
use crate::model::operating_mode::OperatingMode;
use crate::model::parameter::PROTOCOL_COVERAGE;
use crate::model::osc_log::OscLog;
use crate::model::snapshot::CueList;
use crate::model::state::ConsoleState;
use crate::osc::client::{OscClient, OscSender};
use crate::osc::monitor_server::MonitorServer;
use crate::osc::trigger_listener::TriggerListener;
use crate::persistence::show_file::{ConnectionSettings, ShowFile};
use super::UiEvent;
use super::net_interfaces;
use super::theme;

/// State for the Setup tab.
pub struct SetupTabState {
    /// Local IP to bind to (specific interface). Empty or "0.0.0.0" = all interfaces.
    pub local_ip: String,
    /// Interface name (e.g., "en0") for IP_BOUND_IF. Derived from local_ip selection.
    pub interface_name: Option<String>,
    pub console_ip: String,
    pub console_port: String,
    pub local_port: String,
    pub trigger_port: String,
    pub show_file_path: String,
    pub status_message: Option<String>,
    pub operating_mode: OperatingMode,
    /// iPad IP (for Mode 3: real iPad's IP for forwarding responses)
    pub ipad_ip: String,
    /// Console's iPad receive port (daemon sends TO this port on the console)
    pub ipad_console_port: String,
    /// Daemon's iPad receive port (console sends TO this port on the daemon)
    pub ipad_local_port: String,
    /// iPad-side: port the daemon listens on for iPad traffic (Mode 3 only)
    pub ipad_listen_port: String,
    /// iPad-side: port the iPad listens on for daemon responses (Mode 3 only)
    pub ipad_reply_port: String,
    pub ipad_connected: bool,
    pub monitor_port: String,
    /// QLab destination IP — defaults to localhost. Can point to a remote Mac
    /// running QLab when the daemon runs on a separate box (e.g. a Linux host).
    pub qlab_ip: String,
    /// QLab destination port (default 53000 — QLab's standard OSC listen port).
    pub qlab_port: String,
    pub channel_option: ChannelOption,
    pub aux_count: String,
}

impl SetupTabState {
    pub fn new(
        console_ip: &str,
        console_port: u16,
        local_port: u16,
        trigger_port: u16,
        operating_mode: OperatingMode,
        ipad_ip: Option<&str>,
        ipad_send_port: u16,
        ipad_receive_port: u16,
        monitor_port: u16,
    ) -> Self {
        Self {
            local_ip: String::new(),
            interface_name: None,
            console_ip: console_ip.to_string(),
            console_port: console_port.to_string(),
            local_port: local_port.to_string(),
            trigger_port: trigger_port.to_string(),
            show_file_path: String::new(),
            status_message: None,
            operating_mode,
            ipad_ip: ipad_ip.unwrap_or("").to_string(),
            ipad_console_port: if ipad_send_port > 0 {
                ipad_send_port.to_string()
            } else {
                "8022".to_string()
            },
            ipad_local_port: if ipad_receive_port > 0 {
                ipad_receive_port.to_string()
            } else {
                "8021".to_string()
            },
            ipad_listen_port: "9022".to_string(),
            ipad_reply_port: "9021".to_string(),
            ipad_connected: false,
            monitor_port: if monitor_port > 0 {
                monitor_port.to_string()
            } else {
                String::new()
            },
            qlab_ip: "127.0.0.1".to_string(),
            qlab_port: "53000".to_string(),
            channel_option: ChannelOption::Base,
            aux_count: "8".to_string(),
        }
    }
}

/// Draw the Setup tab.
#[allow(clippy::too_many_arguments)]
pub fn draw_setup_tab(
    ui: &mut egui::Ui,
    setup: &mut SetupTabState,
    state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    macro_manager: &Arc<RwLock<MacroManager>>,
    monitor_manager: &Arc<RwLock<MonitorManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
    gang_manager: &Arc<RwLock<GangManager>>,
    snapshot_engine: &mut Option<Arc<SnapshotEngine>>,
    sender: &mut Option<OscSender>,
    connected: &Arc<AtomicBool>,
    cancel_token: &mut Option<CancellationToken>,
    osc_log: &OscLog,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
    egui_ctx: &Arc<std::sync::OnceLock<egui::Context>>,
) {
    let is_connected = connected.load(Ordering::Relaxed);

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        // ── Connection card ──
        theme::card_frame().show(ui, |ui| {
            theme::section_heading(ui, "Connection");

            egui::Grid::new("connection_fields")
                .num_columns(2)
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Network Interface:");
                    ui.add_enabled_ui(!is_connected, |ui| {
                        let interfaces = net_interfaces::list_interfaces();
                        let current_label = if setup.local_ip.is_empty() {
                            "All interfaces (0.0.0.0)".to_string()
                        } else {
                            // Find matching interface for label
                            interfaces.iter()
                                .find(|i| i.ip.to_string() == setup.local_ip)
                                .map(|i| i.label())
                                .unwrap_or_else(|| setup.local_ip.clone())
                        };
                        egui::ComboBox::from_id_salt("nic_select")
                            .selected_text(&current_label)
                            .width(250.0)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(setup.local_ip.is_empty(), "All interfaces (0.0.0.0)").clicked() {
                                    setup.local_ip.clear();
                                    setup.interface_name = None;
                                }
                                for iface in &interfaces {
                                    let label = iface.label();
                                    let selected = setup.local_ip == iface.ip.to_string();
                                    if ui.selectable_label(selected, &label).clicked() {
                                        setup.local_ip = iface.ip.to_string();
                                        setup.interface_name = Some(iface.name.clone());
                                    }
                                }
                            });
                    });
                    ui.end_row();

                    ui.label("Console IP:");
                    ui.add_enabled(!is_connected, egui::TextEdit::singleline(&mut setup.console_ip).desired_width(200.0));
                    ui.end_row();

                    ui.label("Console GP Port (send to):");
                    ui.add_enabled(!is_connected, egui::TextEdit::singleline(&mut setup.console_port).desired_width(80.0));
                    ui.end_row();

                    ui.label("Local GP Port (listen on):");
                    ui.add_enabled(!is_connected, egui::TextEdit::singleline(&mut setup.local_port).desired_width(80.0));
                    ui.end_row();

                    ui.label("Trigger Port (QLab→daemon):");
                    ui.add_enabled(!is_connected, egui::TextEdit::singleline(&mut setup.trigger_port).desired_width(80.0));
                    ui.end_row();

                    ui.label("QLab IP (daemon→QLab):");
                    ui.add_enabled(
                        !is_connected,
                        egui::TextEdit::singleline(&mut setup.qlab_ip)
                            .desired_width(200.0)
                            .hint_text("127.0.0.1"),
                    );
                    ui.end_row();

                    ui.label("QLab Port (daemon→QLab):");
                    ui.add_enabled(
                        !is_connected,
                        egui::TextEdit::singleline(&mut setup.qlab_port)
                            .desired_width(80.0)
                            .hint_text("53000"),
                    );
                    ui.end_row();
                });

            ui.add_space(8.0);

            // Operating mode as toggle buttons
            ui.horizontal(|ui| {
                ui.label("Mode:");
                for mode in [OperatingMode::Mode1, OperatingMode::Mode2, OperatingMode::Mode3] {
                    let is_active = setup.operating_mode == mode;
                    let fill = if is_active { theme::ACCENT_BLUE } else { theme::BG_ELEVATED };
                    let text_color = if is_active { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY };
                    let btn = egui::Button::new(
                        egui::RichText::new(mode.label()).color(text_color),
                    )
                    .fill(fill)
                    .corner_radius(4.0);
                    if ui.add_enabled(!is_connected, btn).clicked() {
                        setup.operating_mode = mode;
                    }
                }
            });

            // Protocol-coverage help card — shows what the currently-selected
            // mode actually reaches and which parameters are unavailable.
            ui.add_space(2.0);
            egui::CollapsingHeader::new("Parameter coverage for this mode")
                .id_salt("protocol_coverage_help")
                .default_open(false)
                .show(ui, |ui| {
                    let uses_ipad = setup.operating_mode.uses_ipad_protocol();
                    ui.label(
                        egui::RichText::new(if uses_ipad {
                            "Mode uses GP OSC + iPad protocol — almost everything is reachable."
                        } else {
                            "Mode 1 is GP OSC only — several parameters require switching to Mode 2 or 3."
                        })
                        .color(theme::TEXT_SECONDARY),
                    );
                    ui.add_space(4.0);
                    egui::Grid::new("protocol_coverage_grid")
                        .num_columns(2)
                        .spacing([12.0, 2.0])
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("Parameter").strong());
                            ui.label(egui::RichText::new("Available?").strong());
                            ui.end_row();
                            for row in PROTOCOL_COVERAGE {
                                let available = row.gp || (uses_ipad && row.ipad);
                                let (mark, color) = if available {
                                    ("yes", theme::TEXT_PRIMARY)
                                } else if row.gp || row.ipad {
                                    ("needs Mode 2/3", theme::TEXT_SECONDARY)
                                } else {
                                    ("console surface only", theme::TEXT_SECONDARY)
                                };
                                ui.label(row.label);
                                ui.label(egui::RichText::new(mark).color(color));
                                ui.end_row();
                            }
                        });
                });

            // Monitor port
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Monitor Port:");
                ui.add_enabled(
                    !is_connected,
                    egui::TextEdit::singleline(&mut setup.monitor_port)
                        .desired_width(80.0)
                        .hint_text("disabled"),
                );
            });

            // Channel option toggle (base vs expanded "+")
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Channels:");
                for opt in [ChannelOption::Base, ChannelOption::Expanded] {
                    let is_active = setup.channel_option == opt;
                    let fill = if is_active { theme::ACCENT_BLUE } else { theme::BG_ELEVATED };
                    let text_color = if is_active { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY };
                    let btn = egui::Button::new(
                        egui::RichText::new(opt.label()).color(text_color),
                    )
                    .fill(fill)
                    .corner_radius(4.0);
                    if ui.add_enabled(!is_connected, btn).clicked() {
                        setup.channel_option = opt;
                        // Update default aux count based on option
                        let total_mix = opt.mix_output_count();
                        let aux: u8 = setup.aux_count.parse().unwrap_or(8);
                        if aux > total_mix {
                            setup.aux_count = "8".to_string();
                        }
                    }
                }
                ui.add_space(8.0);
                ui.label("Aux count:");
                ui.add_enabled(
                    !is_connected,
                    egui::TextEdit::singleline(&mut setup.aux_count)
                        .desired_width(40.0),
                );
                let aux_n: u8 = setup.aux_count.parse().unwrap_or(8);
                let total_mix = setup.channel_option.mix_output_count();
                let group_n = total_mix.saturating_sub(aux_n);
                ui.label(
                    egui::RichText::new(format!(
                        "({} in / {} aux / {} grp)",
                        setup.channel_option.input_count(), aux_n, group_n
                    ))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );
            });

            ui.add_space(8.0);

            // Connect / Disconnect buttons + status
            ui.horizontal(|ui| {
                if !is_connected {
                    let connect_btn = theme::action_button(
                        "Connect",
                        theme::ACCENT_GREEN,
                        egui::Vec2::new(100.0, 36.0),
                    );
                    if ui.add(connect_btn).clicked() {
                        start_connection(
                            setup, state, cue_manager, macro_manager, monitor_manager,
                            eq_palette_manager, gang_manager, snapshot_engine, sender,
                            connected, cancel_token, osc_log,
                            runtime, ui_tx, egui_ctx,
                        );
                    }
                } else {
                    let disconnect_btn = theme::action_button(
                        "Disconnect",
                        theme::ACCENT_RED,
                        egui::Vec2::new(120.0, 36.0),
                    );
                    if ui.add(disconnect_btn).clicked() {
                        do_disconnect(connected, cancel_token, ui_tx);
                    }
                }

                ui.add_space(12.0);
                let (color, label) = if is_connected {
                    (theme::COLOR_CONNECTED, "Connected")
                } else {
                    (theme::COLOR_DISCONNECTED, "Disconnected")
                };
                theme::status_dot(ui, color);
                ui.colored_label(color, label);
            });

            // Status message
            if let Some(msg) = &setup.status_message {
                ui.add_space(4.0);
                ui.colored_label(theme::TEXT_WARNING, msg);
            }
        });

        ui.add_space(8.0);

        // ── iPad Protocol card (when mode 2 or 3) ──
        if setup.operating_mode.uses_ipad_protocol() {
            theme::card_frame().show(ui, |ui| {
                let is_proxy = setup.operating_mode == OperatingMode::Mode3;
                let heading = if is_proxy {
                    "iPad Protocol — Proxy (S21 <> Computer <> iPad)"
                } else {
                    "iPad Protocol — Direct (S21 <> Computer)"
                };
                theme::section_heading(ui, heading);

                if is_proxy {
                    // Three-column layout: S21 | Computer | iPad
                    egui::Grid::new("ipad_proxy_grid")
                        .num_columns(5)
                        .spacing([6.0, 6.0])
                        .show(ui, |ui| {
                            // Header row
                            ui.label(egui::RichText::new("S21").strong().color(theme::ACCENT_BLUE));
                            ui.label("");
                            ui.label(egui::RichText::new("Computer").strong().color(theme::ACCENT_GREEN));
                            ui.label("");
                            ui.label(egui::RichText::new("iPad").strong().color(theme::ACCENT_ORANGE));
                            ui.end_row();

                            // IP row
                            ui.label(egui::RichText::new(&setup.console_ip).color(theme::TEXT_SECONDARY));
                            ui.label("");
                            ui.label(egui::RichText::new("(this machine)").color(theme::TEXT_SECONDARY));
                            ui.label("");
                            ui.add_enabled(!is_connected,
                                egui::TextEdit::singleline(&mut setup.ipad_ip)
                                    .desired_width(120.0)
                                    .hint_text("auto-detect"),
                            );
                            ui.end_row();

                            // S21 Rx < Computer Tx
                            ui.add_enabled(!is_connected,
                                egui::TextEdit::singleline(&mut setup.ipad_console_port).desired_width(60.0),
                            );
                            ui.label(egui::RichText::new("<").color(theme::TEXT_SECONDARY));
                            ui.label(egui::RichText::new("Tx").color(theme::TEXT_SECONDARY));
                            ui.label("");
                            ui.label("");
                            ui.end_row();

                            // S21 Tx > Computer Rx
                            ui.label(egui::RichText::new("Tx").color(theme::TEXT_SECONDARY));
                            ui.label(egui::RichText::new(">").color(theme::TEXT_SECONDARY));
                            ui.add_enabled(!is_connected,
                                egui::TextEdit::singleline(&mut setup.ipad_local_port).desired_width(60.0),
                            );
                            ui.label("");
                            ui.label("");
                            ui.end_row();

                            // Computer Tx > iPad Rx
                            ui.label("");
                            ui.label("");
                            ui.label(egui::RichText::new("Tx").color(theme::TEXT_SECONDARY));
                            ui.label(egui::RichText::new(">").color(theme::TEXT_SECONDARY));
                            ui.add_enabled(!is_connected,
                                egui::TextEdit::singleline(&mut setup.ipad_reply_port).desired_width(60.0),
                            );
                            ui.end_row();

                            // Computer Rx < iPad Tx
                            ui.label("");
                            ui.label("");
                            ui.add_enabled(!is_connected,
                                egui::TextEdit::singleline(&mut setup.ipad_listen_port).desired_width(60.0),
                            );
                            ui.label(egui::RichText::new("<").color(theme::TEXT_SECONDARY));
                            ui.label(egui::RichText::new("Tx").color(theme::TEXT_SECONDARY));
                            ui.end_row();
                        });
                } else {
                    // Mode 2: two-column layout S21 <> Computer
                    egui::Grid::new("ipad_direct_grid")
                        .num_columns(3)
                        .spacing([6.0, 6.0])
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("S21").strong().color(theme::ACCENT_BLUE));
                            ui.label("");
                            ui.label(egui::RichText::new("Computer").strong().color(theme::ACCENT_GREEN));
                            ui.end_row();

                            // S21 Rx < Computer Tx
                            ui.add_enabled(!is_connected,
                                egui::TextEdit::singleline(&mut setup.ipad_console_port).desired_width(60.0),
                            );
                            ui.label(egui::RichText::new("<").color(theme::TEXT_SECONDARY));
                            ui.label(egui::RichText::new("Tx").color(theme::TEXT_SECONDARY));
                            ui.end_row();

                            // S21 Tx > Computer Rx
                            ui.label(egui::RichText::new("Tx").color(theme::TEXT_SECONDARY));
                            ui.label(egui::RichText::new(">").color(theme::TEXT_SECONDARY));
                            ui.add_enabled(!is_connected,
                                egui::TextEdit::singleline(&mut setup.ipad_local_port).desired_width(60.0),
                            );
                            ui.end_row();
                        });
                }

                // iPad status
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let (color, text) = if setup.ipad_connected {
                        (theme::COLOR_CONNECTED, "iPad Connected")
                    } else {
                        (theme::COLOR_DISCONNECTED, "iPad Not Connected")
                    };
                    theme::status_dot(ui, color);
                    ui.colored_label(color, text);
                });
            });

            ui.add_space(8.0);
        }

        // ── Console Info card ──
        theme::card_frame().show(ui, |ui| {
            theme::section_heading(ui, "Console");

            if let Ok(st) = state.try_read() {
                let cfg = &st.config;

                if !cfg.console_name.is_empty() || !cfg.console_serial.is_empty() {
                    egui::Grid::new("console_info")
                        .num_columns(2)
                        .spacing([10.0, 4.0])
                        .show(ui, |ui| {
                            if !cfg.console_name.is_empty() {
                                ui.label("Console:");
                                ui.label(egui::RichText::new(&cfg.console_name).strong());
                                ui.end_row();
                            }
                            if !cfg.console_serial.is_empty() {
                                ui.label("Serial:");
                                ui.label(&cfg.console_serial);
                                ui.end_row();
                            }
                            if let Some(ref session) = cfg.session_filename {
                                ui.label("Session:");
                                ui.label(session);
                                ui.end_row();
                            }
                        });
                    ui.add_space(8.0);
                }

                // Channel counts as colored badges
                ui.label(egui::RichText::new("Channel Configuration").strong());
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    theme::colored_badge(ui, &format!("Inputs: {}", cfg.input_channel_count), theme::CH_INPUT);
                    theme::colored_badge(ui, &format!("Aux: {}", cfg.aux_output_count), theme::CH_AUX);
                    theme::colored_badge(ui, &format!("Groups: {}", cfg.group_output_count), theme::CH_GROUP);
                    theme::colored_badge(ui, &format!("Matrix: {}", cfg.matrix_output_count), theme::CH_MATRIX);
                    theme::colored_badge(ui, &format!("CGs: {}", cfg.control_group_count), theme::CH_CG);
                });
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    theme::colored_badge(ui, &format!("GEQ: {}", cfg.graphic_eq_count), theme::CH_MATRIX);
                    theme::colored_badge(ui, &format!("Mtx In: {}", cfg.matrix_input_count), theme::CH_MATRIX);
                    theme::colored_badge(ui, &format!("Params: {}", st.parameter_count()), theme::ACCENT_BLUE);
                });
            } else {
                ui.label("Loading state...");
            }
        });

        ui.add_space(8.0);

        // ── Show File card ──
        theme::card_frame().show(ui, |ui| {
            theme::section_heading(ui, "Show File");

            ui.horizontal(|ui| {
                ui.label("Path:");
                ui.add(egui::TextEdit::singleline(&mut setup.show_file_path).desired_width(300.0));

                // Browse button using native file dialog
                if ui.add(theme::action_button("Browse...", theme::BG_ELEVATED, egui::Vec2::new(80.0, 28.0))).clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Show Files", &["json"])
                        .add_filter("All Files", &["*"])
                        .pick_file()
                    {
                        setup.show_file_path = path.display().to_string();
                    }
                }
            });

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let load_btn = theme::action_button(
                    "Load Show",
                    theme::ACCENT_BLUE,
                    egui::Vec2::new(100.0, 32.0),
                );
                if ui.add(load_btn).clicked() {
                    load_show_file(setup, cue_manager, macro_manager, monitor_manager, eq_palette_manager, gang_manager, runtime, ui_tx);
                }

                let save_btn = theme::action_button(
                    "Save Show",
                    theme::ACCENT_GREEN,
                    egui::Vec2::new(100.0, 32.0),
                );
                if ui.add(save_btn).clicked() {
                    // If no path set, open a save dialog
                    if setup.show_file_path.is_empty() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Show Files", &["json"])
                            .set_file_name("show.json")
                            .save_file()
                        {
                            setup.show_file_path = path.display().to_string();
                        }
                    }
                    if !setup.show_file_path.is_empty() {
                        save_show_file(setup, state, cue_manager, macro_manager, monitor_manager, eq_palette_manager, gang_manager, runtime, ui_tx);
                    }
                }

                let new_btn = theme::action_button(
                    "New Show",
                    theme::ACCENT_ORANGE,
                    egui::Vec2::new(100.0, 32.0),
                );
                if ui.add(new_btn).clicked() {
                    let cue_mgr = cue_manager.clone();
                    let macro_mgr = macro_manager.clone();
                    let eq_mgr = eq_palette_manager.clone();
                    runtime.spawn(async move {
                        let mut mgr = cue_mgr.write().await;
                        mgr.cue_list = CueList::default();
                        mgr.snapshots.clear();
                        mgr.scope_templates.clear();
                        drop(mgr);
                        let mut mmgr = macro_mgr.write().await;
                        mmgr.macros.clear();
                        mmgr.quick_trigger_ids.clear();
                        drop(mmgr);
                        let mut pmgr = eq_mgr.write().await;
                        pmgr.palettes.clear();
                    });
                    setup.show_file_path.clear();
                    setup.status_message = Some("New show created".into());
                }
            });
        });
    });
}

/// Disconnect from the console: cancel all tasks and reset state.
fn do_disconnect(
    connected: &Arc<AtomicBool>,
    cancel_token: &mut Option<CancellationToken>,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    info!("Disconnecting from console");
    if let Some(token) = cancel_token.take() {
        token.cancel();
    }
    connected.store(false, Ordering::Relaxed);
    let _ = ui_tx.send(UiEvent::Disconnected);
}

#[allow(clippy::too_many_arguments)]
fn start_connection(
    setup: &mut SetupTabState,
    state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    macro_manager: &Arc<RwLock<MacroManager>>,
    monitor_manager: &Arc<RwLock<MonitorManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
    gang_manager: &Arc<RwLock<GangManager>>,
    _snapshot_engine: &mut Option<Arc<SnapshotEngine>>,
    _sender: &mut Option<OscSender>,
    connected: &Arc<AtomicBool>,
    cancel_token: &mut Option<CancellationToken>,
    osc_log: &OscLog,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
    egui_ctx: &Arc<std::sync::OnceLock<egui::Context>>,
) {
    let console_port: u16 = match setup.console_port.parse() {
        Ok(p) => p,
        Err(_) => {
            setup.status_message = Some("Invalid console port".into());
            return;
        }
    };
    let local_port: u16 = match setup.local_port.parse() {
        Ok(p) => p,
        Err(_) => {
            setup.status_message = Some("Invalid local port".into());
            return;
        }
    };
    let trigger_port: u16 = match setup.trigger_port.parse() {
        Ok(p) => p,
        Err(_) => {
            setup.status_message = Some("Invalid trigger port".into());
            return;
        }
    };

    // Parse iPad fields
    let operating_mode = setup.operating_mode;
    let ipad_console_port: u16 = if operating_mode.uses_ipad_protocol() {
        match setup.ipad_console_port.parse() {
            Ok(p) if p > 0 => p,
            _ => {
                setup.status_message = Some("Invalid console iPad port".into());
                return;
            }
        }
    } else {
        0
    };
    let ipad_local_port: u16 = if operating_mode.uses_ipad_protocol() {
        match setup.ipad_local_port.parse() {
            Ok(p) => p,
            Err(_) => {
                setup.status_message = Some("Invalid local iPad port".into());
                return;
            }
        }
    } else {
        0
    };
    let ipad_listen_port: u16 = if operating_mode == OperatingMode::Mode3 {
        setup.ipad_listen_port.parse().unwrap_or(9022)
    } else {
        0
    };
    let ipad_reply_port: u16 = if operating_mode == OperatingMode::Mode3 {
        setup.ipad_reply_port.parse().unwrap_or(9021)
    } else {
        0
    };
    let ipad_ip_str = setup.ipad_ip.clone();
    let bind_ip_str = if setup.local_ip.is_empty() {
        "0.0.0.0".to_string()
    } else {
        setup.local_ip.clone()
    };
    // Derive interface name from local_ip if not explicitly set (e.g., after loading a show file)
    let iface_name = setup.interface_name.clone().or_else(|| {
        if !setup.local_ip.is_empty() {
            net_interfaces::interface_for_ip(&setup.local_ip)
        } else {
            None
        }
    });
    // Update the stored interface_name so subsequent connects don't need to re-derive
    if setup.interface_name.is_none() && iface_name.is_some() {
        setup.interface_name = iface_name.clone();
    }

    let monitor_port: u16 = setup.monitor_port.parse().unwrap_or(0);

    let console_addr_str = format!("{}:{}", setup.console_ip, console_port);
    let console_addr: SocketAddr = match console_addr_str.parse() {
        Ok(a) => a,
        Err(_) => {
            setup.status_message = Some("Invalid console address".into());
            return;
        }
    };
    let bind_ip = bind_ip_str.as_str();
    let local_addr: SocketAddr = format!("{bind_ip}:{local_port}")
        .parse()
        .expect("Invalid local address");
    let trigger_addr: SocketAddr = format!("{bind_ip}:{trigger_port}")
        .parse()
        .expect("Invalid trigger address");

    setup.status_message = Some("Connecting...".into());

    // Create a cancellation token for all tasks in this connection
    let token = CancellationToken::new();
    *cancel_token = Some(token.clone());

    let st = state.clone();
    let cue_mgr = cue_manager.clone();
    let macro_mgr = macro_manager.clone();
    let mon_mgr = monitor_manager.clone();
    let eq_mgr = eq_palette_manager.clone();
    let gang_mgr = gang_manager.clone();
    let conn_flag = connected.clone();
    let tx = ui_tx.clone();
    let ctx = egui_ctx.clone();
    let console_ip = setup.console_ip.clone();
    let log = osc_log.clone();
    let channel_option = setup.channel_option;
    let aux_count: u8 = setup.aux_count.parse().unwrap_or(8);

    runtime.spawn(async move {
        // Apply manual channel config before discovery (discovery may override if it works)
        {
            let mut st_guard = st.write().await;
            st_guard.config.apply_channel_option(channel_option);
            // Override aux/group split from user setting
            let total_mix = channel_option.mix_output_count();
            st_guard.config.aux_output_count = aux_count.min(total_mix);
            st_guard.config.group_output_count = total_mix.saturating_sub(aux_count);
        }

        // Create OscClient manually so we can build GangEngine with the sender
        let client = match OscClient::new(local_addr, console_addr, iface_name.as_deref()).await {
            Ok(c) => c,
            Err(e) => {
                error!("Connection failed: {e}");
                let _ = tx.send(UiEvent::ConnectionFailed(e.to_string()));
                if let Some(ctx) = ctx.get() {
                    ctx.request_repaint();
                }
                return;
            }
        };
        let (osc_sender, rx) = client.into_parts_with_log(Some(log), token.clone());

        // Create GangEngine with the sender
        let gang_engine = Arc::new(RwLock::new(
            GangEngine::new(st.clone(), osc_sender.clone()),
        ));

        let manager = ConnectionManager::connect_from_parts(
            osc_sender, rx, st.clone(), macro_mgr, gang_engine.clone(), gang_mgr, token.clone(),
        );

        info!("Connected to console via UI");
        conn_flag.store(true, Ordering::Relaxed);

        // Create SnapshotEngine (mut so we can set iPad sender before wrapping in Arc)
        let mut snapshot_engine = SnapshotEngine::new(st.clone(), manager.sender());

        // iPad connection (Mode 2 or 3)
        if operating_mode.uses_ipad_protocol() && ipad_console_port > 0 {
            let console_ipad_addr: SocketAddr = format!("{}:{}", console_ip, ipad_console_port)
                .parse()
                .expect("Invalid console iPad address");
            let local_ipad_addr: SocketAddr = format!("{bind_ip_str}:{ipad_local_port}")
                .parse()
                .expect("Invalid local iPad address");

            match operating_mode {
                OperatingMode::Mode2 => {
                    match ipad_connection::connect_mode2(console_ipad_addr, local_ipad_addr, st.clone(), iface_name.as_deref()).await {
                        Ok((ipad_sender, result, _handle)) => {
                            info!(
                                name = %result.config.console_name,
                                "UI Mode 2: iPad protocol connected"
                            );
                            snapshot_engine.set_ipad_sender(Some(ipad_sender.clone()));
                            gang_engine.write().await.set_ipad_sender(Some(ipad_sender));
                            let _ = tx.send(UiEvent::IpadConnected);
                        }
                        Err(e) => {
                            error!("UI Mode 2: iPad connection failed: {e}");
                            let _ = tx.send(UiEvent::IpadConnectionFailed(e.to_string()));
                        }
                    }
                }
                OperatingMode::Mode3 => {
                    // Two-socket proxy:
                    // Socket 1 (console-side): bind to ipad_local_port, send to console:ipad_console_port
                    // Socket 2 (iPad-side): bind to ipad_listen_port, send to iPad:ipad_reply_port
                    let ipad_listener_addr: SocketAddr = format!("{bind_ip_str}:{ipad_listen_port}")
                        .parse()
                        .expect("Invalid iPad listen address");
                    let ipad_target = if !ipad_ip_str.is_empty() {
                        let addr: SocketAddr = format!("{}:{}", ipad_ip_str, ipad_reply_port)
                            .parse()
                            .expect("Invalid iPad target address");
                        Some(addr)
                    } else {
                        None // Will auto-detect from first iPad packet
                    };

                    match ipad_connection::connect_mode3_proxy(
                        console_ipad_addr,
                        local_ipad_addr,
                        ipad_listener_addr,
                        ipad_target,
                        ipad_reply_port,
                        st.clone(),
                        token.clone(),
                        iface_name.clone(),
                    ).await {
                        Ok(ipad_sender) => {
                            info!("UI Mode 3: iPad proxy started");
                            snapshot_engine.set_ipad_sender(Some(ipad_sender.clone()));
                            gang_engine.write().await.set_ipad_sender(Some(ipad_sender));
                            let _ = tx.send(UiEvent::IpadConnected);
                        }
                        Err(e) => {
                            error!("UI Mode 3: iPad proxy setup failed: {e}");
                            let _ = tx.send(UiEvent::IpadConnectionFailed(e.to_string()));
                        }
                    }
                }
                OperatingMode::Mode1 => {}
            }
        }

        let engine = Arc::new(snapshot_engine);

        // Start trigger listener (with cancellation so port is freed on disconnect)
        match TriggerListener::start_with_cancel(trigger_addr, token.clone(), iface_name.as_deref()).await {
            Ok(mut trigger_rx) => {
                let macro_eng = Arc::new(MacroEngine::new(st.clone(), manager.sender()));
                let trigger_cue_mgr = cue_mgr.clone();
                let trigger_macro_mgr = manager.macro_manager();
                let trigger_eq_mgr = eq_mgr.clone();
                let trigger_engine = engine.clone();
                let trigger_macro_eng = macro_eng.clone();
                let trigger_token = token.clone();

                // Spawn trigger processing
                tokio::spawn(async move {
                    use crate::osc::trigger_listener::TriggerEvent;
                    loop {
                        tokio::select! {
                            _ = trigger_token.cancelled() => {
                                info!("Trigger listener shutting down");
                                break;
                            }
                            Some(event) = trigger_rx.recv() => {
                                match event {
                                    TriggerEvent::GoNext => {
                                        let mut mgr = trigger_cue_mgr.write().await;
                                        if let Some(cue) = mgr.go_next() {
                                            let cue = cue.clone();
                                            if let Some(snapshot) = mgr.get_snapshot(&cue.snapshot_id).cloned() {
                                                drop(mgr);
                                                let pmgr = trigger_eq_mgr.read().await;
                                                let result = trigger_engine.recall_cue(&cue, &snapshot, &pmgr.palettes, false).await;
                                                info!(sent = result.parameters_sent, "Trigger GO recall complete");
                                            }
                                        }
                                    }
                                    TriggerEvent::GoPrevious => {
                                        let mut mgr = trigger_cue_mgr.write().await;
                                        if let Some(cue) = mgr.go_previous() {
                                            let cue = cue.clone();
                                            if let Some(snapshot) = mgr.get_snapshot(&cue.snapshot_id).cloned() {
                                                drop(mgr);
                                                let pmgr = trigger_eq_mgr.read().await;
                                                let result = trigger_engine.recall_cue(&cue, &snapshot, &pmgr.palettes, false).await;
                                                info!(sent = result.parameters_sent, "Trigger PREV recall complete");
                                            }
                                        }
                                    }
                                    TriggerEvent::FireCue(number) => {
                                        let mut mgr = trigger_cue_mgr.write().await;
                                        if let Some(cue) = mgr.fire_cue_number(number) {
                                            let cue = cue.clone();
                                            if let Some(snapshot) = mgr.get_snapshot(&cue.snapshot_id).cloned() {
                                                drop(mgr);
                                                let pmgr = trigger_eq_mgr.read().await;
                                                let result = trigger_engine.recall_cue(&cue, &snapshot, &pmgr.palettes, false).await;
                                                info!(number, sent = result.parameters_sent, "Trigger FIRE recall complete");
                                            }
                                        }
                                    }
                                    TriggerEvent::QueryCurrent { reply_addr } => {
                                        let mgr = trigger_cue_mgr.read().await;
                                        let current = mgr.current_cue_number().unwrap_or(-1.0);
                                        info!(current, %reply_addr, "Trigger /cue/current query");
                                    }
                                    TriggerEvent::MacroFire(name) => {
                                        let mgr = trigger_macro_mgr.read().await;
                                        if let Some(macro_def) = mgr.find_by_name_or_id(&name).cloned() {
                                            drop(mgr);
                                            let result = trigger_macro_eng.execute(&macro_def).await;
                                            info!(
                                                name = %result.macro_name,
                                                executed = result.steps_executed,
                                                skipped = result.steps_skipped,
                                                "Trigger MacroFire complete"
                                            );
                                        } else {
                                            tracing::warn!(name, "MacroFire: macro not found");
                                        }
                                    }
                                }
                            }
                            else => break,
                        }
                    }
                });
            }
            Err(e) => {
                error!("Failed to start trigger listener: {e}");
            }
        }

        // Start monitor server (if port configured)
        if monitor_port > 0 {
            let monitor_addr: SocketAddr = format!("0.0.0.0:{}", monitor_port)
                .parse()
                .expect("Invalid monitor address");
            match MonitorServer::start_with_cancel(monitor_addr, token.clone(), iface_name.as_deref()).await {
                Ok((monitor_sender, mut monitor_rx)) => {
                    info!(port = monitor_port, "Monitor server started via UI");
                    let monitor_engine = MonitorEngine::new(st.clone(), manager.sender());
                    let mon_mgr_loop = mon_mgr.clone();
                    let tx_monitor = tx.clone();
                    let monitor_token = token.clone();
                    let _ = tx_monitor.send(UiEvent::MonitorServerStarted);
                    tokio::spawn(async move {
                        let mut last_send_state = std::collections::HashMap::new();
                        let mut poll_interval = tokio::time::interval(
                            std::time::Duration::from_millis(500),
                        );
                        loop {
                            tokio::select! {
                                _ = monitor_token.cancelled() => {
                                    info!("Monitor server shutting down");
                                    break;
                                }
                                Some(cmd) = monitor_rx.recv() => {
                                    let mut mgr = mon_mgr_loop.write().await;
                                    monitor_engine.handle_command(
                                        cmd, &mut mgr, &monitor_sender, true,
                                    ).await;
                                }
                                _ = poll_interval.tick() => {
                                    let mgr = mon_mgr_loop.read().await;
                                    monitor_engine.poll_and_push_state_changes(
                                        &mut last_send_state,
                                        &mgr,
                                        &monitor_sender,
                                    ).await;
                                }
                            }
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to start monitor server: {e}");
                    let _ = tx.send(UiEvent::MonitorServerFailed(e.to_string()));
                }
            }
        }

        let _ = tx.send(UiEvent::ConnectionEstablished);
        if let Some(ctx) = ctx.get() {
            ctx.request_repaint();
        }
    });
}

fn load_show_file(
    setup: &mut SetupTabState,
    cue_manager: &Arc<RwLock<CueManager>>,
    macro_manager: &Arc<RwLock<MacroManager>>,
    monitor_manager: &Arc<RwLock<MonitorManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
    gang_manager: &Arc<RwLock<GangManager>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    // If no path, open a file dialog
    if setup.show_file_path.is_empty() {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Show Files", &["json"])
            .add_filter("All Files", &["*"])
            .pick_file()
        {
            setup.show_file_path = path.display().to_string();
        } else {
            return;
        }
    }

    let path = std::path::PathBuf::from(&setup.show_file_path);
    let cue_mgr = cue_manager.clone();
    let macro_mgr = macro_manager.clone();
    let mon_mgr = monitor_manager.clone();
    let eq_mgr = eq_palette_manager.clone();
    let gang_mgr = gang_manager.clone();
    let tx = ui_tx.clone();
    let path_str = setup.show_file_path.clone();

    runtime.spawn(async move {
        match ShowFile::load(&path).await {
            Ok(show) => {
                let mut mgr = cue_mgr.write().await;
                mgr.cue_list = show.cue_list;
                mgr.snapshots.clear();
                for snap in show.snapshots {
                    mgr.snapshots.insert(snap.id, snap);
                }
                mgr.scope_templates.clear();
                for tmpl in show.scope_templates {
                    mgr.scope_templates.insert(tmpl.id, tmpl);
                }
                drop(mgr);

                // Restore macros
                let mut mmgr = macro_mgr.write().await;
                mmgr.macros.clear();
                for macro_def in show.macros {
                    mmgr.macros.insert(macro_def.id, macro_def);
                }
                mmgr.quick_trigger_ids = show.macro_quick_trigger_ids;
                drop(mmgr);

                // Restore EQ palettes
                let mut pmgr = eq_mgr.write().await;
                pmgr.palettes.clear();
                for palette in show.eq_palettes {
                    pmgr.palettes.insert(palette.id, palette);
                }
                drop(pmgr);

                // Restore monitor clients
                let mut monmgr = mon_mgr.write().await;
                monmgr.clients.clear();
                for client in show.monitor_clients {
                    monmgr.clients.insert(client.id, client);
                }
                drop(monmgr);

                // Restore gang groups
                let mut gmgr = gang_mgr.write().await;
                gmgr.groups.clear();
                for group in show.gang_groups {
                    gmgr.groups.insert(group.id, group);
                }

                info!("Show file loaded: {path_str}");
                let conn = show.connection;
                let _ = tx.send(UiEvent::ShowFileLoaded(path_str, Some(conn)));
            }
            Err(e) => {
                let _ = tx.send(UiEvent::ShowFileError(format!("Load failed: {e}")));
            }
        }
    });
}

fn save_show_file(
    setup: &mut SetupTabState,
    state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    macro_manager: &Arc<RwLock<MacroManager>>,
    monitor_manager: &Arc<RwLock<MonitorManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
    gang_manager: &Arc<RwLock<GangManager>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    if setup.show_file_path.is_empty() {
        setup.status_message = Some("Enter a file path first".into());
        return;
    }

    let path = std::path::PathBuf::from(&setup.show_file_path);
    let st = state.clone();
    let cue_mgr = cue_manager.clone();
    let macro_mgr = macro_manager.clone();
    let mon_mgr = monitor_manager.clone();
    let eq_mgr = eq_palette_manager.clone();
    let gang_mgr = gang_manager.clone();
    let tx = ui_tx.clone();
    let path_str = setup.show_file_path.clone();

    // Capture connection settings from current UI state
    let conn_settings = ConnectionSettings {
        local_ip: setup.local_ip.clone(),
        console_ip: setup.console_ip.clone(),
        console_gp_port: setup.console_port.parse().unwrap_or(8024),
        local_gp_port: setup.local_port.parse().unwrap_or(8023),
        trigger_port: setup.trigger_port.parse().unwrap_or(53001),
        operating_mode: setup.operating_mode,
        channel_option: setup.channel_option,
        aux_count: setup.aux_count.parse().unwrap_or(8),
        ipad_ip: setup.ipad_ip.clone(),
        ipad_send_port: setup.ipad_console_port.parse().unwrap_or(0),
        ipad_receive_port: setup.ipad_local_port.parse().unwrap_or(0),
        ipad_listen_port: setup.ipad_listen_port.parse().unwrap_or(0),
        ipad_reply_port: setup.ipad_reply_port.parse().unwrap_or(0),
        monitor_port: setup.monitor_port.parse().unwrap_or(0),
        qlab_ip: setup.qlab_ip.clone(),
        qlab_port: setup.qlab_port.parse().unwrap_or(53000),
    };

    runtime.spawn(async move {
        let state_guard = st.read().await;
        let mgr = cue_mgr.read().await;
        let mmgr = macro_mgr.read().await;
        let monmgr = mon_mgr.read().await;
        let pmgr = eq_mgr.read().await;
        let gmgr = gang_mgr.read().await;

        let show = ShowFile {
            version: 8,
            console_config: state_guard.config.clone(),
            connection: conn_settings,
            scope_templates: mgr.scope_templates.values().cloned().collect(),
            snapshots: mgr.snapshots.values().cloned().collect(),
            cue_list: mgr.cue_list.clone(),
            macros: mmgr.macros.values().cloned().collect(),
            macro_quick_trigger_ids: mmgr.quick_trigger_ids.clone(),
            eq_palettes: pmgr.palettes.values().cloned().collect(),
            monitor_clients: monmgr.clients.values().cloned().collect(),
            gang_groups: gmgr.groups.values().cloned().collect(),
        };

        drop(state_guard);
        drop(mgr);
        drop(mmgr);
        drop(monmgr);
        drop(pmgr);
        drop(gmgr);

        match show.save(&path).await {
            Ok(()) => {
                info!("Show file saved: {path_str}");
                let _ = tx.send(UiEvent::ShowFileSaved(path_str));
            }
            Err(e) => {
                let _ = tx.send(UiEvent::ShowFileError(format!("Save failed: {e}")));
            }
        }
    });
}
