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

    egui::Window::new(title)
        .collapsible(false)
        .resizable(true)
        .default_width(1100.0)
        .open(&mut still_open)
        .show(ctx, |ui| {
            // Header row: name field + Save/Cancel buttons.
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.add(
                    egui::TextEdit::singleline(&mut state.name)
                        .desired_width(220.0)
                        .hint_text("Drummer, Keys, …"),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let cancel_btn = theme::action_button(
                        "Cancel",
                        theme::ACCENT_RED,
                        egui::Vec2::new(80.0, 28.0),
                    );
                    if ui.add(cancel_btn).clicked() {
                        outcome = Some(PickerOutcome::Cancel);
                    }

                    let save_btn = theme::action_button(
                        "Save",
                        theme::ACCENT_GREEN,
                        egui::Vec2::new(80.0, 28.0),
                    );
                    if ui.add_enabled(state.save_enabled(), save_btn).clicked() {
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

            // Status row pinned to the bottom of the window. We always
            // reserve the same vertical space so the layout doesn't jump
            // when the warning appears or disappears as the operator
            // selects/deselects channels.
            ui.allocate_ui_with_layout(
                egui::Vec2::new(ui.available_width(), 18.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
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

        ui.horizontal(|ui| {
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
            });
        });

        ui.add_space(4.0);

        if state.input_count == 0 {
            ui.colored_label(
                theme::TEXT_SECONDARY,
                "No inputs configured — connect to console or load a show file.",
            );
            return;
        }

        // Render the input tiles row by row.
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
                    if draw_tile(
                        ui,
                        &format!("Input {ch}"),
                        name,
                        theme::CH_INPUT,
                        order,
                        false,
                    )
                    .clicked()
                    {
                        state.toggle_input(ch);
                    }
                }
            });
            n += TILES_PER_INPUT_ROW;
        }
    });
}

fn draw_auxes_panel(ui: &mut egui::Ui, state: &mut ChannelPickerState) {
    let panel_width = TILE_SIZE.x * (TILES_PER_AUX_ROW as f32) + 24.0;
    ui.vertical(|ui| {
        ui.set_width(panel_width);

        ui.horizontal(|ui| {
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
        });

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
                    if draw_tile(ui, &format!("Aux {ch}"), name, theme::CH_AUX, order, stereo)
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
/// the convention used by the desk's picker.
fn draw_tile(
    ui: &mut egui::Ui,
    title: &str,
    name: &str,
    base_color: egui::Color32,
    order: Option<usize>,
    stereo: bool,
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
