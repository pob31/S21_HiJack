//! Visual member picker used by the Gangs tab when creating or editing a gang.
//!
//! Unlike the Monitor picker (inputs + auxes only), a gang can mix *any* of the
//! seven channel types, so this picker lays out two panes inside a modal
//! window: a wide **Available** pane on the left listing every channel grouped
//! by type as coloured tiles, and a narrower **Linked** pane on the right
//! showing the channels currently in the gang. Click a left tile to add it;
//! click a right tile to remove it. Shift-click on the left extends a range
//! within a single type group.
//!
//! Reuses the shared [`super::channel_tile::draw_tile`] renderer and the
//! responsive `theme::channel_tile_width`/`panel_width` sizing, and enumerates
//! channels through [`ChannelGroup`] (the same source the scope editor uses).

use std::collections::{HashMap, HashSet};

use eframe::egui;

use super::channel_tile::{RippleHighlight, draw_tile};
use super::theme;
use crate::model::channel::ChannelId;
use crate::model::config::{ChannelMode, ConsoleConfig};
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
use crate::model::state::ConsoleState;
use crate::ui::scope_editor::ChannelGroup;

/// Tiles per row in the Available pane (the widest group — Inputs — sets the
/// pane width; narrower groups left-align within it).
const AVAIL_COLS: u8 = 8;
/// Tiles per row in the Linked pane.
const LINKED_COLS: u8 = 4;

/// Mutable state for the gang member picker window. The Gangs tab keeps
/// `Option<GangMemberPickerState>` — `Some(_)` while the picker is open.
pub struct GangMemberPickerState {
    /// Drives the window title only (the chosen members flow back the same way
    /// whether creating or editing).
    pub editing: bool,
    /// Working set of selected members. A `HashSet` for O(1) membership tests;
    /// display / save order is always re-derived deterministically (see
    /// [`Self::ordered_members`]), so the set's own ordering is irrelevant.
    pub selected: HashSet<ChannelId>,
    /// Per-group shift-click anchor (channel number within that group's number
    /// space). Keyed by `ChannelGroup` so a range never bridges channel types.
    pub anchor: HashMap<ChannelGroup, u8>,
    /// Every available channel, grouped by type, computed once at open from the
    /// live config. Empty groups (e.g. 0 matrices configured) are skipped so the
    /// Available pane never renders an empty section.
    pub groups: Vec<(ChannelGroup, Vec<ChannelId>)>,
    /// Operator-given channel names (empty / missing → numeric label only).
    pub names: HashMap<ChannelId, String>,
    /// Channels whose tile should draw the stereo bar (stereo auxes / groups).
    pub stereo: HashSet<ChannelId>,
}

/// Result of one frame of the picker window.
pub enum GangPickerOutcome {
    /// Operator clicked Save — carries the chosen members in deterministic
    /// (group, ascending-number) order.
    Save(Vec<ChannelId>),
    /// Operator cancelled (Cancel button, window close, or escape).
    Cancel,
}

impl GangMemberPickerState {
    /// Open the picker seeded from `initial` (empty for a new gang, the gang's
    /// current members when editing).
    pub fn new(initial: &[ChannelId], editing: bool, state: &ConsoleState) -> Self {
        let cfg = &state.config;
        let groups: Vec<(ChannelGroup, Vec<ChannelId>)> = ChannelGroup::all()
            .iter()
            .map(|g| (*g, g.channels_from(cfg)))
            .filter(|(_, chans)| !chans.is_empty())
            .collect();
        let names = collect_names(state, &groups);
        let stereo = collect_stereo(cfg, &groups);
        Self {
            editing,
            selected: initial.iter().cloned().collect(),
            anchor: HashMap::new(),
            groups,
            names,
            stereo,
        }
    }

    /// Selected members in deterministic display / save order: group order
    /// (as in `groups`), then ascending channel number within each group.
    fn ordered_members(&self) -> Vec<ChannelId> {
        let mut out = Vec::new();
        for (_g, chans) in &self.groups {
            for ch in chans {
                if self.selected.contains(ch) {
                    out.push(ch.clone());
                }
            }
        }
        out
    }

