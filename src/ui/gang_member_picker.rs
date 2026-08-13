//! Inline channel-tile grid used by the Gangs tab to build / edit a gang's
//! member list.
//!
//! A gang can mix channel types, so the grid lists every channel grouped by
//! type as coloured tiles; clicking toggles membership, shift-click extends a
//! range within one type group. Two type *buckets* are mutually exclusive with
//! everything else — **Matrix Inputs** and **Graphic EQs** each only gang with
//! their own kind (they expose only their own parameter section, and the gang
//! engine's routing guard wouldn't propagate across the boundary anyway). Once
//! the selection settles on one bucket, the other buckets' tiles are drawn
//! disabled; **Clear all** resets so a different bucket can be picked.
//!
//! Selection lives in the Gangs tab (`Vec<ChannelId>`); this module only
//! renders and mutates it. The per-frame [`ChannelGridData`] snapshot carries
//! the channel layout, names and stereo flags read from live console state.

use std::collections::{HashMap, HashSet};

use eframe::egui;

use super::channel_tile::{RippleHighlight, draw_tile};
use super::theme;
use crate::model::channel::ChannelId;
use crate::model::config::{ChannelMode, ConsoleConfig};
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
use crate::model::state::ConsoleState;
use crate::ui::scope_editor::ChannelGroup;

/// Tiles per row in the channel grid. The grid column (≈⅔ of the form) sizes
/// tiles down to fit; narrower groups left-align within the column.
const AVAIL_COLS: u8 = 8;

/// Per-frame snapshot of the available channels, their names and stereo flags,
/// built from live console state. Cheap to rebuild each frame (a few hundred
/// map lookups) and always current as the operator renames channels on the desk.
pub struct ChannelGridData {
    /// Every available channel, grouped by type; empty groups are skipped so the
    /// grid never renders an empty section.
    pub groups: Vec<(ChannelGroup, Vec<ChannelId>)>,
    /// Operator-given channel names (empty / missing → numeric label only).
    pub names: HashMap<ChannelId, String>,
    /// Channels whose tile should draw the stereo bar (stereo auxes / groups).
    pub stereo: HashSet<ChannelId>,
}

impl ChannelGridData {
    pub fn from_state(state: &ConsoleState) -> Self {
        let cfg = &state.config;
        let groups: Vec<(ChannelGroup, Vec<ChannelId>)> = ChannelGroup::all()
            .iter()
            .map(|g| (*g, g.channels_from(cfg)))
            .filter(|(_, chans)| !chans.is_empty())
            .collect();
        let names = collect_names(state, &groups);
        let stereo = collect_stereo(cfg, &groups);
        Self {
            groups,
            names,
            stereo,
        }
    }
}

/// Mutually-exclusive channel-type buckets. A gang's members must all share one
/// bucket; the picker dims the others once a bucket is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeBucket {
    /// Input, Aux, Group, Matrix, ControlGroup — gang freely among themselves.
    Normal,
    /// Matrix Input — only with other Matrix Inputs.
    MatrixIn,
    /// Graphic EQ — only with other Graphic EQs.
    GraphicEq,
}

fn bucket_of(ch: &ChannelId) -> TypeBucket {
    match ch {
        ChannelId::MatrixInput(_) => TypeBucket::MatrixIn,
        ChannelId::GraphicEq(_) => TypeBucket::GraphicEq,
        _ => TypeBucket::Normal,
    }
}

/// The bucket the current selection is locked to, or `None` when the selection
/// is empty (anything goes) — or, defensively, when it already spans buckets
/// (a legacy gang): then nothing is dimmed so the operator can untangle it.
fn active_bucket(selection: &[ChannelId]) -> Option<TypeBucket> {
    let mut buckets = selection.iter().map(bucket_of);
    let first = buckets.next()?;
    buckets.all(|b| b == first).then_some(first)
}

