//! Sidecar tab — hardware MIDI faders/encoders bound to console
//! parameters or external OSC targets.
//!
//! Layout: left column = device card (ON/OFF rocker + status dot, the
//! display protocol, and an input/output pair per port slot) and the learn
//! card (staged wizard); right column = the binding list with per-row
//! editing.
//!
//! Shared-state discipline matches the other tabs: `try_read` a clone
//! of the config each frame, collect edits as [`Action`]s while
//! drawing, then apply them through `runtime.spawn` writes so the UI
//! thread never blocks on the tokio locks.

use std::sync::Arc;

use eframe::egui;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::console::sidecar_engine::{SidecarMidiEngine, SidecarMidiState};
use crate::console::sidecar_learn::{LearnPhase, LearnShared};
use crate::console::sidecar_service::SvcCmd;
use crate::model::parameter::{ParameterAddress, ParameterValue};
use crate::model::sidecar::{
    BindingTarget, ControlMode, ControlSelector, RelativeMode, SIDECAR_PORTS, SidecarBinding,
    SidecarConfig, SurfaceDisplay, TAPER_LIMIT, Taper, default_taper_for, is_valid_console_target,
    mcu_default_touch_note,
};
use crate::model::state::ConsoleState;
use crate::ui::help::{HelpKey, help};
use crate::ui::setup_tab::SetupTabState;
use crate::ui::theme;

/// Per-frame UI state for the Sidecar tab. Runtime-only.
#[derive(Default)]
pub struct SidecarTabState {
    /// Learn wizard phase.
    pub learn: LearnPhase,
    /// `last_received` snapshot taken when arming the console capture,
    /// so we react only to a *new* touch, not the last stale one.
    learn_baseline: Option<ParameterAddress>,
    /// When re-learning hardware for an existing binding, its id — the
    /// confirm updates that binding instead of creating a new one.
    relearn_id: Option<Uuid>,
    /// Scratch fields for the external-OSC target form.
    raw_host: String,
    raw_port: String,
    raw_path: String,
    /// Expanded (editing) binding row.
    expanded: Option<Uuid>,
    /// Transient status line.
    pub status_message: Option<String>,
}

/// Edits collected while drawing, applied after via `runtime.spawn`.
enum Action {
    SetEnabled(bool),
    SetDisplay(SurfaceDisplay),
    /// Insert a new binding; `replace` removes bindings claiming the
    /// same control (duplicate replace) or the given id (re-learn).
    AddBinding {
        binding: SidecarBinding,
        replace: Option<Uuid>,
    },
    UpdateBinding(SidecarBinding),
    DeleteBinding(Uuid),
    Sync,
}

#[allow(clippy::too_many_arguments)]
pub fn draw_sidecar_tab(
    ui: &mut egui::Ui,
    tab: &mut SidecarTabState,
    setup: &mut SetupTabState,
    config: &Arc<RwLock<SidecarConfig>>,
    midi: &Arc<SidecarMidiEngine>,
    state: &Arc<RwLock<ConsoleState>>,
    last_received: &Arc<RwLock<Option<ParameterAddress>>>,
    learn_shared: &Arc<std::sync::Mutex<LearnShared>>,
    svc_tx: &tokio::sync::mpsc::UnboundedSender<SvcCmd>,
    connected: bool,
    runtime: &tokio::runtime::Handle,
) {
    // Per-frame snapshot; on contention skip this frame (next repaints).
    let Ok(guard) = config.try_read() else {
        return;
    };
    let cfg = guard.clone();
    drop(guard);

    let mut actions: Vec<Action> = Vec::new();

    ui.columns(2, |cols| {
        cols[0].vertical(|ui| {
            draw_device_card(ui, setup, midi, &cfg, &mut actions);
            ui.add_space(8.0);
            draw_learn_card(
                ui,
                tab,
                &cfg,
                state,
                last_received,
                learn_shared,
                connected,
                &mut actions,
            );
            if let Some(msg) = &tab.status_message {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(msg.as_str()).color(theme::label_weak()));
            }
        });
        cols[1].vertical(|ui| {
            draw_bindings_card(ui, tab, &cfg, state, &mut actions);
        });
    });

    apply_actions(actions, config, svc_tx, runtime);
}

// ─── Device card ─────────────────────────────────────────────────────

