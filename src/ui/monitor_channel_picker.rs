//! Visual channel picker used by the Monitor tab when adding or editing a
//! personal-monitoring profile. Renders coloured tiles for every input and
//! aux on the discovered console; the operator clicks tiles to toggle which
//! inputs they can see and which auxes they can mix.
//!
//! Layout follows the spirit of the desk's native picker — a wider Inputs
//! grid on the left, a narrower Auxes grid on the right, with a small dark
//! bar drawn down the right edge of stereo aux tiles.

use std::collections::{HashMap, HashSet};

use eframe::egui;
use uuid::Uuid;

use super::theme;
use crate::model::channel::ChannelId;
use crate::model::config::{ChannelMode, ConsoleConfig};
use crate::model::monitor::MonitorClient;
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
use crate::model::state::ConsoleState;

/// Mutable state for the channel picker window. The Monitor tab keeps
/// `Option<ChannelPickerState>` — `Some(_)` while the picker is open.
pub struct ChannelPickerState {
    /// `None` when adding a new client, `Some(id)` when editing an existing one.
    /// The Monitor tab uses this to choose between `add_client` and
    /// `update_client` on Save.
    pub editing: Option<Uuid>,
    pub name: String,

    pub input_count: u8,
    pub aux_count: u8,
    /// Selected channels in the order the operator clicked them. Used as the
    /// channel order saved into the profile. A small order badge (1, 2, 3, …)
    /// is rendered on each selected tile to make this order visible.
    pub selected_inputs: Vec<u8>,
    pub selected_auxes: Vec<u8>,

    /// Operator-given names for each channel. Empty entries fall back to the
    /// numeric label at render time.
    pub input_names: HashMap<u8, String>,
    pub aux_names: HashMap<u8, String>,
    /// Stereo aux numbers (1-based). Mono auxes are simply absent from the set.
    pub stereo_auxes: HashSet<u8>,

    /// Ripple-select state machine for the Inputs grid. While ripple is
    /// armed, tile clicks pick range endpoints instead of toggling the
    /// channel; the operator confirms or cancels the ripple before the
    /// range is appended to `selected_inputs`.
    pub ripple: RippleState,
}

/// State machine for the Ripple range-select flow on the Inputs grid.
///
/// The flow is: `Off` → click `Ripple` toggle → `Pending` → click first
/// channel → `GotFirst` → click last channel → `Confirming` → click ✓ →
/// `Off` (range applied) or click ✕ → `Off` (discarded). Re-clicking the
/// `Ripple` toggle in any non-`Confirming` state cancels back to `Off`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum RippleState {
    /// Normal mode: tile clicks toggle selection.
    #[default]
    Off,
    /// Operator armed Ripple — next tile click sets the first endpoint.
    Pending,
    /// First endpoint chosen — next tile click sets the last endpoint.
    GotFirst { first: u8 },
    /// Both endpoints chosen — operator must confirm or cancel before the
    /// range is committed.
    Confirming { first: u8, last: u8 },
}

/// Result of one frame of the picker window.
pub enum PickerOutcome {
    /// User clicked Save — caller commits the selection.
    Save {
        editing: Option<Uuid>,
        name: String,
        permitted_auxes: Vec<u8>,
        visible_inputs: Vec<u8>,
    },
    /// User cancelled (X, Cancel button, or escape).
    Cancel,
}

impl ChannelPickerState {
    /// Open the picker for a brand-new profile. Inputs default to all-selected
    /// (preserves the model's "empty visible_inputs = all" invariant when the
    /// operator saves without changing the input grid). Auxes default to none.
    pub fn for_new_client(state: &ConsoleState) -> Self {
        let cfg = &state.config;
        let input_count = cfg.input_channel_count;
        let aux_count = cfg.aux_output_count;

        // Default ordering = ascending. If the operator never deselects an
        // input, save will collapse this back to an empty `Vec` to preserve
        // the "all (and any future inputs)" sentinel.
        let selected_inputs: Vec<u8> = (1..=input_count).collect();
        let selected_auxes: Vec<u8> = Vec::new();

        let input_names = collect_input_names(state, input_count);
        let aux_names = collect_aux_names(state, aux_count);
        let stereo_auxes = collect_stereo_auxes(cfg, aux_count);

        Self {
            editing: None,
            name: String::new(),
            input_count,
            aux_count,
            selected_inputs,
            selected_auxes,
            input_names,
            aux_names,
            stereo_auxes,
            ripple: RippleState::Off,
        }
    }

