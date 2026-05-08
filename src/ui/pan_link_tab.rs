//! Pan Link tab — multi-input grid + multi-aux grid, stage + apply.
//!
//! The operator clicks input tiles in the left grid (single click toggles,
//! Shift+Click extends a range from the last anchor) and aux tiles on the
//! right to stage / unstage links for every selected input. Apply commits
//! the staged bindings; Revert restores the live state. Mono auxes are
//! dimmed and inert. Auxes that are linked for *some but not all* of the
//! selected inputs render with thick diagonal stripes; clicking those
//! promotes them to "all linked" first.
//!
//! Apply does NOT eagerly push the input's main pan to the aux send pan —
//! bindings stay dormant until the runtime mirror loop in
//! `pan_link_engine.rs` next sees a main-pan change. This avoids the aux
//! pan snapping the moment a binding is established.

use std::collections::BTreeSet;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use eframe::egui;
use tokio::sync::RwLock;

use super::theme;
use crate::model::channel::ChannelId;
use crate::model::config::ChannelMode;
use crate::model::pan_link::PanLinkBindings;
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;

/// Per-frame state for the Pan Link tab.
pub struct PanLinkTabState {
    /// Inputs included in the current operation. Aux tile clicks toggle
    /// the link for every input in this set.
    pub selected_inputs: BTreeSet<u8>,
    /// Last clicked input — anchor for Shift+Click range selection.
    pub range_anchor: Option<u8>,
    /// Working copy of the bindings. Tile clicks mutate this; only Apply
    /// writes back to the live `PanLinkBindings`.
    pub staged: PanLinkBindings,
    /// Snapshot of the live bindings as of the last Apply or tab entry.
    /// Used to detect dirty state and to power Revert.
    pub baseline: PanLinkBindings,
    /// True until the next frame after a tab-switch has had a chance to
    /// re-seed `staged` and `baseline` from the live bindings. Defaults
    /// to true so a freshly-constructed state syncs on first frame.
    pub needs_baseline_sync: bool,
}

impl Default for PanLinkTabState {
    fn default() -> Self {
        Self {
            selected_inputs: BTreeSet::new(),
            range_anchor: None,
            staged: PanLinkBindings::default(),
            baseline: PanLinkBindings::default(),
            needs_baseline_sync: true,
        }
    }
}

impl PanLinkTabState {
    fn dirty(&self) -> bool {
        self.staged != self.baseline
    }

    fn revert(&mut self) {
        self.staged = self.baseline.clone();
    }

    fn seed_from_live(&mut self, live: &PanLinkBindings) {
        self.baseline = live.clone();
        self.staged = live.clone();
        self.needs_baseline_sync = false;
    }

    /// Called by `app.rs` whenever the operator is on a tab other than
    /// Pan Link. Re-entry should land on a clean slate even if bindings
    /// were modified elsewhere, so we drop any unapplied stage and clear
    /// the input selection. Idempotent — runs every frame while away.
    pub fn mark_needs_sync(&mut self) {
        self.selected_inputs.clear();
        self.range_anchor = None;
        self.needs_baseline_sync = true;
    }
}

const TILE_SIZE: egui::Vec2 = egui::Vec2::new(82.0, 52.0);
const TILES_PER_INPUT_ROW: u8 = 10;
const TILES_PER_AUX_ROW: u8 = 4;
/// Fixed height reserved for both the Inputs and Auxes panel headers
/// so the first tile row in each column starts at the same y. Tall
/// enough to fit the 24 px action buttons in the inputs header.
const PANEL_HEADER_H: f32 = 28.0;
/// Total width of the input + aux grid content (input panel + 16 px
/// gap + aux panel). The header card and the grids card both cap
/// their content to this value so the Apply / Revert buttons in the
/// header align with the right edge of the aux tiles, and the card
/// frames close off at the same x.
const GRIDS_CONTENT_W: f32 = TILE_SIZE.x * (TILES_PER_INPUT_ROW as f32)
    + 24.0
    + 16.0
    + TILE_SIZE.x * (TILES_PER_AUX_ROW as f32)
    + 24.0;

