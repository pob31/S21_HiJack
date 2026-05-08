use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

use super::UiEvent;
use super::theme;
use crate::console::macro_engine::MacroEngine;
use crate::console::macro_manager::MacroManager;
use crate::model::channel::ChannelId;
use crate::model::macro_def::{MacroDef, MacroStep, MacroStepMode};
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};

/// State for the Macros tab.
pub struct MacrosTabState {
    // Selection
    pub selected_macro_id: Option<Uuid>,

    // Learn mode
    pub learn_name: String,

    // Manual creation
    pub new_macro_name: String,

    // Add step fields
    pub add_step_channel_type: ChannelTypeChoice,
    pub add_step_channel_number: String,
    pub add_step_parameter: ParameterChoice,
    pub add_step_mode: StepModeChoice,
    pub add_step_value: String,
    pub add_step_delay: String,

    // Per-step edit buffers (indexed by step position)
    pub step_mode_edits: Vec<StepModeChoice>,
    pub step_value_edits: Vec<String>,
    pub step_delay_edits: Vec<String>,

    // Feedback
    pub status_message: Option<String>,
    pub last_execution_info: Option<String>,

    // Cached snapshots of the macro list and the selected macro's steps.
    // Refreshed only when `try_read()` succeeds; otherwise the previous
    // values are reused so a contended lock (recording session, gang
    // propagation, etc.) doesn't blank the list mid-frame and make
    // selection feel sticky/unresponsive.
    pub cached_list: Vec<(Uuid, String, usize)>,
    pub cached_steps: Option<CachedSteps>,
}

/// Cached step data for the currently-selected macro. Read once when the
/// `MacroManager` lock is available, then displayed across subsequent frames
/// even if the lock is contended.
#[derive(Clone)]
pub struct CachedSteps {
    pub macro_id: Uuid,
    pub name: String,
    pub steps: Vec<(ParameterAddress, MacroStepMode, u32)>,
    pub mark_dirty: bool,
}

impl Default for MacrosTabState {
    fn default() -> Self {
        Self {
            selected_macro_id: None,
            learn_name: String::new(),
            new_macro_name: String::new(),
            add_step_channel_type: ChannelTypeChoice::Input,
            add_step_channel_number: "1".into(),
            add_step_parameter: ParameterChoice::Fader,
            add_step_mode: StepModeChoice::Fixed,
            add_step_value: "0.0".into(),
            add_step_delay: "0".into(),
            step_mode_edits: Vec::new(),
            step_value_edits: Vec::new(),
            step_delay_edits: Vec::new(),
            cached_list: Vec::new(),
            cached_steps: None,
            status_message: None,
            last_execution_info: None,
        }
    }
}

/// Channel type choices for the Add Step UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelTypeChoice {
    Input,
    Aux,
    Group,
    Matrix,
    ControlGroup,
}

impl ChannelTypeChoice {
    const ALL: [Self; 5] = [
        Self::Input,
        Self::Aux,
        Self::Group,
        Self::Matrix,
        Self::ControlGroup,
    ];

    fn label(&self) -> &'static str {
        match self {
            Self::Input => "Input",
            Self::Aux => "Aux",
            Self::Group => "Group",
            Self::Matrix => "Matrix",
            Self::ControlGroup => "CG",
        }
    }

    #[allow(clippy::wrong_self_convention)] // Copy enum; &self/self equivalent here.
    fn to_channel_id(&self, num: u8) -> ChannelId {
        match self {
            Self::Input => ChannelId::Input(num),
            Self::Aux => ChannelId::Aux(num),
            Self::Group => ChannelId::Group(num),
            Self::Matrix => ChannelId::Matrix(num),
            Self::ControlGroup => ChannelId::ControlGroup(num),
        }
    }
}

/// Parameter choices for the Add Step UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterChoice {
    Fader,
    Mute,
    Solo,
    Pan,
    AnalogGain,
    Trim,
    DelayEnabled,
    DelayTime,
}

impl ParameterChoice {
    const ALL: [Self; 8] = [
        Self::Fader,
        Self::Mute,
        Self::Solo,
        Self::Pan,
        Self::AnalogGain,
        Self::Trim,
        Self::DelayEnabled,
        Self::DelayTime,
    ];