fn draw_device_card(
    ui: &mut egui::Ui,
    setup: &mut SetupTabState,
    midi: &Arc<SidecarMidiEngine>,
    cfg: &SidecarConfig,
    actions: &mut Vec<Action>,
) {
    let snap = midi.snapshot();
    theme::card_frame().show(ui, |ui| {
        theme::section_heading_with(ui, "Sidecar surface", |ui| {
            // Status dot: grey = off, amber = enabled but no device,
            // green = enabled + input connected (macros-tab convention).
            let (color, hover) = if !cfg.enabled {
                (theme::label_weak(), "Sidecar off")
            } else if snap.any_connected() {
                (theme::ACCENT_GREEN, "Sidecar active")
            } else {
                (theme::ACCENT_AMBER, "Enabled — no MIDI device connected")
            };
            theme::status_dot(ui, color).on_hover_text(hover);
        });
        ui.add_space(4.0);

        // ON/OFF rocker — the label IS the state.
        ui.horizontal(|ui| {
            let (label, color, key) = if cfg.enabled {
                ("ON", theme::ACCENT_GREEN, HelpKey::SidecarDisable)
            } else {
                ("OFF", theme::ACCENT_RED, HelpKey::SidecarEnable)
            };
            if ui
                .add(theme::action_button(
                    label,
                    color,
                    egui::Vec2::new(80.0, 32.0),
                ))
                .on_hover_text(help(key))
                .clicked()
            {
                actions.push(Action::SetEnabled(!cfg.enabled));
            }
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(if cfg.enabled {
                    "Sidecar enabled"
                } else {
                    "Sidecar disabled (bindings kept)"
                })
                .color(theme::label_weak()),
            );
        });
        ui.add_space(8.0);

        // Displays and colour feedback (per show).
        ui.horizontal(|ui| {
            theme::row_label(ui, "Displays:", theme::label_weak());
            let combo_response = theme::row_combo(ui, 0, |ui| {
                egui::ComboBox::from_id_salt("sidecar_display_combo")
                    .width(220.0)
                    .selected_text(cfg.display.label())
                    .show_ui(ui, |ui| {
                        for d in [
                            SurfaceDisplay::Off,
                            SurfaceDisplay::Mackie,
                            SurfaceDisplay::AsparionD700,
                        ] {
                            if ui.selectable_label(cfg.display == d, d.label()).clicked()
                                && cfg.display != d
                            {
                                actions.push(Action::SetDisplay(d));
                            }
                        }
                    })
                    .response
            });
            combo_response.on_hover_text(help(HelpKey::SidecarDisplay));
        });

        for port in 0..SIDECAR_PORTS {
            ui.add_space(6.0);
            draw_port_rows(ui, setup, midi, &snap, port);
        }

        if let Some(err) = &snap.last_error {
            ui.label(
                egui::RichText::new(err.as_str())
                    .color(theme::ACCENT_RED)
                    .small(),
            );
        }
    });
}