    /// Open the picker prefilled from an existing client. An empty
    /// `visible_inputs` list (the "all inputs" sentinel) is rendered as
    /// every input selected — the same shape the operator sees for new
    /// profiles, so toggling tiles works the same way.
    pub fn for_edit(client: &MonitorClient, state: &ConsoleState) -> Self {
        let cfg = &state.config;
        let input_count = cfg.input_channel_count;
        let aux_count = cfg.aux_output_count;

        // Preserve the saved order. Empty `visible_inputs` (the "all inputs"
        // sentinel) is rendered as the canonical 1..=N order so toggling /
        // reordering works the same way as for a new profile.
        let selected_inputs: Vec<u8> = if client.visible_inputs.is_empty() {
            (1..=input_count).collect()
        } else {
            client.visible_inputs.clone()
        };
        let selected_auxes: Vec<u8> = client.permitted_auxes.clone();

        Self {
            editing: Some(client.id),
            name: client.name.clone(),
            input_count,
            aux_count,
            selected_inputs,
            selected_auxes,
            input_names: collect_input_names(state, input_count),
            aux_names: collect_aux_names(state, aux_count),
            stereo_auxes: collect_stereo_auxes(cfg, aux_count),
            ripple: RippleState::Off,
        }
    }

    /// Apply a confirmed ripple range to `selected_inputs`. Channels are
    /// appended in click direction (`first`→`last` ascending, or in reverse
    /// when `first > last`), skipping any channel already in the selection
    /// to preserve its existing position in the order list.
    fn apply_ripple(&mut self, first: u8, last: u8) {
        let range: Vec<u8> = if first <= last {
            (first..=last).collect()
        } else {
            (last..=first).rev().collect()
        };
        for ch in range {
            if !self.selected_inputs.contains(&ch) {
                self.selected_inputs.push(ch);
            }
        }
    }

    /// Toggle a channel in `selected_inputs`. Removing preserves the order of
    /// the remaining items; adding pushes to the end so the click order is
    /// visible in the order badges.
    fn toggle_input(&mut self, ch: u8) {
        if let Some(pos) = self.selected_inputs.iter().position(|&v| v == ch) {
            self.selected_inputs.remove(pos);
        } else {
            self.selected_inputs.push(ch);
        }
    }

    fn toggle_aux(&mut self, ch: u8) {
        if let Some(pos) = self.selected_auxes.iter().position(|&v| v == ch) {
            self.selected_auxes.remove(pos);
        } else {
            self.selected_auxes.push(ch);
        }
    }

    fn to_save_outcome(&self) -> PickerOutcome {
        // Collapse "all inputs selected in canonical 1..=N order" back to an
        // empty Vec so the profile keeps the future-proof "any input is
        // permitted" semantic. Any reordering away from canonical means the
        // operator cares about order — save the explicit list.
        let inputs_canonical = self.selected_inputs.len() as u8 == self.input_count
            && self
                .selected_inputs
                .iter()
                .enumerate()
                .all(|(i, &v)| v as usize == i + 1);
        let visible_inputs = if inputs_canonical {
            Vec::new()
        } else {
            self.selected_inputs.clone()
        };

        PickerOutcome::Save {
            editing: self.editing,
            name: self.name.trim().to_string(),
            permitted_auxes: self.selected_auxes.clone(),
            visible_inputs,
        }
    }

    fn save_enabled(&self) -> bool {
        !self.name.trim().is_empty() && !self.selected_auxes.is_empty()
    }
}

fn collect_input_names(state: &ConsoleState, input_count: u8) -> HashMap<u8, String> {
    let mut out = HashMap::new();
    for n in 1..=input_count {
        let name = state
            .get(&ParameterAddress {
                channel: ChannelId::Input(n),
                parameter: ParameterPath::Name,
            })
            .and_then(|v| match v {
                ParameterValue::String(s) if !s.is_empty() => Some(s.clone()),
                _ => None,
            });
        if let Some(s) = name {
            out.insert(n, s);
        }
    }
    out
}

fn collect_aux_names(state: &ConsoleState, aux_count: u8) -> HashMap<u8, String> {
    let mut out = HashMap::new();
    for n in 1..=aux_count {
        let name = state
            .get(&ParameterAddress {
                channel: ChannelId::Aux(n),
                parameter: ParameterPath::Name,
            })
            .and_then(|v| match v {
                ParameterValue::String(s) if !s.is_empty() => Some(s.clone()),
                _ => None,
            });
        if let Some(s) = name {
            out.insert(n, s);
        }
    }
    out
}

fn collect_stereo_auxes(cfg: &ConsoleConfig, aux_count: u8) -> HashSet<u8> {
    let mut out = HashSet::new();
    for n in 1..=aux_count {
        if matches!(cfg.aux_mode(n), Some(ChannelMode::Stereo)) {
            out.insert(n);
        }
    }
    out
}

