use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use super::UiEvent;
use super::net_interfaces;
use super::theme;
use crate::console::connection::ConnectionManager;
use crate::console::cue_manager::CueManager;
use crate::console::gang_engine::GangEngine;
use crate::console::gang_manager::GangManager;
use crate::console::ipad_connection;
use crate::console::macro_engine::MacroEngine;
use crate::console::macro_manager::MacroManager;
use crate::console::monitor_engine::MonitorEngine;
use crate::console::monitor_manager::MonitorManager;
use crate::console::palette_manager::PaletteManager;
use crate::console::pan_link_engine::PanLinkEngine;
use crate::console::snapshot_engine::SnapshotEngine;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::operating_mode::OperatingMode;
use crate::model::osc_log::OscLog;
use crate::model::pan_link::PanLinkBindings;
use crate::model::parameter::PROTOCOL_COVERAGE;
use crate::model::recall_scope::ConsoleRecallConfig;
use crate::model::snapshot::CueList;
use crate::model::state::ConsoleState;
use crate::model::ui_mode::UiMode;
use crate::osc::client::{OscClient, OscSender};
use crate::osc::monitor_server::MonitorServer;
use crate::osc::trigger_listener::TriggerListener;
use crate::persistence::preferences::AppPreferences;
use crate::persistence::show_file::{ConnectionSettings, ShowFile};

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
    /// Inter-message pacing delay in microseconds during snapshot recall.
    pub send_pace_us: u64,
    /// Source-IP CIDR allowlist for the monitor server (audit C2). Round-trips
    /// through the show file. UI editor is a follow-up; for now operators
    /// edit the JSON directly or use the `--monitor-allow-cidr` CLI flag.
    pub monitor_allow_cidrs: Vec<String>,
    /// Source-IP CIDR allowlist for the trigger listener (audit H5). Same
    /// semantics as `monitor_allow_cidrs`.
    pub trigger_allow_cidrs: Vec<String>,
    /// UI display mode — selects which tabs are visible. Persisted both
    /// per-show (in `ConnectionSettings`) and as an app-level default
    /// (in `AppPreferences`) so new sessions resume the operator's last
    /// choice.
    pub ui_mode: UiMode,
    /// Whether the diagnostic tabs (OSC Log, Inspector) are visible.
    /// Operator preference — not persisted per-show.
    pub show_diagnostics: bool,
    /// First-run popup is shown when the app starts with no
    /// `AppPreferences.ui_mode` on disk. Asks the operator to pick a
    /// mode; once dismissed, the choice is saved and this stays false.
    pub show_first_run_popup: bool,
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
        prefs: &AppPreferences,
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
            send_pace_us: 0,
            monitor_allow_cidrs: Vec::new(),
            trigger_allow_cidrs: Vec::new(),
            ui_mode: prefs.ui_mode.unwrap_or_default(),
            show_diagnostics: prefs.show_diagnostics,
            show_first_run_popup: prefs.ui_mode.is_none(),
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
    palette_manager: &Arc<RwLock<PaletteManager>>,
    gang_manager: &Arc<RwLock<GangManager>>,
    pan_link_bindings: &Arc<RwLock<PanLinkBindings>>,
    offline_mode: &Arc<AtomicBool>,
    auto_update_on_recall: &Arc<AtomicBool>,
    console_snapshot_follow: &Arc<AtomicBool>,
    console_recall: &ConsoleRecallConfig,
    dirty_tracker: &Arc<RwLock<DirtyTracker>>,
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

    // First-run popup — shown once per machine until the operator picks a mode.
    if setup.show_first_run_popup {
        draw_first_run_popup(ui, setup);
    }

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        // ── Display Mode card ──
        theme::card_frame().show(ui, |ui| {
            theme::section_heading(ui, "Display Mode");
            ui.label(
                egui::RichText::new(
                    "Streamline the UI by hiding tabs that aren't relevant for the current show.",
                )
                .color(theme::TEXT_SECONDARY)
                .size(theme::FONT_SIZE_BADGE),
            );
            ui.add_space(6.0);

            let mode_before = setup.ui_mode;
            let diag_before = setup.show_diagnostics;

            ui.horizontal(|ui| {
                for mode in UiMode::ALL {
                    ui.radio_value(&mut setup.ui_mode, mode, mode.label());
                }
            });
            ui.add_space(2.0);
            ui.checkbox(
                &mut setup.show_diagnostics,
                "Show diagnostic tabs (OSC Log, Inspector)",
            );

            if setup.ui_mode != mode_before || setup.show_diagnostics != diag_before {
                save_app_preferences(setup);
            }
        });

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
                            palette_manager, gang_manager, pan_link_bindings, offline_mode,
                            auto_update_on_recall, console_snapshot_follow, dirty_tracker,
                            snapshot_engine, sender,
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

            // Console variant readout (auto-derived from discovery, persisted in show file)
            if let Ok(s) = state.try_read() {
                let cfg = &s.config;
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!(
                        "Console: {} ({} inputs, {} mix buses — {} aux / {} group)",
                        cfg.plus_mode.label(),
                        cfg.input_channel_count,
                        cfg.aux_output_count + cfg.group_output_count,
                        cfg.aux_output_count,
                        cfg.group_output_count,
                    ))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );
            }

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
                    load_show_file(setup, state, cue_manager, macro_manager, monitor_manager, palette_manager, gang_manager, pan_link_bindings, connected, runtime, ui_tx);
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
                        save_show_file(
                            setup, state, cue_manager, macro_manager, monitor_manager,
                            palette_manager, gang_manager, pan_link_bindings,
                            auto_update_on_recall.load(Ordering::Relaxed),
                            console_snapshot_follow.load(Ordering::Relaxed),
                            console_recall.clone(),
                            runtime, ui_tx,
                        );
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
                    let pmgr_arc = palette_manager.clone();
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
                        let mut pmgr = pmgr_arc.write().await;
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
    palette_manager: &Arc<RwLock<PaletteManager>>,
    gang_manager: &Arc<RwLock<GangManager>>,
    pan_link_bindings: &Arc<RwLock<PanLinkBindings>>,
    offline_mode: &Arc<AtomicBool>,
    auto_update_on_recall: &Arc<AtomicBool>,
    console_snapshot_follow: &Arc<AtomicBool>,
    dirty_tracker: &Arc<RwLock<DirtyTracker>>,
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
    let pmgr_arc = palette_manager.clone();
    let gang_mgr = gang_manager.clone();
    let pl_bindings = pan_link_bindings.clone();
    let offline = offline_mode.clone();
    let auto_update_flag = auto_update_on_recall.clone();
    let follow_flag = console_snapshot_follow.clone();
    let dirty = dirty_tracker.clone();
    let conn_flag = connected.clone();
    let tx = ui_tx.clone();
    let ctx = egui_ctx.clone();
    let console_ip = setup.console_ip.clone();
    let send_pace_us = setup.send_pace_us;
    let monitor_allow_cidrs = setup.monitor_allow_cidrs.clone();
    let trigger_allow_cidrs = setup.trigger_allow_cidrs.clone();
    let log = osc_log.clone();
    runtime.spawn(async move {
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
        let (mut osc_sender, rx) = client.into_parts_with_log(Some(log), token.clone());
        // Wire the offline gate into the sender so outbound writes are
        // dropped when offline mode is on.
        osc_sender.set_offline_flag(offline.clone());

        // Create GangEngine with the sender
        let gang_engine = Arc::new(RwLock::new(GangEngine::new(st.clone(), osc_sender.clone())));

        // Create PanLinkEngine with the same sender + shared bindings.
        let pan_link_engine = Arc::new(RwLock::new(PanLinkEngine::new(
            st.clone(),
            osc_sender.clone(),
            pl_bindings.clone(),
            dirty.clone(),
        )));

        let daemon = crate::console::connection::DaemonState {
            state: st.clone(),
            macro_manager: macro_mgr,
            gang_engine: gang_engine.clone(),
            gang_manager: gang_mgr,
            pan_link_engine: pan_link_engine.clone(),
            dirty_tracker: dirty.clone(),
            offline_mode: offline.clone(),
        };
        let manager = ConnectionManager::connect_from_parts(osc_sender, rx, daemon, token.clone());

        info!("Connected to console via UI");
        conn_flag.store(true, Ordering::Relaxed);

        // Create SnapshotEngine (mut so we can set iPad sender before wrapping in Arc)
        let mut snapshot_engine = SnapshotEngine::new(st.clone(), manager.sender());
        snapshot_engine.set_dirty_tracker(dirty.clone());
        snapshot_engine.set_pace_us(send_pace_us);
        snapshot_engine.set_cue_manager(cue_mgr.clone());
        snapshot_engine.set_auto_update_flag(auto_update_flag.clone());
        let console_fire_suppression = snapshot_engine.console_fire_suppression();

        // iPad connection (Mode 2 or 3)
        // Channel for console snapshot recall events from the iPad inbound
        // dispatch. The follow-mode dispatcher consumes from the receiver
        // (spawned below after the snapshot engine exists).
        let (snap_event_tx, snap_event_rx) = tokio::sync::mpsc::channel::<i32>(16);
        let mut snap_event_rx = Some(snap_event_rx);

        if operating_mode.uses_ipad_protocol() && ipad_console_port > 0 {
            let console_ipad_addr: SocketAddr = format!("{}:{}", console_ip, ipad_console_port)
                .parse()
                .expect("Invalid console iPad address");
            let local_ipad_addr: SocketAddr = format!("{bind_ip_str}:{ipad_local_port}")
                .parse()
                .expect("Invalid local iPad address");

            match operating_mode {
                OperatingMode::Mode2 => {
                    match ipad_connection::connect_mode2(
                        console_ipad_addr,
                        local_ipad_addr,
                        st.clone(),
                        dirty.clone(),
                        offline.clone(),
                        Some(snap_event_tx.clone()),
                        iface_name.as_deref(),
                    )
                    .await
                    {
                        Ok((ipad_sender, result, _handle)) => {
                            info!(
                                name = %result.config.console_name,
                                "UI Mode 2: iPad protocol connected"
                            );
                            let mut ipad_sender = ipad_sender;
                            ipad_sender.set_offline_flag(offline.clone());
                            snapshot_engine.set_ipad_sender(Some(ipad_sender.clone()));
                            gang_engine
                                .write()
                                .await
                                .set_ipad_sender(Some(ipad_sender.clone()));
                            pan_link_engine
                                .write()
                                .await
                                .set_ipad_sender(Some(ipad_sender));
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
                    let ipad_listener_addr: SocketAddr =
                        format!("{bind_ip_str}:{ipad_listen_port}")
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
                        dirty.clone(),
                        offline.clone(),
                        Some(snap_event_tx.clone()),
                        token.clone(),
                        iface_name.clone(),
                    )
                    .await
                    {
                        Ok(ipad_sender) => {
                            info!("UI Mode 3: iPad proxy started");
                            let mut ipad_sender = ipad_sender;
                            ipad_sender.set_offline_flag(offline.clone());
                            snapshot_engine.set_ipad_sender(Some(ipad_sender.clone()));
                            gang_engine
                                .write()
                                .await
                                .set_ipad_sender(Some(ipad_sender.clone()));
                            pan_link_engine
                                .write()
                                .await
                                .set_ipad_sender(Some(ipad_sender));
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

        // Spawn the follow-mode dispatcher: when the iPad inbound dispatch
        // sees a `/Snapshots/Current_Snapshot` echo and forwards it via
        // `snap_event_tx`, this task looks up the first matching app
        // snapshot in cue-list order and fires its recall. Echoes from
        // our own console-memory writes are filtered via the suppression
        // map populated by `SnapshotEngine::fire_console_memory_if_needed`.
        if let Some(rx) = snap_event_rx.take() {
            let follow_engine = engine.clone();
            let follow_cue_mgr = cue_mgr.clone();
            let follow_palette_mgr = pmgr_arc.clone();
            let follow_flag_clone = follow_flag.clone();
            let suppression = console_fire_suppression.clone();
            tokio::spawn(async move {
                let mut rx = rx;
                while let Some(row) = rx.recv().await {
                    if !follow_flag_clone.load(Ordering::Relaxed) {
                        continue;
                    }
                    // Was this echo caused by our own fire? Drop it.
                    let suppressed = {
                        let mut sup = suppression.write().await;
                        if let Some(when) = sup.get(&row).copied() {
                            // Expire stale entries.
                            if when.elapsed().as_millis()
                                >= crate::console::snapshot_engine::CONSOLE_FIRE_SUPPRESSION_MS
                            {
                                sup.remove(&row);
                                false
                            } else {
                                sup.remove(&row);
                                true
                            }
                        } else {
                            false
                        }
                    };
                    if suppressed {
                        debug!(row, "Follow: suppressed (our own fire)");
                        continue;
                    }

                    // Look up the first matching cue-list snapshot.
                    let target = {
                        let mgr = follow_cue_mgr.read().await;
                        let mut hit = None;
                        for cue in &mgr.cue_list.cues {
                            if let Some(snap) = mgr.snapshots.get(&cue.snapshot_id) {
                                if snap.console_snapshot == Some(row) {
                                    hit = Some(snap.clone());
                                    break;
                                }
                            }
                        }
                        hit
                    };
                    let Some(snapshot) = target else {
                        debug!(row, "Follow: no matching app snapshot");
                        continue;
                    };
                    info!(row, name = %snapshot.name, "Follow: recalling app snapshot");
                    let palettes = follow_palette_mgr.read().await;
                    let scope = snapshot.scope.clone();
                    let result = follow_engine
                        .recall(&snapshot, &scope, &palettes.palettes, false)
                        .await;
                    info!(
                        sent = result.parameters_sent,
                        skipped = result.parameters_skipped,
                        "Follow recall complete"
                    );
                }
            });
        }

        // Start trigger listener (with cancellation so port is freed on disconnect)
        let trigger_allowlist =
            crate::persistence::show_file::parse_cidr_allowlist(&trigger_allow_cidrs);
        match TriggerListener::start_with_cancel(
            trigger_addr,
            token.clone(),
            iface_name.as_deref(),
            trigger_allowlist,
        )
        .await
        {
            Ok(mut trigger_rx) => {
                let mut macro_eng = MacroEngine::new(st.clone(), manager.sender());
                macro_eng.set_dirty_tracker(dirty.clone());
                let macro_eng = Arc::new(macro_eng);
                let trigger_cue_mgr = cue_mgr.clone();
                let trigger_macro_mgr = manager.macro_manager();
                let trigger_palette_mgr = pmgr_arc.clone();
                let trigger_engine = engine.clone();
                let trigger_macro_eng = macro_eng.clone();
                let trigger_token = token.clone();

                // Spawn trigger processing
                tokio::spawn(async move {
                    use crate::console::trigger_dispatch;
                    loop {
                        tokio::select! {
                            _ = trigger_token.cancelled() => {
                                info!("Trigger listener shutting down");
                                break;
                            }
                            Some(event) = trigger_rx.recv() => {
                                trigger_dispatch::handle_trigger_event(
                                    event,
                                    &trigger_cue_mgr,
                                    &trigger_palette_mgr,
                                    &trigger_macro_mgr,
                                    &trigger_macro_eng,
                                    &trigger_engine,
                                    None, // UI path doesn't bind a reply socket
                                )
                                .await;
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
            let monitor_allowlist =
                crate::persistence::show_file::parse_cidr_allowlist(&monitor_allow_cidrs);
            match MonitorServer::start_with_cancel(
                monitor_addr,
                token.clone(),
                iface_name.as_deref(),
                monitor_allowlist,
            )
            .await
            {
                Ok((monitor_sender, mut monitor_rx)) => {
                    info!(port = monitor_port, "Monitor server started via UI");
                    let monitor_engine = MonitorEngine::new(st.clone(), manager.sender());
                    let mon_mgr_loop = mon_mgr.clone();
                    let tx_monitor = tx.clone();
                    let monitor_token = token.clone();
                    let _ = tx_monitor.send(UiEvent::MonitorServerStarted);
                    tokio::spawn(async move {
                        let mut last_send_state = std::collections::HashMap::new();
                        let mut last_aux_state = std::collections::HashMap::new();
                        let mut last_generation: u64 = 0;
                        let mut last_push_times = std::collections::HashMap::new();
                        let mut last_aux_push_times = std::collections::HashMap::new();
                        let mut poll_interval =
                            tokio::time::interval(std::time::Duration::from_millis(10));
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
                                        &mut last_aux_state,
                                        &mut last_generation,
                                        &mut last_push_times,
                                        &mut last_aux_push_times,
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
    state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    macro_manager: &Arc<RwLock<MacroManager>>,
    monitor_manager: &Arc<RwLock<MonitorManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    gang_manager: &Arc<RwLock<GangManager>>,
    pan_link_bindings: &Arc<RwLock<PanLinkBindings>>,
    connected: &Arc<AtomicBool>,
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
    let st = state.clone();
    let cue_mgr = cue_manager.clone();
    let macro_mgr = macro_manager.clone();
    let mon_mgr = monitor_manager.clone();
    let pmgr_arc = palette_manager.clone();
    let gang_mgr = gang_manager.clone();
    let pl_bindings = pan_link_bindings.clone();
    let conn_flag = connected.clone();
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

                // Restore palettes (EQ, Compressor, Gate)
                let mut pmgr = pmgr_arc.write().await;
                pmgr.palettes.clear();
                for palette in show.palettes {
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
                drop(gmgr);

                // Restore pan link bindings
                {
                    let mut pl = pl_bindings.write().await;
                    *pl = show.pan_link;
                }

                // Restore console config (channel counts, plus_mode, bus
                // split) so offline editing works. Skip if already
                // connected — the live console is authoritative.
                if !conn_flag.load(Ordering::Relaxed) {
                    let mut s = st.write().await;
                    s.config = show.console_config.clone();
                }

                info!("Show file loaded: {path_str}");
                let conn = show.connection;
                let recall = show.console_recall;
                let _ = tx.send(UiEvent::ShowFileLoaded(
                    path_str,
                    Some(Box::new(conn)),
                    recall,
                ));
            }
            Err(e) => {
                error!("Load failed for {path_str}: {e}");
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
    palette_manager: &Arc<RwLock<PaletteManager>>,
    gang_manager: &Arc<RwLock<GangManager>>,
    pan_link_bindings: &Arc<RwLock<PanLinkBindings>>,
    auto_update_on_recall: bool,
    console_snapshot_follow: bool,
    console_recall: ConsoleRecallConfig,
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
    let pmgr_arc = palette_manager.clone();
    let gang_mgr = gang_manager.clone();
    let pl_bindings = pan_link_bindings.clone();
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
        ipad_ip: setup.ipad_ip.clone(),
        ipad_send_port: setup.ipad_console_port.parse().unwrap_or(0),
        ipad_receive_port: setup.ipad_local_port.parse().unwrap_or(0),
        ipad_listen_port: setup.ipad_listen_port.parse().unwrap_or(0),
        ipad_reply_port: setup.ipad_reply_port.parse().unwrap_or(0),
        monitor_port: setup.monitor_port.parse().unwrap_or(0),
        qlab_ip: setup.qlab_ip.clone(),
        qlab_port: setup.qlab_port.parse().unwrap_or(53000),
        send_pace_us: setup.send_pace_us,
        auto_update_on_recall,
        console_snapshot_follow,
        monitor_allow_cidrs: setup.monitor_allow_cidrs.clone(),
        trigger_allow_cidrs: setup.trigger_allow_cidrs.clone(),
        ui_mode: setup.ui_mode,
    };

    runtime.spawn(async move {
        let state_guard = st.read().await;
        let mgr = cue_mgr.read().await;
        let mmgr = macro_mgr.read().await;
        let monmgr = mon_mgr.read().await;
        let pmgr = pmgr_arc.read().await;
        let gmgr = gang_mgr.read().await;
        let pl = pl_bindings.read().await;

        let show = ShowFile {
            version: 14,
            console_config: state_guard.config.clone(),
            connection: conn_settings,
            scope_templates: mgr.scope_templates.values().cloned().collect(),
            snapshots: mgr.snapshots.values().cloned().collect(),
            cue_list: mgr.cue_list.clone(),
            macros: mmgr.macros.values().cloned().collect(),
            macro_quick_trigger_ids: mmgr.quick_trigger_ids.clone(),
            palettes: pmgr.palettes.values().cloned().collect(),
            monitor_clients: monmgr.clients.values().cloned().collect(),
            gang_groups: gmgr.groups.values().cloned().collect(),
            console_recall: console_recall.clone(),
            pan_link: pl.clone(),
        };

        drop(state_guard);
        drop(mgr);
        drop(mmgr);
        drop(monmgr);
        drop(pmgr);
        drop(gmgr);
        drop(pl);

        match show.save(&path).await {
            Ok(()) => {
                info!("Show file saved: {path_str}");
                let _ = tx.send(UiEvent::ShowFileSaved(path_str));
            }
            Err(e) => {
                error!("Save failed for {path_str}: {e}");
                let _ = tx.send(UiEvent::ShowFileError(format!("Save failed: {e}")));
            }
        }
    });
}

/// Persist the current UI mode + diagnostic toggle as the application
/// default. Failure is logged at warn level — the in-memory state still
/// applies for the session.
fn save_app_preferences(setup: &SetupTabState) {
    let prefs = AppPreferences {
        ui_mode: Some(setup.ui_mode),
        show_diagnostics: setup.show_diagnostics,
    };
    if let Err(e) = prefs.save() {
        tracing::warn!(error = %e, "Failed to save app preferences");
    }
}

/// Modal welcome popup shown on first launch (no `AppPreferences` on
/// disk yet). Asks the operator to pick a display mode and remembers
/// the choice for next time.
fn draw_first_run_popup(ui: &mut egui::Ui, setup: &mut SetupTabState) {
    let ctx = ui.ctx().clone();
    egui::Window::new("Welcome to S21 HiJack")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(&ctx, |ui| {
            ui.set_min_width(360.0);
            ui.label(
                egui::RichText::new("Choose a display mode to get started.")
                    .strong()
                    .size(theme::FONT_SIZE_BODY),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("You can change this any time on the Setup tab.")
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add_space(10.0);

            let descriptions = [
                (UiMode::Full, "All tabs visible — every feature available."),
                (
                    UiMode::LiveMusic,
                    "Macros, gangs, and personal monitoring. Hides the cueing tab.",
                ),
                (
                    UiMode::Theatre,
                    "Macros, gangs, cueing, and palettes. Hides the monitoring tab.",
                ),
            ];

            for (mode, desc) in descriptions {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_sized(
                            [120.0, 32.0],
                            egui::Button::new(egui::RichText::new(mode.label()).strong()),
                        )
                        .clicked()
                    {
                        setup.ui_mode = mode;
                        setup.show_first_run_popup = false;
                        save_app_preferences(setup);
                    }
                    ui.label(egui::RichText::new(desc).color(theme::TEXT_SECONDARY));
                });
            }
        });
}