/// `uses_ipad_protocol`: true when the active operating mode uses the
/// iPad protocol (Mode 2 / Mode 3). GP OSC alone (Mode 1) doesn't
/// expose the stereo / mono configuration of mix-output buses, so
/// when this is false the stereo filter is bypassed and every aux is
/// rendered as available for pan-linking.
pub fn draw_pan_link_tab(
    ui: &mut egui::Ui,
    tab: &mut PanLinkTabState,
    bindings: &Arc<RwLock<PanLinkBindings>>,
    console_state: &Arc<RwLock<ConsoleState>>,
    _sender: &Option<OscSender>,
    _connected: &Arc<AtomicBool>,
    runtime: &tokio::runtime::Handle,
    uses_ipad_protocol: bool,
) {
    // Snapshot live state for this frame. try_read so we don't deadlock
    // if a write is in flight; the next frame will redraw.
    let Ok(state_guard) = console_state.try_read() else {
        ui.label("Loading console state…");
        return;
    };
    let input_count = state_guard.config.input_channel_count;
    let total_buses = state_guard.config.total_bus_count();
    let mix_types = state_guard.config.mix_output_types.clone();
    let mix_modes = state_guard.config.mix_output_modes.clone();

    let live: PanLinkBindings = match bindings.try_read() {
        Ok(b) => b.clone(),
        Err(_) => {
            ui.label("Loading pan link bindings…");
            return;
        }
    };

    // Seed staged + baseline when:
    //  1. It's the first frame after a tab entry (explicit flag), OR
    //  2. We're clean and the live bindings drifted (e.g. a show-file
    //     load happened while we sat on the tab) — adopt the new live
    //     as baseline so the operator sees the change.
    // While dirty, leave both alone so unapplied work isn't clobbered.
    if tab.needs_baseline_sync || (!tab.dirty() && live != tab.baseline) {
        tab.seed_from_live(&live);
    }

    // Build the aux bus list once — preserves the unified-bus ordering
    // that `bindings` use to key entries. Mono auxes stay in the list so
    // the grid layout matches the desk; they'll just render dimmed.
    //
    // In Mode 1 (GP OSC only) the channel-mode info isn't available, so
    // every aux is treated as if stereo and stays clickable. The engine
    // applies the same rule when it sends pan-link writes.
    let aux_buses: Vec<AuxBusInfo> = (1..=total_buses)
        .filter_map(|bus| {
            let idx = (bus - 1) as usize;
            let is_aux = mix_types.get(idx).copied().unwrap_or(true);
            if !is_aux {
                return None;
            }
            let stereo =
                !uses_ipad_protocol || matches!(mix_modes.get(idx), Some(ChannelMode::Stereo));
            let label = state_guard.config.bus_label(bus);
            Some(AuxBusInfo { bus, label, stereo })
        })
        .collect();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Header card: title + dirty indicator + Apply / Revert.
            // Capped at `GRIDS_CONTENT_W` so the right_to_left
            // Apply/Revert buttons end at the same x as the aux tiles
            // in the grids card below — and so the card frame's right
            // border closes off at that same x.
            theme::card_frame().show(ui, |ui| {
                let card_w = GRIDS_CONTENT_W.min(ui.available_width());
                ui.set_min_width(card_w);
                ui.set_max_width(card_w);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Pan Link")
                            .size(theme::FONT_SIZE_SECTION)
                            .strong()
                            .color(theme::TEXT_PRIMARY),
                    );

                    let dirty = tab.dirty();
                    let staged_count = tab
                        .staged
                        .active
                        .symmetric_difference(&tab.baseline.active)
                        .count();
                    if dirty {
                        ui.add_space(8.0);
                        ui.colored_label(theme::ACCENT_AMBER, format!("● {staged_count} staged"));
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let revert_btn = theme::action_button(
                            "Revert",
                            theme::BG_ELEVATED,
                            egui::Vec2::new(80.0, 26.0),
                        );
                        if ui
                            .add_enabled(dirty, revert_btn)
                            .on_hover_text(
                                "Discard staged changes and resync with the live bindings.",
                            )
                            .clicked()
                        {
                            tab.revert();
                        }
                        ui.add_space(6.0);
                        let apply_btn = theme::action_button(
                            "Apply",
                            theme::ACCENT_GREEN,
                            egui::Vec2::new(80.0, 26.0),
                        );
                        if ui
                            .add_enabled(dirty, apply_btn)
                            .on_hover_text(
                                "Commit staged links. Aux send pans don't move until \
                                 the input main pan next changes.",
                            )
                            .clicked()
                        {
                            let staged_clone = tab.staged.clone();
                            let bindings_arc = bindings.clone();
                            runtime.spawn(async move {
                                let mut guard = bindings_arc.write().await;
                                *guard = staged_clone;
                            });
                            tab.baseline = tab.staged.clone();
                        }
                    });
                });
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new(
                        "Click inputs to select; Shift+Click for a range. Click stereo \
                         aux tiles to stage links across all selected inputs. Diagonal \
                         stripes mean partial — click promotes to all-linked first.",
                    )
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );
            });

            ui.add_space(8.0);

            // Input grid + aux grid, side-by-side. Capped at
            // `GRIDS_CONTENT_W` so the card frame's right border ends
            // exactly at the aux grid's right edge.
            theme::card_frame().show(ui, |ui| {
                let card_w = GRIDS_CONTENT_W.min(ui.available_width());
                ui.set_min_width(card_w);
                ui.set_max_width(card_w);
                ui.horizontal_top(|ui| {
                    draw_inputs_panel(ui, tab, state_guard.deref(), input_count);
                    ui.add_space(16.0);
                    draw_auxes_panel(ui, tab, &aux_buses);
                });
            });
        });

    drop(state_guard);
}