/// One port slot's input and output pickers and its feedback status.
fn draw_port_rows(
    ui: &mut egui::Ui,
    setup: &mut SetupTabState,
    midi: &Arc<SidecarMidiEngine>,
    snap: &SidecarMidiState,
    port: u8,
) {
    let status = snap.ports[usize::from(port)].clone();
    ui.label(egui::RichText::new(format!("Port {}", port + 1)).strong());

    // Input port combo.
    ui.horizontal(|ui| {
        theme::row_label(ui, "Input:", theme::label_weak());
        let configured = setup.sidecar_midi.input(port).cloned();
        let selected_label = match (&configured, &status.connected_input) {
            (Some(name), Some(conn)) if name == conn => format!("{name} ●"),
            (Some(name), _) if snap.available_inputs.iter().any(|n| n == name) => name.clone(),
            (Some(name), _) => format!("{name} (unplugged)"),
            (None, _) if port == 0 => "(select a MIDI input)".to_string(),
            (None, _) => "(not used)".to_string(),
        };
        let combo_response = theme::row_combo(ui, 0, |ui| {
            egui::ComboBox::from_id_salt(("sidecar_input_combo", port))
                .width(220.0)
                .selected_text(selected_label)
                .show_ui(ui, |ui| {
                    ui.set_min_width(220.0);
                    if snap.available_inputs.is_empty() {
                        ui.label(
                            egui::RichText::new("(none — plug one in)").color(theme::label_weak()),
                        );
                    }
                    for name in &snap.available_inputs {
                        let is_sel = configured.as_deref() == Some(name);
                        let label = if status.connected_input.as_deref() == Some(name) {
                            format!("{name} ●")
                        } else {
                            name.clone()
                        };
                        if ui.selectable_label(is_sel, label).clicked() {
                            setup.sidecar_midi.set_input(port, Some(name.clone()));
                            crate::ui::setup_tab::save_app_preferences(setup);
                            midi.connect(
                                port,
                                name.clone(),
                                setup.sidecar_midi.output(port).cloned(),
                            );
                        }
                    }
                })
                .response
        });
        combo_response.on_hover_text(help(HelpKey::SidecarInputPort));

        // Shown while configured, so an unplugged port can be cleared too.
        if configured.is_some()
            && ui
                .small_button("✕")
                .on_hover_text(help(HelpKey::SidecarMidiDisconnect))
                .clicked()
        {
            setup.sidecar_midi.set_input(port, None);
            crate::ui::setup_tab::save_app_preferences(setup);
            midi.disconnect(port);
        }
    });

    // Output port combo (motor, display and colour feedback).
    ui.horizontal(|ui| {
        theme::row_label(ui, "Output:", theme::label_weak());
        let selected_label = match setup.sidecar_midi.output(port) {
            Some(name) => name.clone(),
            None => "Auto (match input)".to_string(),
        };
        let combo_response = theme::row_combo(ui, 0, |ui| {
            egui::ComboBox::from_id_salt(("sidecar_output_combo", port))
                .width(220.0)
                .selected_text(selected_label)
                .show_ui(ui, |ui| {
                    ui.set_min_width(220.0);
                    let auto_sel = setup.sidecar_midi.output(port).is_none();
                    if ui
                        .selectable_label(auto_sel, "Auto (match input)")
                        .clicked()
                    {
                        setup.sidecar_midi.set_output(port, None);
                        crate::ui::setup_tab::save_app_preferences(setup);
                        reconnect_if_configured(setup, midi, port);
                    }
                    for name in &snap.available_outputs {
                        let is_sel = setup.sidecar_midi.output(port) == Some(name);
                        if ui.selectable_label(is_sel, name.clone()).clicked() {
                            setup.sidecar_midi.set_output(port, Some(name.clone()));
                            crate::ui::setup_tab::save_app_preferences(setup);
                            reconnect_if_configured(setup, midi, port);
                        }
                    }
                })
                .response
        });
        combo_response.on_hover_text(help(HelpKey::SidecarOutputPort));
    });

    if let Some(out) = &status.connected_output {
        ui.label(
            egui::RichText::new(format!("Feedback via {out}"))
                .color(theme::label_weak())
                .small(),
        );
    } else if status.connected_input.is_some() {
        ui.label(
            egui::RichText::new(
                "No feedback output — motors and displays will not follow the console",
            )
            .color(theme::ACCENT_AMBER)
            .small(),
        );
    }
}

/// The Sidecar tab's bottom warning, if something stops the sidecar from
/// working: (title, text, help). Most fundamental first, one at a time.
pub fn banner(
    enabled: bool,
    surface_connected: bool,
    console_connected: bool,
) -> Option<(&'static str, &'static str, HelpKey)> {
    if !enabled {
        Some((
            "⚠ SIDECAR OFF",
            "— hardware moves and feedback are paused. Switch it ON above.",
            HelpKey::BannerSidecarOff,
        ))
    } else if !surface_connected {
        Some((
            "⚠ NO SURFACE",
            "— no MIDI port connected. Pick the surface's input under Port 1.",
            HelpKey::BannerSidecarNoSurface,
        ))
    } else if !console_connected {
        Some((
            "⚠ DISCONNECTED",
            "— connect to the console; hardware moves go nowhere until then.",
            HelpKey::BannerSidecarDisconnected,
        ))
    } else {
        None
    }
}

fn reconnect_if_configured(setup: &SetupTabState, midi: &Arc<SidecarMidiEngine>, port: u8) {
    if let Some(input) = setup.sidecar_midi.input(port).cloned() {
        midi.connect(port, input, setup.sidecar_midi.output(port).cloned());
    }
}