const TILE_SIZE: egui::Vec2 = egui::Vec2::new(82.0, 52.0);
const TILES_PER_INPUT_ROW: u8 = 10;
const TILES_PER_AUX_ROW: u8 = 4;
/// Fixed height reserved for both the Inputs and Auxes panel headers
/// so their first tile row starts at the same y. Tall enough to fit
/// the 24 px Ripple / Confirm / Cancel buttons in the inputs header.
const PANEL_HEADER_H: f32 = 28.0;
/// Total width of the input + aux grid content (input panel + 16 px
/// gap + aux panel). The picker's title row, separators, and status
/// row all cap their content to this so Save/Cancel align with the
/// right edge of the aux tiles.
const GRIDS_CONTENT_W: f32 = TILE_SIZE.x * (TILES_PER_INPUT_ROW as f32)
    + 24.0
    + 16.0
    + TILE_SIZE.x * (TILES_PER_AUX_ROW as f32)
    + 24.0;

/// Draw the picker window for one frame. The caller owns the open/closed
/// flag and the picker state; this function returns `Some(outcome)` when
/// the operator clicks Save or Cancel (in which case the caller should
/// drop the state and close the window).
pub fn draw_channel_picker(
    ctx: &egui::Context,
    state: &mut ChannelPickerState,
) -> Option<PickerOutcome> {
    let mut outcome: Option<PickerOutcome> = None;
    let mut still_open = true;

    let title = if state.editing.is_some() {
        "Edit Monitor Profile"
    } else {
        "Add Monitor Profile"
    };

    // Window default width matches the grid content + a small chrome
    // budget so the operator doesn't have to resize on first open.
    egui::Window::new(title)
        .collapsible(false)
        .resizable(true)
        .default_width(GRIDS_CONTENT_W + 32.0)
        .open(&mut still_open)
        .show(ctx, |ui| {
            // Cap the entire picker content to the grid width so the
            // header's Save / Cancel buttons (right_to_left) end at
            // the same x as the aux tiles' right edge.
            let content_w = GRIDS_CONTENT_W.min(ui.available_width());
            ui.set_min_width(content_w);
            ui.set_max_width(content_w);
            // Header row: name field + Save/Cancel buttons, all sized to
            // ROW_H so the label, text box and buttons share a baseline.
            ui.horizontal(|ui| {
                theme::row_label(ui, "Name:", theme::TEXT_PRIMARY);
                theme::padded_text_edit_sized(
                    ui,
                    &mut state.name,
                    220.0,
                    theme::ROW_H,
                    true,
                    "Drummer, Keys, …",
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::row_action_button(ui, "Cancel", theme::ACCENT_RED, 80.0, true) {
                        outcome = Some(PickerOutcome::Cancel);
                    }
                    if theme::row_action_button(
                        ui,
                        "Save",
                        theme::ACCENT_GREEN,
                        80.0,
                        state.save_enabled(),
                    ) {
                        outcome = Some(state.to_save_outcome());
                    }
                });
            });

            ui.add_space(6.0);
            ui.separator();

            // Two-column grid: inputs on the left, auxes on the right.
            ui.horizontal_top(|ui| {
                draw_inputs_panel(ui, state);
                ui.add_space(16.0);
                draw_auxes_panel(ui, state);
            });

            ui.add_space(6.0);
            ui.separator();

            // Status row pinned to the bottom of the window. Always reserves
            // the same vertical space so the layout doesn't jump when the
            // message changes. While a ripple is in progress the status
            // doubles as the prompt that tells the operator what to click
            // next; otherwise it shows save-readiness.
            ui.allocate_ui_with_layout(
                egui::Vec2::new(ui.available_width(), 18.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| match state.ripple {
                    RippleState::Pending => {
                        ui.colored_label(
                            theme::ACCENT_ORANGE,
                            "Ripple: click the FIRST channel of the range.",
                        );
                    }
                    RippleState::GotFirst { first } => {
                        ui.colored_label(
                            theme::ACCENT_ORANGE,
                            format!(
                                "Ripple: first = {first}. Click the LAST channel of the range."
                            ),
                        );
                    }
                    RippleState::Confirming { first, last } => {
                        let count = if first <= last {
                            (last - first + 1) as usize
                        } else {
                            (first - last + 1) as usize
                        };
                        let plural = if count == 1 { "" } else { "s" };
                        let msg = if first <= last {
                            format!(
                                "Ripple: add channels {first} to {last} ({count} channel{plural}) \
                                 to the selection?"
                            )
                        } else {
                            format!(
                                "Ripple: add channels {first} down to {last} ({count} channel\
                                 {plural}, reverse order) to the selection?"
                            )
                        };
                        ui.colored_label(theme::ACCENT_ORANGE, msg);
                    }
                    RippleState::Off => {
                        if state.save_enabled() {
                            ui.colored_label(
                                theme::TEXT_SECONDARY,
                                format!(
                                    "Ready to save — {} input(s), {} aux(es) selected.",
                                    state.selected_inputs.len(),
                                    state.selected_auxes.len(),
                                ),
                            );
                        } else {
                            let reason = if state.name.trim().is_empty() {
                                "Enter a profile name."
                            } else {
                                "Select at least one aux."
                            };
                            ui.colored_label(theme::TEXT_WARNING, reason);
                        }
                    }
                },
            );
        });

    if !still_open && outcome.is_none() {
        outcome = Some(PickerOutcome::Cancel);
    }
    outcome
}