/// Inner channel number of any `ChannelId` variant.
fn channel_number(ch: &ChannelId) -> u16 {
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

/// Uniform tile width so `cols` tiles (with `TILE_GAP` between) fill `pane_w`,
/// clamped to `[TILE_W_MIN, TILE_W_MAX]`.
fn pane_tile_width(pane_w: f32, cols: u8) -> f32 {
    let cols_f = (cols as f32).max(1.0);
    let inner_gaps = theme::TILE_GAP * cols.saturating_sub(1) as f32;
    ((pane_w - inner_gaps) / cols_f).clamp(theme::TILE_W_MIN, theme::TILE_W_MAX)
}

/// Apply a click on an enabled tile. Plain click toggles the channel and
/// re-anchors; shift-click fills the contiguous range from this group's anchor
/// to the clicked number, only within `group`'s own channel list.
fn apply_click(
    selection: &mut Vec<ChannelId>,
    anchor: &mut HashMap<ChannelGroup, u16>,
    data: &ChannelGridData,
    group: ChannelGroup,
    ch: &ChannelId,
    num: u16,
    shift: bool,
) {
    if shift {
        if let Some(&a) = anchor.get(&group) {
            let (lo, hi) = if a <= num { (a, num) } else { (num, a) };
            let in_range: Vec<ChannelId> = data
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
                if !selection.contains(&c) {
                    selection.push(c);
                }
            }
            anchor.insert(group, num);
            return;
        }
    }
    // Plain click — toggle, then re-anchor.
    if let Some(pos) = selection.iter().position(|c| c == ch) {
        selection.remove(pos);
    } else {
        selection.push(ch.clone());
    }
    anchor.insert(group, num);
}