// ─── Learn card ──────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_learn_card(
    ui: &mut egui::Ui,
    tab: &mut SidecarTabState,
    cfg: &SidecarConfig,
    state: &Arc<RwLock<ConsoleState>>,
    last_received: &Arc<RwLock<Option<ParameterAddress>>>,
    learn_shared: &Arc<std::sync::Mutex<LearnShared>>,
    connected: bool,
    actions: &mut Vec<Action>,
) {
    // Phase transitions polled from the shared slots each frame.
    poll_learn_progress(tab, last_received, learn_shared);

    theme::card_frame().show(ui, |ui| {
        theme::section_heading_with(ui, "Learn", |ui| {
            if tab.learn != LearnPhase::Idle
                && ui
                    .small_button("Cancel")
                    .on_hover_text(help(HelpKey::SidecarLearnCancel))
                    .clicked()
            {
                cancel_learn(tab, learn_shared);
            }
        });
        ui.add_space(4.0);

        match tab.learn.clone() {
            LearnPhase::Idle => {
                let key = if connected {
                    HelpKey::SidecarLearnStart
                } else {
                    HelpKey::SidecarLearnNeedsConn
                };
                if ui
                    .add(theme::action_button(
                        "Learn…",
                        theme::ACCENT_GREEN,
                        egui::Vec2::new(120.0, 32.0),
                    ))
                    .on_hover_text(help(key))
                    .clicked()
                {
                    tab.learn_baseline = last_received.try_read().ok().and_then(|g| g.clone());
                    tab.relearn_id = None;
                    tab.learn = LearnPhase::ArmedConsole;
                }
            }
            LearnPhase::ArmedConsole => {
                ui.label(
                    egui::RichText::new("① Move the target on the console…")
                        .strong()
                        .color(theme::ACCENT_BLUE),
                );
                ui.label(
                    egui::RichText::new(help(HelpKey::SidecarLearnConsole).into_owned())
                        .color(theme::label_weak())
                        .small(),
                );
                if !connected {
                    ui.label(
                        egui::RichText::new(
                            "Console not connected — only external OSC \
                             targets can be learned right now.",
                        )
                        .color(theme::ACCENT_AMBER)
                        .small(),
                    );
                }
                ui.add_space(6.0);
                draw_raw_osc_form(ui, tab);
            }
            LearnPhase::GotTarget { target } => {
                ui.label(egui::RichText::new("Target captured:").color(theme::label_weak()));
                ui.label(egui::RichText::new(describe_target(&target, state)).strong());
                ui.add_space(6.0);
                if ui
                    .add(theme::action_button(
                        "② Move the sidecar control…",
                        theme::ACCENT_BLUE,
                        egui::Vec2::new(220.0, 30.0),
                    ))
                    .on_hover_text(help(HelpKey::SidecarLearnHardware))
                    .clicked()
                {
                    LearnShared::arm(learn_shared);
                    tab.learn = LearnPhase::ArmedHardware { target };
                }
            }
            LearnPhase::ArmedHardware { .. } => {
                ui.label(
                    egui::RichText::new("② Now move the sidecar control…")
                        .strong()
                        .color(theme::ACCENT_BLUE),
                );
                ui.label(
                    egui::RichText::new(help(HelpKey::SidecarLearnHardware).into_owned())
                        .color(theme::label_weak())
                        .small(),
                );
            }
            LearnPhase::Ready {
                target,
                port,
                control,
                mode,
            } => {
                ui.label(egui::RichText::new("Ready to bind:").color(theme::label_weak()));
                ui.label(
                    egui::RichText::new(format!(
                        "Port {} · {}  ({})  →  {}",
                        port + 1,
                        control.summary(),
                        mode.summary(),
                        describe_target(&target, state)
                    ))
                    .strong(),
                );
                ui.add_space(6.0);

                // One control drives one binding: confirming over an
                // existing claim replaces it (shown explicitly).
                let duplicate = cfg
                    .binding_for_control(port, &control)
                    .filter(|b| Some(b.id) != tab.relearn_id)
                    .map(|b| (b.id, b.label.clone()));
                if let Some((_, ref label)) = duplicate {
                    ui.label(
                        egui::RichText::new(format!("⚠ This control already drives \"{label}\""))
                            .color(theme::ACCENT_AMBER),
                    );
                }
                let (btn_label, btn_key) = if duplicate.is_some() {
                    ("Replace binding", HelpKey::SidecarLearnReplace)
                } else {
                    ("Confirm", HelpKey::SidecarLearnConfirm)
                };
                if ui
                    .add(theme::action_button(
                        btn_label,
                        theme::ACCENT_GREEN,
                        egui::Vec2::new(140.0, 32.0),
                    ))
                    .on_hover_text(help(btn_key))
                    .clicked()
                {
                    let replace = tab.relearn_id.or(duplicate.map(|(id, _)| id));
                    let binding = build_binding(&target, port, control, mode, tab.relearn_id, cfg);
                    actions.push(Action::AddBinding { binding, replace });
                    tab.status_message = Some("Binding saved".into());
                    cancel_learn(tab, learn_shared);
                }
            }
        }
    });
}