    fn label(&self) -> &'static str {
        match self {
            Self::Fader => "Fader",
            Self::Mute => "Mute",
            Self::Solo => "Solo",
            Self::Pan => "Pan",
            Self::AnalogGain => "Analog Gain",
            Self::Trim => "Trim",
            Self::DelayEnabled => "Delay On",
            Self::DelayTime => "Delay Time",
        }
    }

    #[allow(clippy::wrong_self_convention)] // Copy enum; &self/self equivalent here.
    fn to_parameter_path(&self) -> ParameterPath {
        match self {
            Self::Fader => ParameterPath::Fader,
            Self::Mute => ParameterPath::Mute,
            Self::Solo => ParameterPath::Solo,
            Self::Pan => ParameterPath::Pan,
            Self::AnalogGain => ParameterPath::AnalogGain,
            Self::Trim => ParameterPath::Trim,
            Self::DelayEnabled => ParameterPath::DelayEnabled,
            Self::DelayTime => ParameterPath::DelayTime,
        }
    }
}

/// Step mode choices for UI dropdowns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepModeChoice {
    Toggle,
    Fixed,
    Relative,
}

impl StepModeChoice {
    const ALL: [Self; 3] = [Self::Toggle, Self::Fixed, Self::Relative];

    fn label(&self) -> &'static str {
        match self {
            Self::Toggle => "Toggle",
            Self::Fixed => "Fixed",
            Self::Relative => "Relative",
        }
    }

    fn from_mode(mode: &MacroStepMode) -> Self {
        match mode {
            MacroStepMode::Toggle => Self::Toggle,
            MacroStepMode::Fixed(_) => Self::Fixed,
            MacroStepMode::Relative(_) => Self::Relative,
        }
    }
}

/// Draw the Macros tab.
pub fn draw_macros_tab(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    macro_manager: &Arc<RwLock<MacroManager>>,
    macro_engine: &Option<Arc<MacroEngine>>,
    connected: &Arc<AtomicBool>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let is_connected = connected.load(Ordering::Relaxed);

    // Two-column layout
    let available = ui.available_size();
    let left_width = (available.x * 0.4).min(350.0);
    let panel_height = available.y;

    ui.horizontal(|ui| {
        // ═══ LEFT PANEL ═══
        ui.vertical(|ui| {
            ui.set_width(left_width);
            ui.set_min_height(panel_height);

            egui::ScrollArea::vertical()
                .id_salt("macros_left_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Learn mode card
                    theme::card_frame().show(ui, |ui| {
                        draw_learn_section(ui, macros_state, macro_manager, runtime, ui_tx);
                    });

                    ui.add_space(8.0);

                    // Manual creation + macro list card
                    theme::card_frame().show(ui, |ui| {
                        theme::section_heading(ui, "Macros");

                        // Create new macro
                        ui.horizontal(|ui| {
                            ui.label("Name:");
                            ui.add(
                                egui::TextEdit::singleline(&mut macros_state.new_macro_name)
                                    .desired_width(150.0),
                            );
                            let new_btn = theme::action_button(
                                "New",
                                theme::ACCENT_GREEN,
                                egui::Vec2::new(60.0, 28.0),
                            );
                            if ui.add(new_btn).clicked() && !macros_state.new_macro_name.is_empty()
                            {
                                let name = macros_state.new_macro_name.clone();
                                let mgr_clone = macro_manager.clone();
                                runtime.spawn(async move {
                                    let mut mgr = mgr_clone.write().await;
                                    let macro_def = MacroDef::new(name, Vec::new());
                                    mgr.add_macro(macro_def);
                                });
                                macros_state.new_macro_name.clear();
                            }
                        });

                        ui.add_space(8.0);

                        // Macro list
                        draw_macro_list(ui, macros_state, macro_manager);

                        ui.add_space(8.0);

                        // Action buttons
                        draw_action_buttons(
                            ui,
                            macros_state,
                            macro_manager,
                            macro_engine,
                            is_connected,
                            runtime,
                            ui_tx,
                        );
                    });

                    // Status messages
                    if let Some(info) = &macros_state.last_execution_info {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(info).color(theme::TEXT_SECONDARY));
                    }
                    if let Some(msg) = &macros_state.status_message {
                        ui.add_space(2.0);
                        ui.colored_label(theme::TEXT_WARNING, msg);
                    }
                });
        });

        ui.add_space(4.0);

        // ═══ RIGHT PANEL: Step Editor ═══
        ui.vertical(|ui| {
            ui.set_min_height(panel_height);

            theme::card_frame().show(ui, |ui| {
                draw_step_editor(ui, macros_state, macro_manager, runtime);
            });
        });
    });
}

