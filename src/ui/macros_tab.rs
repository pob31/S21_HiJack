use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

use super::UiEvent;
use super::theme;
use crate::console::cue_manager::CueManager;
use crate::console::macro_engine::MacroEngine;
use crate::console::macro_manager::MacroManager;
use crate::console::palette_manager::PaletteManager;
use crate::model::channel::ChannelId;
use crate::model::macro_def::{MacroDef, MacroStep, MacroStepKind, MacroStepMode};
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterSection, ParameterValue};
use crate::model::state::ConsoleState;

/// State for the Macros tab.
pub struct MacrosTabState {
    // Selection
    pub selected_macro_id: Option<Uuid>,

    // Learn mode
    pub learn_name: String,

    // Manual creation
    pub new_macro_name: String,

    // Add step fields
    /// Top-level kind selector — Parameter (full wizard) vs.
    /// app-internal action (Go / Connect / Run macro / Recall).
    pub add_step_kind: AddStepKindChoice,
    pub add_step_channel_type: ChannelTypeChoice,
    pub add_step_channel_number: String,
    /// Cascading wizard: section within the channel (EQ, Sends, …).
    /// `None` until a channel selection is established.
    pub add_step_section: Option<ParameterSection>,
    /// Concrete parameter path within the selected section.
    pub add_step_parameter_path: Option<ParameterPath>,
    pub add_step_mode: StepModeChoice,
    pub add_step_value: String,
    pub add_step_delay: String,
    /// When ON, the Add Step form continuously mirrors the most-recent
    /// inbound parameter from the console — the operator touches the
    /// physical desk, the form follows.
    pub track_latest_osc: bool,
    /// Last address synced from `last_received` so we don't re-overwrite
    /// the form on every frame.
    pub last_synced: Option<ParameterAddress>,
    /// Target IDs / channel for app-action step kinds.
    pub add_step_target_macro: Option<Uuid>,
    pub add_step_target_snapshot: Option<Uuid>,
    pub add_step_target_palette: Option<Uuid>,
    pub add_step_palette_channel_type: ChannelTypeChoice,
    pub add_step_palette_channel_number: String,
    /// QLab cue number for the QLabGoCue step kind. Stored as a string
    /// because QLab cue numbers are free-form (e.g. "1", "2.5", "Q12").
    pub add_step_qlab_cue_number: String,

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

    // ─── Stream Deck ──────────────────────────────────────────────
    /// Right-column visibility for the Stream Deck setup panel.
    /// Mutually exclusive with `selected_macro_id`: setting one
    /// clears the other so only one occupies the right column.
    pub streamdeck_popup_open: bool,
    /// Index of the currently-selected Stream Deck button slot.
    pub selected_streamdeck_button: Option<usize>,
    /// Combo selection in the Stream Deck "Add step" form: pick from
    /// existing macros.
    pub streamdeck_add_step_target: Option<Uuid>,
    /// Grid layout used when no device is connected — drives the
    /// offline editor so the operator can prepare a button map
    /// without the hardware on the desk. Defaults to `Original`
    /// (3×5, 15 buttons).
    pub streamdeck_virtual_kind: elgato_streamdeck::info::Kind,
    /// User explicitly picked a template from the unified device combo.
    /// Forces the editor into template mode regardless of whether a
    /// device serial is configured. `None` means "follow the configured
    /// serial / connected device". Cleared when the user picks a real
    /// device row.
    pub streamdeck_explicit_template: Option<elgato_streamdeck::info::Kind>,
}

/// Cached step data for the currently-selected macro. Read once when the
/// `MacroManager` lock is available, then displayed across subsequent frames
/// even if the lock is contended.
#[derive(Clone)]
pub struct CachedSteps {
    pub macro_id: Uuid,
    pub name: String,
    /// Each step's kind + delay. `kind` discriminates between OSC
    /// parameter writes and the various app-internal commands so the
    /// per-row rendering knows what to draw.
    pub steps: Vec<(MacroStepKind, u32)>,
    pub mark_dirty: bool,
}

impl Default for MacrosTabState {
    fn default() -> Self {
        Self {
            selected_macro_id: None,
            learn_name: String::new(),
            new_macro_name: String::new(),
            add_step_kind: AddStepKindChoice::Parameter,
            add_step_channel_type: ChannelTypeChoice::Input,
            add_step_channel_number: "1".into(),
            add_step_section: Some(ParameterSection::FaderMutePan),
            add_step_parameter_path: Some(ParameterPath::Fader),
            add_step_mode: StepModeChoice::Fixed,
            add_step_value: "0.0".into(),
            add_step_delay: "0".into(),
            track_latest_osc: true,
            last_synced: None,
            add_step_target_macro: None,
            add_step_target_snapshot: None,
            add_step_target_palette: None,
            add_step_palette_channel_type: ChannelTypeChoice::Input,
            add_step_palette_channel_number: "1".into(),
            add_step_qlab_cue_number: "1".into(),
            step_mode_edits: Vec::new(),
            step_value_edits: Vec::new(),
            step_delay_edits: Vec::new(),
            cached_list: Vec::new(),
            cached_steps: None,
            streamdeck_popup_open: false,
            selected_streamdeck_button: None,
            streamdeck_add_step_target: None,
            streamdeck_virtual_kind: elgato_streamdeck::info::Kind::Original,
            streamdeck_explicit_template: None,
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

    /// Decompose a `ChannelId` into `(kind, number)` for the dropdown
    /// pair. GraphicEq / MatrixInput don't have a corresponding choice
    /// and fall through to `Input` — the caller should then re-enter
    /// the section/parameter cascade fresh.
    fn from_channel_id(channel: &ChannelId) -> (Self, u8) {
        match channel {
            ChannelId::Input(n) => (Self::Input, *n),
            ChannelId::Aux(n) => (Self::Aux, *n),
            ChannelId::Group(n) => (Self::Group, *n),
            ChannelId::Matrix(n) => (Self::Matrix, *n),
            ChannelId::ControlGroup(n) => (Self::ControlGroup, *n),
            ChannelId::GraphicEq(n) | ChannelId::MatrixInput(n) => (Self::Input, *n),
        }
    }
}

/// Top-level macro step kind choices for the Add Step UI. Mirrors the
/// `MacroStepKind` model variants but lives here so the UI can switch
/// between them without binding the dropdown to one particular set of
/// IDs / channels (those are stored separately on `MacrosTabState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddStepKindChoice {
    Parameter,
    GoNextCue,
    GoPreviousCue,
    Connect,
    Disconnect,
    FireMacro,
    RecallSnapshot,
    RecallPalette,
    QLabGo,
    QLabGoCue,
    QLabPanic,
    QLabStop,
    QLabPause,
    QLabResume,
}

impl AddStepKindChoice {
    const ALL: [Self; 14] = [
        Self::Parameter,
        Self::GoNextCue,
        Self::GoPreviousCue,
        Self::Connect,
        Self::Disconnect,
        Self::FireMacro,
        Self::RecallSnapshot,
        Self::RecallPalette,
        Self::QLabGo,
        Self::QLabGoCue,
        Self::QLabPanic,
        Self::QLabStop,
        Self::QLabPause,
        Self::QLabResume,
    ];

