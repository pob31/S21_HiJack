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
use crate::osc::client::OscClient;
use crate::osc::ipad_client::IpadSender;
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
    /// Phase 3 — drives the centered "Parameter coverage" popup window.
    /// Runtime-only; not persisted.
    pub show_coverage_popup: bool,
    /// Suppresses the "console IP not on the selected NIC's network"
    /// warning (red ⚠ next to the IP edit + amber strip at the
    /// bottom). Set when the operator clicks either dismissal site;
    /// cleared whenever the console IP or selected NIC changes so the
    /// warning re-evaluates. Runtime-only; not persisted.
    pub console_ip_warning_dismissed: bool,
    /// Show-file path passed on the command line (positional CLI arg
    /// or via OS file association). Drained on the first frame the
    /// Setup tab draws — `draw_setup_tab` calls `load_show_file` once
    /// and clears the slot. Runtime-only.
    pub pending_initial_load: Option<std::path::PathBuf>,
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
            show_coverage_popup: false,
            console_ip_warning_dismissed: false,
            pending_initial_load: None,
        }
    }
}

/// CLI default for the console IP — used as a sentinel by the
/// NIC-selection auto-fill: if the operator hasn't customised the
/// console IP yet, picking a NIC will rewrite the first three octets
/// to match the chosen interface.
pub const DEFAULT_CONSOLE_IP: &str = "192.168.1.1";

/// Split an IPv4 address into its four octets as string slices.
/// Returns `None` if the input doesn't have exactly four
/// dot-separated parts (e.g. while the user is mid-edit).
fn ipv4_parts(ip: &str) -> Option<[&str; 4]> {
    let mut iter = ip.splitn(4, '.');
    let a = iter.next()?;
    let b = iter.next()?;
    let c = iter.next()?;
    let d = iter.next()?;
    if a.is_empty() || b.is_empty() || c.is_empty() || d.is_empty() {
        return None;
    }
    Some([a, b, c, d])
}

/// True when both `a` and `b` parse as IPv4 and share their first two
/// octets — i.e. they're reachable from each other under a /16 mask
/// (lenient bound; /24 is the common case but /16 catches more).
fn first_two_octets_match(a: &str, b: &str) -> bool {
    match (ipv4_parts(a), ipv4_parts(b)) {
        (Some(pa), Some(pb)) => pa[0] == pb[0] && pa[1] == pb[1],
        _ => false,
    }
}

/// Replace the first three octets of `target` with those of `source`,
/// preserving `target`'s last octet. Used by the NIC auto-fill so
/// picking a NIC like `10.0.5.20` rewrites the default `192.168.1.1`
/// to `10.0.5.1`. Returns `target` unchanged if either side isn't a
/// valid four-octet IPv4 string.
fn align_first_three_octets(target: &str, source: &str) -> String {
    let Some(t) = ipv4_parts(target) else {
        return target.to_string();
    };
    let Some(s) = ipv4_parts(source) else {
        return target.to_string();
    };
    format!("{}.{}.{}.{}", s[0], s[1], s[2], t[3])
}

/// True when the operator has selected a specific NIC and the console
/// IP is on a different /16 — i.e. the warning should fire.
pub fn console_ip_mismatch(setup: &SetupTabState) -> bool {
    !setup.local_ip.is_empty() && !first_two_octets_match(&setup.local_ip, &setup.console_ip)
}

/// Ensure the show-file path has a recognised extension. If it
/// already ends in `.s21show` (the canonical extension) or `.json`
/// (legacy), it's left alone — re-saving over a legacy `.json` show
/// file shouldn't silently rename it. Otherwise `.s21show` is
/// appended. Idempotent and string-only; the caller writes the file.
pub fn ensure_show_file_extension(path: &mut String) {
    if path.is_empty() {
        return;
    }
    let lower = path.to_lowercase();
    if lower.ends_with(".s21show") || lower.ends_with(".json") {
        return;
    }
    path.push_str(".s21show");
}

/// Compact a show-file path for display: `…/<filename>` when the path
/// has a parent directory, just the filename otherwise. Empty paths
/// pass through unchanged so the field reads as empty in the UI.
pub fn truncate_show_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let p = std::path::Path::new(path);
    match p.file_name() {
        Some(name) => {
            let has_parent = p
                .parent()
                .map(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(false);
            if has_parent {
                format!("…/{}", name.to_string_lossy())
            } else {
                name.to_string_lossy().into_owned()
            }
        }
        None => path.to_string(),
    }
}

// ── Connection diagram layout constants ──
//
// Five framed sections form a W: Console + Server + iPad on top,
// QLab + Monitor offset below into the gaps. The Server is the larger
// central hub; the four satellites are smaller. Phase 5 makes section
// widths adaptive to the window width — Console/iPad/QLab/Monitor scale
// between MIN_W_SAT and MAX_W_SAT, the Server hub between MIN_W_HUB and
// MAX_W_HUB. Above MAX, the extra width spills into larger gaps; below
// MAX, the gaps stay tight and the section widths shrink linearly until
// they hit MIN.
const MIN_W_SAT: f32 = 220.0;
const MAX_W_SAT: f32 = 260.0;
const MIN_W_HUB: f32 = 360.0;
const MAX_W_HUB: f32 = 440.0;
const SECT_GAP: f32 = 6.0;
/// Hard cap on the inter-section gap as a defence-in-depth — keeps things
/// sane even if `available_width` over-reports for some reason. With the
/// Phase-6 frame-padding fix, Phase-8 short Connection-Mode labels (so the
/// Server hub really does fit `MAX_W_HUB`), the gap should fill the window
/// up to this generous cap without pushing iPad past the right edge.
const MAX_GAP: f32 = 400.0;
/// Total horizontal padding consumed by `theme::elevated_frame()`
/// (`inner_margin = 10` per side). The 4 satellite frames each add this
/// to their rendered footprint on top of the inner content `w_sat`.
const SAT_FRAME_PAD: f32 = 20.0;
/// Total horizontal padding consumed by `theme::card_frame()` with
/// `outer_margin = 0` (Server hub: `inner_margin = 12` per side).
const HUB_FRAME_PAD: f32 = 24.0;
/// Per-frame stroke overhead — `Frame::stroke` draws 1 px on each side
/// outside the allocated content rect. egui's `allocate_rect` reserves
/// only `inner_margin + outer_margin`, so stroke effectively pushes the
/// rendered footprint 2 px wider than the math suggests. Counted three
/// times (Console + Server + iPad) when sizing the top row.
const FRAME_STROKE_PAD: f32 = 2.0;
/// Small safety pad for AA / sub-pixel rounding so the iPad section's
/// right edge never quite reaches the Connection card's inner edge.
const LAYOUT_SAFETY: f32 = 4.0;
/// Min height for the three top-row sections (Console / Server / iPad).
/// Trimmed from 480 to 420 to remove visible dead space below the content
/// — Console's max content (4 grid rows + iPad-protocol status + identity
/// grid + 2 wrapped channel-config badge rows) still fits comfortably.
const MIN_TOP_HEIGHT: f32 = 420.0;
/// Min height for the two bottom-row sections (QLab / Monitor).
/// Sized to comfortably exceed both panels' natural content heights so
/// they render at the same height — Monitor (Server↔Mobile/Web row only)
/// would otherwise be much shorter than QLab (IP + 3-row port grid).
const MIN_BOT_HEIGHT: f32 = 180.0;
/// Width of every port-number `TextEdit` in the W-diagram. Chosen wide
/// enough to display 5-digit ports (53000, 8001, 8025, …) plus the
/// 6 px inner margin without truncation. Forced via `ui.add_sized`
/// because egui's `Grid` only honours `desired_width` as a hint and
/// can squeeze columns down to the actual text width otherwise.
const PORT_EDIT_W: f32 = 90.0;
/// Height of every port-number `TextEdit`.
const PORT_EDIT_H: f32 = 26.0;
/// Inner margin for port-number `TextEdit`s — gives the digits a bit
/// of breathing room inside the box.
const PORT_EDIT_MARGIN: egui::Margin = egui::Margin::symmetric(6, 4);