fn draw_learn_section(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    macro_manager: &Arc<RwLock<MacroManager>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    theme::section_heading(ui, "Learn Mode");

    let is_recording = macro_manager
        .try_read()
        .map(|mgr| mgr.is_recording())
        .unwrap_or(false);

    if is_recording {
        // Recording state — red card
        egui::Frame::new()
            .fill(theme::COLOR_RECORDING_BG)
            .stroke(egui::Stroke::new(1.0, theme::COLOR_RECORDING))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                let (step_count, elapsed_ms) = macro_manager
                    .try_read()
                    .map(|mgr| (mgr.recording_step_count(), mgr.recording_elapsed_ms()))
                    .unwrap_or((0, 0));

                ui.horizontal(|ui| {
                    ui.colored_label(theme::COLOR_RECORDING, "● REC");
                    ui.label(
                        egui::RichText::new(format!(
                            "{} steps  |  {:.1}s",
                            step_count,
                            elapsed_ms as f64 / 1000.0
                        ))
                        .color(theme::TEXT_PRIMARY),
                    );
                });

                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut macros_state.learn_name);
                });

                ui.horizontal(|ui| {
                    let stop_btn = theme::action_button(
                        "Stop & Save",
                        theme::ACCENT_GREEN,
                        egui::Vec2::new(100.0, 28.0),
                    );
                    if ui.add(stop_btn).clicked() {
                        let name = if macros_state.learn_name.is_empty() {
                            "Recorded Macro".to_string()
                        } else {
                            macros_state.learn_name.clone()
                        };

                        let mgr_clone = macro_manager.clone();
                        let tx = ui_tx.clone();
                        runtime.spawn(async move {
                            let mut mgr = mgr_clone.write().await;
                            if let Some(recording) = mgr.stop_recording() {
                                let step_count = recording.steps.len();
                                let macro_def = recording.to_macro_def(name);
                                mgr.add_macro(macro_def);
                                let _ = tx.send(UiEvent::MacroRecordingStopped { step_count });
                            }
                        });
                        macros_state.learn_name.clear();
                    }

                    let discard_btn = theme::action_button(
                        "Discard",
                        theme::ACCENT_RED,
                        egui::Vec2::new(80.0, 28.0),
                    );
                    if ui.add(discard_btn).clicked() {
                        let mgr_clone = macro_manager.clone();
                        runtime.spawn(async move {
                            let mut mgr = mgr_clone.write().await;
                            mgr.stop_recording();
                        });
                    }
                });
            });

        // Request repaint while recording to update elapsed time
        ui.ctx().request_repaint();
    } else {
        // Not recording
        let learn_btn = theme::action_button(
            "Learn (Record)",
            theme::ACCENT_RED,
            egui::Vec2::new(130.0, 32.0),
        );
        if ui.add(learn_btn).clicked() {
            let mgr_clone = macro_manager.clone();
            runtime.spawn(async move {
                let mut mgr = mgr_clone.write().await;
                mgr.start_recording();
            });
        }
    }
}