    fn label(&self) -> &'static str {
        match self {
            Self::Parameter => "Parameter",
            Self::GoNextCue => "Go (next cue)",
            Self::GoPreviousCue => "Go Back (previous cue)",
            Self::Connect => "Connect",
            Self::Disconnect => "Disconnect",
            Self::FireMacro => "Run Macro",
            Self::RecallSnapshot => "Recall Snapshot",
            Self::RecallPalette => "Recall Palette",
            Self::QLabGo => "QLab Go",
            Self::QLabGoCue => "QLab Go Cue #",
            Self::QLabPanic => "QLab Panic",
            Self::QLabStop => "QLab Stop",
            Self::QLabPause => "QLab Pause",
            Self::QLabResume => "QLab Resume",
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
#[allow(clippy::too_many_arguments)]
pub fn draw_macros_tab(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    macro_manager: &Arc<RwLock<MacroManager>>,
    macro_engine: &Option<Arc<MacroEngine>>,
    connected: &Arc<AtomicBool>,
    state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    last_received: &Arc<RwLock<Option<ParameterAddress>>>,
    streamdeck_engine: &Arc<crate::console::streamdeck_engine::StreamDeckEngine>,
    streamdeck_config: &Arc<RwLock<crate::model::streamdeck::StreamDeckConfig>>,
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
                    // ── Top row: Learn Mode | Stream Deck ──
                    // Each card forces a vertical inner layout — the
                    // outer `horizontal_top` would otherwise propagate
                    // a left_to_right layout into the card body, which
                    // puts the title and button on the *same* row
                    // instead of stacking them.
                    let row_w = ui.available_width();
                    let inter = ui.spacing().item_spacing.x;
                    const SD_W: f32 = 175.0;
                    let learn_w = (row_w - SD_W - inter).max(150.0);
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui(egui::Vec2::new(learn_w, 0.0), |ui| {
                            ui.set_min_width(learn_w);
                            ui.set_max_width(learn_w);
                            theme::card_frame().show(ui, |ui| {
                                ui.vertical(|ui| {
                                    ui.set_min_width(learn_w - 24.0);
                                    ui.set_max_width(learn_w - 24.0);
                                    draw_learn_section(
                                        ui,
                                        macros_state,
                                        macro_manager,
                                        runtime,
                                        ui_tx,
                                    );
                                });
                            });
                        });
                        ui.allocate_ui(egui::Vec2::new(SD_W, 0.0), |ui| {
                            ui.set_min_width(SD_W);
                            ui.set_max_width(SD_W);
                            theme::card_frame().show(ui, |ui| {
                                ui.vertical(|ui| {
                                    ui.set_min_width(SD_W - 24.0);
                                    ui.set_max_width(SD_W - 24.0);
                                    draw_streamdeck_launcher(
                                        ui,
                                        macros_state,
                                        streamdeck_engine,
                                        streamdeck_config,
                                        runtime,
                                    );
                                });
                            });
                        });
                    });

                    ui.add_space(8.0);