/// Render a port-number `TextEdit` with the standard width / height /
/// margin. `enabled` controls interaction (becomes greyed out when
/// `is_connected` is true). Optional `hint` paints placeholder text
/// when the field is empty.
fn port_edit_enabled(
    ui: &mut egui::Ui,
    value: &mut String,
    enabled: bool,
    hint: &str,
    hover: &str,
) -> egui::Response {
    let resp = ui
        .add_enabled_ui(enabled, |ui| {
            let mut edit = egui::TextEdit::singleline(value).margin(PORT_EDIT_MARGIN);
            if !hint.is_empty() {
                edit = edit.hint_text(hint);
            }
            ui.add_sized([PORT_EDIT_W, PORT_EDIT_H], edit)
        })
        .inner;
    resp.on_hover_text(hover)
}

/// Render a port-number `TextEdit` whose visibility is gated by `visible`
/// — used for the Mode-1 iPad rows in the Console satellite which keep
/// their slot but render invisibly so the grid layout doesn't shift.
/// Forces the same width / height / margin as `port_edit_enabled`.
fn port_edit_visible(
    ui: &mut egui::Ui,
    value: &mut String,
    visible: bool,
    enabled: bool,
    hover: &str,
) -> egui::Response {
    let builder = if visible {
        egui::UiBuilder::new()
    } else {
        egui::UiBuilder::new().invisible()
    };
    let resp = ui
        .scope_builder(builder, |ui| {
            ui.add_enabled_ui(visible && enabled, |ui| {
                ui.add_sized(
                    [PORT_EDIT_W, PORT_EDIT_H],
                    egui::TextEdit::singleline(value).margin(PORT_EDIT_MARGIN),
                )
            })
            .inner
        })
        .inner;
    if visible {
        resp.on_hover_text(hover)
    } else {
        resp
    }
}

/// Compute (w_sat, w_hub, inter-section gap) from `available_width`. See the
/// constants block above for the scaling rules.
///
/// `available_width` is the *outer* card-body width that the diagram must fit
/// inside. The 3 top-row frames consume an overhead of inner-margin plus 1 px
/// stroke each side, plus a small AA / rounding pad, on top of the content
/// widths — see `frame_overhead` below.
fn compute_diagram_widths(available_width: f32) -> (f32, f32, f32) {
    let frame_overhead = 2.0 * (SAT_FRAME_PAD + FRAME_STROKE_PAD)
        + (HUB_FRAME_PAD + FRAME_STROKE_PAD)
        + LAYOUT_SAFETY;
    let avail = (available_width - frame_overhead).max(0.0);
    let target = (avail - 2.0 * SECT_GAP).max(0.0);
    let max_total = 2.0 * MAX_W_SAT + MAX_W_HUB;
    let min_total = 2.0 * MIN_W_SAT + MIN_W_HUB;
    if target >= max_total {
        let extra = ((target - max_total) / 2.0).min(MAX_GAP - SECT_GAP);
        (MAX_W_SAT, MAX_W_HUB, SECT_GAP + extra)
    } else if target >= min_total {
        let scale = (target - min_total) / (max_total - min_total);
        (
            MIN_W_SAT + scale * (MAX_W_SAT - MIN_W_SAT),
            MIN_W_HUB + scale * (MAX_W_HUB - MIN_W_HUB),
            SECT_GAP,
        )
    } else {
        (MIN_W_SAT, MIN_W_HUB, SECT_GAP)
    }
}