fn draw_macro_list(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    macro_manager: &Arc<RwLock<MacroManager>>,
) {
    // Refresh the cached list opportunistically. When the manager lock is
    // contended (e.g. recording is on, or a load/save is in flight)
    // `try_read()` returns None and we render the cache from the previous
    // frame instead of an empty list — that prevents row flicker which made
    // selection feel sticky during the demo.
    if let Ok(mgr) = macro_manager.try_read() {
        macros_state.cached_list = mgr
            .sorted_macros()
            .into_iter()
            .map(|m| (m.id, m.name.clone(), m.steps.len()))
            .collect();
    }

    if macros_state.cached_list.is_empty() {
        ui.label(egui::RichText::new("No macros defined").color(theme::TEXT_SECONDARY));
        return;
    }

    let list = macros_state.cached_list.clone();
    egui::ScrollArea::vertical()
        .id_salt("macro_list_scroll")
        .max_height(200.0)
        .show(ui, |ui| {
            for (id, name, step_count) in list.iter() {
                let selected = macros_state.selected_macro_id == Some(*id);

                // Allocate the row rect first so we can interact with
                // the whole strip — gives us precise hover detection
                // without the click-target / hover-area mismatch the
                // earlier `interact` chain produced.
                let row_height = 24.0;
                let row_w = ui.available_width();
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(row_w, row_height), egui::Sense::click());

                // Background: bright when selected, hover-highlight
                // when the pointer is over the row, faint panel
                // otherwise. Selection also gets a blue left border
                // to make the active row unmistakable.
                let bg = if selected {
                    theme::BG_ELEVATED
                } else if response.hovered() {
                    // Halfway between panel and elevated so the row
                    // visibly responds to the pointer without
                    // looking selected.
                    theme::BG_ELEVATED
                } else {
                    theme::BG_PANEL
                };
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 4.0, bg);
                if selected {
                    painter.rect_stroke(
                        rect,
                        4.0,
                        egui::Stroke::new(1.5, theme::ACCENT_BLUE),
                        egui::StrokeKind::Inside,
                    );
                }

                // Pointer cursor on hover so the row feels clickable.
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }

                // Render the label + step count inside the row rect.
                let text_y = rect.center().y;
                painter.text(
                    egui::pos2(rect.min.x + 10.0, text_y),
                    egui::Align2::LEFT_CENTER,
                    name,
                    egui::FontId::proportional(14.0),
                    theme::TEXT_PRIMARY,
                );
                let count_label = format!("{} steps", step_count);
                painter.text(
                    egui::pos2(rect.max.x - 10.0, text_y),
                    egui::Align2::RIGHT_CENTER,
                    count_label,
                    egui::FontId::proportional(12.0),
                    theme::TEXT_SECONDARY,
                );

                if response.clicked() {
                    macros_state.selected_macro_id = Some(*id);
                    macros_state.step_mode_edits.clear();
                    macros_state.step_value_edits.clear();
                    macros_state.step_delay_edits.clear();
                }
                ui.add_space(2.0);
            }
        });
}

fn draw_action_buttons(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    macro_manager: &Arc<RwLock<MacroManager>>,
    macro_engine: &Option<Arc<MacroEngine>>,
    is_connected: bool,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let has_selection = macros_state.selected_macro_id.is_some();

    ui.horizontal(|ui| {
        // Run Macro
        let run_btn = theme::action_button("Run", theme::ACCENT_GREEN, egui::Vec2::new(70.0, 28.0));
        if ui
            .add_enabled(has_selection && is_connected, run_btn)
            .clicked()
        {
            if let Some(id) = macros_state.selected_macro_id {
                fire_macro_by_id(id, macro_manager, macro_engine, runtime, ui_tx);
            }
        }

        // (Macros will be driven externally — e.g. Streamdeck —
        // when that lands. No in-app quick-trigger control today.)

        // Delete
        let del_btn =
            theme::action_button("Delete", theme::ACCENT_RED, egui::Vec2::new(70.0, 28.0));
        if ui.add_enabled(has_selection, del_btn).clicked() {
            if let Some(id) = macros_state.selected_macro_id {
                let mgr_clone = macro_manager.clone();
                runtime.spawn(async move {
                    let mut mgr = mgr_clone.write().await;
                    mgr.remove_macro(id);
                });
                macros_state.selected_macro_id = None;
                macros_state.step_mode_edits.clear();
                macros_state.step_value_edits.clear();
                macros_state.step_delay_edits.clear();
            }
        }
    });
}