                    // Manual creation + macro list card
                    theme::card_frame().show(ui, |ui| {
                        theme::section_heading(ui, "Macros");

                        // Create new macro — explicitly size every
                        // element to the same row height so the
                        // vertical centring math is unambiguous (the
                        // default button_padding of 12×8 would render
                        // the New button taller than the 28 px
                        // TextEdit, breaking the row).
                        const ROW_H: f32 = 28.0;
                        ui.horizontal(|ui| {
                            ui.add_sized([40.0, ROW_H], egui::Label::new("Name:"));
                            ui.add_sized(
                                [200.0, ROW_H],
                                egui::TextEdit::singleline(&mut macros_state.new_macro_name)
                                    .margin(theme::TEXT_EDIT_MARGIN),
                            );
                            let mut clicked = false;
                            ui.scope(|ui| {
                                ui.spacing_mut().button_padding = egui::Vec2::new(12.0, 4.0);
                                let new_btn = theme::action_button(
                                    "New",
                                    theme::ACCENT_GREEN,
                                    egui::Vec2::new(60.0, ROW_H),
                                );
                                clicked = ui.add(new_btn).clicked();
                            });
                            if clicked && !macros_state.new_macro_name.is_empty() {
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

        // ═══ RIGHT PANEL: Stream Deck setup OR Step editor ═══
        // Mutually exclusive: clicking Setup… on the SD card opens
        // the SD panel here (same right-column real estate); clicking
        // a macro in the left-column list opens the step editor for
        // that macro instead. Each click also flips the other side
        // so we never show both.
        ui.vertical(|ui| {
            ui.set_min_height(panel_height);

            theme::card_frame().show(ui, |ui| {
                if macros_state.streamdeck_popup_open {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Stream Deck setup")
                                .size(theme::FONT_SIZE_SECTION)
                                .strong()
                                .color(theme::TEXT_PRIMARY),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(theme::action_button(
                                    "Close",
                                    theme::BG_ELEVATED,
                                    egui::Vec2::new(60.0, 24.0),
                                ))
                                .on_hover_text(
                                    "Close Stream Deck setup and go back to the \
                                         macro step editor.",
                                )
                                .clicked()
                            {
                                macros_state.streamdeck_popup_open = false;
                                macros_state.selected_streamdeck_button = None;
                            }
                        });
                    });
                    ui.add_space(2.0);
                    let sep_w = ui.available_width();
                    let (sep_rect, _) =
                        ui.allocate_exact_size(egui::Vec2::new(sep_w, 1.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(sep_rect, 0.0, theme::BORDER_SUBTLE);
                    ui.add_space(6.0);
                    draw_streamdeck_panel(
                        ui,
                        macros_state,
                        streamdeck_engine,
                        streamdeck_config,
                        macro_manager,
                        runtime,
                    );
                } else {
                    draw_step_editor(
                        ui,
                        macros_state,
                        macro_manager,
                        state,
                        cue_manager,
                        palette_manager,
                        last_received,
                        runtime,
                    );
                }
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
                    // Selecting a macro takes the right column away
                    // from any open Stream Deck setup.
                    macros_state.streamdeck_popup_open = false;
                    macros_state.selected_streamdeck_button = None;
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

        // Delete — long-press to confirm, matching the Connect /
        // Disconnect transport buttons on the Setup tab. Hold for half
        // a second; releasing early or dragging off cancels.
        if theme::long_press_button(
            ui,
            "Delete",
            theme::ACCENT_RED,
            egui::Vec2::new(70.0, 28.0),
            has_selection,
            theme::LONG_PRESS_DURATION_MS,
        ) {
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

#[allow(clippy::too_many_arguments)]
fn draw_step_editor(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    macro_manager: &Arc<RwLock<MacroManager>>,
    state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    last_received: &Arc<RwLock<Option<ParameterAddress>>>,
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
                        .map(|s| (s.kind.clone(), s.delay_ms))
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

    // Ensure edit buffers match step count. The mode / value buffers
    // are only meaningful for `Parameter`-kind steps; for app-action
    // kinds they get default placeholder values that the UI never
    // surfaces (the rendering loop skips the mode/value combos for
    // non-Parameter rows).
    let step_count = steps.len();
    if macros_state.step_mode_edits.len() != step_count {
        macros_state.step_mode_edits = steps
            .iter()
            .map(|(kind, _)| match kind {
                MacroStepKind::Parameter { mode, .. } => StepModeChoice::from_mode(mode),
                _ => StepModeChoice::Fixed,
            })
            .collect();
        macros_state.step_value_edits = steps
            .iter()
            .map(|(kind, _)| match kind {
                MacroStepKind::Parameter { mode, .. } => mode_value_string(mode),
                _ => String::new(),
            })
            .collect();
        macros_state.step_delay_edits = steps.iter().map(|(_, d)| d.to_string()).collect();
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
                for (i, (kind, _delay)) in steps.iter().enumerate() {
                    theme::elevated_frame().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            theme::colored_badge(ui, &format!("#{}", i + 1), theme::BG_ELEVATED);
                            match kind {
                                MacroStepKind::Parameter { address, .. } => {
                                    ui.label(
                                        egui::RichText::new(format!("{}", address))
                                            .color(theme::TEXT_PRIMARY),
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
                                }
                                _ => {
                                    // App-action steps render as a
                                    // single descriptive label — no
                                    // mode / value editor. Delay is
                                    // still editable below.
                                    ui.label(
                                        egui::RichText::new(describe_step_kind(kind))
                                            .color(theme::TEXT_PRIMARY)
                                            .strong(),
                                    );
                                }
                            }

                            ui.separator();

                            // Delay field — applies to every kind.
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
        draw_add_step(
            ui,
            macros_state,
            selected_id,
            macro_manager,
            state,
            cue_manager,
            palette_manager,
            last_received,
            runtime,
        );
    });
}

/// Draw the "Add Step" controls.
#[allow(clippy::too_many_arguments)]
fn draw_add_step(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    macro_id: Uuid,
    macro_manager: &Arc<RwLock<MacroManager>>,
    state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    last_received: &Arc<RwLock<Option<ParameterAddress>>>,
    runtime: &tokio::runtime::Handle,
) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Add Step")
                .strong()
                .color(theme::TEXT_PRIMARY),
        );
        ui.add_space(8.0);
        ui.checkbox(&mut macros_state.track_latest_osc, "Track latest OSC")
            .on_hover_text(
                "Mirror the most-recent inbound parameter from the console into \
                 the form below so you can hit Add Step without retyping.",
            );
    });

    // Step kind selector — drives which sub-form is rendered below.
    ui.horizontal(|ui| {
        ui.label("Kind:");
        egui::ComboBox::from_id_salt("add_step_kind")
            .width(180.0)
            .selected_text(macros_state.add_step_kind.label())
            .show_ui(ui, |ui| {
                for k in AddStepKindChoice::ALL {
                    ui.selectable_value(&mut macros_state.add_step_kind, k, k.label());
                }
            });
    });

    match macros_state.add_step_kind {
        AddStepKindChoice::Parameter => {
            draw_parameter_wizard(ui, macros_state, state, last_received);
        }
        AddStepKindChoice::FireMacro => {
            draw_fire_macro_picker(ui, macros_state, macro_id, macro_manager);
        }
        AddStepKindChoice::RecallSnapshot => {
            draw_snapshot_picker(ui, macros_state, cue_manager);
        }
        AddStepKindChoice::RecallPalette => {
            draw_palette_picker(ui, macros_state, palette_manager);
        }
        AddStepKindChoice::QLabGoCue => {
            ui.horizontal(|ui| {
                ui.label("Cue #:");
                ui.add(
                    egui::TextEdit::singleline(&mut macros_state.add_step_qlab_cue_number)
                        .desired_width(80.0)
                        .hint_text("e.g. 1, 2.5, Q12"),
                )
                .on_hover_text(
                    "QLab cue number — free-form string (numbers, letters, dots). \
                     Sent as `/cue/<number>/start`.",
                );
            });
        }
        AddStepKindChoice::GoNextCue
        | AddStepKindChoice::GoPreviousCue
        | AddStepKindChoice::Connect
        | AddStepKindChoice::Disconnect
        | AddStepKindChoice::QLabGo
        | AddStepKindChoice::QLabPanic
        | AddStepKindChoice::QLabStop
        | AddStepKindChoice::QLabPause
        | AddStepKindChoice::QLabResume => {
            // No additional fields for these kinds.
        }
    }

    // Delay applies to every kind.
    ui.horizontal(|ui| {
        ui.label("Delay:");
        ui.add(egui::TextEdit::singleline(&mut macros_state.add_step_delay).desired_width(50.0));
        ui.label("ms");
    });

    let add_btn =
        theme::action_button("Add Step", theme::ACCENT_GREEN, egui::Vec2::new(90.0, 28.0));
    if ui.add(add_btn).clicked() {
        let delay_ms: u32 = macros_state.add_step_delay.parse().unwrap_or(0);

        let Some(kind) = build_step_kind(macros_state, macro_id) else {
            macros_state.status_message =
                Some("Add Step: required field missing for this kind".into());
            return;
        };

        let step = MacroStep { kind, delay_ms };

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

/// Cascading-dropdown wizard for picking a `Parameter` step's address +
/// mode + value. Channel Type → Channel # → Section → Parameter.
fn draw_parameter_wizard(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    state: &Arc<RwLock<ConsoleState>>,
    last_received: &Arc<RwLock<Option<ParameterAddress>>>,
) {
    // Live-OSC sync — runs before rendering so the dropdowns reflect
    // the latest address in this same frame. Only resyncs when the
    // address changes.
    if macros_state.track_latest_osc {
        if let Ok(latest) = last_received.try_read() {
            if let Some(addr) = latest.as_ref() {
                let differs = macros_state
                    .last_synced
                    .as_ref()
                    .map(|prev| prev != addr)
                    .unwrap_or(true);
                if differs {
                    apply_address_to_form(macros_state, addr);
                    // Pull the current value from console state so
                    // the operator can hit Add Step without retyping.
                    if let Ok(s) = state.try_read() {
                        if let Some(value) = s.get(addr) {
                            macros_state.add_step_mode = StepModeChoice::Fixed;
                            macros_state.add_step_value = format!("{value}");
                        }
                    }
                    macros_state.last_synced = Some(addr.clone());
                }
            }
        }
    }

    let config = state
        .try_read()
        .map(|s| s.config.clone())
        .unwrap_or_default();

    ui.horizontal(|ui| {
        // Channel type
        egui::ComboBox::from_id_salt("add_ch_type")
            .width(70.0)
            .selected_text(macros_state.add_step_channel_type.label())
            .show_ui(ui, |ui| {
                for ch in ChannelTypeChoice::ALL {
                    if ui
                        .selectable_value(&mut macros_state.add_step_channel_type, ch, ch.label())
                        .changed()
                    {
                        // Reset the section + parameter so they're
                        // valid for the new channel type.
                        macros_state.add_step_section = None;
                        macros_state.add_step_parameter_path = None;
                    }
                }
            });

        // Channel number
        ui.add(
            egui::TextEdit::singleline(&mut macros_state.add_step_channel_number)
                .desired_width(30.0),
        );

        // Section
        let ch_num: u8 = macros_state
            .add_step_channel_number
            .parse()
            .unwrap_or(1)
            .max(1);
        let channel = macros_state.add_step_channel_type.to_channel_id(ch_num);
        let sections = ParameterSection::applicable_to(&channel);
        // Clamp section if it's no longer applicable.
        if let Some(sec) = &macros_state.add_step_section {
            if !sections.contains(sec) {
                macros_state.add_step_section = None;
                macros_state.add_step_parameter_path = None;
            }
        }
        if macros_state.add_step_section.is_none() {
            macros_state.add_step_section = sections.first().cloned();
        }
        let section_label = macros_state
            .add_step_section
            .as_ref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "—".into());
        egui::ComboBox::from_id_salt("add_section")
            .width(140.0)
            .selected_text(section_label)
            .show_ui(ui, |ui| {
                for sec in &sections {
                    let label = sec.to_string();
                    let mut tmp = macros_state.add_step_section.clone();
                    if ui
                        .selectable_value(&mut tmp, Some(sec.clone()), label)
                        .changed()
                    {
                        macros_state.add_step_section = tmp;
                        macros_state.add_step_parameter_path = None;
                    }
                }
            });

        // Parameter (within section)
        let paths = match &macros_state.add_step_section {
            Some(sec) => sec.paths_for(&channel, &config),
            None => Vec::new(),
        };
        if let Some(p) = &macros_state.add_step_parameter_path {
            if !paths.contains(p) {
                macros_state.add_step_parameter_path = None;
            }
        }
        if macros_state.add_step_parameter_path.is_none() {
            macros_state.add_step_parameter_path = paths.first().cloned();
        }
        let path_label = macros_state
            .add_step_parameter_path
            .as_ref()
            .map(|p| p.label_with_config(&config))
            .unwrap_or_else(|| "—".into());
        egui::ComboBox::from_id_salt("add_param")
            .width(220.0)
            .selected_text(path_label)
            .show_ui(ui, |ui| {
                for p in &paths {
                    let label = p.label_with_config(&config);
                    let mut tmp = macros_state.add_step_parameter_path.clone();
                    ui.selectable_value(&mut tmp, Some(p.clone()), label);
                    if tmp != macros_state.add_step_parameter_path {
                        macros_state.add_step_parameter_path = tmp;
                    }
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
    });
}

/// Mirror an inbound parameter address into the Add Step form's
/// channel / section / path selectors.
fn apply_address_to_form(macros_state: &mut MacrosTabState, addr: &ParameterAddress) {
    let (ch_choice, num) = ChannelTypeChoice::from_channel_id(&addr.channel);
    macros_state.add_step_channel_type = ch_choice;
    macros_state.add_step_channel_number = num.to_string();
    macros_state.add_step_section = Some(addr.parameter.section());
    macros_state.add_step_parameter_path = Some(addr.parameter.clone());
}

fn draw_fire_macro_picker(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    current_macro_id: Uuid,
    macro_manager: &Arc<RwLock<MacroManager>>,
) {
    let macros: Vec<(Uuid, String)> = macro_manager
        .try_read()
        .map(|mgr| {
            mgr.sorted_macros()
                .iter()
                .filter(|m| m.id != current_macro_id)
                .map(|m| (m.id, m.name.clone()))
                .collect()
        })
        .unwrap_or_default();

    let selected_label = macros_state
        .add_step_target_macro
        .and_then(|id| {
            macros
                .iter()
                .find(|(mid, _)| *mid == id)
                .map(|(_, n)| n.clone())
        })
        .unwrap_or_else(|| "— select macro —".into());

    ui.horizontal(|ui| {
        ui.label("Macro:");
        egui::ComboBox::from_id_salt("add_step_target_macro")
            .width(220.0)
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                for (id, name) in &macros {
                    ui.selectable_value(&mut macros_state.add_step_target_macro, Some(*id), name);
                }
            });
    });
}

fn draw_snapshot_picker(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    cue_manager: &Arc<RwLock<CueManager>>,
) {
    let mut snapshots: Vec<(Uuid, String)> = cue_manager
        .try_read()
        .map(|mgr| {
            mgr.snapshots
                .values()
                .map(|s| (s.id, s.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    snapshots.sort_by(|a, b| a.1.cmp(&b.1));

    let selected_label: String = macros_state
        .add_step_target_snapshot
        .and_then(|id| {
            snapshots
                .iter()
                .find(|(sid, _)| *sid == id)
                .map(|(_, n)| n.clone())
        })
        .unwrap_or_else(|| "— select snapshot —".into());

    ui.horizontal(|ui| {
        ui.label("Snapshot:");
        egui::ComboBox::from_id_salt("add_step_target_snapshot")
            .width(220.0)
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                for (id, name) in &snapshots {
                    ui.selectable_value(
                        &mut macros_state.add_step_target_snapshot,
                        Some(*id),
                        name,
                    );
                }
            });
    });
}

fn draw_palette_picker(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    palette_manager: &Arc<RwLock<PaletteManager>>,
) {
    let mut palettes: Vec<(Uuid, String)> = palette_manager
        .try_read()
        .map(|mgr| {
            mgr.palettes
                .values()
                .map(|p| (p.id, p.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    palettes.sort_by(|a, b| a.1.cmp(&b.1));

    let selected_label: String = macros_state
        .add_step_target_palette
        .and_then(|id| {
            palettes
                .iter()
                .find(|(pid, _)| *pid == id)
                .map(|(_, n)| n.clone())
        })
        .unwrap_or_else(|| "— select palette —".into());

    ui.horizontal(|ui| {
        ui.label("Palette:");
        egui::ComboBox::from_id_salt("add_step_target_palette")
            .width(220.0)
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                for (id, name) in &palettes {
                    ui.selectable_value(&mut macros_state.add_step_target_palette, Some(*id), name);
                }
            });
    });

    ui.horizontal(|ui| {
        ui.label("Target channel:");
        egui::ComboBox::from_id_salt("add_step_palette_ch_type")
            .width(70.0)
            .selected_text(macros_state.add_step_palette_channel_type.label())
            .show_ui(ui, |ui| {
                for ch in ChannelTypeChoice::ALL {
                    ui.selectable_value(
                        &mut macros_state.add_step_palette_channel_type,
                        ch,
                        ch.label(),
                    );
                }
            });
        ui.add(
            egui::TextEdit::singleline(&mut macros_state.add_step_palette_channel_number)
                .desired_width(30.0),
        );
    });
}

/// Build a `MacroStepKind` from the current Add Step form state.
/// Returns `None` if a required field for the chosen kind is missing
/// (e.g. no target snapshot selected).
fn build_step_kind(
    macros_state: &MacrosTabState,
    _current_macro_id: Uuid,
) -> Option<MacroStepKind> {
    match macros_state.add_step_kind {
        AddStepKindChoice::Parameter => {
            let ch_num: u8 = macros_state
                .add_step_channel_number
                .parse()
                .unwrap_or(1)
                .max(1);
            let channel = macros_state.add_step_channel_type.to_channel_id(ch_num);
            let parameter = macros_state.add_step_parameter_path.clone()?;
            let mode = match macros_state.add_step_mode {
                StepModeChoice::Toggle => MacroStepMode::Toggle,
                StepModeChoice::Fixed => {
                    MacroStepMode::Fixed(parse_parameter_value(&macros_state.add_step_value))
                }
                StepModeChoice::Relative => {
                    MacroStepMode::Relative(macros_state.add_step_value.parse().unwrap_or(0.0))
                }
            };
            Some(MacroStepKind::Parameter {
                address: ParameterAddress { channel, parameter },
                mode,
            })
        }
        AddStepKindChoice::GoNextCue => Some(MacroStepKind::GoNextCue),
        AddStepKindChoice::GoPreviousCue => Some(MacroStepKind::GoPreviousCue),
        AddStepKindChoice::Connect => Some(MacroStepKind::Connect),
        AddStepKindChoice::Disconnect => Some(MacroStepKind::Disconnect),
        AddStepKindChoice::QLabGo => Some(MacroStepKind::QLabGo),
        AddStepKindChoice::QLabGoCue => {
            let cue_number = macros_state.add_step_qlab_cue_number.trim().to_string();
            if cue_number.is_empty() {
                return None;
            }
            Some(MacroStepKind::QLabGoCue { cue_number })
        }
        AddStepKindChoice::QLabPanic => Some(MacroStepKind::QLabPanic),
        AddStepKindChoice::QLabStop => Some(MacroStepKind::QLabStop),
        AddStepKindChoice::QLabPause => Some(MacroStepKind::QLabPause),
        AddStepKindChoice::QLabResume => Some(MacroStepKind::QLabResume),
        AddStepKindChoice::FireMacro => macros_state
            .add_step_target_macro
            .map(|id| MacroStepKind::FireMacro { id }),
        AddStepKindChoice::RecallSnapshot => macros_state
            .add_step_target_snapshot
            .map(|id| MacroStepKind::RecallSnapshot { id }),
        AddStepKindChoice::RecallPalette => {
            let id = macros_state.add_step_target_palette?;
            let ch_num: u8 = macros_state
                .add_step_palette_channel_number
                .parse()
                .unwrap_or(1)
                .max(1);
            let channel = macros_state
                .add_step_palette_channel_type
                .to_channel_id(ch_num);
            Some(MacroStepKind::RecallPalette { id, channel })
        }
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
            let new_mode_choice = macros_state.step_mode_edits[i];
            let value_str = macros_state.step_value_edits[i].clone();
            let mgr_clone = macro_manager.clone();
            runtime.spawn(async move {
                let mut mgr = mgr_clone.write().await;
                if let Some(m) = mgr.get_macro_mut(&macro_id) {
                    if let Some(step) = m.steps.get_mut(i) {
                        // Mode/value edits only apply to Parameter
                        // steps. App-action kinds (GoNextCue etc.)
                        // ignore the edit silently — the UI shouldn't
                        // expose mode/value fields for them anyway.
                        if let MacroStepKind::Parameter { mode, .. } = &mut step.kind {
                            *mode = match new_mode_choice {
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

/// Compact human-readable label for a non-Parameter macro step kind.
/// Used by the steps list to render app-action rows. Parameter steps
/// have their own dedicated rendering path that shows the address +
/// mode + value.
fn describe_step_kind(kind: &MacroStepKind) -> String {
    match kind {
        MacroStepKind::Parameter { .. } => "Parameter".into(),
        MacroStepKind::GoNextCue => "Go (next cue)".into(),
        MacroStepKind::GoPreviousCue => "Go Back (previous cue)".into(),
        MacroStepKind::Connect => "Connect to console".into(),
        MacroStepKind::Disconnect => "Disconnect from console".into(),
        MacroStepKind::FireMacro { id } => format!("Run Macro {id}"),
        MacroStepKind::RecallSnapshot { id } => format!("Recall Snapshot {id}"),
        MacroStepKind::RecallPalette { id, channel } => {
            format!("Recall Palette {id} on {channel}")
        }
        MacroStepKind::QLabGo => "QLab Go".into(),
        MacroStepKind::QLabGoCue { cue_number } => format!("QLab Go Cue #{cue_number}"),
        MacroStepKind::QLabPanic => "QLab Panic".into(),
        MacroStepKind::QLabStop => "QLab Stop".into(),
        MacroStepKind::QLabPause => "QLab Pause".into(),
        MacroStepKind::QLabResume => "QLab Resume".into(),
    }
}

/// Extract the value string from a MacroStepMode.
fn mode_value_string(mode: &MacroStepMode) -> String {
    match mode {
        MacroStepMode::Toggle => String::new(),
        MacroStepMode::Fixed(v) => format!("{v}"),
        MacroStepMode::Relative(offset) => format!("{offset}"),
    }
}

// ═══ Stream Deck UI ════════════════════════════════════════════════
//
// The Stream Deck UI lives in a floating `egui::Window` rather than
// inline in the left column for two reasons:
//   1. Some devices (XL: 4×8) have button grids that are wider than
//      the 350 px left column will ever be — even at MK1 sizes a long
//      macro list pushed the section off-screen.
//   2. The popup floats over the rest of the tab so the operator can
//      drag it around and dismiss it without losing context.

/// Compact launcher button in the left column: opens / closes the
/// floating Stream Deck panel. Shows a tiny status dot so connection
/// state is visible at a glance even with the panel closed.
fn draw_streamdeck_launcher(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    engine: &Arc<crate::console::streamdeck_engine::StreamDeckEngine>,
    config: &Arc<RwLock<crate::model::streamdeck::StreamDeckConfig>>,
    runtime: &tokio::runtime::Handle,
) {
    let cfg_snapshot = config
        .try_read()
        .ok()
        .map(|c| c.clone())
        .unwrap_or_default();
    let connected = engine.is_connected();
    let dot_color = if connected {
        theme::COLOR_CONNECTED
    } else if cfg_snapshot.enabled {
        theme::COLOR_CONNECTING
    } else {
        theme::TEXT_DISABLED
    };

    // Card heading: title text + right-aligned status dot, then a
    // separator line below — mirrors `theme::section_heading`'s look
    // but inlines the dot so the separator paints across the full row
    // (a plain `section_heading` inside a `ui.horizontal` runs the
    // separator next to the title on the same row instead).
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Stream Deck")
                .size(theme::FONT_SIZE_SECTION)
                .strong()
                .color(theme::TEXT_PRIMARY),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            theme::status_dot(ui, dot_color);
        });
    });
    ui.add_space(2.0);
    let sep_w = ui.available_width();
    let (sep_rect, _) = ui.allocate_exact_size(egui::Vec2::new(sep_w, 1.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(sep_rect, 0.0, theme::BORDER_SUBTLE);
    ui.add_space(6.0);

    // ── State toggle + Setup… on a single row ──
    // Label is the current STATE ("ON" / "OFF") and colour matches
    // it — like a physical rocker. "Enable" / "Disable" labels read
    // ambiguously (does it describe state or action?); ON / OFF is
    // unambiguous and pairs with the colour at a glance.
    let toggle_label = if cfg_snapshot.enabled { "ON" } else { "OFF" };
    let toggle_color = if cfg_snapshot.enabled {
        theme::ACCENT_GREEN
    } else {
        theme::ACCENT_RED
    };
    const BTN_W: f32 = 64.0;
    const BTN_H: f32 = 32.0;
    ui.horizontal(|ui| {
        let toggle_resp = ui
            .add_sized(
                [BTN_W, BTN_H],
                theme::action_button(toggle_label, toggle_color, egui::Vec2::new(BTN_W, BTN_H)),
            )
            .on_hover_text(if cfg_snapshot.enabled {
                "Disable the Stream Deck integration. Disconnects the \
                 device but preserves your button maps."
            } else {
                "Enable the Stream Deck integration. Auto-connects to \
                 the previously-selected device if it's plugged in; \
                 otherwise open Setup… to pick one."
            });
        if toggle_resp.clicked() {
            let new_enabled = !cfg_snapshot.enabled;
            let cfg = config.clone();
            runtime.spawn(async move {
                cfg.write().await.enabled = new_enabled;
            });
            if new_enabled {
                if let Some(serial) = cfg_snapshot.device_serial.clone() {
                    let available = engine.available_devices();
                    if available.iter().any(|d| d.serial == serial) {
                        engine.connect(serial);
                    }
                }
            } else {
                engine.disconnect();
            }
        }
        if ui
            .add_sized(
                [BTN_W, BTN_H],
                theme::action_button("Setup…", theme::BG_ELEVATED, egui::Vec2::new(BTN_W, BTN_H)),
            )
            .on_hover_text(
                "Open the Stream Deck panel — device selection, \
                 button grid, per-button macro sequences.",
            )
            .clicked()
        {
            macros_state.streamdeck_popup_open = !macros_state.streamdeck_popup_open;
            if macros_state.streamdeck_popup_open {
                // Opening SD setup takes the right column from the
                // step editor; clear macro selection so the two
                // never compete.
                macros_state.selected_macro_id = None;
            } else {
                macros_state.selected_streamdeck_button = None;
            }
        }
    });
}

/// Body of the Stream Deck setup panel. Rendered in the right
/// column of the Macros tab when `streamdeck_popup_open` is true,
/// taking over from the macro step editor (mutually exclusive).
fn draw_streamdeck_panel(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    engine: &Arc<crate::console::streamdeck_engine::StreamDeckEngine>,
    config: &Arc<RwLock<crate::model::streamdeck::StreamDeckConfig>>,
    macro_manager: &Arc<RwLock<MacroManager>>,
    runtime: &tokio::runtime::Handle,
) {
    // Read a snapshot of the config + connected device. `try_read`
    // keeps the UI responsive even when the engine is mid-write.
    let cfg_snapshot = config
        .try_read()
        .ok()
        .map(|c| c.clone())
        .unwrap_or_default();
    let connected = engine.connected_device();
    let available = engine.available_devices();
    let enabled = cfg_snapshot.enabled;

    // The Enable / Disable toggle lives in the Stream Deck card on
    // the Macros tab itself — not duplicated here. This panel is
    // strictly for device selection + per-button editing.

    // ── Unified device combo: connected/available devices on top,
    // separator, offline templates below. Selecting a real device
    // saves the serial and (when enabled) connects; selecting a
    // template forces template mode for offline editing without
    // touching the configured serial — so the auto-reconnect still
    // works when the device is plugged back in.
    use elgato_streamdeck::info::Kind as SdKind;

    let template_kinds: [SdKind; 7] = [
        SdKind::Original,
        SdKind::Mini,
        SdKind::Mk2,
        SdKind::Xl,
        SdKind::Plus,
        SdKind::Pedal,
        SdKind::Neo,
    ];

    let saved_serial_kind: Option<SdKind> = cfg_snapshot
        .device_serial
        .as_deref()
        .and_then(|s| available.iter().find(|d| d.serial == s).map(|d| d.kind));

    // Editing layout precedence: connected > explicit template >
    // saved serial's kind (when available) > virtual_kind fallback.
    let kind = if let Some(c) = &connected {
        c.kind
    } else if let Some(k) = macros_state.streamdeck_explicit_template {
        k
    } else if let Some(k) = saved_serial_kind {
        k
    } else {
        macros_state.streamdeck_virtual_kind
    };

    let selected_label: String = if let Some(c) = &connected {
        c.label.clone()
    } else if let Some(k) = macros_state.streamdeck_explicit_template {
        format!("Template: {}", virtual_kind_label(k))
    } else if let Some(s) = cfg_snapshot.device_serial.as_deref() {
        available
            .iter()
            .find(|d| d.serial == s)
            .map(|d| format!("{} (not connected)", d.label))
            .unwrap_or_else(|| format!("{s} (unplugged)"))
    } else if available.is_empty() {
        "No device detected".into()
    } else {
        "Select device…".into()
    };

    ui.horizontal(|ui| {
        ui.add_sized([60.0, 26.0], egui::Label::new("Device:"));
        ui.scope(|ui| {
            ui.spacing_mut().button_padding = egui::Vec2::new(12.0, 4.0);
            egui::ComboBox::from_id_salt("streamdeck_device")
                .width(260.0)
                .selected_text(selected_label)
                .show_ui(ui, |ui| {
                    ui.label(
                        egui::RichText::new("Connected / Available")
                            .small()
                            .color(theme::TEXT_SECONDARY),
                    );
                    if available.is_empty() {
                        ui.label(
                            egui::RichText::new("(none — plug one in)")
                                .small()
                                .color(theme::TEXT_SECONDARY),
                        );
                    }
                    for dev in &available {
                        let is_selected = connected.is_none()
                            && macros_state.streamdeck_explicit_template.is_none()
                            && cfg_snapshot.device_serial.as_deref() == Some(dev.serial.as_str())
                            || connected
                                .as_ref()
                                .map(|c| c.serial == dev.serial)
                                .unwrap_or(false);
                        let suffix = if connected
                            .as_ref()
                            .map(|c| c.serial == dev.serial)
                            .unwrap_or(false)
                        {
                            "  ● connected"
                        } else {
                            ""
                        };
                        let label = format!("{}  ({}){suffix}", dev.label, dev.serial);
                        if ui.selectable_label(is_selected, label).clicked() {
                            let serial = dev.serial.clone();
                            let cfg = config.clone();
                            let s = serial.clone();
                            runtime.spawn(async move {
                                cfg.write().await.device_serial = Some(s);
                            });
                            macros_state.streamdeck_explicit_template = None;
                            if enabled {
                                engine.connect(serial);
                            }
                        }
                    }

                    ui.separator();
                    ui.label(
                        egui::RichText::new("Templates (offline editing)")
                            .small()
                            .color(theme::TEXT_SECONDARY),
                    );
                    for k in template_kinds {
                        let is_selected = connected.is_none()
                            && macros_state.streamdeck_explicit_template == Some(k);
                        let label = format!("Template: {}", virtual_kind_label(k));
                        if ui.selectable_label(is_selected, label).clicked() {
                            macros_state.streamdeck_explicit_template = Some(k);
                            macros_state.streamdeck_virtual_kind = k;
                            // If a real device is currently connected,
                            // disconnect so the template view actually
                            // takes effect. The configured `device_serial`
                            // is kept so plugging the device back in
                            // auto-reconnects.
                            if connected.is_some() {
                                engine.disconnect();
                            }
                        }
                    }
                });
        });
    });

    ui.add_space(4.0);

    // ── Status line ──
    let status = match (&connected, enabled) {
        (Some(c), _) => format!("Connected: {}", c.label),
        (None, true) => "Disconnected — editing offline.".into(),
        (None, false) => "OFF — editing offline.".into(),
    };
    ui.colored_label(
        if connected.is_some() {
            theme::COLOR_CONNECTED
        } else {
            theme::TEXT_SECONDARY
        },
        status,
    );

    // ── Button grid + Add-step panel side-by-side ──
    ui.add_space(8.0);
    let cols_f = kind.column_count().max(1) as f32;
    let rows = kind.row_count().max(1) as usize;
    let key_count = kind.key_count();
    let spacing = 4.0_f32;
    // Match the device's actual LCD pixel size (72 px on MK1) — no
    // point rendering bigger than the real button on screen.
    let cell_w = 72.0_f32;
    let cell_h = cell_w;
    let grid_w = cols_f * cell_w + (cols_f - 1.0) * spacing;

    let macro_names: std::collections::HashMap<Uuid, String> = macro_manager
        .try_read()
        .map(|mgr| {
            mgr.macros
                .iter()
                .map(|(id, m)| (*id, m.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    let sorted_macros: Vec<(Uuid, String)> = {
        let mut v: Vec<_> = macro_names
            .iter()
            .map(|(id, name)| (*id, name.clone()))
            .collect();
        v.sort_by_key(|a| a.1.to_lowercase());
        v
    };

    let mut clicked_idx: Option<usize> = None;
    let selected = macros_state.selected_streamdeck_button;
    let buttons = cfg_snapshot.buttons.clone();
    ui.horizontal_top(|ui| {
        // Left: button grid
        ui.allocate_ui(egui::Vec2::new(grid_w, 0.0), |ui| {
            ui.vertical(|ui| {
                for r in 0..rows {
                    ui.horizontal(|ui| {
                        for col in 0..(cols_f as usize) {
                            let idx = r * cols_f as usize + col;
                            if idx >= key_count as usize {
                                break;
                            }
                            let next = buttons.get(idx).and_then(|b| b.next_step());
                            let label = next
                                .map(|s| {
                                    macro_names
                                        .get(&s.macro_id)
                                        .cloned()
                                        .unwrap_or_else(|| "(deleted)".into())
                                })
                                .unwrap_or_else(|| "—".into());
                            let step_color = next
                                .map(|s| s.color)
                                .unwrap_or(crate::model::streamdeck::StepColor::BLACK);
                            let has_step = next.is_some();
                            let is_selected = selected == Some(idx);
                            // Empty cell → keep the existing UI tone so
                            // it reads as a placeholder. A populated
                            // cell uses the step's chosen LCD color, so
                            // the on-screen grid mirrors what shows on
                            // the deck. Selection adds a blue stroke
                            // ring so the active cell still pops.
                            let fill = if has_step {
                                step_color_to_color32(step_color)
                            } else if is_selected {
                                theme::ACCENT_BLUE
                            } else {
                                theme::BG_INPUT
                            };
                            let stroke_color = if is_selected {
                                theme::ACCENT_BLUE
                            } else {
                                theme::BORDER_SUBTLE
                            };
                            let text_color = if has_step {
                                let t = step_color.contrast_text();
                                egui::Color32::from_rgb(t.r, t.g, t.b)
                            } else {
                                theme::TEXT_PRIMARY
                            };
                            let btn = egui::Button::new(
                                egui::RichText::new(label).color(text_color).small(),
                            )
                            .fill(fill)
                            .stroke(egui::Stroke::new(if is_selected { 2.0 } else { 1.0 }, stroke_color))
                            .corner_radius(4.0)
                            .min_size(egui::Vec2::new(cell_w, cell_h))
                            .truncate();
                            if ui.add_sized([cell_w, cell_h], btn).clicked() {
                                clicked_idx = Some(idx);
                            }
                            if col + 1 < cols_f as usize {
                                ui.add_space(spacing);
                            }
                        }
                    });
                    if r + 1 < rows {
                        ui.add_space(spacing);
                    }
                }
            });
        });

        // Right: Add-step (only when a button is selected). No
        // "Button #N — Close" header — the selected button itself is
        // the visual indicator of selection (highlighted blue), and
        // re-clicking it on the grid deselects.
        if let Some(button_idx) = selected {
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.set_min_width(220.0);
                ui.set_max_width(260.0);

                theme::elevated_frame().show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!("Add step to button #{}", button_idx + 1))
                            .strong()
                            .color(theme::TEXT_PRIMARY),
                    );
                    let selected_label = macros_state
                        .streamdeck_add_step_target
                        .and_then(|id| {
                            sorted_macros
                                .iter()
                                .find(|(mid, _)| *mid == id)
                                .map(|(_, n)| n.clone())
                        })
                        .unwrap_or_else(|| "— select macro —".into());
                    ui.scope(|ui| {
                        ui.spacing_mut().button_padding = egui::Vec2::new(12.0, 4.0);
                        egui::ComboBox::from_id_salt("sd_add_step_macro")
                            .width(200.0)
                            .selected_text(selected_label)
                            .show_ui(ui, |ui| {
                                for (id, name) in &sorted_macros {
                                    ui.selectable_value(
                                        &mut macros_state.streamdeck_add_step_target,
                                        Some(*id),
                                        name,
                                    );
                                }
                                if sorted_macros.is_empty() {
                                    ui.colored_label(
                                        theme::TEXT_SECONDARY,
                                        "(no macros — create one in the left panel)",
                                    );
                                }
                            });
                    });
                    ui.add_space(4.0);
                    if ui
                        .add(theme::action_button(
                            "Add step",
                            theme::ACCENT_GREEN,
                            egui::Vec2::new(80.0, 26.0),
                        ))
                        .clicked()
                    {
                        if let Some(macro_id) = macros_state.streamdeck_add_step_target {
                            let cfg = config.clone();
                            let mgr = macro_manager.clone();
                            let eng = engine.clone();
                            runtime.spawn(async move {
                                let mut cfg_w = cfg.write().await;
                                // Grow on demand: when editing a
                                // template offline (no device connect
                                // path triggered), the buttons vec may
                                // not yet cover this index.
                                if cfg_w.buttons.len() <= button_idx {
                                    cfg_w
                                        .buttons
                                        .resize_with(button_idx + 1, Default::default);
                                }
                                if let Some(b) = cfg_w.buttons.get_mut(button_idx) {
                                    b.steps.push(crate::model::streamdeck::StreamDeckStep {
                                        macro_id,
                                        ..Default::default()
                                    });
                                    let mgr_r = mgr.read().await;
                                    let next = b.next_step();
                                    let label = next
                                        .and_then(|s| {
                                            mgr_r.get_macro(&s.macro_id).map(|m| m.name.clone())
                                        })
                                        .unwrap_or_default();
                                    let bg = next
                                        .map(|s| s.color)
                                        .unwrap_or(crate::model::streamdeck::StepColor::BLACK);
                                    drop(mgr_r);
                                    eng.refresh_button(button_idx as u8, label, bg);
                                }
                            });
                        }
                    }
                });
            });
        }
    });

    if let Some(idx) = clicked_idx {
        // Re-clicking the already-selected button toggles selection
        // off (replaces the dropped Close button). Otherwise select
        // the newly-clicked button.
        if macros_state.selected_streamdeck_button == Some(idx) {
            macros_state.selected_streamdeck_button = None;
            macros_state.streamdeck_add_step_target = None;
        } else {
            macros_state.selected_streamdeck_button = Some(idx);
            macros_state.streamdeck_add_step_target = None;
        }
    }

    // ── Below: explanation + step list (when button selected) ──
    if let Some(button_idx) = macros_state.selected_streamdeck_button {
        let button = cfg_snapshot
            .buttons
            .get(button_idx)
            .cloned()
            .unwrap_or_default();
        let next_name = button
            .next_step()
            .map(|s| {
                macro_names
                    .get(&s.macro_id)
                    .cloned()
                    .unwrap_or_else(|| "(deleted)".into())
            })
            .unwrap_or_else(|| "—".into());

        ui.add_space(8.0);
        // Wrap explanation to the grid's width — keeps the body
        // visually grouped with the grid above instead of stretching
        // across the whole popup.
        ui.allocate_ui(egui::Vec2::new(grid_w, 0.0), |ui| {
            ui.set_max_width(grid_w);
            ui.label(
                egui::RichText::new(format!(
                    "Each press fires the next step in this list, then advances \
                     the cursor. Wraps back to step 1 after the last step. \
                     Currently next-to-fire: {next_name}"
                ))
                .small()
                .color(theme::TEXT_SECONDARY),
            );
        });
        ui.add_space(6.0);

        // Step list with drag-handles + × delete + color picker.
        draw_streamdeck_step_list(
            ui,
            button_idx,
            &button,
            &macro_names,
            cfg_snapshot.user_swatches.clone(),
            config,
            macro_manager,
            engine,
            runtime,
        );
    }

    // Drop selection if it points beyond the current grid's button
    // count — swapping to a smaller virtual layout or connecting a
    // smaller device shouldn't leave a stale out-of-range selection.
    if let Some(idx) = macros_state.selected_streamdeck_button {
        if (idx as u8) >= key_count {
            macros_state.selected_streamdeck_button = None;
        }
    }
}

fn virtual_kind_label(kind: elgato_streamdeck::info::Kind) -> &'static str {
    use elgato_streamdeck::info::Kind as K;
    match kind {
        K::Original | K::OriginalV2 => "Stream Deck Original — 3×5 (15 buttons)",
        K::Mk2 => "Stream Deck Mk2 — 3×5 (15 buttons)",
        K::Mini | K::MiniMk2 => "Stream Deck Mini — 2×3 (6 buttons)",
        K::Xl | K::XlV2 => "Stream Deck XL — 4×8 (32 buttons)",
        K::Plus => "Stream Deck Plus — 2×4 (8 keys + dials)",
        K::Pedal => "Stream Deck Pedal — 1×3 (3 pedals)",
        K::Neo => "Stream Deck Neo — 2×4 (8 keys)",
        _ => "Stream Deck (custom)",
    }
}

/// Step list for the currently-selected Stream Deck button.
/// Each row is a drag-source: drag onto another row to reorder.
/// `×` deletes the step. The colored chip on each row opens the
/// swatch picker, persisting changes (and the user_swatches set)
/// back to the show file. Step changes refresh the LCD label +
/// background asynchronously via the engine.
#[allow(clippy::too_many_arguments)]
fn draw_streamdeck_step_list(
    ui: &mut egui::Ui,
    button_idx: usize,
    button: &crate::model::streamdeck::StreamDeckButton,
    macro_names: &std::collections::HashMap<Uuid, String>,
    mut user_swatches: Vec<crate::model::streamdeck::StepColor>,
    config: &Arc<RwLock<crate::model::streamdeck::StreamDeckConfig>>,
    macro_manager: &Arc<RwLock<MacroManager>>,
    engine: &Arc<crate::console::streamdeck_engine::StreamDeckEngine>,
    runtime: &tokio::runtime::Handle,
) {
    if button.steps.is_empty() {
        ui.label(
            egui::RichText::new("No steps yet — pick a macro and click Add step on the right.")
                .color(theme::TEXT_SECONDARY),
        );
        return;
    }

    // Deferred actions so we mutate after the loop.
    let mut delete_at: Option<usize> = None;
    let mut move_from_to: Option<(usize, usize)> = None;
    let mut color_change: Option<(usize, crate::model::streamdeck::StepColor)> = None;
    let user_swatches_initial = user_swatches.clone();

    egui::ScrollArea::vertical()
        .id_salt("sd_button_step_scroll")
        .max_height((ui.available_height() - 24.0).max(80.0))
        .show(ui, |ui| {
            for (i, step) in button.steps.iter().enumerate() {
                let is_current = i == button.current_step as usize;
                let row_inner = theme::elevated_frame().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Drag handle. Only this region initiates a
                        // drag — clicks on the swatch, name, and Del
                        // elsewhere in the row work normally. The
                        // bundled font lacks `⋮` and `≡`, so we
                        // paint a 2×3 dot grip ourselves.
                        draw_drag_handle(ui, i);
                        theme::colored_badge(
                            ui,
                            &format!("#{}", i + 1),
                            if is_current {
                                theme::ACCENT_BLUE
                            } else {
                                theme::BG_ELEVATED
                            },
                        );
                        // LCD background color for this step.
                        let mut color = step.color;
                        if color_swatch_picker(
                            ui,
                            ("sd_step_color", button_idx, i),
                            &mut color,
                            &mut user_swatches,
                        ) && color != step.color
                        {
                            color_change = Some((i, color));
                        }
                        let name = macro_names
                            .get(&step.macro_id)
                            .cloned()
                            .unwrap_or_else(|| "(deleted)".into());
                        ui.label(egui::RichText::new(name).color(theme::TEXT_PRIMARY));
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui
                                    .small_button(
                                        egui::RichText::new("Del").color(theme::ACCENT_RED),
                                    )
                                    .on_hover_text("Delete this step")
                                    .clicked()
                                {
                                    delete_at = Some(i);
                                }
                            },
                        );
                    });
                });
                let response = row_inner.response;

                // If a drag is hovering over this row, draw a thin
                // accent line to show where it would land on drop.
                if let Some(payload) = response.dnd_hover_payload::<usize>() {
                    if *payload != i {
                        let stroke = egui::Stroke::new(2.0, theme::ACCENT_BLUE);
                        ui.painter()
                            .hline(response.rect.x_range(), response.rect.top(), stroke);
                    }
                }
                if let Some(payload) = response.dnd_release_payload::<usize>() {
                    let from = *payload;
                    if from != i {
                        move_from_to = Some((from, i));
                    }
                }

                ui.add_space(2.0);
            }
        });

    if let Some(idx) = delete_at {
        let cfg = config.clone();
        let mgr = macro_manager.clone();
        let eng = engine.clone();
        runtime.spawn(async move {
            let mut cfg_w = cfg.write().await;
            if let Some(b) = cfg_w.buttons.get_mut(button_idx) {
                if idx < b.steps.len() {
                    b.steps.remove(idx);
                    if b.current_step as usize >= b.steps.len() {
                        b.current_step = 0;
                    }
                }
                let mgr_r = mgr.read().await;
                let next = b.next_step();
                let label = next
                    .and_then(|s| mgr_r.get_macro(&s.macro_id).map(|m| m.name.clone()))
                    .unwrap_or_default();
                let bg = next
                    .map(|s| s.color)
                    .unwrap_or(crate::model::streamdeck::StepColor::BLACK);
                drop(mgr_r);
                eng.refresh_button(button_idx as u8, label, bg);
            }
        });
    }

    if let Some((from, to)) = move_from_to {
        let cfg = config.clone();
        let mgr = macro_manager.clone();
        let eng = engine.clone();
        runtime.spawn(async move {
            let mut cfg_w = cfg.write().await;
            if let Some(b) = cfg_w.buttons.get_mut(button_idx) {
                if from < b.steps.len() && to < b.steps.len() {
                    let item = b.steps.remove(from);
                    b.steps.insert(to, item);
                }
                let mgr_r = mgr.read().await;
                let next = b.next_step();
                let label = next
                    .and_then(|s| mgr_r.get_macro(&s.macro_id).map(|m| m.name.clone()))
                    .unwrap_or_default();
                let bg = next
                    .map(|s| s.color)
                    .unwrap_or(crate::model::streamdeck::StepColor::BLACK);
                drop(mgr_r);
                eng.refresh_button(button_idx as u8, label, bg);
            }
        });
    }

    if let Some((idx, new_color)) = color_change {
        let cfg = config.clone();
        let mgr = macro_manager.clone();
        let eng = engine.clone();
        runtime.spawn(async move {
            let mut cfg_w = cfg.write().await;
            if let Some(b) = cfg_w.buttons.get_mut(button_idx) {
                if let Some(s) = b.steps.get_mut(idx) {
                    s.color = new_color;
                }
                let mgr_r = mgr.read().await;
                let next = b.next_step();
                let label = next
                    .and_then(|s| mgr_r.get_macro(&s.macro_id).map(|m| m.name.clone()))
                    .unwrap_or_default();
                let bg = next
                    .map(|s| s.color)
                    .unwrap_or(crate::model::streamdeck::StepColor::BLACK);
                drop(mgr_r);
                eng.refresh_button(button_idx as u8, label, bg);
            }
        });
    }

    if user_swatches != user_swatches_initial {
        let cfg = config.clone();
        let new_swatches = user_swatches.clone();
        runtime.spawn(async move {
            cfg.write().await.user_swatches = new_swatches;
        });
    }
}

/// Painted drag-handle: a 2×3 dot grip the user can grab to reorder a
/// step row. Returns the response so callers can read drag state if
/// they need to. Sets the dnd payload to `payload` while being dragged.
///
/// The bundled NotoSans build lacks `⋮`, `≡` and friends — those
/// glyphs render as empty boxes — so we paint dots ourselves.
fn draw_drag_handle(ui: &mut egui::Ui, payload: usize) -> egui::Response {
    let size = egui::Vec2::new(14.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let resp = resp.on_hover_cursor(egui::CursorIcon::Grab);
    let painter = ui.painter();
    let dot_color = if resp.hovered() || resp.dragged() {
        theme::TEXT_PRIMARY
    } else {
        theme::TEXT_SECONDARY
    };
    let center = rect.center();
    for &dx in &[-3.0_f32, 3.0] {
        for &dy in &[-6.5_f32, 0.0, 6.5] {
            painter.circle_filled(center + egui::vec2(dx, dy), 1.6, dot_color);
        }
    }
    resp.dnd_set_drag_payload(payload);
    resp
}

// ─── Stream Deck color swatch picker ─────────────────────────────────

/// The eleven hard-coded standard swatches: black, white, then the
/// rainbow. One row in the picker popup.
fn standard_swatches() -> [(crate::model::streamdeck::StepColor, &'static str); 11] {
    use crate::model::streamdeck::StepColor;
    [
        (StepColor::new(0, 0, 0), "Black"),
        (StepColor::new(255, 255, 255), "White"),
        (StepColor::new(255, 0, 0), "Red"),
        (StepColor::new(255, 140, 0), "Orange"),
        (StepColor::new(255, 230, 0), "Yellow"),
        (StepColor::new(0, 200, 0), "Green"),
        (StepColor::new(0, 200, 200), "Cyan"),
        (StepColor::new(0, 90, 255), "Blue"),
        (StepColor::new(75, 0, 200), "Indigo"),
        (StepColor::new(170, 0, 255), "Violet"),
        (StepColor::new(255, 0, 200), "Magenta"),
    ]
}

fn step_color_to_color32(c: crate::model::streamdeck::StepColor) -> egui::Color32 {
    egui::Color32::from_rgb(c.r, c.g, c.b)
}

/// Render a swatch button. `selected` draws a brighter border so the
/// operator can see which swatch is currently active. The chip is
/// generously sized at 22×22 so it reads clearly against the row
/// frame and is an easy click target.
fn swatch_button(
    ui: &mut egui::Ui,
    color: crate::model::streamdeck::StepColor,
    selected: bool,
) -> egui::Response {
    let size = egui::Vec2::new(22.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, step_color_to_color32(color));
    let stroke = if selected {
        egui::Stroke::new(2.0, theme::ACCENT_BLUE)
    } else {
        egui::Stroke::new(1.0, theme::TEXT_SECONDARY)
    };
    painter.rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
    resp
}

/// Color picker triggered by clicking a small colored chip. Returns
/// `true` iff `current` was modified by user interaction this frame
/// (so the caller can persist the change).
///
/// Layout inside the popup, top-to-bottom:
///   • Standard swatches row (black, white, rainbow)
///   • User swatches row (right-click to remove) + "Save" button to add
///     the current color
///   • egui's built-in RGB color picker for ad-hoc colors
pub fn color_swatch_picker(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash,
    current: &mut crate::model::streamdeck::StepColor,
    user_swatches: &mut Vec<crate::model::streamdeck::StepColor>,
) -> bool {
    use crate::model::streamdeck::StepColor;

    let popup_id = ui.id().with(("sd_color_picker", &id_salt));
    let chip_resp = swatch_button(ui, *current, false).on_hover_text("Pick a color");
    let mut changed = false;
    egui::Popup::from_toggle_button_response(&chip_resp)
        .id(popup_id)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(220.0);

            ui.label(egui::RichText::new("Standard").color(theme::TEXT_SECONDARY).small());
            ui.horizontal_wrapped(|ui| {
                for (color, name) in standard_swatches() {
                    if swatch_button(ui, color, color == *current)
                        .on_hover_text(name)
                        .clicked()
                    {
                        *current = color;
                        changed = true;
                    }
                }
            });

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Yours (right-click to remove)")
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            let mut remove_idx: Option<usize> = None;
            ui.horizontal_wrapped(|ui| {
                if user_swatches.is_empty() {
                    ui.label(
                        egui::RichText::new("(none yet — pick a color below and Save)")
                            .small()
                            .color(theme::TEXT_SECONDARY),
                    );
                }
                for (i, color) in user_swatches.iter().enumerate() {
                    let resp = swatch_button(ui, *color, *color == *current);
                    if resp.clicked() {
                        *current = *color;
                        changed = true;
                    }
                    if resp.secondary_clicked() {
                        remove_idx = Some(i);
                    }
                }
            });
            if let Some(i) = remove_idx {
                user_swatches.remove(i);
                changed = true;
            }

            ui.add_space(4.0);
            ui.separator();
            ui.label(
                egui::RichText::new("Custom")
                    .color(theme::TEXT_SECONDARY)
                    .small(),
            );
            ui.horizontal(|ui| {
                let mut rgb = [current.r, current.g, current.b];
                if egui::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
                    *current = StepColor::new(rgb[0], rgb[1], rgb[2]);
                    changed = true;
                }
                if ui
                    .small_button("Save swatch")
                    .on_hover_text("Add the current color to your saved swatches (per show)")
                    .clicked()
                    && !user_swatches.contains(current)
                {
                    user_swatches.push(*current);
                    changed = true;
                }
            });
        });

    changed
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