/// Advance the learn phase from the shared capture slots.
fn poll_learn_progress(
    tab: &mut SidecarTabState,
    last_received: &Arc<RwLock<Option<ParameterAddress>>>,
    learn_shared: &Arc<std::sync::Mutex<LearnShared>>,
) {
    match &tab.learn {
        LearnPhase::ArmedConsole => {
            if let Ok(guard) = last_received.try_read()
                && let Some(addr) = guard.clone()
                && Some(&addr) != tab.learn_baseline.as_ref()
            {
                // TotalGain never reaches last_received (dropped in
                // process_message), but keep the gate anyway: only
                // continuous, writable parameters are bindable.
                if is_valid_console_target(&addr) {
                    tab.learn = LearnPhase::GotTarget {
                        target: BindingTarget::ConsoleParameter { address: addr },
                    };
                    tab.status_message = None;
                } else {
                    tab.learn_baseline = Some(addr.clone());
                    tab.status_message = Some(format!(
                        "{} isn't a continuous parameter — move a fader, send, pan or gain",
                        addr.parameter.label()
                    ));
                }
            }
        }
        LearnPhase::ArmedHardware { target } => {
            if let Some((port, control, mode)) = LearnShared::take_result(learn_shared) {
                tab.learn = LearnPhase::Ready {
                    target: target.clone(),
                    port,
                    control,
                    mode,
                };
            } else if LearnShared::take_timed_out(learn_shared) {
                // The capture let go so bound controls work again.
                tab.learn = LearnPhase::GotTarget {
                    target: target.clone(),
                };
                tab.status_message = Some(format!(
                    "Nothing was detected for {} s, so learning stopped. Arm it again to retry.",
                    crate::console::sidecar_learn::LEARN_TIMEOUT.as_secs()
                ));
            }
        }
        _ => {}
    }
}

fn cancel_learn(tab: &mut SidecarTabState, learn_shared: &Arc<std::sync::Mutex<LearnShared>>) {
    LearnShared::disarm(learn_shared);
    tab.learn = LearnPhase::Idle;
    tab.learn_baseline = None;
    tab.relearn_id = None;
}

/// The "…or bind an external OSC target" branch of step ①.
fn draw_raw_osc_form(ui: &mut egui::Ui, tab: &mut SidecarTabState) {
    ui.separator();
    ui.label(egui::RichText::new("…or bind an external OSC target").color(theme::label_weak()));
    egui::Grid::new("sidecar_raw_osc_grid")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("Host");
            ui.text_edit_singleline(&mut tab.raw_host);
            ui.end_row();
            ui.label("Port");
            ui.add(egui::TextEdit::singleline(&mut tab.raw_port).desired_width(80.0));
            ui.end_row();
            ui.label("Path");
            ui.text_edit_singleline(&mut tab.raw_path);
            ui.end_row();
        });
    let host_ok = !tab.raw_host.trim().is_empty();
    let port_ok = tab.raw_port.trim().parse::<u16>().is_ok();
    let path_ok = tab.raw_path.trim().starts_with('/');
    let ready = host_ok && port_ok && path_ok;
    if ui
        .add_enabled(
            ready,
            theme::action_button(
                "Use external target",
                theme::ACCENT_BLUE,
                egui::Vec2::new(180.0, 28.0),
            ),
        )
        .on_hover_text(help(HelpKey::SidecarLearnRawOsc))
        .clicked()
    {
        tab.learn = LearnPhase::GotTarget {
            target: BindingTarget::RawOsc {
                target_id: None,
                host: Some(tab.raw_host.trim().to_string()),
                port: tab.raw_port.trim().parse().ok(),
                path: tab.raw_path.trim().to_string(),
                args: Vec::new(),
            },
        };
    }
}

/// Human-readable target summary ("Input 12 Fader (−3.5 dB)" /
/// "10.0.0.9:9000 /lights/dim").
fn describe_target(target: &BindingTarget, state: &Arc<RwLock<ConsoleState>>) -> String {
    match target {
        BindingTarget::ConsoleParameter { address } => {
            let value = state
                .try_read()
                .ok()
                .and_then(|s| s.get(address).cloned())
                .and_then(|v| match v {
                    ParameterValue::Float(f) => Some(format!("  ({f:.1})")),
                    _ => None,
                })
                .unwrap_or_default();
            format!("{} {}{}", address.channel, address.parameter.label(), value)
        }
        BindingTarget::RawOsc {
            host, port, path, ..
        } => format!(
            "{}:{} {}",
            host.as_deref().unwrap_or("?"),
            port.map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
            path
        ),
    }
}