fn draw_inputs_panel(ui: &mut egui::Ui, state: &mut ChannelPickerState) {
    let panel_width = TILE_SIZE.x * (TILES_PER_INPUT_ROW as f32) + 24.0;
    ui.vertical(|ui| {
        ui.set_width(panel_width);

        // Fixed-height header so the first tile row aligns with the
        // aux panel's first tile row.
        ui.allocate_ui_with_layout(
            egui::Vec2::new(panel_width, PANEL_HEADER_H),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new("Inputs")
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "({} of {})",
                        state.selected_inputs.len(),
                        state.input_count
                    ))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match state.ripple {
                        RippleState::Confirming { first, last } => {
                            // Confirmation pair replaces the normal header buttons.
                            // Order in right-to-left layout: Cancel on the right,
                            // Confirm to its left. Plain text labels because the
                            // bundled egui font doesn't render check-mark / X
                            // dingbats.
                            let cancel_btn = theme::action_button(
                                "Cancel",
                                theme::ACCENT_RED,
                                egui::Vec2::new(70.0, 24.0),
                            );
                            if ui.add(cancel_btn).clicked() {
                                state.ripple = RippleState::Off;
                            }
                            let confirm_btn = theme::action_button(
                                "Confirm",
                                theme::ACCENT_GREEN,
                                egui::Vec2::new(80.0, 24.0),
                            );
                            if ui.add(confirm_btn).clicked() {
                                state.apply_ripple(first, last);
                                state.ripple = RippleState::Off;
                            }
                        }
                        _ => {
                            // Ripple toggle button — orange when armed, neutral otherwise.
                            // Clicking while armed (Pending / GotFirst) cancels back to Off.
                            let ripple_armed = matches!(
                                state.ripple,
                                RippleState::Pending | RippleState::GotFirst { .. }
                            );
                            let ripple_btn = theme::action_button(
                                "Ripple",
                                if ripple_armed {
                                    theme::ACCENT_ORANGE
                                } else {
                                    theme::BG_ELEVATED
                                },
                                egui::Vec2::new(70.0, 24.0),
                            );
                            if ui.add(ripple_btn).clicked() {
                                state.ripple = if ripple_armed {
                                    RippleState::Off
                                } else {
                                    RippleState::Pending
                                };
                            }

                            let deselect = theme::action_button(
                                "Deselect all",
                                theme::BG_ELEVATED,
                                egui::Vec2::new(95.0, 24.0),
                            );
                            if ui.add(deselect).clicked() {
                                state.selected_inputs.clear();
                            }
                            let select_all = theme::action_button(
                                "Select all",
                                theme::BG_ELEVATED,
                                egui::Vec2::new(85.0, 24.0),
                            );
                            if ui.add(select_all).clicked() {
                                state.selected_inputs = (1..=state.input_count).collect();
                            }
                        }
                    }
                });
            },
        );

        ui.add_space(4.0);

        if state.input_count == 0 {
            ui.colored_label(
                theme::TEXT_SECONDARY,
                "No inputs configured — connect to console or load a show file.",
            );
            return;
        }

        // Render the input tiles row by row. During ripple, tile clicks
        // pick range endpoints instead of toggling selection. Endpoints
        // get an orange outline overlay; tiles between endpoints during
        // Confirming get a translucent orange tint to preview the range.
        let mut next_ripple: Option<RippleState> = None;
        let mut n: u8 = 1;
        while n <= state.input_count {
            ui.horizontal(|ui| {
                let row_end = (n + TILES_PER_INPUT_ROW - 1).min(state.input_count);
                for ch in n..=row_end {
                    let order = state
                        .selected_inputs
                        .iter()
                        .position(|&v| v == ch)
                        .map(|i| i + 1);
                    let name = state.input_names.get(&ch).map(String::as_str).unwrap_or("");
                    let highlight = ripple_highlight_for(state.ripple, ch);
                    let tile_resp = draw_tile(
                        ui,
                        &format!("Input {ch}"),
                        name,
                        theme::CH_INPUT,
                        order,
                        false,
                        highlight,
                    );
                    if tile_resp.clicked() {
                        match state.ripple {
                            RippleState::Off => state.toggle_input(ch),
                            RippleState::Pending => {
                                next_ripple = Some(RippleState::GotFirst { first: ch });
                            }
                            RippleState::GotFirst { first } => {
                                next_ripple = Some(RippleState::Confirming { first, last: ch });
                            }
                            // While the operator is staring at the
                            // confirm/cancel buttons, tile clicks are no-ops
                            // — they have to commit one way or the other.
                            RippleState::Confirming { .. } => {}
                        }
                    }
                }
            });
            n += TILES_PER_INPUT_ROW;
        }
        if let Some(r) = next_ripple {
            state.ripple = r;
        }
    });
}