struct AuxBusInfo {
    /// Unified mix-output bus index (1-based).
    bus: u8,
    label: String,
    stereo: bool,
}

fn draw_inputs_panel(
    ui: &mut egui::Ui,
    tab: &mut PanLinkTabState,
    state: &ConsoleState,
    input_count: u8,
) {
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
                    egui::RichText::new(format!("({} selected)", tab.selected_inputs.len()))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let clear_btn = theme::action_button(
                        "Clear selection",
                        theme::BG_ELEVATED,
                        egui::Vec2::new(110.0, 24.0),
                    );
                    if ui
                        .add_enabled(!tab.selected_inputs.is_empty(), clear_btn)
                        .clicked()
                    {
                        tab.selected_inputs.clear();
                        tab.range_anchor = None;
                    }
                });
            },
        );
        ui.add_space(4.0);

        if input_count == 0 {
            ui.colored_label(
                theme::TEXT_SECONDARY,
                "No inputs configured — connect to console or load a show file.",
            );
            return;
        }

        let shift_held = ui.input(|i| i.modifiers.shift);
        let mut clicked_input: Option<u8> = None;
        let mut n: u8 = 1;
        while n <= input_count {
            ui.horizontal(|ui| {
                let row_end = (n + TILES_PER_INPUT_ROW - 1).min(input_count);
                for ch in n..=row_end {
                    let selected = tab.selected_inputs.contains(&ch);
                    let has_link = tab.staged.active.iter().any(|(input, _)| *input == ch);
                    let name = state
                        .get(&ParameterAddress {
                            channel: ChannelId::Input(ch),
                            parameter: ParameterPath::Name,
                        })
                        .and_then(|v| match v {
                            ParameterValue::String(s) if !s.is_empty() => Some(s.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    if draw_input_tile(ui, ch, &name, selected, has_link).clicked() {
                        clicked_input = Some(ch);
                    }
                }
            });
            n += TILES_PER_INPUT_ROW;
        }

        if let Some(ch) = clicked_input {
            apply_input_click(tab, ch, shift_held);
        }
    });
}