/// Assemble a binding with sensible defaults from the learned halves.
/// For a re-learn (`relearn_id`), settings from the existing binding
/// carry over; only the control/mode (and derived touch) change.
fn build_binding(
    target: &BindingTarget,
    port: u8,
    control: ControlSelector,
    mode: ControlMode,
    relearn_id: Option<Uuid>,
    cfg: &SidecarConfig,
) -> SidecarBinding {
    let existing = relearn_id.and_then(|id| cfg.bindings.iter().find(|b| b.id == id));
    let taper = existing.map(|b| b.taper).unwrap_or_else(|| match target {
        BindingTarget::ConsoleParameter { address } => default_taper_for(address),
        BindingTarget::RawOsc { .. } => Taper::Linear { min: 0.0, max: 1.0 },
    });
    let label = existing
        .map(|b| b.label.clone())
        .unwrap_or_else(|| match target {
            BindingTarget::ConsoleParameter { address } => {
                format!("{} {}", address.channel, address.parameter.label())
            }
            BindingTarget::RawOsc { path, .. } => path.clone(),
        });
    let touch = match control {
        ControlSelector::PitchBend { channel } => mcu_default_touch_note(channel),
        _ => existing.and_then(|b| b.touch),
    };
    SidecarBinding {
        id: relearn_id.unwrap_or_else(Uuid::new_v4),
        label,
        control,
        mode,
        target: target.clone(),
        taper,
        motor_feedback: existing.map(|b| b.motor_feedback).unwrap_or(true),
        touch,
        relative_step: existing.map(|b| b.relative_step).unwrap_or(1.0 / 300.0),
        enabled: existing.map(|b| b.enabled).unwrap_or(true),
        port,
        color: existing.and_then(|b| b.color),
    }
}

// ─── Bindings card ───────────────────────────────────────────────────

fn draw_bindings_card(
    ui: &mut egui::Ui,
    tab: &mut SidecarTabState,
    cfg: &SidecarConfig,
    state: &Arc<RwLock<ConsoleState>>,
    actions: &mut Vec<Action>,
) {
    theme::card_frame().show(ui, |ui| {
        theme::section_heading_with(ui, &format!("Bindings ({})", cfg.bindings.len()), |ui| {
            // Greyed out while the sidecar is off: the service ignores a
            // sync then, and a button that silently does nothing reads as
            // broken. Switching ON syncs by itself.
            if ui
                .add_enabled(cfg.enabled, egui::Button::new("Sync surface now").small())
                .on_hover_text(help(HelpKey::SidecarSyncNow))
                .on_disabled_hover_text(help(HelpKey::SidecarSyncOff))
                .clicked()
            {
                actions.push(Action::Sync);
            }
        });
        ui.add_space(4.0);

        if cfg.bindings.is_empty() {
            ui.label(
                egui::RichText::new(
                    "No bindings yet — press Learn, move a console fader, \
                     then move a sidecar control.",
                )
                .color(theme::label_weak()),
            );
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for b in &cfg.bindings {
                    draw_binding_row(ui, tab, cfg, b, state, actions);
                    ui.separator();
                }
            });
    });
}

fn draw_binding_row(
    ui: &mut egui::Ui,
    tab: &mut SidecarTabState,
    cfg: &SidecarConfig,
    b: &SidecarBinding,
    state: &Arc<RwLock<ConsoleState>>,
    actions: &mut Vec<Action>,
) {
    let mut edited = b.clone();
    let mut changed = false;

    ui.horizontal(|ui| {
        if ui
            .checkbox(&mut edited.enabled, "")
            .on_hover_text(help(HelpKey::SidecarBindingEnabled))
            .changed()
        {
            changed = true;
        }
        let label_edit = egui::TextEdit::singleline(&mut edited.label)
            .desired_width(140.0)
            .id_salt(("sidecar_label", b.id));
        if ui.add(label_edit).changed() {
            changed = true;
        }
        ui.label(
            egui::RichText::new(format!(
                "P{} · {} ({})",
                b.port + 1,
                b.control.summary(),
                b.mode.summary()
            ))
            .color(theme::label_weak()),
        );
        ui.label("→");
        ui.label(describe_target(&b.target, state));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let expanded = tab.expanded == Some(b.id);
            if ui
                .small_button(if expanded { "▾" } else { "Edit" })
                .clicked()
            {
                tab.expanded = if expanded { None } else { Some(b.id) };
            }
            if b.mode.is_absolute()
                && matches!(b.target, BindingTarget::ConsoleParameter { .. })
                && ui
                    .checkbox(&mut edited.motor_feedback, "motor")
                    .on_hover_text(help(HelpKey::SidecarBindingFeedback))
                    .changed()
            {
                changed = true;
            }
        });
    });

    if tab.expanded == Some(b.id) {
        ui.indent(("sidecar_expand", b.id), |ui| {
            changed |= draw_binding_editor(ui, tab, cfg, &mut edited, actions);
        });
    }

    if changed {
        actions.push(Action::UpdateBinding(edited));
    }
}