/// Decide what kind of ripple-mode highlight (if any) a tile should get.
fn ripple_highlight_for(ripple: RippleState, ch: u8) -> RippleHighlight {
    match ripple {
        RippleState::Off | RippleState::Pending => RippleHighlight::None,
        RippleState::GotFirst { first } => {
            if ch == first {
                RippleHighlight::Endpoint
            } else {
                RippleHighlight::None
            }
        }
        RippleState::Confirming { first, last } => {
            if ch == first || ch == last {
                RippleHighlight::Endpoint
            } else {
                let (lo, hi) = if first <= last {
                    (first, last)
                } else {
                    (last, first)
                };
                if ch > lo && ch < hi {
                    RippleHighlight::InRange
                } else {
                    RippleHighlight::None
                }
            }
        }
    }
}

/// What kind of ripple-mode visual marker a tile should carry. Layered on
/// top of the standard tile fill / selected outline so existing selection
/// state is still visible while the operator picks a ripple range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RippleHighlight {
    /// No ripple decoration.
    None,
    /// One of the two range endpoints — bright orange outline on top of the
    /// tile so the operator can see which two tiles they've picked.
    Endpoint,
    /// Sits between the two endpoints during `Confirming` — translucent
    /// orange overlay previewing what's about to be added.
    InRange,
}

fn draw_auxes_panel(ui: &mut egui::Ui, state: &mut ChannelPickerState) {
    let panel_width = TILE_SIZE.x * (TILES_PER_AUX_ROW as f32) + 24.0;
    ui.vertical(|ui| {
        ui.set_width(panel_width);

        // Fixed-height header so the first tile row aligns with the
        // input panel's first tile row.
        ui.allocate_ui_with_layout(
            egui::Vec2::new(panel_width, PANEL_HEADER_H),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new("Auxes")
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "({} of {})",
                        state.selected_auxes.len(),
                        state.aux_count
                    ))
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );
            },
        );

        ui.add_space(4.0);

        if state.aux_count == 0 {
            ui.colored_label(
                theme::TEXT_SECONDARY,
                "No auxes configured — connect to console or load a show file.",
            );
            return;
        }

        let mut n: u8 = 1;
        while n <= state.aux_count {
            ui.horizontal(|ui| {
                let row_end = (n + TILES_PER_AUX_ROW - 1).min(state.aux_count);
                for ch in n..=row_end {
                    let order = state
                        .selected_auxes
                        .iter()
                        .position(|&v| v == ch)
                        .map(|i| i + 1);
                    let stereo = state.stereo_auxes.contains(&ch);
                    let name = state.aux_names.get(&ch).map(String::as_str).unwrap_or("");
                    if draw_tile(
                        ui,
                        &format!("Aux {ch}"),
                        name,
                        theme::CH_AUX,
                        order,
                        stereo,
                        RippleHighlight::None,
                    )
                    .clicked()
                    {
                        state.toggle_aux(ch);
                    }
                }
            });
            n += TILES_PER_AUX_ROW;
        }
    });
}

