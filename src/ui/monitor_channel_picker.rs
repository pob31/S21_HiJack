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

use super::channel_tile::{RippleHighlight, draw_tile};
use super::help::{HelpKey, help};
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
    /// Optional per-profile PIN (web login). Empty string = no PIN (name-only).
    pub pin: String,

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
        /// `None` when the PIN field is blank (name-only login).
        pin: Option<String>,
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
            pin: String::new(),
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
            pin: client.pin.clone().unwrap_or_default(),
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

        let pin = {
            let p = self.pin.trim();
            if p.is_empty() {
                None
            } else {
                Some(p.to_string())
            }
        };

        PickerOutcome::Save {
            editing: self.editing,
            name: self.name.trim().to_string(),
            pin,
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

const TILES_PER_INPUT_ROW: u8 = 10;
const TILES_PER_AUX_ROW: u8 = 4;
/// Gap between controls inside a panel header (labels / buttons). The grid rows
/// force `item_spacing.x = 0` for exact tile math, so the headers reset it to
/// this. Tile sizing itself is responsive — see [`theme::channel_tile_width`].
const HEADER_GAP: f32 = 8.0;

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

    // Clamp the window to the host viewport (an egui Window larger than the main
    // window's content area gets clipped). The grids inside are responsive —
    // tiles stretch to fill the window width — so there is no fixed content
    // width to cap, and the window stays freely resizable between a sensible
    // minimum (tiles at their floor) and the viewport.
    let avail = ctx.content_rect().size();
    let max_w = (avail.x - 16.0).max(360.0);
    let max_h = (avail.y - 16.0).max(320.0);
    let min_w = (theme::grids_min_width(TILES_PER_INPUT_ROW, TILES_PER_AUX_ROW) + 24.0).min(max_w);
    // Comfortable initial width: tiles at ~88 pt.
    let default_w = (theme::panel_width(88.0, TILES_PER_INPUT_ROW)
        + theme::TILE_COLUMN_GAP
        + theme::panel_width(88.0, TILES_PER_AUX_ROW)
        + 28.0)
        .clamp(min_w, max_w);
    egui::Window::new(title)
        .collapsible(false)
        .resizable(true)
        .default_width(default_w)
        .min_width(min_w)
        .max_width(max_w)
        .max_height(max_h)
        .open(&mut still_open)
        .show(ctx, |ui| {
            // Header row: name field + Save/Cancel buttons, all sized to
            // ROW_H so the label, text box and buttons share a baseline.
            ui.horizontal(|ui| {
                theme::row_label(ui, "Name:", theme::label_color());
                theme::padded_text_edit_sized(
                    ui,
                    &mut state.name,
                    220.0,
                    theme::ROW_H,
                    true,
                    "Drummer, Keys, …",
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::row_action_button(
                        ui,
                        "Cancel",
                        theme::ACCENT_RED,
                        80.0,
                        true,
                        help(HelpKey::MonitorPickerCancel),
                    ) {
                        outcome = Some(PickerOutcome::Cancel);
                    }
                    if theme::row_action_button(
                        ui,
                        "Save",
                        theme::ACCENT_GREEN,
                        80.0,
                        state.save_enabled(),
                        help(HelpKey::MonitorPickerSave),
                    ) {
                        outcome = Some(state.to_save_outcome());
                    }
                });
            });

            // Optional per-profile PIN (web login). Blank = name-only.
            // The explanation lives in a hover bubble so the row stays compact.
            ui.horizontal(|ui| {
                theme::row_label(ui, "PIN:", theme::label_color());
                theme::padded_text_edit_sized(
                    ui,
                    &mut state.pin,
                    220.0,
                    theme::ROW_H,
                    true,
                    "optional",
                )
                .on_hover_text(help(HelpKey::MonitorProfilePin));
            });

            ui.add_space(6.0);
            ui.separator();

            // Two-column grid: inputs on the left, auxes on the right. Tiles are
            // responsive — a single uniform width makes the two grids fill the
            // window, so resizing the window resizes the tiles (no clipping, no
            // horizontal scroll). Vertical scroll only, for a tall input count in
            // a short window. Headers and grids go in two separate shared rows so
            // the input and aux tile rows always line up.
            egui::ScrollArea::vertical()
                // Fill the width (responsive tiles) but shrink to content
                // vertically, so the window is only as tall as the channel
                // grids need — not always near-full-screen — while still
                // scrolling when a tall input count exceeds the viewport.
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    // A few px of slack so float rounding never clips the last
                    // aux column. The enclosing rows force item_spacing.x = 0 so
                    // the only inter-panel gap is the explicit COLUMN_GAP and the
                    // panel widths stay exact.
                    let avail = (ui.available_width() - 4.0).max(theme::TILE_W_MIN);
                    let tile_w =
                        theme::channel_tile_width(avail, TILES_PER_INPUT_ROW, TILES_PER_AUX_ROW);
                    let in_w = theme::panel_width(tile_w, TILES_PER_INPUT_ROW);
                    let aux_w = theme::panel_width(tile_w, TILES_PER_AUX_ROW);
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.horizontal_top(|ui| {
                        draw_inputs_header(ui, state, in_w);
                        ui.add_space(theme::TILE_COLUMN_GAP);
                        draw_auxes_header(ui, state, aux_w);
                    });
                    ui.add_space(4.0);
                    ui.horizontal_top(|ui| {
                        draw_inputs_grid(ui, state, tile_w, in_w);
                        ui.add_space(theme::TILE_COLUMN_GAP);
                        draw_auxes_grid(ui, state, tile_w, aux_w);
                    });
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
                                theme::label_weak(),
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

fn draw_inputs_header(ui: &mut egui::Ui, state: &mut ChannelPickerState, panel_w: f32) {
    ui.vertical(|ui| {
        ui.set_width(panel_w);
        ui.spacing_mut().item_spacing.x = HEADER_GAP; // reset (grid row forces 0)
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Inputs")
                    .strong()
                    .color(theme::label_color()),
            );
            ui.label(
                egui::RichText::new(format!(
                    "({} of {})",
                    state.selected_inputs.len(),
                    state.input_count
                ))
                .color(theme::label_weak())
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
                        if ui
                            .add(cancel_btn)
                            .on_hover_text(help(HelpKey::MonitorRippleCancel))
                            .clicked()
                        {
                            state.ripple = RippleState::Off;
                        }
                        let confirm_btn = theme::action_button(
                            "Confirm",
                            theme::ACCENT_GREEN,
                            egui::Vec2::new(80.0, 24.0),
                        );
                        if ui
                            .add(confirm_btn)
                            .on_hover_text(help(HelpKey::MonitorRippleConfirm))
                            .clicked()
                        {
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
                                theme::btn_neutral()
                            },
                            egui::Vec2::new(70.0, 24.0),
                        );
                        if ui
                            .add(ripple_btn)
                            .on_hover_text(help(HelpKey::MonitorRipple))
                            .clicked()
                        {
                            state.ripple = if ripple_armed {
                                RippleState::Off
                            } else {
                                RippleState::Pending
                            };
                        }

                        let deselect = theme::action_button(
                            "Deselect all",
                            theme::btn_neutral(),
                            egui::Vec2::new(95.0, 24.0),
                        );
                        if ui
                            .add(deselect)
                            .on_hover_text(help(HelpKey::MonitorDeselectAll))
                            .clicked()
                        {
                            state.selected_inputs.clear();
                        }
                        let select_all = theme::action_button(
                            "Select all",
                            theme::btn_neutral(),
                            egui::Vec2::new(85.0, 24.0),
                        );
                        if ui
                            .add(select_all)
                            .on_hover_text(help(HelpKey::MonitorSelectAll))
                            .clicked()
                        {
                            state.selected_inputs = (1..=state.input_count).collect();
                        }
                    }
                }
            });
        });
    });
}