/// Expanded per-binding editor. Returns true when `edited` changed.
fn draw_binding_editor(
    ui: &mut egui::Ui,
    tab: &mut SidecarTabState,
    cfg: &SidecarConfig,
    edited: &mut SidecarBinding,
    actions: &mut Vec<Action>,
) -> bool {
    let mut changed = false;

    // Mode (CC controls only — pitch bend is what it is).
    if let ControlSelector::Cc { cc, .. } = edited.control {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Mode").color(theme::label_weak()));
            // A 14-bit pair needs its LSB on cc + 32, a real controller only
            // up to CC 95. Above that the motor's LSB write landed on another
            // controller; learn already stops there.
            let pair = (cc < 96).then(|| {
                (
                    ControlMode::Absolute14 { lsb_cc: cc + 32 },
                    "Absolute 14-bit (CC pair)",
                )
            });
            let modes: Vec<(ControlMode, &str)> = [(ControlMode::Absolute7, "Absolute 7-bit")]
                .into_iter()
                .chain(pair)
                .chain([
                    (
                        ControlMode::Relative(RelativeMode::TwosComplement),
                        "Relative (2's complement)",
                    ),
                    (
                        ControlMode::Relative(RelativeMode::BinaryOffset),
                        "Relative (binary offset)",
                    ),
                    (
                        ControlMode::Relative(RelativeMode::SignMagnitude),
                        "Relative (sign-magnitude)",
                    ),
                ])
                .collect();
            let current = modes
                .iter()
                .find(|(m, _)| *m == edited.mode)
                .map(|(_, l)| *l)
                .unwrap_or("custom");
            egui::ComboBox::from_id_salt(("sidecar_mode", edited.id))
                .width(220.0)
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (mode, label) in modes {
                        if ui.selectable_label(edited.mode == mode, label).clicked()
                            && edited.mode != mode
                        {
                            edited.mode = mode;
                            changed = true;
                        }
                    }
                })
                .response
                .on_hover_text(help(HelpKey::SidecarBindingMode));
        });
    }

    // Taper.
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Curve").color(theme::label_weak()));
        let is_fader = matches!(edited.taper, Taper::FaderDb { .. });
        egui::ComboBox::from_id_salt(("sidecar_taper", edited.id))
            .width(120.0)
            .selected_text(if is_fader { "Fader law" } else { "Linear" })
            .show_ui(ui, |ui| {
                if ui.selectable_label(is_fader, "Fader law").clicked() && !is_fader {
                    edited.taper = Taper::FaderDb { max_db: 10.0 };
                    changed = true;
                }
                if ui.selectable_label(!is_fader, "Linear").clicked() && is_fader {
                    edited.taper = Taper::Linear { min: 0.0, max: 1.0 };
                    changed = true;
                }
            })
            .response
            .on_hover_text(help(HelpKey::SidecarBindingTaper));

        match &mut edited.taper {
            Taper::FaderDb { max_db } => {
                ui.label(egui::RichText::new("max dB").color(theme::label_weak()));
                if ui
                    .add(egui::DragValue::new(max_db).range(0.0..=10.0).speed(0.5))
                    .changed()
                {
                    changed = true;
                }
            }
            Taper::Linear { min, max } => {
                // Finite bounds: egui parses "inf" and "nan", which made every
                // move NaN and saved the show as unloadable (audit R3). Out of
                // range, both clamp to the limit.
                let bounds = -TAPER_LIMIT..=TAPER_LIMIT;
                ui.label(egui::RichText::new("min").color(theme::label_weak()));
                if ui
                    .add(egui::DragValue::new(min).range(bounds.clone()).speed(0.1))
                    .changed()
                {
                    changed = true;
                }
                ui.label(egui::RichText::new("max").color(theme::label_weak()));
                if ui
                    .add(egui::DragValue::new(max).range(bounds).speed(0.1))
                    .changed()
                {
                    changed = true;
                }
            }
        }
    });

    // Encoder sensitivity, shown as "ticks for full travel".
    if matches!(edited.mode, ControlMode::Relative(_)) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Full travel").color(theme::label_weak()));
            let mut ticks = (1.0 / edited.relative_step.max(1e-6)).round() as i32;
            if ui
                .add(
                    egui::DragValue::new(&mut ticks)
                        .range(10..=2000)
                        .suffix(" ticks"),
                )
                .on_hover_text(help(HelpKey::SidecarBindingStep))
                .changed()
            {
                edited.relative_step = 1.0 / ticks.max(10) as f32;
                changed = true;
            }
        });
    }

    if let Some(touch) = &edited.touch {
        ui.label(
            egui::RichText::new(format!("Touch sense: {}", touch.summary()))
                .color(theme::label_weak())
                .small(),
        );
    }

    // Port. A port where another binding already claims this control is
    // off limits: one control drives one binding.
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Port").color(theme::label_weak()));
        for port in 0..SIDECAR_PORTS {
            let taken = cfg
                .binding_for_control(port, &edited.control)
                .is_some_and(|other| other.id != edited.id);
            if ui
                .add_enabled(
                    !taken,
                    egui::Button::selectable(edited.port == port, format!("{}", port + 1)),
                )
                .on_hover_text(help(HelpKey::SidecarBindingPort))
                .clicked()
                && edited.port != port
            {
                edited.port = port;
                changed = true;
            }
        }
    });

    // Strip colour, for controls that sit on a strip.
    if edited.display_strip().is_some() {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Colour").color(theme::label_weak()));
            let mut rgb = edited.strip_color();
            if ui
                .color_edit_button_srgb(&mut rgb)
                .on_hover_text(help(HelpKey::SidecarBindingColor))
                .changed()
            {
                edited.color = Some(rgb);
                changed = true;
            }
            if edited.color.is_some() {
                if ui.small_button("Auto").clicked() {
                    edited.color = None;
                    changed = true;
                }
            } else {
                ui.label(
                    egui::RichText::new("auto (channel type)")
                        .color(theme::label_weak())
                        .small(),
                );
            }
        });
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .button("Re-learn control")
            .on_hover_text(help(HelpKey::SidecarBindingRelearn))
            .clicked()
        {
            tab.relearn_id = Some(edited.id);
            tab.learn = LearnPhase::GotTarget {
                target: edited.target.clone(),
            };
        }
        if theme::long_press_button(
            ui,
            "Delete",
            theme::ACCENT_RED,
            egui::Vec2::new(90.0, 26.0),
            true,
            theme::LONG_PRESS_DURATION_MS,
        ) {
            actions.push(Action::DeleteBinding(edited.id));
            tab.expanded = None;
        }
    });

    changed
}