    /// Handle a click on an Available tile. Plain click toggles the channel and
    /// re-anchors; shift-click fills the contiguous range from this group's
    /// anchor to the clicked number — only within `group`'s own channel list,
    /// so a range never spans channel types.
    fn apply_click(&mut self, group: ChannelGroup, ch: &ChannelId, num: u8, shift: bool) {
        if shift {
            if let Some(&anchor) = self.anchor.get(&group) {
                let (lo, hi) = if anchor <= num {
                    (anchor, num)
                } else {
                    (num, anchor)
                };
                // Collect first (releases the `groups` borrow) then insert.
                let in_range: Vec<ChannelId> = self
                    .groups
                    .iter()
                    .find(|(g, _)| *g == group)
                    .map(|(_, chans)| {
                        chans
                            .iter()
                            .filter(|c| (lo..=hi).contains(&channel_number(c)))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                for c in in_range {
                    self.selected.insert(c);
                }
                self.anchor.insert(group, num);
                return;
            }
        }
        // Plain click — toggle, then re-anchor.
        if !self.selected.insert(ch.clone()) {
            self.selected.remove(ch);
        }
        self.anchor.insert(group, num);
    }
}

/// Inner channel number of any `ChannelId` variant.
fn channel_number(ch: &ChannelId) -> u8 {
    match ch {
        ChannelId::Input(n)
        | ChannelId::Aux(n)
        | ChannelId::Group(n)
        | ChannelId::Matrix(n)
        | ChannelId::ControlGroup(n)
        | ChannelId::GraphicEq(n)
        | ChannelId::MatrixInput(n) => *n,
    }
}

fn collect_names(
    state: &ConsoleState,
    groups: &[(ChannelGroup, Vec<ChannelId>)],
) -> HashMap<ChannelId, String> {
    let mut out = HashMap::new();
    for (_g, chans) in groups {
        for ch in chans {
            if let Some(ParameterValue::String(s)) = state.get(&ParameterAddress {
                channel: ch.clone(),
                parameter: ParameterPath::Name,
            }) {
                if !s.is_empty() {
                    out.insert(ch.clone(), s.clone());
                }
            }
        }
    }
    out
}

fn collect_stereo(
    cfg: &ConsoleConfig,
    groups: &[(ChannelGroup, Vec<ChannelId>)],
) -> HashSet<ChannelId> {
    let mut out = HashSet::new();
    for (_g, chans) in groups {
        for ch in chans {
            let is_stereo = match ch {
                ChannelId::Aux(n) => matches!(cfg.aux_mode(*n), Some(ChannelMode::Stereo)),
                ChannelId::Group(n) => matches!(
                    cfg.group_modes.get((*n as usize).saturating_sub(1)),
                    Some(ChannelMode::Stereo)
                ),
                _ => false,
            };
            if is_stereo {
                out.insert(ch.clone());
            }
        }
    }
    out
}

/// Draw the picker window for one frame. Returns `Some(outcome)` when the
/// operator clicks Save or Cancel (the caller then drops the state and closes
/// the window).
pub fn draw_gang_member_picker(
    ctx: &egui::Context,
    state: &mut GangMemberPickerState,
) -> Option<GangPickerOutcome> {
    let mut outcome: Option<GangPickerOutcome> = None;
    let mut still_open = true;

    let title = if state.editing {
        "Edit Gang Members"
    } else {
        "Pick Gang Members"
    };

    // Clamp the window to the host viewport; the grids inside are responsive so
    // there's no fixed content width to cap — it stays resizable between the
    // tiles-at-floor minimum and the viewport.
    let avail = ctx.content_rect().size();
    let max_w = (avail.x - 16.0).max(420.0);
    let max_h = (avail.y - 16.0).max(320.0);
    let min_w = (theme::grids_min_width(AVAIL_COLS, LINKED_COLS) + 28.0).min(max_w);
    let default_w = (theme::panel_width(64.0, AVAIL_COLS)
        + theme::TILE_COLUMN_GAP
        + theme::panel_width(64.0, LINKED_COLS)
        + 40.0)
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
            let shift_held = ui.input(|i| i.modifiers.shift);

            // Header: live count + Clear all / Save / Cancel.
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} selected", state.selected.len()))
                        .strong()
                        .color(theme::label_color()),
                );
                let hint = if state.selected.len() < 2 {
                    "· pick at least 2"
                } else {
                    "· shift-click for a range"
                };
                ui.label(egui::RichText::new(hint).small().color(theme::label_weak()));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::row_action_button(
                        ui,
                        "Cancel",
                        theme::ACCENT_RED,
                        80.0,
                        true,
                        "Discard changes and close",
                    ) {
                        outcome = Some(GangPickerOutcome::Cancel);
                    }
                    let save_enabled = state.selected.len() >= 2;
                    if theme::row_action_button(
                        ui,
                        "Save",
                        theme::ACCENT_GREEN,
                        80.0,
                        save_enabled,
                        "Use these members (a gang needs at least 2)",
                    ) {
                        outcome = Some(GangPickerOutcome::Save(state.ordered_members()));
                    }
                    ui.add_space(12.0);
                    if theme::row_action_button(
                        ui,
                        "Clear all",
                        theme::btn_neutral(),
                        80.0,
                        !state.selected.is_empty(),
                        "Remove every member",
                    ) {
                        state.selected.clear();
                    }
                });
            });

            ui.add_space(6.0);
            ui.separator();

            // Body: Available pane (left, wide) | Linked pane (right, narrow).
            // Vertical scroll only; tiles are responsive so the two panes fill
            // the window width without horizontal scroll.
            egui::ScrollArea::vertical()
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let avail_w = (ui.available_width() - 4.0)
                        .max(theme::grids_min_width(AVAIL_COLS, LINKED_COLS));
                    let tile_w = theme::channel_tile_width(avail_w, AVAIL_COLS, LINKED_COLS);
                    let left_w = theme::panel_width(tile_w, AVAIL_COLS);
                    let right_w = theme::panel_width(tile_w, LINKED_COLS);
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.horizontal_top(|ui| {
                        draw_available_pane(ui, state, tile_w, left_w, shift_held);
                        ui.add_space(theme::TILE_COLUMN_GAP);
                        draw_linked_pane(ui, state, tile_w, right_w);
                    });
                });
        });

    if !still_open && outcome.is_none() {
        outcome = Some(GangPickerOutcome::Cancel);
    }
    outcome
}