/// Render a framed peer section with a coloured title row and an optional
/// right-aligned status indicator (dot + colored label).
///
/// `use_card_frame` picks the heavier `card_frame()` for the Server hub;
/// the satellites use the lighter `elevated_frame()`.
/// `min_height` keeps the section box at a constant height across modes
/// so the bottom row stays anchored.
fn peer_section(
    ui: &mut egui::Ui,
    title: &str,
    title_color: egui::Color32,
    width: f32,
    min_height: f32,
    use_card_frame: bool,
    header_status: Option<(egui::Color32, &str)>,
    body: impl FnOnce(&mut egui::Ui),
) {
    // The Server hub uses the heavier `card_frame` for visual prominence,
    // but its default outer_margin (4 px symmetric) would push it out of
    // line with the elevated_frame satellites in the W-row. Force zero
    // outer_margin so all five sections sit flush in the horizontal flow.
    let frame = if use_card_frame {
        theme::card_frame().outer_margin(egui::Margin::ZERO)
    } else {
        theme::elevated_frame()
    };
    frame.show(ui, |ui| {
        // Force a vertical layout — when a section sits inside a parent
        // horizontal layout (the W-diagram rows), the frame's inner UI
        // inherits that direction and lays the title, IP field, grid all
        // out side-by-side, blowing the section's width budget.
        ui.vertical(|ui| {
            ui.set_min_width(width);
            ui.set_max_width(width);
            ui.set_min_height(min_height);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .strong()
                        .color(title_color)
                        .size(theme::FONT_SIZE_BODY),
                );
                if let Some((color, text)) = header_status {
                    let avail = ui.available_size();
                    ui.allocate_ui_with_layout(
                        avail,
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.colored_label(color, egui::RichText::new(text).small());
                            theme::status_dot(ui, color);
                        },
                    );
                }
            });
            ui.add_space(4.0);
            body(ui);
        });
    });
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
    stream_deck_config: &Arc<RwLock<crate::model::streamdeck::StreamDeckConfig>>,
    offline_mode: &Arc<AtomicBool>,
    auto_update_on_recall: &Arc<AtomicBool>,
    console_snapshot_follow: &Arc<AtomicBool>,
    console_recall: &ConsoleRecallConfig,
    dirty_tracker: &Arc<RwLock<DirtyTracker>>,
    last_received: &Arc<RwLock<Option<crate::model::parameter::ParameterAddress>>>,
    pending_engines: &Arc<std::sync::Mutex<Option<crate::ui::PendingEngines>>>,
    connected: &Arc<AtomicBool>,
    cancel_token: &mut Option<CancellationToken>,
    osc_log: &OscLog,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
    egui_ctx: &Arc<std::sync::OnceLock<egui::Context>>,
) {
    let is_connected = connected.load(Ordering::Relaxed);

    // Auto-load on startup: a CLI positional show-file (or a file
    // opened via OS association) is queued in `pending_initial_load`
    // and drained on the first Setup-tab draw. Loading is async; the
    // helper spawns a task and reports progress via `ui_tx`, so this
    // returns immediately and the rest of the frame proceeds normally.
    if let Some(path) = setup.pending_initial_load.take() {
        setup.show_file_path = path.display().to_string();
        load_show_file(
            setup,
            state,
            cue_manager,
            macro_manager,
            monitor_manager,
            palette_manager,
            gang_manager,
            pan_link_bindings,
            stream_deck_config,
            connected,
            runtime,
            ui_tx,
        );
    }

    // First-run popup — shown once per machine until the operator picks a mode.
    if setup.show_first_run_popup {
        draw_first_run_popup(ui, setup);
    }

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        // ── Connection card (W-diagram: hub + 4 satellites) ──
        // Phase 2: the Server hub absorbs Display Mode + Connection Mode +
        // Show File controls; the Console satellite absorbs Channel Config
        // and the console identity readout. The right-column cards
        // (Show File, Console) are gone.
        theme::card_frame().show(ui, |ui| {
            // ── Top strip: heading + Connect/Disconnect + global status ──
            // The heading replaces the previous `theme::section_heading()` call
            // so the Connect button lives at the top of the card next to the
            // status, instead of below the diagram.
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Connection")
                        .size(theme::FONT_SIZE_SECTION)
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );
                let avail = ui.available_size();
                ui.allocate_ui_with_layout(
                    avail,
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        // Status label/dot is intentionally NOT shown here —
                        // the outer app header already shows global connected
                        // state, and the Console / iPad satellite headers show
                        // per-peer state. Duplicating it next to the button
                        // adds noise without information.
                        if !is_connected {
                            // Sized to fit "Disconnect" so the button
                            // doesn't change width across the connect /
                            // disconnect transition — keeps the right-
                            // aligned controls anchored.
                            if theme::long_press_button(
                                ui,
                                "Connect",
                                theme::ACCENT_GREEN,
                                egui::Vec2::new(120.0, 32.0),
                                true,
                                theme::LONG_PRESS_DURATION_MS,
                            ) {
                                start_connection(
                                    setup, state, cue_manager, macro_manager, monitor_manager,
                                    palette_manager, gang_manager, pan_link_bindings, offline_mode,
                                    auto_update_on_recall, console_snapshot_follow, dirty_tracker,
                                    last_received,
                                    pending_engines,
                                    connected, cancel_token, osc_log,
                                    runtime, ui_tx, egui_ctx,
                                );
                            }
                        } else if theme::long_press_button(
                            ui,
                            "Disconnect",
                            theme::ACCENT_RED,
                            egui::Vec2::new(120.0, 32.0),
                            true,
                            theme::LONG_PRESS_DURATION_MS,
                        ) {
                            do_disconnect(connected, cancel_token, ui_tx);
                        }
                    },
                );
            });
            ui.add_space(2.0);
            // Match the underline emitted by `theme::section_heading()` so this
            // hand-rolled strip looks consistent with section headings elsewhere.
            let strip_w = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(strip_w, 1.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 0.0, theme::BORDER_SUBTLE);
            ui.add_space(6.0);

            let uses_ipad = setup.operating_mode.uses_ipad_protocol();
            let is_proxy = setup.operating_mode == OperatingMode::Mode3;

            let console_status = if is_connected {
                Some((theme::COLOR_CONNECTED, "Connected"))
            } else {
                Some((theme::COLOR_DISCONNECTED, "Disconnected"))
            };
            let ipad_status = if is_proxy {
                Some(if setup.ipad_connected {
                    (theme::COLOR_CONNECTED, "Connected")
                } else {
                    (theme::COLOR_DISCONNECTED, "Not connected")
                })
            } else {
                None
            };

            // ── Top row: Console (small) | Server (large hub) | iPad (small) ──
            // Section widths and inter-section gap come from `compute_diagram_widths`
            // (see helper above) so the diagram fits any window width: shrink
            // section widths down to MIN when squeezed, grow gaps when there's
            // extra space.
            let (w_sat, w_hub, gap) = compute_diagram_widths(ui.available_width());

            ui.horizontal(|ui| {
                // Zero the inter-item spacing so the row's total horizontal
                // footprint is exactly `2*w_sat + w_hub + 2*gap` (matching
                // what `compute_diagram_widths` solved for). Default
                // `item_spacing.x = 10` would otherwise insert 40 px of
                // unaccounted space between siblings, pushing the iPad
                // satellite past the right edge of the card.
                ui.spacing_mut().item_spacing.x = 0.0;
                // Console (S21) satellite — top-left
                peer_section(ui, "Console (S21)", theme::ACCENT_BLUE, w_sat, MIN_TOP_HEIGHT, false, console_status, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("IP:");
                        let mismatch = console_ip_mismatch(setup);
                        let show_warn_icon = mismatch && !setup.console_ip_warning_dismissed;
                        // Reserve 24 px on the right of the edit for the
                        // warning icon button. Keeping it constant avoids
                        // the edit width jumping when the warning appears
                        // or is dismissed.
                        let edit_w = w_sat - 100.0 - 24.0;
                        let resp = theme::padded_text_edit(
                            ui,
                            &mut setup.console_ip,
                            edit_w,
                            !is_connected,
                            "",
                        )
                        .on_hover_text("S21 console IP address.");
                        if resp.changed() {
                            // Operator edited the IP — let the warning
                            // re-evaluate against the new value.
                            setup.console_ip_warning_dismissed = false;
                        }
                        if show_warn_icon {
                            let warn_btn = egui::Button::new(
                                egui::RichText::new("⚠")
                                    .color(theme::ACCENT_RED)
                                    .strong(),
                            )
                            .frame(false)
                            .min_size(egui::Vec2::new(20.0, 20.0));
                            if ui
                                .add(warn_btn)
                                .on_hover_text(
                                    "Console IP isn't on the selected NIC's network \
                                     (first two octets differ — unreachable even with /16). \
                                     Click to dismiss.",
                                )
                                .clicked()
                            {
                                setup.console_ip_warning_dismissed = true;
                            }
                        }
                    });
                    ui.add_space(6.0);

                    egui::Grid::new("console_flow_grid")
                        .num_columns(3)
                        .spacing([6.0, 6.0])
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("S21").strong().color(theme::ACCENT_BLUE));
                            ui.label("");
                            ui.label(egui::RichText::new("Server").strong().color(theme::ACCENT_GREEN));
                            ui.end_row();

                            port_edit_enabled(
                                ui,
                                &mut setup.console_port,
                                !is_connected,
                                "",
                                "Console port — daemon sends GP OSC here.",
                            );
                            ui.label(egui::RichText::new("←").color(theme::TEXT_SECONDARY));
                            ui.label(egui::RichText::new("Tx").color(theme::TEXT_SECONDARY));
                            ui.end_row();

                            ui.label(egui::RichText::new("Tx").color(theme::TEXT_SECONDARY));
                            ui.label(egui::RichText::new("→").color(theme::TEXT_SECONDARY));
                            port_edit_enabled(
                                ui,
                                &mut setup.local_port,
                                !is_connected,
                                "",
                                "Local port the daemon listens on for GP OSC from the console.",
                            );
                            ui.end_row();

                            // iPad rows are always rendered so the grid emits
                            // the same height in every Connection Mode; in
                            // Mode 1 they're invisible (and thus disabled, by
                            // egui semantics) so identity + channel config
                            // below stay at a fixed Y-coordinate.
                            port_edit_visible(
                                ui,
                                &mut setup.ipad_console_port,
                                uses_ipad,
                                !is_connected,
                                "Console iPad-protocol port — daemon sends here.",
                            );
                            ui.add_visible(
                                uses_ipad,
                                egui::Label::new(egui::RichText::new("←").color(theme::TEXT_SECONDARY)),
                            );
                            ui.add_visible(
                                uses_ipad,
                                egui::Label::new(egui::RichText::new("Tx").color(theme::TEXT_SECONDARY)),
                            );
                            ui.end_row();

                            ui.add_visible(
                                uses_ipad,
                                egui::Label::new(egui::RichText::new("Tx").color(theme::TEXT_SECONDARY)),
                            );
                            ui.add_visible(
                                uses_ipad,
                                egui::Label::new(egui::RichText::new("→").color(theme::TEXT_SECONDARY)),
                            );
                            port_edit_visible(
                                ui,
                                &mut setup.ipad_local_port,
                                uses_ipad,
                                !is_connected,
                                "Local port the daemon listens on for iPad-protocol traffic.",
                            );
                            ui.end_row();
                        });

                    // Always render a status row so its height matches across
                    // all three Connection Modes. Mode 2 shows the actual iPad
                    // protocol status; Modes 1 and 3 use a transparent
                    // placeholder of the exact same allocation pattern, so the
                    // identity grid + channel-config badges below stay at a
                    // fixed Y-coordinate.
                    ui.horizontal(|ui| {
                        let (color, text) = if uses_ipad && !is_proxy {
                            if setup.ipad_connected {
                                (theme::COLOR_CONNECTED, "iPad protocol connected")
                            } else {
                                (theme::COLOR_DISCONNECTED, "iPad protocol disconnected")
                            }
                        } else {
                            (egui::Color32::TRANSPARENT, "iPad protocol")
                        };
                        theme::status_dot(ui, color);
                        ui.colored_label(color, text);
                    });

                    // ── Console identity + channel configuration ──
                    if let Ok(st) = state.try_read() {
                        let cfg = &st.config;
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);

                        if !cfg.console_name.is_empty() || !cfg.console_serial.is_empty() {
                            egui::Grid::new("console_identity_grid")
                                .num_columns(2)
                                .spacing([8.0, 2.0])
                                .show(ui, |ui| {
                                    if !cfg.console_name.is_empty() {
                                        ui.label(egui::RichText::new("Console:").color(theme::TEXT_SECONDARY).small());
                                        ui.label(egui::RichText::new(&cfg.console_name).strong().small());
                                        ui.end_row();
                                    }
                                    if !cfg.console_serial.is_empty() {
                                        ui.label(egui::RichText::new("Serial:").color(theme::TEXT_SECONDARY).small());
                                        ui.label(egui::RichText::new(&cfg.console_serial).small());
                                        ui.end_row();
                                    }
                                    if let Some(ref session) = cfg.session_filename {
                                        ui.label(egui::RichText::new("Session:").color(theme::TEXT_SECONDARY).small());
                                        ui.label(egui::RichText::new(session).small());
                                        ui.end_row();
                                    }
                                });
                            ui.add_space(6.0);
                        }

                        ui.label(egui::RichText::new("Channel Configuration").color(theme::TEXT_SECONDARY).small());
                        ui.add_space(2.0);
                        // 5 channel-count pills sized to fit 3 per row
                        // with the panel's item_spacing as the gutter,
                        // then a Params pill on its own right-justified
                        // row at half the panel width (room for 5-digit
                        // values up to ~12000).
                        let panel_w = ui.available_width();
                        let item_spacing = ui.spacing().item_spacing.x;
                        let badge_w = ((panel_w - 2.0 * item_spacing) / 3.0).max(40.0);
                        let badges = [
                            (format!("Inputs: {}", cfg.input_channel_count), theme::CH_INPUT),
                            (format!("Aux: {}", cfg.aux_output_count), theme::CH_AUX),
                            (format!("Groups: {}", cfg.group_output_count), theme::CH_GROUP),
                            (format!("Matrix: {}", cfg.matrix_output_count), theme::CH_MATRIX),
                            (format!("CGs: {}", cfg.control_group_count), theme::CH_CG),
                        ];
                        for chunk in badges.chunks(3) {
                            ui.horizontal(|ui| {
                                for (text, color) in chunk {
                                    theme::colored_badge_sized(ui, text, *color, badge_w);
                                }
                            });
                            ui.add_space(2.0);
                        }
                        // Params on its own row, right-justified.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            theme::colored_badge_sized(
                                ui,
                                &format!("Params: {}", st.parameter_count()),
                                theme::ACCENT_BLUE,
                                panel_w * 0.5,
                            );
                        });
                    }
                });

                ui.add_space(gap);

                // Server (This Computer) — central hub
                peer_section(ui, "This Computer", theme::ACCENT_GREEN, w_hub, MIN_TOP_HEIGHT, true, None, |ui| {
                    // Each main labeled row is its own 2-column grid that shares
                    // a fixed `LABEL_COL_W`, so labels line up across sections
                    // and widgets all start at the same X. Sub-rows
                    // (continuation lines under a label) render *outside* the
                    // grid with a matching `add_space` indent — avoids the
                    // "missing label" look of empty col-0 cells and gives the
                    // sub-row content the full hub width to grow into.
                    const LABEL_COL_W: f32 = 140.0;
                    const GRID_SPACING_X: f32 = 14.0;
                    let subrow_indent = LABEL_COL_W + GRID_SPACING_X;
                    let server_grid = |id: &'static str| {
                        egui::Grid::new(id)
                            .num_columns(2)
                            .spacing([GRID_SPACING_X, 8.0])
                            .min_col_width(LABEL_COL_W)
                    };
                    // Single shared button width used by every button
                    // row in this panel: Display-Mode toggles, Network
                    // combo, Connection-Mode toggles, Parameter coverage,
                    // Open…, Save / Save As… / New. With 3 buttons +
                    // 2 gaps spanning (w_hub − subrow_indent), every
                    // row reaches the same right edge — visually
                    // regular at every window size.
                    let item_spacing_x = ui.spacing().item_spacing.x;
                    let action_btn_w = ((w_hub - subrow_indent - 2.0 * item_spacing_x) / 3.0)
                        .max(50.0);
                    let action_row_w = 3.0 * action_btn_w + 2.0 * item_spacing_x;

                    // ── Display Mode ──
                    server_grid("server_display_grid").show(ui, |ui| {
                        ui.label("Display Mode:");
                        ui.horizontal(|ui| {
                            // All 3 toggles share `action_btn_w` so the
                            // row spans `action_row_w` — same width as
                            // every other button row in this panel.
                            ui.spacing_mut().button_padding = egui::Vec2::new(10.0, 6.0);
                            for mode in UiMode::ALL {
                                let is_active = setup.ui_mode == mode;
                                let fill = if is_active { theme::ACCENT_BLUE } else { theme::BG_ELEVATED };
                                let text_color = if is_active { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY };
                                let btn = egui::Button::new(
                                    egui::RichText::new(mode.label()).color(text_color),
                                )
                                .fill(fill)
                                .corner_radius(4.0)
                                .min_size(egui::Vec2::new(action_btn_w, 28.0));
                                if ui
                                    .add_sized([action_btn_w, 28.0], btn)
                                    .clicked()
                                    && setup.ui_mode != mode
                                {
                                    setup.ui_mode = mode;
                                    save_app_preferences(setup);
                                }
                            }
                        });
                        ui.end_row();
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(subrow_indent);
                        let diag_resp = ui
                            .checkbox(&mut setup.show_diagnostics, "Show diagnostic tabs")
                            .on_hover_text("Adds OSC Log and Inspector tabs to the main tab bar.");
                        if diag_resp.changed() {
                            save_app_preferences(setup);
                        }
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ── Network + bind-IP read-out ──
                    server_grid("server_network_grid").show(ui, |ui| {
                        ui.label("Network:");
                        ui.add_enabled_ui(!is_connected, |ui| {
                            let interfaces = net_interfaces::list_interfaces();
                            let current_label = if setup.local_ip.is_empty() {
                                "All interfaces (0.0.0.0)".to_string()
                            } else {
                                interfaces
                                    .iter()
                                    .find(|i| i.ip.to_string() == setup.local_ip)
                                    .map(|i| i.label())
                                    .unwrap_or_else(|| setup.local_ip.clone())
                            };
                            egui::ComboBox::from_id_salt("nic_select")
                                .selected_text(&current_label)
                                .width(action_row_w)
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_label(
                                            setup.local_ip.is_empty(),
                                            "All interfaces (0.0.0.0)",
                                        )
                                        .clicked()
                                    {
                                        setup.local_ip.clear();
                                        setup.interface_name = None;
                                        // Selection changed — let the
                                        // mismatch warning re-evaluate.
                                        setup.console_ip_warning_dismissed = false;
                                    }
                                    for iface in &interfaces {
                                        let label = iface.label();
                                        let selected = setup.local_ip == iface.ip.to_string();
                                        if ui.selectable_label(selected, &label).clicked() {
                                            setup.local_ip = iface.ip.to_string();
                                            setup.interface_name = Some(iface.name.clone());
                                            // If the operator hasn't
                                            // customised the console IP
                                            // yet, rewrite the first
                                            // three octets to match
                                            // the chosen NIC.
                                            if setup.console_ip == DEFAULT_CONSOLE_IP {
                                                setup.console_ip = align_first_three_octets(
                                                    &setup.console_ip,
                                                    &setup.local_ip,
                                                );
                                            }
                                            setup.console_ip_warning_dismissed = false;
                                        }
                                    }
                                });
                        });
                        ui.end_row();
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ── Connection Mode ──
                    server_grid("server_conn_grid").show(ui, |ui| {
                        ui.label("Connection Mode:");
                        ui.horizontal(|ui| {
                            ui.spacing_mut().button_padding = egui::Vec2::new(10.0, 6.0);
                            for mode in [OperatingMode::Mode1, OperatingMode::Mode2, OperatingMode::Mode3] {
                                let is_active = setup.operating_mode == mode;
                                let fill = if is_active { theme::ACCENT_BLUE } else { theme::BG_ELEVATED };
                                let text_color = if is_active { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY };
                                let btn = egui::Button::new(
                                    egui::RichText::new(mode.short_label()).color(text_color),
                                )
                                .fill(fill)
                                .corner_radius(4.0)
                                .min_size(egui::Vec2::new(action_btn_w, 28.0));
                                if ui
                                    .add_enabled_ui(!is_connected, |ui| {
                                        ui.add_sized([action_btn_w, 28.0], btn)
                                    })
                                    .inner
                                    .clicked()
                                {
                                    setup.operating_mode = mode;
                                }
                            }
                        });
                        ui.end_row();
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(subrow_indent);
                        // Parameter coverage spans the full action row
                        // width — keeps the central panel's button
                        // grid edges aligned even though this is one
                        // button instead of three.
                        if ui
                            .add(theme::action_button(
                                "Parameter coverage…",
                                theme::BG_ELEVATED,
                                egui::Vec2::new(action_row_w, 28.0),
                            ))
                            .clicked()
                        {
                            setup.show_coverage_popup = true;
                        }
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);

                    // ── Show File ──
                    server_grid("server_show_file_grid").show(ui, |ui| {
                        ui.label("Show file:");
                        ui.horizontal(|ui| {
                            // Display only the filename with a leading
                            // "…/" so a long path doesn't blow out the
                            // panel. The full path stays in
                            // `setup.show_file_path` (source of truth);
                            // we render a non-interactive TextEdit so
                            // the look matches an editable field, with
                            // the full path on hover.
                            let mut display = truncate_show_path(&setup.show_file_path);
                            // Field + Open span the same `action_row_w`
                            // as the Save / Save As… / New row below,
                            // so the grid lines up: edit + gap + Open
                            // = 3*action_btn_w + 2*gaps  →
                            // edit = 2*action_btn_w + 1*gap.
                            let edit_w = 2.0 * action_btn_w + item_spacing_x;
                            let resp = ui.add_sized(
                                [edit_w, theme::TEXT_EDIT_HEIGHT],
                                egui::TextEdit::singleline(&mut display)
                                    .margin(theme::TEXT_EDIT_MARGIN)
                                    .interactive(false),
                            );
                            if !setup.show_file_path.is_empty() {
                                resp.on_hover_text(setup.show_file_path.clone());
                            }
                            // Compact button padding here so the Open
                            // button renders at the same height as the
                            // text field above (default button_padding
                            // of 12x8 makes it ~30 px tall, breaking
                            // the row's vertical centring).
                            ui.scope(|ui| {
                                ui.spacing_mut().button_padding =
                                    egui::Vec2::new(8.0, 4.0);
                                if ui
                                    .add(theme::action_button(
                                        "Open…",
                                        theme::BG_ELEVATED,
                                        egui::Vec2::new(action_btn_w, theme::TEXT_EDIT_HEIGHT),
                                    ))
                                    .on_hover_text(
                                        "Pick a show file and load it. Save As… to save to \
                                         a new path.",
                                    )
                                    .clicked()
                                {
                                    if let Some(path) = rfd::FileDialog::new()
                                        .add_filter("Show files", &["s21show", "json"])
                                        .add_filter("All files", &["*"])
                                        .pick_file()
                                    {
                                        setup.show_file_path = path.display().to_string();
                                        load_show_file(
                                            setup, state, cue_manager, macro_manager,
                                            monitor_manager, palette_manager, gang_manager,
                                            pan_link_bindings, stream_deck_config, connected,
                                            runtime, ui_tx,
                                        );
                                    }
                                }
                            });
                        });
                        ui.end_row();
                    });
                    ui.add_space(4.0);
                    // 3 buttons (Save / Save As… / New) reusing the
                    // shared `action_btn_w` so the row mirrors every
                    // other button row in this panel. Load was
                    // dropped — Open… picks AND loads in one step.
                    ui.horizontal(|ui| {
                        ui.add_space(subrow_indent);
                        let save_btn = theme::action_button(
                            "Save",
                            theme::ACCENT_GREEN,
                            egui::Vec2::new(action_btn_w, 28.0),
                        );
                        if ui
                            .add(save_btn)
                            .on_hover_text(
                                "Save to the path shown above. If empty, prompts for a \
                                 location.",
                            )
                            .clicked()
                        {
                            if setup.show_file_path.is_empty()
                                && let Some(path) = rfd::FileDialog::new()
                                    .add_filter("Show files", &["s21show", "json"])
                                    .set_file_name("show.s21show")
                                    .save_file()
                            {
                                setup.show_file_path = path.display().to_string();
                            }
                            if !setup.show_file_path.is_empty() {
                                ensure_show_file_extension(&mut setup.show_file_path);
                                save_show_file(
                                    setup, state, cue_manager, macro_manager, monitor_manager,
                                    palette_manager, gang_manager, pan_link_bindings,
                                    stream_deck_config,
                                    auto_update_on_recall.load(Ordering::Relaxed),
                                    console_snapshot_follow.load(Ordering::Relaxed),
                                    console_recall.clone(),
                                    runtime, ui_tx,
                                );
                            }
                        }

                        let save_as_btn = theme::action_button(
                            "Save As…",
                            theme::ACCENT_GREEN,
                            egui::Vec2::new(action_btn_w, 28.0),
                        );
                        if ui
                            .add(save_as_btn)
                            .on_hover_text(
                                "Pick a new location and save there. Useful when the \
                                 path field already points at a different file.",
                            )
                            .clicked()
                        {
                            // Pre-seed the dialog with the current path's
                            // directory + filename when available; otherwise
                            // fall back to the default `show.s21show`.
                            let mut dlg = rfd::FileDialog::new()
                                .add_filter("Show files", &["s21show", "json"]);
                            if !setup.show_file_path.is_empty() {
                                let p = std::path::Path::new(&setup.show_file_path);
                                if let Some(dir) = p.parent() {
                                    dlg = dlg.set_directory(dir);
                                }
                                if let Some(name) = p.file_name() {
                                    dlg = dlg.set_file_name(name.to_string_lossy());
                                }
                            } else {
                                dlg = dlg.set_file_name("show.s21show");
                            }
                            if let Some(path) = dlg.save_file() {
                                setup.show_file_path = path.display().to_string();
                                ensure_show_file_extension(&mut setup.show_file_path);
                                save_show_file(
                                    setup, state, cue_manager, macro_manager, monitor_manager,
                                    palette_manager, gang_manager, pan_link_bindings,
                                    stream_deck_config,
                                    auto_update_on_recall.load(Ordering::Relaxed),
                                    console_snapshot_follow.load(Ordering::Relaxed),
                                    console_recall.clone(),
                                    runtime, ui_tx,
                                );
                            }
                        }

                        let new_btn = theme::action_button(
                            "New",
                            theme::ACCENT_ORANGE,
                            egui::Vec2::new(action_btn_w, 28.0),
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
                                drop(mmgr);
                                let mut pmgr = pmgr_arc.write().await;
                                pmgr.palettes.clear();
                            });
                            setup.show_file_path.clear();
                            setup.status_message = Some("New show created".into());
                        }
                    });
                });

                ui.add_space(gap);

                // iPad satellite — slot stays a constant w_sat × MIN_TOP_HEIGHT
                // box across all modes (so centering math doesn't change), but
                // the visible content varies:
                //   Mode 1 → greyed-out frame (the console handles iPad-protocol directly).
                //   Mode 2 → frame with explanatory text only (no IP / ports / status).
                //   Mode 3 → full IP + Tx/Rx + status content.
                match setup.operating_mode {
                    OperatingMode::Mode1 => {
                        peer_section(ui, "iPad", theme::TEXT_DISABLED, w_sat, MIN_TOP_HEIGHT, false, None, |ui| {
                            ui.label(
                                egui::RichText::new(
                                    "Mode 1 is GP OSC only — the iPad protocol is not used. \
                                     Switch to Mode 2 or Mode 3 to expose iPad-protocol features.",
                                )
                                .color(theme::TEXT_DISABLED)
                                .small(),
                            );
                        });
                    }
                    OperatingMode::Mode2 => {
                        peer_section(ui, "iPad", theme::ACCENT_ORANGE, w_sat, MIN_TOP_HEIGHT, false, None, |ui| {
                            ui.label(
                                egui::RichText::new(
                                    "In Mode 2 the server replaces the iPad on the iPad-protocol \
                                     socket — no separate iPad device can be connected. To run \
                                     the official DiGiCo iPad app at the same time, switch to \
                                     Mode 3 (iPad Proxy).",
                                )
                                .color(theme::TEXT_SECONDARY)
                                .small(),
                            );
                        });
                    }
                    OperatingMode::Mode3 => {
                        peer_section(ui, "iPad", theme::ACCENT_ORANGE, w_sat, MIN_TOP_HEIGHT, false, ipad_status, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("IP:");
                                theme::padded_text_edit(
                                    ui,
                                    &mut setup.ipad_ip,
                                    w_sat - 120.0,
                                    !is_connected,
                                    "auto-detect",
                                )
                                .on_hover_text(
                                    "iPad device IP — leave blank to auto-detect from first inbound packet.",
                                );
                            });
                            ui.add_space(6.0);

                            egui::Grid::new("ipad_flow_grid")
                                .num_columns(3)
                                .spacing([6.0, 6.0])
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new("Server").strong().color(theme::ACCENT_GREEN));
                                    ui.label("");
                                    ui.label(egui::RichText::new("iPad").strong().color(theme::ACCENT_ORANGE));
                                    ui.end_row();

                                    ui.label(egui::RichText::new("Tx").color(theme::TEXT_SECONDARY));
                                    ui.label(egui::RichText::new("→").color(theme::TEXT_SECONDARY));
                                    port_edit_enabled(
                                        ui,
                                        &mut setup.ipad_reply_port,
                                        !is_connected,
                                        "",
                                        "Port on the iPad — daemon sends to it here.",
                                    );
                                    ui.end_row();

                                    port_edit_enabled(
                                        ui,
                                        &mut setup.ipad_listen_port,
                                        !is_connected,
                                        "",
                                        "Local port the daemon listens on for iPad→daemon traffic.",
                                    );
                                    ui.label(egui::RichText::new("←").color(theme::TEXT_SECONDARY));
                                    ui.label(egui::RichText::new("Tx").color(theme::TEXT_SECONDARY));
                                    ui.end_row();
                                });
                        });
                    }
                }
            });

            // Bottom-row activity follows the Display Mode tab rules:
            //   LiveMusic hides Snapshots → no cueing → grey out QLab.
            //   Theatre hides Monitor → no mobile clients → grey out Monitor.
            //   Full shows both fully active.
            // Both boxes always render so the diagram stays balanced; the
            // inactive one shows as a greyed-out shell rather than vanishing.
            let qlab_active = setup.ui_mode != UiMode::LiveMusic;
            let monitor_active = setup.ui_mode != UiMode::Theatre;

            ui.add_space(SECT_GAP);

            // ── Bottom row: QLab (offset left) | Monitor (offset right) ──
            // Offsets land each box in the gap between the top-row sections.
            ui.horizontal(|ui| {
                // Same zero-item-spacing trick as the top row so the offset
                // arithmetic below matches the actual rendered positions.
                ui.spacing_mut().item_spacing.x = 0.0;
                // Bottom row anchors QLab / Monitor at the *boundaries* of
                // the central Server hub:
                //   QLab.center    = Server.left_edge
                //   Monitor.center = Server.right_edge
                // Top-row x layout:  Console=[0, S], gap, Server=[S+g, S+g+H],
                //                    gap, iPad=[S+2g+H, 2S+2g+H].
                // QLab.left = (S+g) − S/2 = S/2 + g
                // Monitor.left − QLab.right = (S/2 + g + H) − (3S/2 + g) = H − S
                // Rendered widths include inner_margin + stroke so they
                // match what egui actually allocates.
                let sat_rendered = w_sat + SAT_FRAME_PAD + FRAME_STROKE_PAD;
                let hub_rendered = w_hub + HUB_FRAME_PAD + FRAME_STROKE_PAD;
                let qlab_lead = sat_rendered / 2.0 + gap;
                let inner_gap = (hub_rendered - sat_rendered).max(0.0);

                ui.add_space(qlab_lead);

                if qlab_active {
                // QLab satellite — bottom-left
                peer_section(ui, "QLab", theme::ACCENT_AMBER, w_sat, MIN_BOT_HEIGHT, false, None, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("IP:");
                        theme::padded_text_edit(
                            ui,
                            &mut setup.qlab_ip,
                            w_sat - 100.0,
                            !is_connected,
                            "127.0.0.1",
                        )
                        .on_hover_text("QLab host — usually 127.0.0.1 if QLab runs on this machine.");
                    });
                    ui.add_space(6.0);

                    egui::Grid::new("qlab_flow_grid")
                        .num_columns(3)
                        .spacing([6.0, 6.0])
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("Server").strong().color(theme::ACCENT_GREEN));
                            ui.label("");
                            ui.label(egui::RichText::new("QLab").strong().color(theme::ACCENT_AMBER));
                            ui.end_row();

                            port_edit_enabled(
                                ui,
                                &mut setup.trigger_port,
                                !is_connected,
                                "",
                                "Local port the daemon listens on for cue triggers from QLab.",
                            );
                            ui.label(egui::RichText::new("←").color(theme::TEXT_SECONDARY));
                            ui.label(egui::RichText::new("Tx").color(theme::TEXT_SECONDARY));
                            ui.end_row();

                            ui.label(egui::RichText::new("Tx").color(theme::TEXT_SECONDARY));
                            ui.label(egui::RichText::new("→").color(theme::TEXT_SECONDARY));
                            port_edit_enabled(
                                ui,
                                &mut setup.qlab_port,
                                !is_connected,
                                "53000",
                                "QLab's OSC listen port — daemon sends cue-build commands here.",
                            );
                            ui.end_row();
                        });
                });
                } else {
                    peer_section(ui, "QLab", theme::TEXT_DISABLED, w_sat, MIN_BOT_HEIGHT, false, None, |ui| {
                        ui.label(
                            egui::RichText::new(
                                "Live Music mode hides Snapshots, so QLab cue triggering is \
                                 disabled. Switch to Full or Theatre mode to enable QLab.",
                            )
                            .color(theme::TEXT_DISABLED)
                            .small(),
                        );
                    });
                }

                // Inner gap: span the Server's extra width plus gaps so
                // Monitor lands in the Server↔iPad gap.
                ui.add_space(inner_gap);

                if monitor_active {
                // Monitor satellite — bottom-right
                peer_section(ui, "Monitor", theme::TEXT_SECONDARY, w_sat, MIN_BOT_HEIGHT, false, None, |ui| {
                    egui::Grid::new("monitor_flow_grid")
                        .num_columns(3)
                        .spacing([6.0, 6.0])
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("Server").strong().color(theme::ACCENT_GREEN));
                            ui.label("");
                            ui.label(egui::RichText::new("Mobile / Web").strong().color(theme::TEXT_PRIMARY));
                            ui.end_row();

                            // Editable port goes directly in the flow grid —
                            // no separate "Port:" row above, since the value
                            // is the only knob this satellite exposes.
                            port_edit_enabled(
                                ui,
                                &mut setup.monitor_port,
                                !is_connected,
                                "off",
                                "Local port for the Flutter monitor app — leave blank to disable.",
                            );
                            ui.label(egui::RichText::new("↔").color(theme::TEXT_SECONDARY));
                            ui.label(
                                egui::RichText::new("any LAN client").color(theme::TEXT_SECONDARY),
                            );
                            ui.end_row();
                        });
                });
                } else {
                    peer_section(ui, "Monitor", theme::TEXT_DISABLED, w_sat, MIN_BOT_HEIGHT, false, None, |ui| {
                        ui.label(
                            egui::RichText::new(
                                "Theatre mode targets a single operator on this machine, so the \
                                 mobile / web monitor is disabled. Switch to Full or Live Music \
                                 mode to enable it.",
                            )
                            .color(theme::TEXT_DISABLED)
                            .small(),
                        );
                    });
                }
            });

            // Status message — one-line warning when the daemon couldn't bring
            // a port up, etc. The Connect button itself moved to the top-strip.
            if let Some(msg) = &setup.status_message {
                ui.add_space(8.0);
                ui.colored_label(theme::TEXT_WARNING, msg);
            }
        });

        // ── Parameter-coverage popup ──
        // Centered modal-ish window so toggling it never reflows the diagram.
        // Closes via the title-bar X, the Escape key, or any click outside
        // the window's rect — the conventional dismiss-modal pattern.
        if setup.show_coverage_popup {
            let mut open = setup.show_coverage_popup;
            let popup = egui::Window::new("Parameter coverage for this mode")
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    let uses_ipad = setup.operating_mode.uses_ipad_protocol();
                    ui.label(
                        egui::RichText::new(if uses_ipad {
                            "Mode uses GP OSC + iPad protocol — almost everything is reachable."
                        } else {
                            "Mode 1 is GP OSC only — several parameters require switching to Mode 2 or 3."
                        })
                        .color(theme::TEXT_SECONDARY),
                    );
                    ui.add_space(6.0);
                    egui::Grid::new("protocol_coverage_grid_popup")
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
            setup.show_coverage_popup = open;

            // Click-outside-to-close: only treat clicks that land outside the
            // window's full rect as a dismiss. Skip frames where the
            // click was inside the window itself (interactions with grid
            // contents, scrolling, etc.) so the popup doesn't fight its own
            // event handling.
            if setup.show_coverage_popup {
                if let Some(popup) = popup {
                    let popup_rect = popup.response.rect;
                    let clicked_outside = ui.ctx().input(|i| {
                        i.pointer.any_click()
                            && !i
                                .pointer
                                .interact_pos()
                                .map(|p| popup_rect.contains(p))
                                .unwrap_or(false)
                    });
                    if clicked_outside {
                        setup.show_coverage_popup = false;
                    }
                }
                if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
                    setup.show_coverage_popup = false;
                }
            }
        }
    });
}