fn apply_input_click(tab: &mut PanLinkTabState, ch: u8, shift_held: bool) {
    if shift_held {
        if let Some(anchor) = tab.range_anchor {
            let (lo, hi) = if anchor <= ch {
                (anchor, ch)
            } else {
                (ch, anchor)
            };
            for c in lo..=hi {
                tab.selected_inputs.insert(c);
            }
            tab.range_anchor = Some(ch);
            return;
        }
    }
    // Plain click — toggle, and re-anchor.
    if !tab.selected_inputs.insert(ch) {
        tab.selected_inputs.remove(&ch);
    }
    tab.range_anchor = Some(ch);
}

fn draw_auxes_panel(ui: &mut egui::Ui, tab: &mut PanLinkTabState, aux_buses: &[AuxBusInfo]) {
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
            },
        );
        ui.add_space(4.0);

        if aux_buses.is_empty() {
            ui.colored_label(theme::TEXT_SECONDARY, "No auxes configured.");
            return;
        }

        let mut clicked_aux: Option<u8> = None;
        for chunk in aux_buses.chunks(TILES_PER_AUX_ROW as usize) {
            ui.horizontal(|ui| {
                for info in chunk {
                    let aux_state = compute_aux_state(tab, info);
                    if draw_aux_tile(ui, &info.label, info.stereo, aux_state).clicked()
                        && info.stereo
                        && !tab.selected_inputs.is_empty()
                    {
                        clicked_aux = Some(info.bus);
                    }
                }
            });
        }

        if let Some(bus) = clicked_aux {
            // Click cycle: Mixed → All-linked, All-linked → None,
            // None → All-linked. Always promotes Mixed to All on first click.
            let inputs: Vec<u8> = tab.selected_inputs.iter().copied().collect();
            let all_linked = inputs.iter().all(|i| tab.staged.is_active(*i, bus));
            let target = !all_linked;
            for i in inputs {
                tab.staged.set_active(i, bus, target);
            }
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuxState {
    Mono,
    NoSelection,
    NoneLinked,
    AllLinked,
    Mixed,
}

fn compute_aux_state(tab: &PanLinkTabState, info: &AuxBusInfo) -> AuxState {
    if !info.stereo {
        return AuxState::Mono;
    }
    if tab.selected_inputs.is_empty() {
        return AuxState::NoSelection;
    }
    let total = tab.selected_inputs.len();
    let linked = tab
        .selected_inputs
        .iter()
        .filter(|i| tab.staged.is_active(**i, info.bus))
        .count();
    if linked == 0 {
        AuxState::NoneLinked
    } else if linked == total {
        AuxState::AllLinked
    } else {
        AuxState::Mixed
    }
}

fn draw_input_tile(
    ui: &mut egui::Ui,
    ch: u8,
    name: &str,
    selected: bool,
    has_link: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(TILE_SIZE, egui::Sense::click());
    let painter = ui.painter_at(rect);

    let fill = if selected {
        theme::CH_INPUT
    } else {
        blend(theme::CH_INPUT, theme::BG_ELEVATED, 0.7)
    };
    let hover_fill = if response.hovered() {
        blend(fill, theme::TEXT_PRIMARY, 0.85)
    } else {
        fill
    };
    painter.rect_filled(rect, 4.0, hover_fill);

    let stroke = if selected {
        egui::Stroke::new(2.0, theme::TEXT_PRIMARY)
    } else {
        egui::Stroke::new(1.0, theme::BORDER_SUBTLE)
    };
    painter.rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);

    // Channel number — top line.
    painter.text(
        egui::pos2(rect.center().x, rect.min.y + 14.0),
        egui::Align2::CENTER_CENTER,
        format!("Input {ch}"),
        egui::FontId::proportional(13.0),
        theme::TEXT_PRIMARY,
    );
    if !name.is_empty() {
        painter.text(
            egui::pos2(rect.center().x, rect.min.y + 34.0),
            egui::Align2::CENTER_CENTER,
            name,
            egui::FontId::proportional(11.0),
            theme::TEXT_PRIMARY,
        );
    }

    // Has-link dot — small green disc in the bottom-right corner. Visible
    // regardless of selection so the operator sees which inputs are wired
    // even when nothing is currently selected. Outlined with the dark
    // background so it stays legible on both the saturated (selected)
    // and dimmed (unselected) tile fills.
    if has_link {
        let center = egui::pos2(rect.max.x - 9.0, rect.max.y - 9.0);
        painter.circle_filled(center, 5.0, theme::ACCENT_GREEN);
        painter.circle_stroke(center, 5.0, egui::Stroke::new(1.0, theme::BG_DARK));
    }

    let hover = if name.is_empty() {
        format!("Input {ch}")
    } else {
        format!("Input {ch} — {name}")
    };
    response.on_hover_text(hover)
}