/// Left pane: every available channel, grouped by type. Click adds / toggles;
/// shift-click extends a range within the clicked group.
fn draw_available_pane(
    ui: &mut egui::Ui,
    state: &mut GangMemberPickerState,
    tile_w: f32,
    pane_w: f32,
    shift_held: bool,
) {
    // Collected here so the click is applied *after* the `&state.groups` borrow
    // below has been released (apply_click needs `&mut state`).
    let mut clicked: Option<(ChannelGroup, ChannelId, u8)> = None;

    ui.allocate_ui_with_layout(
        egui::Vec2::new(pane_w, ui.available_height()),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_min_width(pane_w);
            ui.set_max_width(pane_w);
            // The enclosing row forced item_spacing.x = 0 for exact tile math;
            // reset it so tiles within a row get the standard gap.
            ui.spacing_mut().item_spacing.x = theme::TILE_GAP;

            ui.label(
                egui::RichText::new("Available channels")
                    .strong()
                    .color(theme::label_color()),
            );

            if state.groups.is_empty() {
                ui.add_space(4.0);
                ui.colored_label(
                    theme::label_weak(),
                    "No channels — connect to console or load a show file.",
                );
                return;
            }

            for (group, chans) in &state.groups {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(group.label())
                        .strong()
                        .small()
                        .color(theme::label_weak()),
                );
                ui.add_space(2.0);
                for row in chans.chunks(AVAIL_COLS as usize) {
                    ui.horizontal(|ui| {
                        for ch in row {
                            let name = state.names.get(ch).map(String::as_str).unwrap_or("");
                            let selected = state.selected.contains(ch);
                            let stereo = state.stereo.contains(ch);
                            if draw_tile(
                                ui,
                                egui::vec2(tile_w, theme::TILE_H),
                                &ch.to_string(),
                                name,
                                theme::channel_color(ch),
                                selected.then_some(0),
                                stereo,
                                RippleHighlight::None,
                            )
                            .clicked()
                            {
                                clicked = Some((*group, ch.clone(), channel_number(ch)));
                            }
                        }
                    });
                }
            }
        },
    );

    if let Some((group, ch, num)) = clicked {
        state.apply_click(group, &ch, num, shift_held);
    }
}