fn draw_step_editor(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    macro_manager: &Arc<RwLock<MacroManager>>,
    runtime: &tokio::runtime::Handle,
) {
    let Some(selected_id) = macros_state.selected_macro_id else {
        theme::section_heading(ui, "Step Editor");
        ui.label(
            egui::RichText::new("Select a macro to edit its steps").color(theme::TEXT_SECONDARY),
        );
        return;
    };

    // Refresh the cached step view when the lock is available; otherwise
    // fall back to whatever we cached on the last successful read. Same
    // reason as `draw_macro_list`: a contended manager lock should not blank
    // out the editor mid-frame.
    if let Ok(mgr) = macro_manager.try_read() {
        match mgr.get_macro(&selected_id) {
            Some(m) => {
                macros_state.cached_steps = Some(CachedSteps {
                    macro_id: m.id,
                    name: m.name.clone(),
                    steps: m
                        .steps
                        .iter()
                        .map(|s| (s.address.clone(), s.mode.clone(), s.delay_ms))
                        .collect(),
                    mark_dirty: m.mark_dirty,
                });
            }
            None => {
                // Macro was actually deleted while we held the lock — clear cache.
                if macros_state
                    .cached_steps
                    .as_ref()
                    .is_some_and(|c| c.macro_id == selected_id)
                {
                    macros_state.cached_steps = None;
                }
            }
        }
    }

    // If our cached steps don't match the current selection, treat as not
    // yet loaded (don't show stale data from a previously-selected macro).
    let cache_matches = macros_state
        .cached_steps
        .as_ref()
        .is_some_and(|c| c.macro_id == selected_id);

    let Some(cached) = macros_state.cached_steps.clone().filter(|_| cache_matches) else {
        ui.label(egui::RichText::new("Loading macro…").color(theme::TEXT_SECONDARY));
        return;
    };
    let macro_name = cached.name;
    let steps = cached.steps;
    let mark_dirty = cached.mark_dirty;

    theme::section_heading(ui, &format!("Steps: {macro_name}"));

    // Mark-dirty toggle
    let mut dirty_toggle = mark_dirty;
    if ui
        .checkbox(&mut dirty_toggle, "Track as modified parameters")
        .changed()
    {
        let mgr = macro_manager.clone();
        let new_val = dirty_toggle;
        runtime.spawn(async move {
            let mut mgr = mgr.write().await;
            if let Some(m) = mgr.get_macro_mut(&selected_id) {
                m.mark_dirty = new_val;
                m.touch();
            }
        });
    }

    // Ensure edit buffers match step count
    let step_count = steps.len();
    if macros_state.step_mode_edits.len() != step_count {
        macros_state.step_mode_edits = steps
            .iter()
            .map(|(_, m, _)| StepModeChoice::from_mode(m))
            .collect();
        macros_state.step_value_edits =
            steps.iter().map(|(_, m, _)| mode_value_string(m)).collect();
        macros_state.step_delay_edits = steps.iter().map(|(_, _, d)| d.to_string()).collect();
    }

    // Deferred actions
    let mut action: Option<StepAction> = None;

    if steps.is_empty() {
        ui.label(
            egui::RichText::new("No steps — add one below or use Learn mode")
                .color(theme::TEXT_SECONDARY),
        );
    } else {
        // Reserve enough vertical room below the scroll for the
        // Add Step section (heading + 2 horizontal rows + button +
        // elevated_frame margins ≈ 160 px). 200 leaves a comfortable
        // gap so the Add Step button is never clipped at the bottom.
        egui::ScrollArea::vertical()
            .id_salt("step_editor_scroll")
            .max_height((ui.available_height() - 200.0).max(80.0))
            .show(ui, |ui| {
                for (i, (addr, _mode, _delay)) in steps.iter().enumerate() {
                    theme::elevated_frame().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            theme::colored_badge(ui, &format!("#{}", i + 1), theme::BG_ELEVATED);
                            ui.label(
                                egui::RichText::new(format!("{}", addr)).color(theme::TEXT_PRIMARY),
                            );

                            ui.separator();

                            // Mode ComboBox
                            let mode_id = ui.id().with(("step_mode", i));
                            egui::ComboBox::from_id_salt(mode_id)
                                .width(80.0)
                                .selected_text(macros_state.step_mode_edits[i].label())
                                .show_ui(ui, |ui| {
                                    for choice in StepModeChoice::ALL {
                                        if ui
                                            .selectable_value(
                                                &mut macros_state.step_mode_edits[i],
                                                choice,
                                                choice.label(),
                                            )
                                            .changed()
                                        {
                                            action = Some(StepAction::UpdateMode(i));
                                        }
                                    }
                                });

                            // Value field (for Fixed/Relative)
                            match macros_state.step_mode_edits[i] {
                                StepModeChoice::Fixed | StepModeChoice::Relative => {
                                    let resp = ui.add(
                                        egui::TextEdit::singleline(
                                            &mut macros_state.step_value_edits[i],
                                        )
                                        .desired_width(60.0),
                                    );
                                    if resp.lost_focus() {
                                        action = Some(StepAction::UpdateMode(i));
                                    }
                                }
                                StepModeChoice::Toggle => {}
                            }

                            ui.separator();

                            // Delay field
                            ui.label("ms:");
                            let delay_resp = ui.add(
                                egui::TextEdit::singleline(&mut macros_state.step_delay_edits[i])
                                    .desired_width(50.0),
                            );
                            if delay_resp.lost_focus() {
                                action = Some(StepAction::UpdateDelay(i));
                            }
                        });

                        // Reorder + delete + keep-only buttons. Use
                        // text labels rather than `▲ ▼ ✕ ⊙` glyphs —
                        // the bundled egui font doesn't ship with
                        // those characters and they render as empty
                        // boxes on this build.
                        ui.horizontal(|ui| {
                            if i > 0
                                && ui
                                    .small_button("Up")
                                    .on_hover_text("Move this step up")
                                    .clicked()
                            {
                                action = Some(StepAction::MoveUp(i));
                            }
                            if i < step_count - 1
                                && ui
                                    .small_button("Dn")
                                    .on_hover_text("Move this step down")
                                    .clicked()
                            {
                                action = Some(StepAction::MoveDown(i));
                            }
                            if ui
                                .small_button("Del")
                                .on_hover_text("Delete this step")
                                .clicked()
                            {
                                action = Some(StepAction::Delete(i));
                            }
                            // Keep only this step's value for its
                            // (channel, parameter) — drops every other
                            // step in the macro that targets the same
                            // address. Useful when a Learn-mode
                            // recording captured several intermediate
                            // fader positions and the operator only
                            // wants to keep the final one.
                            if ui
                                .small_button("Keep")
                                .on_hover_text(
                                    "Keep only this step for its (channel, parameter); \
                                     remove the rest",
                                )
                                .clicked()
                            {
                                action = Some(StepAction::KeepOnly(i));
                            }
                        });
                    });
                    ui.add_space(2.0);
                }
            });
    }

    // Process deferred action
    if let Some(act) = action {
        apply_step_action(act, selected_id, macros_state, macro_manager, runtime);
    }

    ui.add_space(8.0);

    // Add Step section
    theme::elevated_frame().show(ui, |ui| {
        draw_add_step(ui, macros_state, selected_id, macro_manager, runtime);
    });
}