fn draw_aux_tile(ui: &mut egui::Ui, label: &str, stereo: bool, state: AuxState) -> egui::Response {
    let clickable = matches!(
        state,
        AuxState::NoneLinked | AuxState::AllLinked | AuxState::Mixed
    );
    let sense = if clickable {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(TILE_SIZE, sense);
    let painter = ui.painter_at(rect);

    let base = theme::CH_AUX;
    let fill = match state {
        AuxState::Mono => theme::BG_ELEVATED,
        AuxState::NoSelection => blend(base, theme::BG_ELEVATED, 0.4),
        AuxState::NoneLinked => blend(base, theme::BG_ELEVATED, 0.55),
        AuxState::AllLinked => base,
        AuxState::Mixed => blend(base, theme::BG_ELEVATED, 0.55),
    };
    let hover_fill = if clickable && response.hovered() {
        blend(fill, theme::TEXT_PRIMARY, 0.85)
    } else {
        fill
    };
    painter.rect_filled(rect, 4.0, hover_fill);

    // Diagonal stripes for the Mixed state. The clipped painter (above)
    // restricts drawing to the tile rect, so we can use long line
    // segments and let clipping handle the corners. Three 45-degree
    // stripes evenly distributed along the diagonal span give a
    // recognisable "partial" hatch without obscuring the label.
    if matches!(state, AuxState::Mixed) {
        let stripe_color = blend(base, theme::TEXT_PRIMARY, 0.7);
        let stripe = egui::Stroke::new(6.0, stripe_color);
        let c_min = rect.min.y - rect.max.x;
        let c_max = rect.max.y - rect.min.x;
        let span = c_max - c_min;
        // Lines have form y = x + c. To draw, pick two x-values far
        // outside the rect and compute matching y. The painter is
        // clipped, so the off-rect ends are discarded automatically.
        let big = (rect.width() + rect.height()) * 2.0;
        for frac in [0.25_f32, 0.5, 0.75] {
            let c = c_min + frac * span;
            let p1 = egui::pos2(rect.min.x - big, rect.min.x - big + c);
            let p2 = egui::pos2(rect.max.x + big, rect.max.x + big + c);
            painter.line_segment([p1, p2], stripe);
        }
    }

    let stroke = if matches!(state, AuxState::AllLinked) {
        egui::Stroke::new(2.0, theme::TEXT_PRIMARY)
    } else {
        egui::Stroke::new(1.0, theme::BORDER_SUBTLE)
    };
    painter.rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);

    // Stereo bar — dark vertical slab on the right edge, copied from the
    // Monitor picker so the Pan Link grid reads the same way.
    if stereo {
        let bar_rect = egui::Rect::from_min_max(
            egui::pos2(rect.max.x - 6.0, rect.min.y + 4.0),
            egui::pos2(rect.max.x - 2.0, rect.max.y - 4.0),
        );
        let bar_fill = blend(fill, egui::Color32::BLACK, 0.55);
        painter.rect_filled(bar_rect, 2.0, bar_fill);
    }

    let text_color = match state {
        AuxState::Mono => theme::TEXT_DISABLED,
        _ => theme::TEXT_PRIMARY,
    };
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(13.0),
        text_color,
    );

    let hover = match state {
        AuxState::Mono => format!("{label} — mono, can't pan-link"),
        AuxState::NoSelection => format!("{label} — select inputs to link"),
        AuxState::NoneLinked => format!("{label} — none of the selected inputs are linked"),
        AuxState::AllLinked => format!("{label} — all selected inputs linked"),
        AuxState::Mixed => format!("{label} — partially linked (click to link all)"),
    };
    response.on_hover_text(hover)
}