/// Disconnect from the console: cancel all tasks and reset state.
pub(crate) fn do_disconnect(
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
pub(crate) fn start_connection(
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
    last_received: &Arc<RwLock<Option<crate::model::parameter::ParameterAddress>>>,
    pending_engines: &Arc<std::sync::Mutex<Option<crate::ui::PendingEngines>>>,
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
    let last_recv = last_received.clone();
    let conn_flag = connected.clone();
    let tx = ui_tx.clone();
    let ctx = egui_ctx.clone();
    let console_ip = setup.console_ip.clone();
    let send_pace_us = setup.send_pace_us;
    let monitor_allow_cidrs = setup.monitor_allow_cidrs.clone();
    let trigger_allow_cidrs = setup.trigger_allow_cidrs.clone();
    let log = osc_log.clone();
    let pending = pending_engines.clone();
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
            gang_mgr.clone(),
        )));

        let daemon = crate::console::connection::DaemonState {
            state: st.clone(),
            macro_manager: macro_mgr.clone(),
            gang_engine: gang_engine.clone(),
            gang_manager: gang_mgr,
            pan_link_engine: pan_link_engine.clone(),
            dirty_tracker: dirty.clone(),
            offline_mode: offline.clone(),
            last_received: last_recv.clone(),
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

        // Captured iPad sender to hand back to the App alongside the engines.
        let mut app_ipad_sender: Option<IpadSender> = None;

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
                                .set_ipad_sender(Some(ipad_sender.clone()));
                            app_ipad_sender = Some(ipad_sender);
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
                                .set_ipad_sender(Some(ipad_sender.clone()));
                            app_ipad_sender = Some(ipad_sender);
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

        // Construct MacroEngine here, BEFORE the trigger-listener branch, so
        // that even when the trigger port can't be bound (already in use by
        // another instance, blocked by the OS, etc.) the UI Run-macro button
        // still has an engine to work with. Previously this lived inside the
        // trigger-listener `Ok` arm and was lost on bind failure, leaving
        // `App.macro_engine` permanently `None`.
        let mut macro_eng =
            MacroEngine::new(st.clone(), manager.sender(), macro_mgr.clone(), tx.clone());
        macro_eng.set_dirty_tracker(dirty.clone());
        let macro_eng = Arc::new(macro_eng);

        // Hand the freshly-built engines back to the App so UI buttons can
        // use them. This is the missing wire-up: the App fields used to be
        // initialised to `None` and never populated, so Run / Recall buttons
        // looked enabled but silently no-oped at runtime.
        if let Ok(mut slot) = pending.lock() {
            *slot = Some(crate::ui::PendingEngines {
                sender: manager.sender(),
                snapshot_engine: engine.clone(),
                macro_engine: macro_eng.clone(),
                ipad_sender: app_ipad_sender.clone(),
            });
        }

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
    stream_deck_config: &Arc<RwLock<crate::model::streamdeck::StreamDeckConfig>>,
    connected: &Arc<AtomicBool>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    // If no path, open a file dialog
    if setup.show_file_path.is_empty() {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Show files", &["s21show", "json"])
            .add_filter("All files", &["*"])
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
    let sd_config = stream_deck_config.clone();
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

                // Restore Stream Deck config (device + per-button maps).
                // The engine will pick up the new state on the next
                // frame via the UI's Connect/Disconnect logic.
                {
                    let mut sd = sd_config.write().await;
                    *sd = show.stream_deck;
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
    stream_deck_config: &Arc<RwLock<crate::model::streamdeck::StreamDeckConfig>>,
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
    let sd_config = stream_deck_config.clone();
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
        let sd = sd_config.read().await;

        let show = ShowFile {
            version: 15,
            console_config: state_guard.config.clone(),
            connection: conn_settings,
            scope_templates: mgr.scope_templates.values().cloned().collect(),
            snapshots: mgr.snapshots.values().cloned().collect(),
            cue_list: mgr.cue_list.clone(),
            macros: mmgr.macros.values().cloned().collect(),
            palettes: pmgr.palettes.values().cloned().collect(),
            monitor_clients: monmgr.clients.values().cloned().collect(),
            gang_groups: gmgr.groups.values().cloned().collect(),
            console_recall: console_recall.clone(),
            pan_link: pl.clone(),
            stream_deck: sd.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_two_octets_match_under_slash_16() {
        assert!(first_two_octets_match("192.168.1.1", "192.168.5.42"));
        assert!(first_two_octets_match("10.0.5.20", "10.0.99.1"));
        assert!(!first_two_octets_match("192.168.1.1", "10.0.0.1"));
        assert!(!first_two_octets_match("192.168.1.1", "192.169.1.1"));
        // Garbage / partial input → no match.
        assert!(!first_two_octets_match("not-an-ip", "192.168.1.1"));
        assert!(!first_two_octets_match("", "192.168.1.1"));
    }

    #[test]
    fn align_first_three_octets_preserves_last() {
        assert_eq!(
            align_first_three_octets("192.168.1.1", "10.0.5.20"),
            "10.0.5.1"
        );
        assert_eq!(
            align_first_three_octets("192.168.1.42", "172.16.99.1"),
            "172.16.99.42"
        );
        // No-op when source already matches.
        assert_eq!(
            align_first_three_octets("192.168.1.1", "192.168.1.10"),
            "192.168.1.1"
        );
        // Bad input → return target unchanged.
        assert_eq!(align_first_three_octets("garbage", "10.0.0.1"), "garbage");
        assert_eq!(
            align_first_three_octets("192.168.1.1", "garbage"),
            "192.168.1.1"
        );
    }

    #[test]
    fn ensure_show_file_extension_appends_when_missing() {
        let mut p = "/tmp/mygig".to_string();
        ensure_show_file_extension(&mut p);
        assert_eq!(p, "/tmp/mygig.s21show");
    }

    #[test]
    fn ensure_show_file_extension_keeps_s21show() {
        let mut p = "/tmp/mygig.s21show".to_string();
        ensure_show_file_extension(&mut p);
        assert_eq!(p, "/tmp/mygig.s21show");
    }

    #[test]
    fn ensure_show_file_extension_keeps_legacy_json() {
        // Re-saving over a legacy JSON file keeps the .json extension
        // — no surprise renames.
        let mut p = "/tmp/oldshow.json".to_string();
        ensure_show_file_extension(&mut p);
        assert_eq!(p, "/tmp/oldshow.json");
    }

    #[test]
    fn ensure_show_file_extension_handles_uppercase() {
        let mut p = "/tmp/Show.S21SHOW".to_string();
        ensure_show_file_extension(&mut p);
        assert_eq!(p, "/tmp/Show.S21SHOW");
    }

    #[test]
    fn ensure_show_file_extension_empty_is_noop() {
        let mut p = String::new();
        ensure_show_file_extension(&mut p);
        assert!(p.is_empty());
    }

    #[test]
    fn ensure_show_file_extension_unknown_extension_appends() {
        // A weird extension (.txt) gets `.s21show` appended on top.
        let mut p = "/tmp/notes.txt".to_string();
        ensure_show_file_extension(&mut p);
        assert_eq!(p, "/tmp/notes.txt.s21show");
    }
}