// ─── Action application ──────────────────────────────────────────────

fn apply_actions(
    actions: Vec<Action>,
    config: &Arc<RwLock<SidecarConfig>>,
    svc_tx: &tokio::sync::mpsc::UnboundedSender<SvcCmd>,
    runtime: &tokio::runtime::Handle,
) {
    if actions.is_empty() {
        return;
    }
    let config = config.clone();
    let svc_tx = svc_tx.clone();
    runtime.spawn(async move {
        for action in actions {
            match action {
                Action::SetEnabled(on) => {
                    config.write().await.enabled = on;
                    if on {
                        // Console wins on enable.
                        let _ = svc_tx.send(SvcCmd::SyncSurface);
                    }
                }
                Action::SetDisplay(display) => {
                    // The service notices on its next refresh: it blanks
                    // what the old protocol wrote, then paints the new one.
                    config.write().await.display = display;
                }
                Action::AddBinding { binding, replace } => {
                    let mut cfg = config.write().await;
                    cfg.bindings.retain(|b| {
                        Some(b.id) != replace
                            && !(b.port == binding.port && b.control == binding.control)
                    });
                    cfg.bindings.push(binding);
                    drop(cfg);
                    let _ = svc_tx.send(SvcCmd::SyncSurface);
                }
                Action::UpdateBinding(binding) => {
                    let mut cfg = config.write().await;
                    if let Some(slot) = cfg.bindings.iter_mut().find(|b| b.id == binding.id) {
                        *slot = binding;
                    }
                }
                Action::DeleteBinding(id) => {
                    config.write().await.bindings.retain(|b| b.id != id);
                }
                Action::Sync => {
                    let _ = svc_tx.send(SvcCmd::SyncSurface);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One warning at a time, the most fundamental first; none when
    /// everything is up.
    #[test]
    fn banner_names_the_first_thing_in_the_way() {
        let key = |e, s, c| banner(e, s, c).map(|(_, _, k)| k);
        assert_eq!(key(false, false, false), Some(HelpKey::BannerSidecarOff));
        assert_eq!(key(false, true, true), Some(HelpKey::BannerSidecarOff));
        assert_eq!(
            key(true, false, true),
            Some(HelpKey::BannerSidecarNoSurface)
        );
        assert_eq!(
            key(true, true, false),
            Some(HelpKey::BannerSidecarDisconnected)
        );
        assert_eq!(key(true, true, true), None);
    }
}