/// One coloured channel tile. Returns the click response so the caller can
/// toggle selection state. `order` is the 1-based position of this channel
/// in the picker's selection list (`None` = not currently selected). When
/// `stereo` is true a darker vertical bar is painted along the right edge —
/// the convention used by the desk's picker. `ripple_highlight` overlays
/// an orange marker for endpoints / range-preview when a ripple is in
/// progress.
fn draw_tile(
    ui: &mut egui::Ui,
    title: &str,
    name: &str,
    base_color: egui::Color32,
    order: Option<usize>,
    stereo: bool,
    ripple_highlight: RippleHighlight,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(TILE_SIZE, egui::Sense::click());

    let painter = ui.painter_at(rect);
    let selected = order.is_some();

    // Tile fill: full-saturation when selected, dimmed when not. Hover gets a
    // slight tint so the operator sees what's about to be clicked.
    let fill = if selected {
        base_color
    } else {
        // Mix base_color towards BG_ELEVATED at ~30% strength.
        blend(base_color, theme::BG_ELEVATED, 0.7)
    };
    let hover_fill = if response.hovered() {
        blend(fill, theme::TEXT_PRIMARY, 0.85)
    } else {
        fill
    };
    painter.rect_filled(rect, 4.0, hover_fill);

    // Outer stroke: bright when selected, subtle otherwise.
    let stroke = if selected {
        egui::Stroke::new(2.0, theme::TEXT_PRIMARY)
    } else {
        egui::Stroke::new(1.0, theme::BORDER_SUBTLE)
    };
    painter.rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);

    // Stereo bar: a darker rectangle pinned to the right edge.
    if stereo {
        let bar_rect = egui::Rect::from_min_max(
            egui::pos2(rect.max.x - 6.0, rect.min.y + 4.0),
            egui::pos2(rect.max.x - 2.0, rect.max.y - 4.0),
        );
        let bar_fill = blend(fill, egui::Color32::BLACK, 0.55);
        painter.rect_filled(bar_rect, 2.0, bar_fill);
    }

    // Ripple decoration — drawn on top of the standard tile so existing
    // selection state (the white outline + order badge) is still visible
    // while the operator picks a range.
    match ripple_highlight {
        RippleHighlight::None => {}
        RippleHighlight::Endpoint => {
            painter.rect_stroke(
                rect,
                4.0,
                egui::Stroke::new(3.0, theme::ACCENT_ORANGE),
                egui::StrokeKind::Inside,
            );
        }
        RippleHighlight::InRange => {
            // Translucent orange wash over the tile to preview the range.
            let overlay = egui::Color32::from_rgba_premultiplied(
                theme::ACCENT_ORANGE.r() / 3,
                theme::ACCENT_ORANGE.g() / 3,
                theme::ACCENT_ORANGE.b() / 3,
                90,
            );
            painter.rect_filled(rect, 4.0, overlay);
        }
    }

    // Title (top line) and name (bottom line). Name falls back to nothing
    // when offline / the console hasn't reported it.
    let title_pos = egui::pos2(rect.center().x, rect.min.y + 14.0);
    painter.text(
        title_pos,
        egui::Align2::CENTER_CENTER,
        title,
        egui::FontId::proportional(13.0),
        theme::TEXT_PRIMARY,
    );
    if !name.is_empty() {
        let name_pos = egui::pos2(rect.center().x, rect.min.y + 34.0);
        painter.text(
            name_pos,
            egui::Align2::CENTER_CENTER,
            name,
            egui::FontId::proportional(11.0),
            theme::TEXT_PRIMARY,
        );
    }

    // Order badge: small dark circle in the bottom-right corner of the tile
    // showing the click order (1, 2, 3, …). Visible only on selected tiles.
    // Bottom-right placement keeps it clear of the title and name text. When
    // the tile also has the stereo bar on the right edge, the badge nudges
    // slightly further left so the two don't fight for the same pixels.
    if let Some(n) = order {
        let right_inset = if stereo { 16.0 } else { 11.0 };
        let badge_center = egui::pos2(rect.max.x - right_inset, rect.max.y - 11.0);
        let badge_radius = 9.0;
        painter.circle_filled(badge_center, badge_radius, theme::BG_DARK);
        painter.circle_stroke(
            badge_center,
            badge_radius,
            egui::Stroke::new(1.0, theme::TEXT_PRIMARY),
        );
        painter.text(
            badge_center,
            egui::Align2::CENTER_CENTER,
            n.to_string(),
            egui::FontId::proportional(10.0),
            theme::TEXT_PRIMARY,
        );
    }

    response.on_hover_text(if name.is_empty() {
        title.to_string()
    } else {
        format!("{title} — {name}")
    })
}

