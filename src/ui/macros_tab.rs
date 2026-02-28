use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

use crate::console::macro_engine::MacroEngine;
use crate::console::macro_manager::MacroManager;
use crate::model::channel::ChannelId;
use crate::model::macro_def::{MacroDef, MacroStep, MacroStepMode};
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
use super::theme;
use super::UiEvent;

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
    Gain,
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
        Self::Gain,
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
            Self::Gain => "Gain",
            Self::Trim => "Trim",
            Self::DelayEnabled => "Delay On",
            Self::DelayTime => "Delay Time",
        }
    }

    fn to_parameter_path(&self) -> ParameterPath {
        match self {
            Self::Fader => ParameterPath::Fader,
            Self::Mute => ParameterPath::Mute,
            Self::Solo => ParameterPath::Solo,
            Self::Pan => ParameterPath::Pan,
            Self::Gain => ParameterPath::Gain,
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

    ui.horizontal(|ui| {
        // ═══ LEFT PANEL ═══
        ui.vertical(|ui| {
            ui.set_width(left_width);

            // Learn mode controls
            draw_learn_section(ui, macros_state, macro_manager, runtime, ui_tx);

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);

            // Manual creation
            draw_create_section(ui, macros_state, macro_manager, runtime);

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);

            // Macro list
            draw_macro_list(ui, macros_state, macro_manager);

            ui.add_space(8.0);

            // Action buttons
            draw_action_buttons(
                ui, macros_state, macro_manager, macro_engine,
                is_connected, runtime, ui_tx,
            );

            // Status messages
            ui.add_space(4.0);
            if let Some(info) = &macros_state.last_execution_info {
                ui.label(egui::RichText::new(info).weak());
            }
            if let Some(msg) = &macros_state.status_message {
                ui.colored_label(egui::Color32::YELLOW, msg);
            }
        });

        ui.separator();

        // ═══ RIGHT PANEL: Step Editor ═══
        ui.vertical(|ui| {
            draw_step_editor(ui, macros_state, macro_manager, runtime);
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
    ui.heading("Learn Mode");

    let is_recording = macro_manager
        .try_read()
        .map(|mgr| mgr.is_recording())
        .unwrap_or(false);

    if is_recording {
        // Recording state
        let (step_count, elapsed_ms) = macro_manager
            .try_read()
            .map(|mgr| (mgr.recording_step_count(), mgr.recording_elapsed_ms()))
            .unwrap_or((0, 0));

        ui.horizontal(|ui| {
            ui.colored_label(theme::COLOR_RECORDING, "● REC");
            ui.label(format!("{} steps  |  {:.1}s", step_count, elapsed_ms as f64 / 1000.0));
        });

        ui.horizontal(|ui| {
            ui.label("Name:");
            ui.text_edit_singleline(&mut macros_state.learn_name);
        });

        ui.horizontal(|ui| {
            if ui.button("Stop & Save").clicked() {
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

            if ui.button("Discard").clicked() {
                let mgr_clone = macro_manager.clone();
                runtime.spawn(async move {
                    let mut mgr = mgr_clone.write().await;
                    mgr.stop_recording();
                });
            }
        });

        // Request repaint while recording to update elapsed time
        ui.ctx().request_repaint();
    } else {
        // Not recording
        if ui.button("Learn (Record)").clicked() {
            let mgr_clone = macro_manager.clone();
            runtime.spawn(async move {
                let mut mgr = mgr_clone.write().await;
                mgr.start_recording();
            });
        }
    }
}

fn draw_create_section(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    macro_manager: &Arc<RwLock<MacroManager>>,
    runtime: &tokio::runtime::Handle,
) {
    ui.horizontal(|ui| {
        ui.label("Name:");
        ui.add(egui::TextEdit::singleline(&mut macros_state.new_macro_name).desired_width(150.0));
        if ui.button("New Macro").clicked() && !macros_state.new_macro_name.is_empty() {
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
}

fn draw_macro_list(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    macro_manager: &Arc<RwLock<MacroManager>>,
) {
    ui.label(egui::RichText::new("Macros").strong());

    let macros_info: Vec<(Uuid, String, bool)> = macro_manager
        .try_read()
        .map(|mgr| {
            mgr.sorted_macros()
                .into_iter()
                .map(|m| (m.id, m.name.clone(), mgr.is_quick_trigger(&m.id)))
                .collect()
        })
        .unwrap_or_default();

    if macros_info.is_empty() {
        ui.weak("No macros defined");
        return;
    }

    egui::ScrollArea::vertical()
        .max_height(200.0)
        .show(ui, |ui| {
            for (id, name, is_qt) in &macros_info {
                let label = if *is_qt {
                    format!("[QT] {name}")
                } else {
                    name.clone()
                };
                let selected = macros_state.selected_macro_id == Some(*id);
                if ui.selectable_label(selected, &label).clicked() {
                    macros_state.selected_macro_id = Some(*id);
                    // Reset step edit buffers when selection changes
                    macros_state.step_mode_edits.clear();
                    macros_state.step_value_edits.clear();
                    macros_state.step_delay_edits.clear();
                }
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
        if ui.add_enabled(has_selection && is_connected, egui::Button::new("Run Macro")).clicked() {
            if let Some(id) = macros_state.selected_macro_id {
                fire_macro_by_id(id, macro_manager, macro_engine, runtime, ui_tx);
            }
        }

        // Toggle Quick Trigger
        if ui.add_enabled(has_selection, egui::Button::new("Toggle Quick")).clicked() {
            if let Some(id) = macros_state.selected_macro_id {
                let mgr_clone = macro_manager.clone();
                runtime.spawn(async move {
                    let mut mgr = mgr_clone.write().await;
                    mgr.toggle_quick_trigger(id);
                });
            }
        }

        // Delete
        if ui.add_enabled(has_selection, egui::Button::new("Delete")).clicked() {
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
        ui.heading("Step Editor");
        ui.weak("Select a macro to edit its steps");
        return;
    };

    // Read macro data
    let macro_data: Option<(String, Vec<(ParameterAddress, MacroStepMode, u32)>)> = macro_manager
        .try_read()
        .ok()
        .and_then(|mgr| {
            mgr.get_macro(&selected_id).map(|m| {
                (
                    m.name.clone(),
                    m.steps
                        .iter()
                        .map(|s| (s.address.clone(), s.mode.clone(), s.delay_ms))
                        .collect(),
                )
            })
        });

    let Some((macro_name, steps)) = macro_data else {
        ui.weak("Macro not found");
        macros_state.selected_macro_id = None;
        return;
    };

    ui.heading(format!("Steps: {macro_name}"));
    ui.add_space(4.0);

    // Ensure edit buffers match step count
    let step_count = steps.len();
    if macros_state.step_mode_edits.len() != step_count {
        macros_state.step_mode_edits = steps.iter().map(|(_, m, _)| StepModeChoice::from_mode(m)).collect();
        macros_state.step_value_edits = steps.iter().map(|(_, m, _)| mode_value_string(m)).collect();
        macros_state.step_delay_edits = steps.iter().map(|(_, _, d)| d.to_string()).collect();
    }

    // Deferred actions
    let mut action: Option<StepAction> = None;

    if steps.is_empty() {
        ui.weak("No steps — add one below or use Learn mode");
    } else {
        egui::ScrollArea::vertical()
            .max_height(ui.available_height() - 120.0)
            .show(ui, |ui| {
                for (i, (addr, _mode, _delay)) in steps.iter().enumerate() {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(format!("#{}", i + 1));
                            ui.label(format!("{}", addr));

                            ui.separator();

                            // Mode ComboBox
                            let mode_id = ui.id().with(("step_mode", i));
                            egui::ComboBox::from_id_salt(mode_id)
                                .width(80.0)
                                .selected_text(macros_state.step_mode_edits[i].label())
                                .show_ui(ui, |ui| {
                                    for choice in StepModeChoice::ALL {
                                        if ui.selectable_value(
                                            &mut macros_state.step_mode_edits[i],
                                            choice,
                                            choice.label(),
                                        ).changed() {
                                            action = Some(StepAction::UpdateMode(i));
                                        }
                                    }
                                });

                            // Value field (for Fixed/Relative)
                            match macros_state.step_mode_edits[i] {
                                StepModeChoice::Fixed | StepModeChoice::Relative => {
                                    let resp = ui.add(
                                        egui::TextEdit::singleline(&mut macros_state.step_value_edits[i])
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

                        // Reorder + delete buttons
                        ui.horizontal(|ui| {
                            if i > 0 {
                                if ui.small_button("▲").clicked() {
                                    action = Some(StepAction::MoveUp(i));
                                }
                            }
                            if i < step_count - 1 {
                                if ui.small_button("▼").clicked() {
                                    action = Some(StepAction::MoveDown(i));
                                }
                            }
                            if ui.small_button("✕").clicked() {
                                action = Some(StepAction::Delete(i));
                            }
                        });
                    });
                }
            });
    }

    // Process deferred action
    if let Some(act) = action {
        apply_step_action(act, selected_id, macros_state, macro_manager, runtime);
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    // Add Step section
    draw_add_step(ui, macros_state, selected_id, macro_manager, runtime);
}

/// Draw the "Add Step" controls.
fn draw_add_step(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    macro_id: Uuid,
    macro_manager: &Arc<RwLock<MacroManager>>,
    runtime: &tokio::runtime::Handle,
) {
    ui.label(egui::RichText::new("Add Step").strong());

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
        ui.add(egui::TextEdit::singleline(&mut macros_state.add_step_channel_number).desired_width(30.0));

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
                ui.add(egui::TextEdit::singleline(&mut macros_state.add_step_value).desired_width(60.0));
            }
            StepModeChoice::Toggle => {}
        }

        // Delay
        ui.label("Delay:");
        ui.add(egui::TextEdit::singleline(&mut macros_state.add_step_delay).desired_width(50.0));
        ui.label("ms");
    });

    if ui.button("Add Step").clicked() {
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
    }
}

/// Fire a macro by ID — used by both Macros tab and Live tab quick-trigger.
pub fn fire_macro_by_id(
    id: Uuid,
    macro_manager: &Arc<RwLock<MacroManager>>,
    macro_engine: &Option<Arc<MacroEngine>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let Some(engine) = macro_engine.clone() else { return };
    let mgr_clone = macro_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        let mgr = mgr_clone.read().await;
        let Some(macro_def) = mgr.get_macro(&id).cloned() else { return };
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