fn blend(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let lerp = |x: u8, y: u8| ((x as f32) * t + (y as f32) * (1.0 - t)).round() as u8;
    egui::Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(initial: &[(u8, u8)]) -> PanLinkTabState {
        let mut s = PanLinkTabState::default();
        for (i, a) in initial {
            s.staged.set_active(*i, *a, true);
            s.baseline.set_active(*i, *a, true);
        }
        s.needs_baseline_sync = false;
        s
    }

    #[test]
    fn dirty_detects_staged_changes() {
        let mut s = state_with(&[]);
        assert!(!s.dirty());
        s.staged.set_active(1, 5, true);
        assert!(s.dirty());
        s.revert();
        assert!(!s.dirty());
    }

    #[test]
    fn shift_click_extends_range_from_anchor() {
        let mut s = state_with(&[]);
        apply_input_click(&mut s, 5, false); // anchor at 5
        apply_input_click(&mut s, 8, true); // shift-click 8 → range 5..=8
        let v: Vec<u8> = s.selected_inputs.iter().copied().collect();
        assert_eq!(v, vec![5, 6, 7, 8]);
        assert_eq!(s.range_anchor, Some(8));
    }

    #[test]
    fn plain_click_toggles_input_and_re_anchors() {
        let mut s = state_with(&[]);
        apply_input_click(&mut s, 3, false);
        assert!(s.selected_inputs.contains(&3));
        assert_eq!(s.range_anchor, Some(3));
        apply_input_click(&mut s, 3, false);
        assert!(s.selected_inputs.is_empty());
    }

    #[test]
    fn aux_state_classifies_selection_vs_links() {
        let info = AuxBusInfo {
            bus: 5,
            label: "Aux 5".into(),
            stereo: true,
        };
        let mono = AuxBusInfo {
            bus: 6,
            label: "Aux 6".into(),
            stereo: false,
        };

        let mut s = state_with(&[(1, 5), (2, 5)]);
        assert_eq!(compute_aux_state(&s, &info), AuxState::NoSelection);
        assert_eq!(compute_aux_state(&s, &mono), AuxState::Mono);

        s.selected_inputs.insert(1);
        s.selected_inputs.insert(2);
        assert_eq!(compute_aux_state(&s, &info), AuxState::AllLinked);

        s.selected_inputs.insert(3);
        assert_eq!(compute_aux_state(&s, &info), AuxState::Mixed);

        let mut s2 = state_with(&[]);
        s2.selected_inputs.insert(7);
        assert_eq!(compute_aux_state(&s2, &info), AuxState::NoneLinked);
    }

    #[test]
    fn seed_from_live_clears_dirty() {
        let mut s = PanLinkTabState::default();
        let mut live = PanLinkBindings::default();
        live.set_active(2, 4, true);
        s.seed_from_live(&live);
        assert!(!s.dirty());
        assert!(s.staged.is_active(2, 4));
        assert!(!s.needs_baseline_sync);
    }
}