/// Draw the "Add Step" controls.
fn draw_add_step(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    macro_id: Uuid,
    macro_manager: &Arc<RwLock<MacroManager>>,
    runtime: &tokio::runtime::Handle,
) {
    ui.label(
        egui::RichText::new("Add Step")
            .strong()
            .color(theme::TEXT_PRIMARY),
    );

    ui.horizontal(|ui| {
        // Channel type
        egui::ComboBox::from_id_salt("add_ch_type")
            .width(70.0)
            .selected_text(macros_state.add_step_channel_type.label())
            .show_ui(ui, |ui| {
                for ch in ChannelTypeChoice::ALL {
                    ui.selectable_value(&mut macros_state.add_step_channel_type, ch, ch.label());
                }
            });

        // Channel number
        ui.add(
            egui::TextEdit::singleline(&mut macros_state.add_step_channel_number)
                .desired_width(30.0),
        );

        // Parameter
        egui::ComboBox::from_id_salt("add_param")
            .width(80.0)
            .selected_text(macros_state.add_step_parameter.label())
            .show_ui(ui, |ui| {
                for p in ParameterChoice::ALL {
                    ui.selectable_value(&mut macros_state.add_step_parameter, p, p.label());
                }
            });
    });

    ui.horizontal(|ui| {
        // Mode
        egui::ComboBox::from_id_salt("add_mode")
            .width(80.0)
            .selected_text(macros_state.add_step_mode.label())
            .show_ui(ui, |ui| {
                for m in StepModeChoice::ALL {
                    ui.selectable_value(&mut macros_state.add_step_mode, m, m.label());
                }
            });

        // Value (for Fixed/Relative)
        match macros_state.add_step_mode {
            StepModeChoice::Fixed | StepModeChoice::Relative => {
                ui.label("Value:");
                ui.add(
                    egui::TextEdit::singleline(&mut macros_state.add_step_value)
                        .desired_width(60.0),
                );
            }
            StepModeChoice::Toggle => {}
        }

        // Delay
        ui.label("Delay:");
        ui.add(egui::TextEdit::singleline(&mut macros_state.add_step_delay).desired_width(50.0));
        ui.label("ms");
    });

    let add_btn =
        theme::action_button("Add Step", theme::ACCENT_GREEN, egui::Vec2::new(90.0, 28.0));
    if ui.add(add_btn).clicked() {
        let ch_num: u8 = macros_state.add_step_channel_number.parse().unwrap_or(1);
        let channel = macros_state.add_step_channel_type.to_channel_id(ch_num);
        let parameter = macros_state.add_step_parameter.to_parameter_path();
        let delay_ms: u32 = macros_state.add_step_delay.parse().unwrap_or(0);

        let mode = match macros_state.add_step_mode {
            StepModeChoice::Toggle => MacroStepMode::Toggle,
            StepModeChoice::Fixed => {
                let value = parse_parameter_value(&macros_state.add_step_value);
                MacroStepMode::Fixed(value)
            }
            StepModeChoice::Relative => {
                let offset: f32 = macros_state.add_step_value.parse().unwrap_or(0.0);
                MacroStepMode::Relative(offset)
            }
        };

        let step = MacroStep {
            address: ParameterAddress { channel, parameter },
            mode,
            delay_ms,
        };

        let mgr_clone = macro_manager.clone();
        runtime.spawn(async move {
            let mut mgr = mgr_clone.write().await;
            if let Some(m) = mgr.get_macro_mut(&macro_id) {
                m.steps.push(step);
                m.touch();
            }
        });

        // Reset edit buffers so they refresh on next frame
        macros_state.step_mode_edits.clear();
        macros_state.step_value_edits.clear();
        macros_state.step_delay_edits.clear();
    }
}

