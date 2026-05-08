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
    /// Floating popup-window visibility. Operator opens it via the
    /// "Stream Deck…" button in the left column; the popup hosts the
    /// device combo, button grid, and per-button step editor so
    /// nothing in the cramped left column gets pushed off-screen.
    pub streamdeck_popup_open: bool,
    /// Index of the currently-selected Stream Deck button slot inside
    /// the popup — drives the inline step editor below the grid.
    pub selected_streamdeck_button: Option<usize>,
    /// Combo selection in the Stream Deck "Add step" form: pick from
    /// existing macros.
    pub streamdeck_add_step_target: Option<Uuid>,
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
            step_mode_edits: Vec::new(),
            step_value_edits: Vec::new(),
            step_delay_edits: Vec::new(),
            cached_list: Vec::new(),
            cached_steps: None,
            streamdeck_popup_open: false,
            selected_streamdeck_button: None,
            streamdeck_add_step_target: None,
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
}

impl AddStepKindChoice {
    const ALL: [Self; 8] = [
        Self::Parameter,
        Self::GoNextCue,
        Self::GoPreviousCue,
        Self::Connect,
        Self::Disconnect,
        Self::FireMacro,
        Self::RecallSnapshot,
        Self::RecallPalette,
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
                    // The Stream Deck card is pinned to a narrow fixed
                    // width — just enough for its title and the
                    // Enable + Setup… button pair — so it doesn't pad
                    // the column with dead space. Learn takes the
                    // remaining width.
                    let row_w = ui.available_width();
                    let inter = ui.spacing().item_spacing.x;
                    const SD_W: f32 = 180.0;
                    let learn_w = (row_w - SD_W - inter).max(160.0);
                    ui.horizontal_top(|ui| {
                        ui.allocate_ui(egui::Vec2::new(learn_w, 0.0), |ui| {
                            ui.set_min_width(learn_w);
                            ui.set_max_width(learn_w);
                            theme::card_frame().show(ui, |ui| {
                                ui.set_min_width(learn_w - 24.0);
                                draw_learn_section(ui, macros_state, macro_manager, runtime, ui_tx);
                            });
                        });
                        ui.allocate_ui(egui::Vec2::new(SD_W, 0.0), |ui| {
                            ui.set_min_width(SD_W);
                            ui.set_max_width(SD_W);
                            theme::card_frame().show(ui, |ui| {
                                ui.set_min_width(SD_W - 24.0);
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

        // ═══ RIGHT PANEL: Step Editor ═══
        ui.vertical(|ui| {
            ui.set_min_height(panel_height);

            theme::card_frame().show(ui, |ui| {
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
            });
        });
    });

    // ─── Stream Deck popup window (independent of the two-column
    // layout above so the device's button grid can be any size). ──
    draw_streamdeck_popup(
        ui.ctx(),
        macros_state,
        streamdeck_engine,
        streamdeck_config,
        macro_manager,
        runtime,
    );
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
        AddStepKindChoice::GoNextCue
        | AddStepKindChoice::GoPreviousCue
        | AddStepKindChoice::Connect
        | AddStepKindChoice::Disconnect => {
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

    // Card heading with right-aligned status dot.
    ui.horizontal(|ui| {
        theme::section_heading(ui, "Stream Deck");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            theme::status_dot(ui, dot_color);
        });
    });

    // ── Enable + Setup buttons on a single row ──
    let toggle_label = if cfg_snapshot.enabled {
        "Disable"
    } else {
        "Enable"
    };
    let toggle_color = if cfg_snapshot.enabled {
        theme::ACCENT_RED
    } else {
        theme::ACCENT_GREEN
    };
    ui.horizontal(|ui| {
        if ui
            .add(theme::action_button(
                toggle_label,
                toggle_color,
                egui::Vec2::new(70.0, 28.0),
            ))
            .on_hover_text(if cfg_snapshot.enabled {
                "Disable the Stream Deck integration. Disconnects the \
                 device but preserves your button maps."
            } else {
                "Enable the Stream Deck integration. Auto-connects to \
                 the previously-selected device if it's plugged in; \
                 otherwise open Setup… to pick one."
            })
            .clicked()
        {
            let new_enabled = !cfg_snapshot.enabled;
            let cfg = config.clone();
            runtime.spawn(async move {
                cfg.write().await.enabled = new_enabled;
            });
            if new_enabled {
                // Auto-connect to the saved serial if it's currently
                // plugged in; otherwise stay in idle and let Setup pick.
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
            .add(theme::action_button(
                "Setup…",
                theme::BG_ELEVATED,
                egui::Vec2::new(70.0, 28.0),
            ))
            .on_hover_text(
                "Open the Stream Deck panel — device selection, \
                 button grid, per-button macro sequences.",
            )
            .clicked()
        {
            macros_state.streamdeck_popup_open = !macros_state.streamdeck_popup_open;
        }
    });
}

/// Floating window with the full Stream Deck UI: enable toggle,
/// device combo, status, button grid, and (when a button is
/// selected) the inline step editor below the grid.
fn draw_streamdeck_popup(
    ctx: &egui::Context,
    macros_state: &mut MacrosTabState,
    engine: &Arc<crate::console::streamdeck_engine::StreamDeckEngine>,
    config: &Arc<RwLock<crate::model::streamdeck::StreamDeckConfig>>,
    macro_manager: &Arc<RwLock<MacroManager>>,
    runtime: &tokio::runtime::Handle,
) {
    if !macros_state.streamdeck_popup_open {
        // Drop selection when popup is closed so reopening doesn't
        // surprise the operator with a previously-selected button.
        macros_state.selected_streamdeck_button = None;
        return;
    }
    let mut open = macros_state.streamdeck_popup_open;
    egui::Window::new("Stream Deck")
        .open(&mut open)
        .resizable(true)
        .collapsible(false)
        .default_width(420.0)
        .min_width(320.0)
        .max_width(900.0)
        .show(ctx, |ui| {
            draw_streamdeck_panel(ui, macros_state, engine, config, macro_manager, runtime);
        });
    macros_state.streamdeck_popup_open = open;
}

/// Body of the Stream Deck panel — used by the popup. Header row
/// has the enable toggle + status dot.
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

    // ── Enable toggle ──
    let mut enabled = cfg_snapshot.enabled;
    let toggle_label = if enabled { "Disable" } else { "Enable" };
    let toggle_color = if enabled {
        theme::ACCENT_RED
    } else {
        theme::ACCENT_GREEN
    };
    if ui
        .add(theme::action_button(
            toggle_label,
            toggle_color,
            egui::Vec2::new(100.0, 26.0),
        ))
        .clicked()
    {
        enabled = !enabled;
        let cfg = config.clone();
        runtime.spawn(async move {
            cfg.write().await.enabled = enabled;
        });
        if enabled {
            // Try connect to the saved serial if any (and present).
            if let Some(serial) = cfg_snapshot.device_serial.clone() {
                if available.iter().any(|d| d.serial == serial) {
                    engine.connect(serial);
                }
            }
        } else {
            engine.disconnect();
        }
    }

    ui.add_space(4.0);

    // ── Device combo ──
    if enabled {
        let selected_label = match (cfg_snapshot.device_serial.as_deref(), connected.as_ref()) {
            (_, Some(c)) => c.label.clone(),
            (Some(s), None) => available
                .iter()
                .find(|d| d.serial == s)
                .map(|d| format!("{} (not connected)", d.label))
                .unwrap_or_else(|| format!("{s} (unplugged)")),
            (None, None) => {
                if available.is_empty() {
                    "No device detected".into()
                } else {
                    "Select device…".into()
                }
            }
        };
        ui.horizontal(|ui| {
            ui.add_sized([60.0, 26.0], egui::Label::new("Device:"));
            ui.scope(|ui| {
                ui.spacing_mut().button_padding = egui::Vec2::new(12.0, 4.0);
                egui::ComboBox::from_id_salt("streamdeck_device")
                    .width(200.0)
                    .selected_text(selected_label)
                    .show_ui(ui, |ui| {
                        for dev in &available {
                            let is_selected =
                                cfg_snapshot.device_serial.as_deref() == Some(dev.serial.as_str());
                            let label = format!("{}  ({})", dev.label, dev.serial);
                            if ui.selectable_label(is_selected, label).clicked() {
                                let serial = dev.serial.clone();
                                let cfg = config.clone();
                                let s = serial.clone();
                                runtime.spawn(async move {
                                    cfg.write().await.device_serial = Some(s);
                                });
                                engine.connect(serial);
                            }
                        }
                        if available.is_empty() {
                            ui.colored_label(
                                theme::TEXT_SECONDARY,
                                "(no Stream Deck found — plug one in)",
                            );
                        }
                    });
            });
        });
    }

    ui.add_space(4.0);

    // ── Status line ──
    let status = match &connected {
        Some(c) => format!("Connected: {}", c.label),
        None if enabled => "Disconnected".into(),
        None => "Off".into(),
    };
    ui.colored_label(
        if connected.is_some() {
            theme::COLOR_CONNECTED
        } else {
            theme::TEXT_SECONDARY
        },
        status,
    );

    // ── Button grid (only when connected) ──
    if let Some(c) = connected {
        ui.add_space(8.0);
        let avail_w = ui.available_width();
        let cols = c.column_count.max(1) as f32;
        let rows = c.row_count.max(1) as usize;
        let spacing = 4.0_f32;
        // Match the device's actual LCD pixel size (72 px on MK1) —
        // no point rendering bigger than the real button on screen.
        let cell_w = ((avail_w - spacing * (cols - 1.0)) / cols).clamp(40.0, 72.0);
        let cell_h = cell_w;

        // Cache macro names for the labels.
        let macro_names: std::collections::HashMap<Uuid, String> = macro_manager
            .try_read()
            .map(|mgr| {
                mgr.macros
                    .iter()
                    .map(|(id, m)| (*id, m.name.clone()))
                    .collect()
            })
            .unwrap_or_default();

        let buttons = &cfg_snapshot.buttons;
        let mut clicked_idx: Option<usize> = None;
        for r in 0..rows {
            ui.horizontal(|ui| {
                for col in 0..(cols as usize) {
                    let idx = r * cols as usize + col;
                    if idx >= c.key_count as usize {
                        break;
                    }
                    let label = buttons
                        .get(idx)
                        .and_then(|b| b.next_step())
                        .map(|s| {
                            macro_names
                                .get(&s.macro_id)
                                .cloned()
                                .unwrap_or_else(|| "(deleted)".into())
                        })
                        .unwrap_or_else(|| "—".into());
                    let is_selected = macros_state.selected_streamdeck_button == Some(idx);
                    let fill = if is_selected {
                        theme::ACCENT_BLUE
                    } else {
                        theme::BG_INPUT
                    };
                    let stroke_color = if is_selected {
                        theme::ACCENT_BLUE
                    } else {
                        theme::BORDER_SUBTLE
                    };
                    let btn = egui::Button::new(
                        egui::RichText::new(label)
                            .color(theme::TEXT_PRIMARY)
                            .small(),
                    )
                    .fill(fill)
                    .stroke(egui::Stroke::new(1.0, stroke_color))
                    .corner_radius(4.0)
                    .min_size(egui::Vec2::new(cell_w, cell_h))
                    .truncate();
                    if ui.add_sized([cell_w, cell_h], btn).clicked() {
                        clicked_idx = Some(idx);
                    }
                    if col + 1 < cols as usize {
                        ui.add_space(spacing);
                    }
                }
            });
            if r + 1 < rows {
                ui.add_space(spacing);
            }
        }
        if let Some(idx) = clicked_idx {
            macros_state.selected_streamdeck_button = Some(idx);
            macros_state.streamdeck_add_step_target = None;
        }
    }

    // ── Inline per-button step editor (popup only) ──
    if macros_state.selected_streamdeck_button.is_some() {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        draw_streamdeck_button_editor(ui, macros_state, engine, config, macro_manager, runtime);
    }
}

/// Right-column editor for the currently-selected Stream Deck button.
/// Shows the step list (each step = "fire macro X") plus an add-step
/// combo. Step changes update the LCD label live via the engine.
fn draw_streamdeck_button_editor(
    ui: &mut egui::Ui,
    macros_state: &mut MacrosTabState,
    engine: &Arc<crate::console::streamdeck_engine::StreamDeckEngine>,
    config: &Arc<RwLock<crate::model::streamdeck::StreamDeckConfig>>,
    macro_manager: &Arc<RwLock<MacroManager>>,
    runtime: &tokio::runtime::Handle,
) {
    use crate::model::streamdeck::StreamDeckStep;

    let Some(button_idx) = macros_state.selected_streamdeck_button else {
        return;
    };

    // Snapshot config + macros for read-only display
    let cfg_snapshot = config
        .try_read()
        .ok()
        .map(|c| c.clone())
        .unwrap_or_default();
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

    let button = cfg_snapshot
        .buttons
        .get(button_idx)
        .cloned()
        .unwrap_or_default();
    let key_count = engine.connected_device().map(|c| c.key_count).unwrap_or(0);

    ui.horizontal(|ui| {
        theme::section_heading(ui, &format!("Stream Deck Button #{}", button_idx + 1));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(theme::action_button(
                    "Close",
                    theme::BG_ELEVATED,
                    egui::Vec2::new(60.0, 24.0),
                ))
                .on_hover_text("Deselect this button — go back to editing macros.")
                .clicked()
            {
                macros_state.selected_streamdeck_button = None;
                macros_state.streamdeck_add_step_target = None;
            }
        });
    });

    ui.label(
        egui::RichText::new(format!(
            "Each press fires the next step in this list, then advances the cursor. \
             Wraps back to step 1 after the last step. Currently next-to-fire: \
             {}",
            button
                .next_step()
                .map(|s| macro_names
                    .get(&s.macro_id)
                    .cloned()
                    .unwrap_or_else(|| "(deleted)".into()))
                .unwrap_or_else(|| "—".into()),
        ))
        .small()
        .color(theme::TEXT_SECONDARY),
    );

    ui.add_space(8.0);

    // ── Step list ──
    enum SdStepAction {
        MoveUp(usize),
        MoveDown(usize),
        Delete(usize),
    }
    let mut action: Option<SdStepAction> = None;

    if button.steps.is_empty() {
        ui.label(egui::RichText::new("No steps yet — add one below.").color(theme::TEXT_SECONDARY));
    } else {
        egui::ScrollArea::vertical()
            .id_salt("sd_button_step_scroll")
            .max_height((ui.available_height() - 140.0).max(80.0))
            .show(ui, |ui| {
                for (i, step) in button.steps.iter().enumerate() {
                    theme::elevated_frame().show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let is_current = i == button.current_step as usize;
                            theme::colored_badge(
                                ui,
                                &format!("#{}", i + 1),
                                if is_current {
                                    theme::ACCENT_BLUE
                                } else {
                                    theme::BG_ELEVATED
                                },
                            );
                            let name = macro_names
                                .get(&step.macro_id)
                                .cloned()
                                .unwrap_or_else(|| "(deleted)".into());
                            ui.label(egui::RichText::new(name).color(theme::TEXT_PRIMARY));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("Del").clicked() {
                                        action = Some(SdStepAction::Delete(i));
                                    }
                                    if i + 1 < button.steps.len() && ui.small_button("Dn").clicked()
                                    {
                                        action = Some(SdStepAction::MoveDown(i));
                                    }
                                    if i > 0 && ui.small_button("Up").clicked() {
                                        action = Some(SdStepAction::MoveUp(i));
                                    }
                                },
                            );
                        });
                    });
                    ui.add_space(2.0);
                }
            });
    }

    ui.add_space(8.0);

    // ── Add step ──
    theme::elevated_frame().show(ui, |ui| {
        ui.label(
            egui::RichText::new("Add step")
                .strong()
                .color(theme::TEXT_PRIMARY),
        );
        ui.horizontal(|ui| {
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
                    .width(220.0)
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
            if ui
                .add(theme::action_button(
                    "Add",
                    theme::ACCENT_GREEN,
                    egui::Vec2::new(60.0, 26.0),
                ))
                .clicked()
            {
                if let Some(macro_id) = macros_state.streamdeck_add_step_target {
                    let cfg = config.clone();
                    let mgr = macro_manager.clone();
                    let eng = engine.clone();
                    runtime.spawn(async move {
                        let mut cfg_w = cfg.write().await;
                        if let Some(b) = cfg_w.buttons.get_mut(button_idx) {
                            b.steps.push(StreamDeckStep { macro_id });
                            // Push the now-current step's label to LCD.
                            let mgr_r = mgr.read().await;
                            let label = b
                                .next_step()
                                .and_then(|s| mgr_r.get_macro(&s.macro_id))
                                .map(|m| m.name.clone())
                                .unwrap_or_default();
                            drop(mgr_r);
                            eng.refresh_button(button_idx as u8, label);
                        }
                    });
                }
            }
        });
    });

    // Apply deferred action.
    if let Some(act) = action {
        let cfg = config.clone();
        let mgr = macro_manager.clone();
        let eng = engine.clone();
        runtime.spawn(async move {
            let mut cfg_w = cfg.write().await;
            let Some(button) = cfg_w.buttons.get_mut(button_idx) else {
                return;
            };
            match act {
                SdStepAction::MoveUp(i) => {
                    if i > 0 && i < button.steps.len() {
                        button.steps.swap(i, i - 1);
                    }
                }
                SdStepAction::MoveDown(i) => {
                    if i + 1 < button.steps.len() {
                        button.steps.swap(i, i + 1);
                    }
                }
                SdStepAction::Delete(i) => {
                    if i < button.steps.len() {
                        button.steps.remove(i);
                        if button.current_step as usize >= button.steps.len() {
                            button.current_step = 0;
                        }
                    }
                }
            }
            let mgr_r = mgr.read().await;
            let label = button
                .next_step()
                .and_then(|s| mgr_r.get_macro(&s.macro_id))
                .map(|m| m.name.clone())
                .unwrap_or_default();
            drop(mgr_r);
            eng.refresh_button(button_idx as u8, label);
        });
    }

    // Sanity: if the UI selection points past the current device's
    // button count (e.g. operator deselected the device or swapped to
    // a smaller model), drop the selection.
    if (button_idx as u8) >= key_count {
        macros_state.selected_streamdeck_button = None;
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