fn draw_inputs_grid(ui: &mut egui::Ui, state: &mut ChannelPickerState, tile_w: f32, panel_w: f32) {
    ui.vertical(|ui| {
        ui.set_width(panel_w);
        ui.spacing_mut().item_spacing.x = theme::TILE_GAP; // reset (grid row forces 0)

        if state.input_count == 0 {
            ui.colored_label(
                theme::label_weak(),
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
                        egui::vec2(tile_w, theme::TILE_H),
                        &format!("Input {ch}"),
                        name,
                        theme::CH_INPUT,
                        order,
                        false,
                        highlight,
                        true,
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

fn draw_auxes_header(ui: &mut egui::Ui, state: &mut ChannelPickerState, panel_w: f32) {
    ui.vertical(|ui| {
        ui.set_width(panel_w);
        ui.spacing_mut().item_spacing.x = HEADER_GAP; // reset (grid row forces 0)
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Auxes")
                    .strong()
                    .color(theme::label_color()),
            );
            ui.label(
                egui::RichText::new(format!(
                    "({} of {})",
                    state.selected_auxes.len(),
                    state.aux_count
                ))
                .color(theme::label_weak())
                .small(),
            );
        });
    });
}

fn draw_auxes_grid(ui: &mut egui::Ui, state: &mut ChannelPickerState, tile_w: f32, panel_w: f32) {
    ui.vertical(|ui| {
        ui.set_width(panel_w);
        ui.spacing_mut().item_spacing.x = theme::TILE_GAP; // reset (grid row forces 0)

        if state.aux_count == 0 {
            ui.colored_label(
                theme::label_weak(),
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
                        egui::vec2(tile_w, theme::TILE_H),
                        &format!("Aux {ch}"),
                        name,
                        theme::CH_AUX,
                        order,
                        stereo,
                        RippleHighlight::None,
                        true,
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
                pin: _,
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