/// Actions deferred from the step editor to avoid borrow conflicts.
enum StepAction {
    MoveUp(usize),
    MoveDown(usize),
    Delete(usize),
    UpdateMode(usize),
    UpdateDelay(usize),
    /// Keep only the step at this index; remove every other step targeting
    /// the same `(channel, parameter)` address.
    KeepOnly(usize),
}

fn apply_step_action(
    action: StepAction,
    macro_id: Uuid,
    macros_state: &mut MacrosTabState,
    macro_manager: &Arc<RwLock<MacroManager>>,
    runtime: &tokio::runtime::Handle,
) {
    match action {
        StepAction::MoveUp(i) => {
            let mgr_clone = macro_manager.clone();
            runtime.spawn(async move {
                let mut mgr = mgr_clone.write().await;
                if let Some(m) = mgr.get_macro_mut(&macro_id) {
                    if i > 0 && i < m.steps.len() {
                        m.steps.swap(i, i - 1);
                        m.touch();
                    }
                }
            });
            // Swap edit buffers too
            if i > 0 && i < macros_state.step_mode_edits.len() {
                macros_state.step_mode_edits.swap(i, i - 1);
                macros_state.step_value_edits.swap(i, i - 1);
                macros_state.step_delay_edits.swap(i, i - 1);
            }
        }
        StepAction::MoveDown(i) => {
            let mgr_clone = macro_manager.clone();
            runtime.spawn(async move {
                let mut mgr = mgr_clone.write().await;
                if let Some(m) = mgr.get_macro_mut(&macro_id) {
                    if i + 1 < m.steps.len() {
                        m.steps.swap(i, i + 1);
                        m.touch();
                    }
                }
            });
            if i + 1 < macros_state.step_mode_edits.len() {
                macros_state.step_mode_edits.swap(i, i + 1);
                macros_state.step_value_edits.swap(i, i + 1);
                macros_state.step_delay_edits.swap(i, i + 1);
            }
        }
        StepAction::Delete(i) => {
            let mgr_clone = macro_manager.clone();
            runtime.spawn(async move {
                let mut mgr = mgr_clone.write().await;
                if let Some(m) = mgr.get_macro_mut(&macro_id) {
                    if i < m.steps.len() {
                        m.steps.remove(i);
                        m.touch();
                    }
                }
            });
            // Clear edit buffers to force refresh
            macros_state.step_mode_edits.clear();
            macros_state.step_value_edits.clear();
            macros_state.step_delay_edits.clear();
        }
        StepAction::UpdateMode(i) => {
            let new_mode = macros_state.step_mode_edits[i];
            let value_str = macros_state.step_value_edits[i].clone();
            let mgr_clone = macro_manager.clone();
            runtime.spawn(async move {
                let mut mgr = mgr_clone.write().await;
                if let Some(m) = mgr.get_macro_mut(&macro_id) {
                    if let Some(step) = m.steps.get_mut(i) {
                        step.mode = match new_mode {
                            StepModeChoice::Toggle => MacroStepMode::Toggle,
                            StepModeChoice::Fixed => {
                                let value = parse_parameter_value(&value_str);
                                MacroStepMode::Fixed(value)
                            }
                            StepModeChoice::Relative => {
                                let offset: f32 = value_str.parse().unwrap_or(0.0);
                                MacroStepMode::Relative(offset)
                            }
                        };
                        m.touch();
                    }
                }
            });
        }
        StepAction::UpdateDelay(i) => {
            let delay_str = macros_state.step_delay_edits[i].clone();
            let mgr_clone = macro_manager.clone();
            runtime.spawn(async move {
                let mut mgr = mgr_clone.write().await;
                if let Some(m) = mgr.get_macro_mut(&macro_id) {
                    if let Some(step) = m.steps.get_mut(i) {
                        step.delay_ms = delay_str.parse().unwrap_or(step.delay_ms);
                        m.touch();
                    }
                }
            });
        }
        StepAction::KeepOnly(i) => {
            let mgr_clone = macro_manager.clone();
            runtime.spawn(async move {
                let mut mgr = mgr_clone.write().await;
                if let Some(m) = mgr.get_macro_mut(&macro_id) {
                    if let Some((_new_idx, removed)) = m.keep_only_step(i) {
                        info!(
                            macro_id = %macro_id,
                            removed,
                            "Macro: kept step #{i}, removed {removed} duplicates",
                        );
                    }
                }
            });
            // Invalidate the edit buffers so they rebuild from the new step list.
            macros_state.step_mode_edits.clear();
            macros_state.step_value_edits.clear();
            macros_state.step_delay_edits.clear();
        }
    }
}