fn blend(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    // Linear blend `t` of `a` towards `b`. t=1.0 returns a, t=0.0 returns b.
    let lerp = |x: u8, y: u8| ((x as f32) * t + (y as f32) * (1.0 - t)).round() as u8;
    egui::Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::{ChannelMode, ConsoleConfig};
    use crate::model::state::ConsoleState;

    fn config_with_inputs_and_auxes(input_count: u8, aux_count: u8) -> ConsoleConfig {
        let mut c = ConsoleConfig::default();
        c.input_channel_count = input_count;
        c.aux_output_count = aux_count;
        c.group_output_count = 0;
        c.mix_output_types = (0..aux_count).map(|_| true).collect();
        c.mix_output_modes = (0..aux_count).map(|_| ChannelMode::Mono).collect();
        c
    }

    #[test]
    fn new_client_picker_starts_with_all_inputs_no_auxes() {
        let state = ConsoleState::new(config_with_inputs_and_auxes(48, 8));
        let picker = ChannelPickerState::for_new_client(&state);
        assert_eq!(picker.selected_inputs.len(), 48);
        assert!(picker.selected_auxes.is_empty());
        assert_eq!(picker.editing, None);
        assert!(picker.name.is_empty());
    }

    #[test]
    fn save_collapses_canonical_input_selection_to_empty_vec() {
        let state = ConsoleState::new(config_with_inputs_and_auxes(48, 8));
        let mut picker = ChannelPickerState::for_new_client(&state);
        picker.name = "Drummer".into();
        picker.toggle_aux(1);

        match picker.to_save_outcome() {
            PickerOutcome::Save {
                visible_inputs,
                permitted_auxes,
                name,
                editing,
            } => {
                assert!(
                    visible_inputs.is_empty(),
                    "1..=N in canonical order must save as empty Vec",
                );
                assert_eq!(permitted_auxes, vec![1]);
                assert_eq!(name, "Drummer");
                assert!(editing.is_none());
            }
            PickerOutcome::Cancel => panic!("expected Save"),
        }
    }

    #[test]
    fn save_preserves_partial_input_selection_in_click_order() {
        let state = ConsoleState::new(config_with_inputs_and_auxes(48, 8));
        let mut picker = ChannelPickerState::for_new_client(&state);
        picker.name = "Bass".into();
        picker.toggle_aux(2);
        // Deselect 5 and 10 — leaves 46 inputs in 1..=N order minus 5,10.
        picker.toggle_input(5);
        picker.toggle_input(10);

        match picker.to_save_outcome() {
            PickerOutcome::Save { visible_inputs, .. } => {
                assert_eq!(visible_inputs.len(), 46);
                assert!(!visible_inputs.contains(&5));
                assert!(!visible_inputs.contains(&10));
                // The remaining 46 inputs are still in canonical order
                // because we never reordered, just removed two.
                assert!(visible_inputs.windows(2).all(|w| w[0] < w[1]));
            }
            PickerOutcome::Cancel => panic!("expected Save"),
        }
    }

    #[test]
    fn save_preserves_click_order_when_reselected() {
        // Operator deselects everything, then clicks 7, 1, 12 in that order.
        // Save must preserve that exact order.
        let state = ConsoleState::new(config_with_inputs_and_auxes(48, 8));
        let mut picker = ChannelPickerState::for_new_client(&state);
        picker.name = "Keys".into();
        picker.toggle_aux(3);
        picker.selected_inputs.clear();
        picker.toggle_input(7);
        picker.toggle_input(1);
        picker.toggle_input(12);

        match picker.to_save_outcome() {
            PickerOutcome::Save {
                visible_inputs,
                permitted_auxes,
                ..
            } => {
                assert_eq!(visible_inputs, vec![7, 1, 12]);
                assert_eq!(permitted_auxes, vec![3]);
            }
            PickerOutcome::Cancel => panic!("expected Save"),
        }
    }

    #[test]
    fn save_preserves_aux_click_order() {
        let state = ConsoleState::new(config_with_inputs_and_auxes(48, 8));
        let mut picker = ChannelPickerState::for_new_client(&state);
        picker.name = "FOH".into();
        // Click order: 4, 2, 7
        picker.toggle_aux(4);
        picker.toggle_aux(2);
        picker.toggle_aux(7);

        match picker.to_save_outcome() {
            PickerOutcome::Save {
                permitted_auxes, ..
            } => {
                assert_eq!(permitted_auxes, vec![4, 2, 7]);
            }
            PickerOutcome::Cancel => panic!("expected Save"),
        }
    }

    #[test]
    fn save_preserves_full_selection_in_custom_order() {
        // Operator wants all inputs but in a custom order — must NOT collapse
        // to the empty-Vec sentinel because the order is meaningful.
        let state = ConsoleState::new(config_with_inputs_and_auxes(4, 8));
        let mut picker = ChannelPickerState::for_new_client(&state);
        picker.name = "Custom".into();
        picker.toggle_aux(1);
        picker.selected_inputs = vec![3, 1, 4, 2];

        match picker.to_save_outcome() {
            PickerOutcome::Save { visible_inputs, .. } => {
                assert_eq!(visible_inputs, vec![3, 1, 4, 2]);
            }
            PickerOutcome::Cancel => panic!("expected Save"),
        }
    }

    #[test]
    fn toggle_input_removes_then_re_adds_at_end() {
        let state = ConsoleState::new(config_with_inputs_and_auxes(8, 4));
        let mut picker = ChannelPickerState::for_new_client(&state);
        // Starts as [1..=8].
        picker.toggle_input(3); // removes 3 → [1, 2, 4, 5, 6, 7, 8]
        assert_eq!(picker.selected_inputs, vec![1, 2, 4, 5, 6, 7, 8]);
        picker.toggle_input(3); // re-adds at end → [1, 2, 4, 5, 6, 7, 8, 3]
        assert_eq!(picker.selected_inputs, vec![1, 2, 4, 5, 6, 7, 8, 3]);
    }

    #[test]
    fn edit_picker_loads_existing_order_verbatim() {
        let state = ConsoleState::new(config_with_inputs_and_auxes(60, 8));
        let client = MonitorClient::new("Custom".into(), vec![5, 1, 3], vec![10, 4, 7]);
        let picker = ChannelPickerState::for_edit(&client, &state);
        assert_eq!(picker.selected_auxes, vec![5, 1, 3]);
        assert_eq!(picker.selected_inputs, vec![10, 4, 7]);
        assert_eq!(picker.editing, Some(client.id));
    }

    #[test]
    fn edit_picker_expands_empty_visible_inputs_to_all() {
        let state = ConsoleState::new(config_with_inputs_and_auxes(60, 8));
        let client = MonitorClient::new("FOH".into(), vec![1, 2], vec![]);
        let picker = ChannelPickerState::for_edit(&client, &state);
        assert_eq!(picker.selected_inputs.len(), 60);
        assert_eq!(picker.selected_auxes.len(), 2);
        assert_eq!(picker.editing, Some(client.id));
    }

    #[test]
    fn ripple_ascending_appends_in_order_skipping_duplicates() {
        let state = ConsoleState::new(config_with_inputs_and_auxes(20, 4));
        let mut picker = ChannelPickerState::for_new_client(&state);
        // Start clean: deselect all and pre-select a couple of channels
        // so we can verify `apply_ripple` skips them in place.
        picker.selected_inputs.clear();
        picker.selected_inputs.push(15);
        picker.selected_inputs.push(5);

        picker.apply_ripple(3, 7);
        // 3, 4 added; 5 already selected (kept at original position 1);
        // 6, 7 added.
        assert_eq!(picker.selected_inputs, vec![15, 5, 3, 4, 6, 7]);
    }

    #[test]
    fn ripple_descending_appends_in_reverse() {
        let state = ConsoleState::new(config_with_inputs_and_auxes(20, 4));
        let mut picker = ChannelPickerState::for_new_client(&state);
        picker.selected_inputs.clear();

        // first > last: append in reverse direction.
        picker.apply_ripple(10, 5);
        assert_eq!(picker.selected_inputs, vec![10, 9, 8, 7, 6, 5]);
    }

    #[test]
    fn ripple_single_channel_when_endpoints_equal() {
        let state = ConsoleState::new(config_with_inputs_and_auxes(20, 4));
        let mut picker = ChannelPickerState::for_new_client(&state);
        picker.selected_inputs.clear();

        picker.apply_ripple(7, 7);
        assert_eq!(picker.selected_inputs, vec![7]);
    }

    #[test]
    fn ripple_state_default_is_off() {
        let state = ConsoleState::new(config_with_inputs_and_auxes(20, 4));
        let picker = ChannelPickerState::for_new_client(&state);
        assert_eq!(picker.ripple, RippleState::Off);
    }

    #[test]
    fn ripple_descending_skips_already_selected_in_reverse_walk() {
        let state = ConsoleState::new(config_with_inputs_and_auxes(20, 4));
        let mut picker = ChannelPickerState::for_new_client(&state);
        picker.selected_inputs.clear();
        picker.selected_inputs.push(8);

        // Descending 10 → 5: 10, 9 added, 8 skipped (already there), 7, 6, 5 added.
        picker.apply_ripple(10, 5);
        assert_eq!(picker.selected_inputs, vec![8, 10, 9, 7, 6, 5]);
    }

    #[test]
    fn save_disabled_when_name_or_auxes_missing() {
        let state = ConsoleState::new(config_with_inputs_and_auxes(48, 8));
        let mut picker = ChannelPickerState::for_new_client(&state);
        assert!(!picker.save_enabled(), "no name, no auxes");

        picker.name = "Keys".into();
        assert!(!picker.save_enabled(), "name without auxes");

        picker.toggle_aux(1);
        assert!(picker.save_enabled());

        picker.name = "   ".into();
        assert!(!picker.save_enabled(), "whitespace-only name doesn't count");
    }

    #[test]
    fn stereo_auxes_picked_up_from_config() {
        let mut cfg = config_with_inputs_and_auxes(48, 4);
        cfg.mix_output_modes = vec![
            ChannelMode::Mono,
            ChannelMode::Stereo,
            ChannelMode::Mono,
            ChannelMode::Stereo,
        ];
        let state = ConsoleState::new(cfg);
        let picker = ChannelPickerState::for_new_client(&state);
        assert!(!picker.stereo_auxes.contains(&1));
        assert!(picker.stereo_auxes.contains(&2));
        assert!(!picker.stereo_auxes.contains(&3));
        assert!(picker.stereo_auxes.contains(&4));
    }
}