/// Render the channel grid into `width`, mutating `selection` / `anchor` on
/// click. Tiles of channel types incompatible with the current selection are
/// drawn disabled (non-interactive). A header row shows the count and a
/// "Clear all" button.
pub fn draw_channel_grid(
    ui: &mut egui::Ui,
    data: &ChannelGridData,
    selection: &mut Vec<ChannelId>,
    anchor: &mut HashMap<ChannelGroup, u16>,
    width: f32,
    shift_held: bool,
) {
    let active = active_bucket(selection);
    let tile_w = pane_tile_width(width, AVAIL_COLS);

    let mut clicked: Option<(ChannelGroup, ChannelId, u16)> = None;
    let mut clear = false;

    ui.allocate_ui_with_layout(
        egui::Vec2::new(width, ui.available_height()),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_min_width(width);
            ui.set_max_width(width);
            ui.spacing_mut().item_spacing.x = theme::TILE_GAP;

            // Header: "Channels · N selected            [ Clear all ]"
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Channels")
                        .strong()
                        .color(theme::label_color()),
                );
                ui.label(
                    egui::RichText::new(format!("· {} selected", selection.len()))
                        .small()
                        .color(theme::label_weak()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::row_action_button(
                        ui,
                        "Clear all",
                        theme::btn_neutral(),
                        90.0,
                        !selection.is_empty(),
                        "Remove every member (and unlock all channel types)",
                    ) {
                        clear = true;
                    }
                });
            });

            if data.groups.is_empty() {
                ui.add_space(4.0);
                ui.colored_label(
                    theme::label_weak(),
                    "No channels — connect to console or load a show file.",
                );
                return;
            }

            for (group, chans) in &data.groups {
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
                            let enabled = active.is_none_or(|b| b == bucket_of(ch));
                            let name = data.names.get(ch).map(String::as_str).unwrap_or("");
                            let selected = selection.contains(ch);
                            let stereo = data.stereo.contains(ch);
                            if draw_tile(
                                ui,
                                egui::vec2(tile_w, theme::TILE_H),
                                &ch.to_string(),
                                name,
                                theme::channel_color(ch),
                                selected.then_some(0),
                                stereo,
                                RippleHighlight::None,
                                enabled,
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

    if clear {
        selection.clear();
        anchor.clear();
    } else if let Some((group, ch, num)) = clicked {
        apply_click(selection, anchor, data, group, &ch, num, shift_held);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::ConsoleConfig;

    fn data(groups: Vec<(ChannelGroup, Vec<ChannelId>)>) -> ChannelGridData {
        ChannelGridData {
            groups,
            names: HashMap::new(),
            stereo: HashSet::new(),
        }
    }

    fn inputs(n: u16) -> Vec<ChannelId> {
        (1..=n).map(ChannelId::Input).collect()
    }
    fn auxes(n: u16) -> Vec<ChannelId> {
        (1..=n).map(ChannelId::Aux).collect()
    }

    #[test]
    fn from_state_skips_empty_groups() {
        let mut cfg = ConsoleConfig::default();
        cfg.input_channel_count = 4;
        cfg.aux_output_count = 2;
        cfg.group_output_count = 0;
        cfg.matrix_output_count = 0;
        cfg.control_group_count = 0;
        cfg.graphic_eq_count = 0;
        cfg.matrix_input_count = 0;
        let state = ConsoleState::new(cfg);

        let d = ChannelGridData::from_state(&state);
        let labels: Vec<ChannelGroup> = d.groups.iter().map(|(g, _)| *g).collect();
        assert_eq!(labels, vec![ChannelGroup::Inputs, ChannelGroup::Aux]);
    }

    #[test]
    fn plain_click_toggles_and_anchors() {
        let d = data(vec![(ChannelGroup::Inputs, inputs(8))]);
        let mut sel = Vec::new();
        let mut anchor = HashMap::new();
        apply_click(
            &mut sel,
            &mut anchor,
            &d,
            ChannelGroup::Inputs,
            &ChannelId::Input(3),
            3,
            false,
        );
        assert!(sel.contains(&ChannelId::Input(3)));
        assert_eq!(anchor.get(&ChannelGroup::Inputs), Some(&3));
        apply_click(
            &mut sel,
            &mut anchor,
            &d,
            ChannelGroup::Inputs,
            &ChannelId::Input(3),
            3,
            false,
        );
        assert!(!sel.contains(&ChannelId::Input(3)));
    }

    #[test]
    fn shift_click_fills_range_within_group() {
        let d = data(vec![(ChannelGroup::Inputs, inputs(8))]);
        let mut sel = Vec::new();
        let mut anchor = HashMap::new();
        apply_click(
            &mut sel,
            &mut anchor,
            &d,
            ChannelGroup::Inputs,
            &ChannelId::Input(2),
            2,
            false,
        );
        apply_click(
            &mut sel,
            &mut anchor,
            &d,
            ChannelGroup::Inputs,
            &ChannelId::Input(5),
            5,
            true,
        );
        for n in 2..=5 {
            assert!(sel.contains(&ChannelId::Input(n)), "missing {n}");
        }
        assert!(!sel.contains(&ChannelId::Input(1)));
        assert!(!sel.contains(&ChannelId::Input(6)));
    }

    #[test]
    fn shift_does_not_bridge_channel_types() {
        let d = data(vec![
            (ChannelGroup::Inputs, inputs(8)),
            (ChannelGroup::Aux, auxes(8)),
        ]);
        let mut sel = Vec::new();
        let mut anchor = HashMap::new();
        apply_click(
            &mut sel,
            &mut anchor,
            &d,
            ChannelGroup::Inputs,
            &ChannelId::Input(2),
            2,
            false,
        );
        // Shift-click in Aux: no Aux anchor → plain toggle, no inputs pulled in.
        apply_click(
            &mut sel,
            &mut anchor,
            &d,
            ChannelGroup::Aux,
            &ChannelId::Aux(5),
            5,
            true,
        );
        assert!(sel.contains(&ChannelId::Aux(5)));
        assert!(!sel.contains(&ChannelId::Aux(2)));
        assert!(sel.contains(&ChannelId::Input(2)));
        assert!(!sel.contains(&ChannelId::Input(3)));
    }

    #[test]
    fn exclusivity_buckets() {
        // Empty → nothing locked.
        assert_eq!(active_bucket(&[]), None);
        // Normal selection locks out MatrixIn / GraphicEq.
        let b = active_bucket(&[ChannelId::Input(1), ChannelId::Aux(2)]);
        assert_eq!(b, Some(TypeBucket::Normal));
        assert!(b != Some(bucket_of(&ChannelId::MatrixInput(1))));
        assert!(b != Some(bucket_of(&ChannelId::GraphicEq(1))));
        // Matrix In selection locks to MatrixIn only.
        assert_eq!(
            active_bucket(&[ChannelId::MatrixInput(3)]),
            Some(TypeBucket::MatrixIn)
        );
        assert_eq!(bucket_of(&ChannelId::Input(1)), TypeBucket::Normal);
        assert_eq!(bucket_of(&ChannelId::GraphicEq(1)), TypeBucket::GraphicEq);
        // Graphic EQ selection locks to GraphicEq only.
        assert_eq!(
            active_bucket(&[ChannelId::GraphicEq(2)]),
            Some(TypeBucket::GraphicEq)
        );
    }

    #[test]
    fn mixed_legacy_selection_locks_nothing() {
        // Defensive: a selection spanning buckets dims nothing.
        assert_eq!(
            active_bucket(&[ChannelId::Input(1), ChannelId::MatrixInput(2)]),
            None
        );
    }
}