/// Fire a macro by ID — used from the Macros tab Run button.
pub fn fire_macro_by_id(
    id: Uuid,
    macro_manager: &Arc<RwLock<MacroManager>>,
    macro_engine: &Option<Arc<MacroEngine>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let Some(engine) = macro_engine.clone() else {
        tracing::error!(macro_id = %id, "Macro Run clicked but macro_engine is None");
        let _ = ui_tx.send(UiEvent::MacroExecutionFailed(
            "macro engine not initialised — reconnect to console".into(),
        ));
        return;
    };
    let mgr_clone = macro_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        let mgr = mgr_clone.read().await;
        let Some(macro_def) = mgr.get_macro(&id).cloned() else {
            let _ = tx.send(UiEvent::MacroExecutionFailed(
                "macro no longer exists".into(),
            ));
            return;
        };
        drop(mgr);

        let result = engine.execute(&macro_def).await;
        info!(
            name = %result.macro_name,
            executed = result.steps_executed,
            skipped = result.steps_skipped,
            "Macro executed from UI"
        );
        let _ = tx.send(UiEvent::MacroExecuted {
            name: result.macro_name,
            steps_executed: result.steps_executed,
            steps_skipped: result.steps_skipped,
        });
    });
}

/// Extract the value string from a MacroStepMode.
fn mode_value_string(mode: &MacroStepMode) -> String {
    match mode {
        MacroStepMode::Toggle => String::new(),
        MacroStepMode::Fixed(v) => format!("{v}"),
        MacroStepMode::Relative(offset) => format!("{offset}"),
    }
}

/// Parse a string as a ParameterValue, trying bool, int, then float.
fn parse_parameter_value(s: &str) -> ParameterValue {
    let s = s.trim();
    if s.eq_ignore_ascii_case("true") {
        return ParameterValue::Bool(true);
    }
    if s.eq_ignore_ascii_case("false") {
        return ParameterValue::Bool(false);
    }
    if let Ok(i) = s.parse::<i32>() {
        // If it looks like a pure integer (no decimal point), use Int
        if !s.contains('.') {
            return ParameterValue::Int(i);
        }
    }
    if let Ok(f) = s.parse::<f32>() {
        return ParameterValue::Float(f);
    }
    ParameterValue::String(s.to_string())
}