/// Right pane: the channels currently in the gang, grouped by type. Click a
/// tile to remove it.
fn draw_linked_pane(
    ui: &mut egui::Ui,
    state: &mut GangMemberPickerState,
    tile_w: f32,
    pane_w: f32,
) {
    let mut to_remove: Option<ChannelId> = None;

    ui.allocate_ui_with_layout(
        egui::Vec2::new(pane_w, ui.available_height()),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_min_width(pane_w);
            ui.set_max_width(pane_w);
            ui.spacing_mut().item_spacing.x = theme::TILE_GAP;

            ui.label(
                egui::RichText::new(format!("In this gang ({})", state.selected.len()))
                    .strong()
                    .color(theme::label_color()),
            );

            if state.selected.is_empty() {
                ui.add_space(4.0);
                ui.colored_label(
                    theme::label_weak(),
                    "No members yet — click tiles on the left to add them.",
                );
                return;
            }

            for (group, chans) in &state.groups {
                let members: Vec<&ChannelId> = chans
                    .iter()
                    .filter(|c| state.selected.contains(*c))
                    .collect();
                if members.is_empty() {
                    continue;
                }
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(group.label())
                        .strong()
                        .small()
                        .color(theme::label_weak()),
                );
                ui.add_space(2.0);
                for row in members.chunks(LINKED_COLS as usize) {
                    ui.horizontal(|ui| {
                        for ch in row {
                            let name = state.names.get(*ch).map(String::as_str).unwrap_or("");
                            let stereo = state.stereo.contains(*ch);
                            if draw_tile(
                                ui,
                                egui::vec2(tile_w, theme::TILE_H),
                                &ch.to_string(),
                                name,
                                theme::channel_color(ch),
                                Some(0),
                                stereo,
                                RippleHighlight::None,
                            )
                            .clicked()
                            {
                                to_remove = Some((*ch).clone());
                            }
                        }
                    });
                }
            }
        },
    );

    if let Some(ch) = to_remove {
        state.selected.remove(&ch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::ConsoleConfig;

    fn picker(groups: Vec<(ChannelGroup, Vec<ChannelId>)>) -> GangMemberPickerState {
        GangMemberPickerState {
            editing: false,
            selected: HashSet::new(),
            anchor: HashMap::new(),
            groups,
            names: HashMap::new(),
            stereo: HashSet::new(),
        }
    }

    fn inputs(n: u8) -> Vec<ChannelId> {
        (1..=n).map(ChannelId::Input).collect()
    }
    fn auxes(n: u8) -> Vec<ChannelId> {
        (1..=n).map(ChannelId::Aux).collect()
    }

    #[test]
    fn new_seeds_selection_and_skips_empty_groups() {
        let mut cfg = ConsoleConfig::default();
        cfg.input_channel_count = 4;
        cfg.aux_output_count = 2;
        cfg.group_output_count = 0;
        cfg.matrix_output_count = 0;
        cfg.control_group_count = 0;
        cfg.graphic_eq_count = 0;
        cfg.matrix_input_count = 0;
        let state = ConsoleState::new(cfg);

        let p = GangMemberPickerState::new(&[ChannelId::Input(1), ChannelId::Aux(2)], true, &state);
        assert!(p.editing);
        assert!(p.selected.contains(&ChannelId::Input(1)));
        assert!(p.selected.contains(&ChannelId::Aux(2)));
        // Only Inputs + Aux groups are non-empty.
        let labels: Vec<ChannelGroup> = p.groups.iter().map(|(g, _)| *g).collect();
        assert_eq!(labels, vec![ChannelGroup::Inputs, ChannelGroup::Aux]);
    }

    #[test]
    fn ordered_members_is_group_then_ascending() {
        let mut p = picker(vec![
            (ChannelGroup::Inputs, inputs(5)),
            (ChannelGroup::Aux, auxes(3)),
        ]);
        p.selected.insert(ChannelId::Aux(2));
        p.selected.insert(ChannelId::Input(4));
        p.selected.insert(ChannelId::Input(1));
        assert_eq!(
            p.ordered_members(),
            vec![ChannelId::Input(1), ChannelId::Input(4), ChannelId::Aux(2)]
        );
    }

    #[test]
    fn plain_click_toggles_and_anchors() {
        let mut p = picker(vec![(ChannelGroup::Inputs, inputs(8))]);
        p.apply_click(ChannelGroup::Inputs, &ChannelId::Input(3), 3, false);
        assert!(p.selected.contains(&ChannelId::Input(3)));
        assert_eq!(p.anchor.get(&ChannelGroup::Inputs), Some(&3));
        // Clicking again removes it.
        p.apply_click(ChannelGroup::Inputs, &ChannelId::Input(3), 3, false);
        assert!(!p.selected.contains(&ChannelId::Input(3)));
    }

    #[test]
    fn shift_click_fills_range_within_group() {
        let mut p = picker(vec![(ChannelGroup::Inputs, inputs(8))]);
        p.apply_click(ChannelGroup::Inputs, &ChannelId::Input(2), 2, false); // anchor at 2
        p.apply_click(ChannelGroup::Inputs, &ChannelId::Input(5), 5, true); // range 2..=5
        for n in 2..=5 {
            assert!(p.selected.contains(&ChannelId::Input(n)), "missing {n}");
        }
        assert!(!p.selected.contains(&ChannelId::Input(1)));
        assert!(!p.selected.contains(&ChannelId::Input(6)));
    }

    #[test]
    fn shift_range_does_not_bridge_channel_types() {
        let mut p = picker(vec![
            (ChannelGroup::Inputs, inputs(8)),
            (ChannelGroup::Aux, auxes(8)),
        ]);
        // Anchor lives in Inputs.
        p.apply_click(ChannelGroup::Inputs, &ChannelId::Input(2), 2, false);
        // Shift-click in Aux has no Aux anchor → falls back to a plain toggle,
        // and must NOT pull in any inputs.
        p.apply_click(ChannelGroup::Aux, &ChannelId::Aux(5), 5, true);
        assert!(p.selected.contains(&ChannelId::Aux(5)));
        assert!(!p.selected.contains(&ChannelId::Aux(2)));
        // Inputs untouched apart from the anchor click.
        assert!(p.selected.contains(&ChannelId::Input(2)));
        assert!(!p.selected.contains(&ChannelId::Input(3)));
    }
}
