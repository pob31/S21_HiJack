use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use eframe::egui;
use tokio::sync::RwLock;

use super::gang_member_picker::{ChannelGridData, draw_channel_grid};
use super::help::{HelpHover, HelpKey, help};
use super::status::StatusMessage;
use super::theme;
use crate::console::gang_manager::GangManager;
use crate::model::channel::ChannelId;
use crate::model::gang::{GangGroup, GangMode, GangPanMode};
use crate::model::parameter::{
    FADER_GANG_FLOOR_DB, FADER_INF_DB, ParameterAddress, ParameterPath, ParameterSection,
};
use crate::model::state::ConsoleState;
use crate::ui::scope_editor::ChannelGroup;
use uuid::Uuid;

/// Audible-level spread (in dB, measured in gang-floored space) above which a
/// fader-linked gang warns the operator that a real offset will be kept.
const GANG_SPREAD_WARN_DB: f32 = 20.0;

/// Label for a parameter section *within the Gangs tab*. Pan is now a
/// separate per-gang control, so the `FaderMutePan` section reads as
/// "Fader/Mute" here (it links fader, mute and solo). Everywhere else the
/// section keeps its full "Fader/Mute/Pan" name.
fn gang_section_label(section: &ParameterSection) -> String {
    match section {
        ParameterSection::FaderMutePan => "Fader/Mute".to_string(),
        other => other.to_string(),
    }
}

/// Largest fader-level gap across `members`, measured in gang-floored space
/// (everything below the floor collapses to a single point, so a set of parked
/// faders reports no spread). Unknown faders default to −inf. Mirrors the
/// flooring the gang engine applies, so the warning matches actual behaviour.
fn floored_fader_spread(state: &ConsoleState, members: &[ChannelId]) -> Option<f32> {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for ch in members {
        let db = state
            .get(&ParameterAddress {
                channel: ch.clone(),
                parameter: ParameterPath::Fader,
            })
            .and_then(|v| v.as_float())
            .unwrap_or(FADER_INF_DB)
            .max(FADER_GANG_FLOOR_DB);
        lo = lo.min(db);
        hi = hi.max(db);
    }
    (lo.is_finite() && hi.is_finite()).then_some(hi - lo)
}

/// Draw an OFF / ON / REV segmented control for a gang's pan mode. Returns the
/// newly-clicked mode, if any. `enabled` gates the whole control; `rev_enabled`
/// additionally gates the REVERSED button (pairs only — it needs exactly two
/// members). A currently-selected REV stays highlighted even when disabled.
fn pan_mode_buttons(
    ui: &mut egui::Ui,
    current: GangPanMode,
    enabled: bool,
    rev_enabled: bool,
) -> Option<GangPanMode> {
    let mut clicked = None;
    let opts = [
        (GangPanMode::Off, "OFF", HelpKey::GangPanOff, true),
        (GangPanMode::On, "ON", HelpKey::GangPanOn, true),
        (
            GangPanMode::Reversed,
            "REV",
            HelpKey::GangPanReversed,
            rev_enabled,
        ),
    ];
    for (mode, label, key, opt_enabled) in opts {
        let is_selected = current == mode;
        let btn_enabled = enabled && opt_enabled;
        let mut btn = egui::Button::new(egui::RichText::new(label).small())
            .selected(is_selected)
            .corner_radius(4.0);
        // Unselected-but-available toggles take the input-field background so
        // they stand out from the grey form row instead of reading grey-on-
        // grey. A disabled REV (not an exact pair) keeps the dimmed default.
        if !is_selected && btn_enabled {
            btn = btn.fill(theme::bg_input());
        }
        let resp = ui.add_enabled(btn_enabled, btn).on_hover_text(help(key));
        if resp.clicked() {
            clicked = Some(mode);
        }
    }
    clicked
}

/// Union of the parameter sections applicable to the distinct channel *types*
/// present in `members`. With no members the union is every section variant,
/// so the operator can pre-pick sections before choosing members (and the
/// `retain` that follows prunes the set once real members narrow it). The gang
/// engine still resolves per-pair applicability at propagation time.
fn applicable_sections_for(members: &[ChannelId]) -> HashSet<ParameterSection> {
    if members.is_empty() {
        return ParameterSection::all_variants().iter().cloned().collect();
    }
    let mut seen: Vec<std::mem::Discriminant<ChannelId>> = Vec::new();
    let mut out: HashSet<ParameterSection> = HashSet::new();
    for m in members {
        let d = std::mem::discriminant(m);
        if !seen.contains(&d) {
            seen.push(d);
            out.extend(ParameterSection::applicable_to(m));
        }
    }
    out
}

/// Reset the New/Edit Gang form to its empty defaults (name, members, anchor,
/// sections, pan). Does **not** touch `editing_gang_id` or `status_message` —
/// callers decide those.
fn reset_gang_form(tab: &mut GangsTabState) {
    tab.new_gang_name.clear();
    tab.new_gang_members.clear();
    tab.member_anchor.clear();
    tab.new_gang_sections = HashSet::from([ParameterSection::FaderMutePan]);
    tab.new_gang_pan_mode = GangPanMode::On;
}

/// Validate the staged gang and, if it passes, create or update it (async) and
/// reset the form. Sets `tab.status_message` either way. Validation: non-empty
/// name, ≥2 members, ≥1 linked section, and Reversed pan only for an exact pair.
fn try_save_gang(
    tab: &mut GangsTabState,
    gang_manager: &Arc<RwLock<GangManager>>,
    state: &Arc<RwLock<ConsoleState>>,
    runtime: &tokio::runtime::Handle,
) {
    if tab.new_gang_name.trim().is_empty() {
        tab.status_message = Some(StatusMessage::with_help(
            "Enter a gang name",
            HelpKey::GangNewName,
        ));
        return;
    }
    let members = tab.new_gang_members.clone();
    if members.len() < 2 {
        tab.status_message = Some(StatusMessage::with_help(
            "A gang needs at least 2 members",
            HelpKey::GangsWarnMinMembers,
        ));
        return;
    }
    if tab.new_gang_sections.is_empty() {
        tab.status_message = Some(StatusMessage::with_help(
            "Select at least one section",
            HelpKey::GangsWarnNoSection,
        ));
        return;
    }
    if tab.new_gang_pan_mode == GangPanMode::Reversed && members.len() != 2 {
        tab.status_message = Some(StatusMessage::with_help(
            "Reversed pan needs exactly 2 members",
            HelpKey::GangsWarnReversedPan,
        ));
        return;
    }

    let name = tab.new_gang_name.trim().to_string();
    let sections = tab.new_gang_sections.clone();
    let pan_mode = tab.new_gang_pan_mode;
    let mgr_clone = gang_manager.clone();

    // Heads-up if this gang links the fader and its members sit at very
    // different audible levels: in Relative mode that offset is preserved, so
    // they won't be matched. The engine already collapses the inaudible bottom
    // of the track, so this flags only real, above-floor spreads. Non-blocking.
    let spread = if sections.contains(&ParameterSection::FaderMutePan) {
        state
            .try_read()
            .ok()
            .and_then(|st| floored_fader_spread(&st, &members).filter(|s| *s > GANG_SPREAD_WARN_DB))
    } else {
        None
    };
    let spread_note = |base: String| match spread {
        Some(s) => StatusMessage::with_help(
            format!("{base} — faders span {s:.0} dB; in Relative mode they keep that offset"),
            HelpKey::GangsWarnFaderSpread,
        ),
        None => base.into(),
    };

    let was_editing = tab.editing_gang_id.is_some();
    if let Some(edit_id) = tab.editing_gang_id.take() {
        runtime.spawn(async move {
            let mut mgr = mgr_clone.write().await;
            if let Some(group) = mgr.groups.get_mut(&edit_id) {
                group.name = name;
                group.members = members;
                group.linked_sections = sections;
                group.pan_mode = Some(pan_mode);
            }
        });
        tab.status_message = Some(spread_note("Gang updated".into()));
    } else {
        let mut group = GangGroup::new(name.clone(), members, sections);
        group.pan_mode = Some(pan_mode);
        runtime.spawn(async move {
            mgr_clone.write().await.add_group(group);
        });
        tab.status_message = Some(spread_note(format!("Added gang '{name}'")));
    }

    reset_gang_form(tab);
    // Finishing an edit collapses the builder again; adding a brand-new gang
    // leaves it expanded so the operator can keep building.
    if was_editing {
        tab.form_collapsed = true;
    }
}

/// One-line tooltip explaining what a `ParameterSection` actually links
/// when included in a gang. Surfaces in the UI on hover so the operator
/// doesn't have to guess at non-obvious sections (Matrix Sends, Graphic
/// EQ, CG Membership).
fn section_tooltip(section: &ParameterSection) -> std::borrow::Cow<'static, str> {
    let key = match section {
        ParameterSection::FaderMutePan => HelpKey::GangFaderMutePan,
        ParameterSection::Name => HelpKey::GangName,
        ParameterSection::InputGain => HelpKey::GangInputGain,
        ParameterSection::Trim => HelpKey::GangTrim,
        ParameterSection::Polarity => HelpKey::GangPolarity,
        ParameterSection::BalanceWidth => HelpKey::GangBalanceWidth,
        ParameterSection::Delay => HelpKey::GangDelay,
        ParameterSection::Digitube => HelpKey::GangDigitube,
        ParameterSection::Eq => HelpKey::GangEq,
        ParameterSection::Dyn1 => HelpKey::GangDyn1,
        ParameterSection::Dyn2 => HelpKey::GangDyn2,
        ParameterSection::Sends => HelpKey::GangSends,
        ParameterSection::GroupRouting => HelpKey::GangGroupRouting,
        ParameterSection::Inserts => HelpKey::GangInserts,
        ParameterSection::CgMembership => HelpKey::GangCgMembership,
        ParameterSection::GraphicEq => HelpKey::GangGraphicEq,
        ParameterSection::MatrixSends => HelpKey::GangMatrixSends,
    };
    help(key)
}

/// Pre-compute which `ParameterSection` variants fit on each row given
/// the available width. The widths are measured the same way
/// [`theme::toggle_block`] sizes itself (label width + `2 * padding_x`,
/// floored at the 80 px minimum), with `item_spacing.x` accounted for
/// between siblings on the same row.
///
/// Used because `ui.horizontal_wrapped` and explicit
/// `Layout::with_main_wrap(true)` allocations both refused to break
/// the tile row in this tab's nesting (form card → horizontal_top →
/// allocated top_down sub-region → wrap layout). Building the rows
/// upfront and emitting one `ui.horizontal` per row sidesteps the
/// inheritance issue entirely.
fn wrap_section_tiles(ui: &egui::Ui, available_w: f32) -> Vec<Vec<&'static ParameterSection>> {
    const PADDING_X: f32 = 10.0;
    const MIN_W: f32 = 80.0;
    let item_spacing = ui.spacing().item_spacing.x;
    let font_id = egui::FontId::proportional(theme::FONT_SIZE_BADGE);

    let mut rows: Vec<Vec<&'static ParameterSection>> = vec![Vec::new()];
    let mut current_w = 0.0_f32;

    for section in ParameterSection::all_variants() {
        let label = gang_section_label(section);
        let galley = ui
            .painter()
            .layout_no_wrap(label, font_id.clone(), theme::TEXT_PRIMARY);
        let tile_w = (galley.size().x + PADDING_X * 2.0).max(MIN_W);

        let row_is_empty = rows.last().map(|r| r.is_empty()).unwrap_or(true);
        let needed = if row_is_empty {
            tile_w
        } else {
            current_w + item_spacing + tile_w
        };

        if needed > available_w && !row_is_empty {
            rows.push(Vec::new());
            current_w = tile_w;
        } else {
            current_w = needed;
        }

        rows.last_mut().unwrap().push(section);
    }

    rows
}

/// Same row-wrap idea as [`wrap_section_tiles`] but for the
/// [`theme::colored_badge`] sizing — text width plus an 8 px horizontal
/// padding on each side, no minimum width. Returns `Vec<&ParameterSection>`
/// per row, in the same order as the input slice.
fn wrap_badges<'a>(
    ui: &egui::Ui,
    available_w: f32,
    sections: &'a [&'a ParameterSection],
) -> Vec<Vec<&'a ParameterSection>> {
    const PADDING_X: f32 = 8.0;
    let item_spacing = ui.spacing().item_spacing.x;
    let font_id = egui::FontId::proportional(theme::FONT_SIZE_BADGE);

    let mut rows: Vec<Vec<&'a ParameterSection>> = vec![Vec::new()];
    let mut current_w = 0.0_f32;

    for section in sections {
        let label = gang_section_label(section);
        let galley = ui
            .painter()
            .layout_no_wrap(label, font_id.clone(), theme::TEXT_PRIMARY);
        let badge_w = galley.size().x + PADDING_X * 2.0;

        let row_is_empty = rows.last().map(|r| r.is_empty()).unwrap_or(true);
        let needed = if row_is_empty {
            badge_w
        } else {
            current_w + item_spacing + badge_w
        };

        if needed > available_w && !row_is_empty {
            rows.push(Vec::new());
            current_w = badge_w;
        } else {
            current_w = needed;
        }

        rows.last_mut().unwrap().push(*section);
    }

    rows
}

/// Per-tab UI state for the Gangs tab.
pub struct GangsTabState {
    pub new_gang_name: String,
    /// Staged member list for the gang being created / edited. Picked by
    /// clicking tiles in the inline channel grid, not typed.
    pub new_gang_members: Vec<ChannelId>,
    /// Per-group shift-click anchor for the inline channel grid (channel number
    /// within that group's number space). Keyed by `ChannelGroup` so a range
    /// never bridges channel types. Cleared on save / cancel / edit-load.
    pub member_anchor: HashMap<ChannelGroup, u8>,
    pub new_gang_sections: HashSet<ParameterSection>,
    /// Pan link mode for the gang being created / edited (independent of the
    /// linked sections above).
    pub new_gang_pan_mode: GangPanMode,
    pub editing_gang_id: Option<uuid::Uuid>,
    /// Collapse the New/Edit Gang builder body (channel grid + parameters) to
    /// just its title row, so the Gang Groups list below is reachable without
    /// scrolling past the full picker. The title row (name + Add/Save) stays
    /// visible; editing a gang auto-expands it, finishing an edit re-collapses.
    pub form_collapsed: bool,
    /// One-shot guard for the initial collapse state. The first time the Gangs
    /// tab is drawn (i.e. first switched to) the builder starts expanded if the
    /// session has no gangs yet, collapsed otherwise. After that the operator's
    /// own toggling (and the edit flow) owns the state.
    pub collapse_initialized: bool,
    pub status_message: Option<StatusMessage>,
}

impl Default for GangsTabState {
    fn default() -> Self {
        Self {
            new_gang_name: String::new(),
            new_gang_members: Vec::new(),
            member_anchor: HashMap::new(),
            new_gang_sections: HashSet::from([ParameterSection::FaderMutePan]),
            new_gang_pan_mode: GangPanMode::On,
            editing_gang_id: None,
            form_collapsed: false,
            collapse_initialized: false,
            status_message: None,
        }
    }
}

/// Draw the Gangs tab.
///
/// The disconnected hint ("connect to console for gang propagation to
/// take effect") is rendered by `app.rs` as a bottom-anchored banner so
/// the tab's vertical layout stays put when the connection state flips,
/// which is why this function no longer takes a `connected` handle.
pub fn draw_gangs_tab(
    ui: &mut egui::Ui,
    tab: &mut GangsTabState,
    gang_manager: &Arc<RwLock<GangManager>>,
    state: &Arc<RwLock<ConsoleState>>,
    runtime: &tokio::runtime::Handle,
) {
    // Header + form sit in the top region and stay anchored — only the
    // gang list below scrolls. ui.vertical hosts both, with the scroll
    // area auto_shrink(false) consuming the residual height. Without
    // this split, the form would scroll out of view as the gang list
    // grew, forcing the operator to scroll back up to add or edit.
    let mgr = runtime.block_on(gang_manager.read());
    let active_count = mgr.groups.values().filter(|g| g.enabled).count();
    let total_count = mgr.groups.len();

    // First time the tab is shown this session: start expanded if there are no
    // gangs yet (so the operator can build one straight away), collapsed if the
    // session already has gangs (so the list below is front-and-centre). After
    // this one-shot the operator's toggling / the edit flow owns the state.
    if !tab.collapse_initialized {
        tab.collapse_initialized = true;
        tab.form_collapsed = total_count > 0;
    }

    ui.vertical(|ui| {
        // Header
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Smart Ganging")
                    .size(theme::FONT_SIZE_SECTION)
                    .strong()
                    .color(theme::label_color()),
            );
            ui.add_space(12.0);
            theme::colored_badge(
                ui,
                &format!("{active_count} active / {total_count} total"),
                theme::btn_neutral(),
            );
        });

        ui.add_space(8.0);

        // ── Add / Edit gang form card ──
        //
        // Title row: heading + Name field + Add/Save (+ Cancel). Body splits
        // into the channel tile grid (≈⅔, left) and the parameter column
        // (≈⅓, right): Pan mode on top, then the Linked Sections grid — the
        // same inputs/aux proportions as the Monitor picker.
        let editing = tab.editing_gang_id.is_some();
        // Owned snapshot of the available channels for the inline grid, built
        // under a brief read so the lock is released before rendering. None this
        // frame if the lock is busy → the grid shows a "loading" placeholder.
        let grid_data = state
            .try_read()
            .ok()
            .map(|st| ChannelGridData::from_state(&st));
        let shift_held = ui.input(|i| i.modifiers.shift);
        theme::card_frame().show(ui, |ui| {
            let card_w = ui.available_width();
            ui.set_min_width(card_w);
            ui.set_max_width(card_w);

            // Applicable section set: union over the picked members' channel
            // types (empty → all variants). Prune any now-inapplicable selected
            // section so the right column reflects the real options.
            let applicable: HashSet<ParameterSection> =
                applicable_sections_for(&tab.new_gang_members);
            tab.new_gang_sections.retain(|s| applicable.contains(s));

            // ── Title row: collapse toggle + heading + Name + Add/Save (+ Cancel) ──
            // Always visible; collapsing hides only the body so the Gang Groups
            // list below is reachable without scrolling past the full picker.
            let header_title = if editing { "Edit Gang" } else { "New Gang" };
            ui.horizontal(|ui| {
                // Clickable disclosure triangle — painted (not a font glyph) so
                // it renders regardless of the bundled font. ▶ collapsed / ▼ open.
                let (tri_rect, tri_resp) =
                    ui.allocate_exact_size(egui::Vec2::splat(16.0), egui::Sense::click());
                let c = tri_rect.center();
                let r = 4.5;
                let tri_color = if tri_resp.hovered() {
                    theme::label_color()
                } else {
                    theme::label_weak()
                };
                let pts = if tab.form_collapsed {
                    vec![
                        egui::pos2(c.x - r * 0.6, c.y - r),
                        egui::pos2(c.x + r, c.y),
                        egui::pos2(c.x - r * 0.6, c.y + r),
                    ]
                } else {
                    vec![
                        egui::pos2(c.x - r, c.y - r * 0.6),
                        egui::pos2(c.x + r, c.y - r * 0.6),
                        egui::pos2(c.x, c.y + r),
                    ]
                };
                ui.painter().add(egui::Shape::convex_polygon(
                    pts,
                    tri_color,
                    egui::Stroke::NONE,
                ));
                if tri_resp.clicked() {
                    tab.form_collapsed = !tab.form_collapsed;
                }

                ui.add_space(2.0);
                // The heading is also a click target for the toggle.
                let head = ui.add(
                    egui::Label::new(
                        egui::RichText::new(header_title)
                            .size(theme::FONT_SIZE_SECTION)
                            .strong()
                            .color(theme::label_color()),
                    )
                    .sense(egui::Sense::click()),
                );
                if head.clicked() {
                    tab.form_collapsed = !tab.form_collapsed;
                }
                // When collapsed, surface the staged member count so the operator
                // sees something is in progress without expanding.
                if tab.form_collapsed && !tab.new_gang_members.is_empty() {
                    ui.label(
                        egui::RichText::new(format!("· {} selected", tab.new_gang_members.len()))
                            .color(theme::label_weak()),
                    );
                }

                // Right side — Name + Add/Save (+ Cancel), compacted the same way
                // `theme::section_heading_with` does so the row stays short. The
                // first widget added sits furthest right.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let body_h = ui.text_style_height(&egui::TextStyle::Body);
                    ui.spacing_mut().button_padding.y = 0.0;
                    ui.spacing_mut().interact_size.y = body_h;

                    if editing
                        && theme::row_action_button(
                            ui,
                            "Cancel",
                            theme::btn_neutral(),
                            80.0,
                            true,
                            help(HelpKey::GangCancelEdit),
                        )
                    {
                        tab.editing_gang_id = None;
                        reset_gang_form(tab);
                        // Leaving the edit collapses the builder again.
                        tab.form_collapsed = true;
                        tab.status_message = None;
                    }
                    let (btn_text, btn_color) = if editing {
                        ("Save", theme::ACCENT_BLUE)
                    } else {
                        ("Add Gang", theme::ACCENT_GREEN)
                    };
                    if theme::row_action_button(
                        ui,
                        btn_text,
                        btn_color,
                        100.0,
                        true,
                        help(HelpKey::GangSave),
                    ) {
                        try_save_gang(tab, gang_manager, state, runtime);
                    }
                    ui.add_space(8.0);
                    theme::padded_text_edit_sized(
                        ui,
                        &mut tab.new_gang_name,
                        220.0,
                        theme::ROW_H,
                        true,
                        "Gang name",
                    )
                    .on_hover_text(help(HelpKey::GangNewName));
                    theme::row_label(ui, "Name:", theme::label_weak());
                });
            });
            // Underline rule (matches the section-heading look).
            ui.add_space(2.0);
            {
                let rule_w = ui.available_width();
                let (rule_rect, _) =
                    ui.allocate_exact_size(egui::Vec2::new(rule_w, 1.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(rule_rect, 0.0, theme::border_subtle());
            }
            ui.add_space(6.0);

            if !tab.form_collapsed {
                // ── Body: channel grid (≈⅔) | parameter column (≈⅓) ──
                // Capped-height scroll so a tall channel count scrolls here while
                // the gang list below stays reachable.
                let body_h = (ui.available_height() - 220.0).max(220.0);
                egui::ScrollArea::vertical()
                    .id_salt("gang_form_body")
                    .max_height(body_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let gap = theme::TILE_COLUMN_GAP;
                        let avail = ui.available_width();
                        let right_w = ((avail - gap) / 3.0).floor().max(180.0);
                        let left_w = (avail - gap - right_w).max(theme::TILE_W_MIN);
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.horizontal_top(|ui| {
                            // LEFT ⅔ — channel tiles (selection shown in place).
                            match &grid_data {
                                Some(data) => {
                                    draw_channel_grid(
                                        ui,
                                        data,
                                        &mut tab.new_gang_members,
                                        &mut tab.member_anchor,
                                        left_w,
                                        shift_held,
                                    );
                                }
                                None => {
                                    ui.allocate_ui_with_layout(
                                        egui::Vec2::new(left_w, ui.available_height()),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            ui.set_min_width(left_w);
                                            ui.set_max_width(left_w);
                                            ui.colored_label(
                                                theme::label_weak(),
                                                "Loading channels…",
                                            );
                                        },
                                    );
                                }
                            }

                            ui.add_space(gap);

                            // RIGHT ⅓ — Pan on top, then the Linked Sections grid.
                            ui.allocate_ui_with_layout(
                                egui::Vec2::new(right_w, ui.available_height()),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.set_min_width(right_w);
                                    ui.set_max_width(right_w);
                                    // Restore a normal horizontal gap (the enclosing
                                    // row forced item_spacing.x = 0 for tile math).
                                    ui.spacing_mut().item_spacing.x = 8.0;

                                    // Pan link mode — its own control, REV only for
                                    // an exact pair.
                                    ui.label(
                                        egui::RichText::new("Pan")
                                            .strong()
                                            .color(theme::label_color()),
                                    );
                                    ui.add_space(2.0);
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().button_padding =
                                            egui::Vec2::new(12.0, 4.0);
                                        let member_count = tab.new_gang_members.len();
                                        if let Some(m) = pan_mode_buttons(
                                            ui,
                                            tab.new_gang_pan_mode,
                                            true,
                                            member_count == 2,
                                        ) {
                                            tab.new_gang_pan_mode = m;
                                        }
                                    });

                                    ui.add_space(8.0);
                                    ui.separator();
                                    ui.add_space(4.0);

                                    ui.label(
                                        egui::RichText::new("Linked Sections")
                                            .strong()
                                            .color(theme::label_color()),
                                    );
                                    ui.add_space(4.0);
                                    // Manual row wrapping — horizontal_wrapped won't
                                    // break rows in this nesting.
                                    let rows = wrap_section_tiles(ui, right_w);
                                    for row in &rows {
                                        ui.horizontal(|ui| {
                                            for section in row {
                                                let is_applicable = applicable.contains(section);
                                                let active =
                                                    tab.new_gang_sections.contains(section);
                                                let builder = if is_applicable {
                                                    egui::UiBuilder::new()
                                                } else {
                                                    egui::UiBuilder::new().invisible()
                                                };
                                                let resp = ui
                                                    .scope_builder(builder, |ui| {
                                                        theme::toggle_block(
                                                            ui,
                                                            &gang_section_label(section),
                                                            active,
                                                        )
                                                        .on_hover_text(section_tooltip(section))
                                                    })
                                                    .inner;
                                                if is_applicable && resp.clicked() {
                                                    if active {
                                                        tab.new_gang_sections.remove(*section);
                                                    } else {
                                                        tab.new_gang_sections
                                                            .insert((*section).clone());
                                                    }
                                                }
                                            }
                                        });
                                    }
                                },
                            );
                        });
                    });
            }

            // Status message
            if let Some(ref msg) = tab.status_message {
                ui.add_space(4.0);
                let resp = ui.colored_label(theme::TEXT_WARNING, &msg.text);
                if let Some(key) = msg.help {
                    resp.on_hover_help_inline(key);
                }
            }
        });

        ui.add_space(8.0);

        // ── Gang list card ── card_frame wraps a fixed "Gang Groups"
        // heading at the top with an embedded ScrollArea below it, so
        // the heading stays visible while the list scrolls. Putting
        // the ScrollArea inside the card (instead of wrapping the
        // card in the scroll) is what keeps the heading sticky —
        // when the operator scrolls through many gangs they still see
        // which section they're in.
        //
        // `set_min_width(available)` is applied at every nesting
        // level — Frames in egui size to their content by default, so
        // without these explicit hints the gang-list card and each
        // per-gang frame would shrink to the width of their longest
        // row instead of spanning the window.
        theme::card_frame().show(ui, |ui| {
            let card_w = ui.available_width();
            ui.set_min_width(card_w);
            ui.set_max_width(card_w);
            theme::section_heading(ui, "Gang Groups");

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let scroll_w = ui.available_width();
                    ui.set_min_width(scroll_w);
                    ui.set_max_width(scroll_w);

                    let groups: Vec<GangGroup> = mgr.sorted_groups().into_iter().cloned().collect();
                    drop(mgr);

                    if groups.is_empty() {
                        ui.label(
                            egui::RichText::new("No gang groups configured.")
                                .color(theme::label_weak()),
                        )
                        .on_hover_help_inline(HelpKey::GangsInfoEmpty);
                    } else {
                        let mut to_remove = None;
                        let mut to_edit = None;
                        let mut to_toggle = None;
                        let mut to_pause: Option<(Uuid, bool)> = None;
                        let mut to_set_mode: Option<(Uuid, GangMode)> = None;
                        let mut to_set_pan_mode: Option<(Uuid, GangPanMode)> = None;

                        for group in &groups {
                            let bg = if !group.enabled || group.paused {
                                theme::bg_panel() // dimmed when disabled or paused
                            } else {
                                theme::bg_elevated()
                            };
                            egui::Frame::new()
                                .fill(bg)
                                .stroke(egui::Stroke::new(1.0, theme::border_subtle()))
                                .corner_radius(6.0)
                                .inner_margin(egui::Margin::same(8))
                                .show(ui, |ui| {
                                    let row_w = ui.available_width();
                                    ui.set_min_width(row_w);
                                    ui.set_max_width(row_w);

                                    // Controls + name + member badge — single
                                    // line, no wrap. These are stable in
                                    // count (4 small buttons + name + 1 badge)
                                    // and read better as a single horizontal
                                    // strip.
                                    ui.horizontal(|ui| {
                                        // Enable/disable toggle
                                        let toggle_color = if group.enabled {
                                            theme::ACCENT_GREEN
                                        } else {
                                            theme::btn_neutral()
                                        };
                                        let toggle_label = if group.enabled { "ON" } else { "OFF" };
                                        let toggle_btn = egui::Button::new(
                                            egui::RichText::new(toggle_label)
                                                .color(theme::on_fill_text(toggle_color))
                                                .strong()
                                                .small(),
                                        )
                                        .fill(toggle_color)
                                        .corner_radius(4.0);
                                        if ui
                                            .add(toggle_btn)
                                            .on_hover_text(help(HelpKey::GangEnableToggle))
                                            .clicked()
                                        {
                                            to_toggle = Some((group.id, !group.enabled));
                                        }

                                        // Pause button
                                        let pause_color = if group.paused {
                                            theme::ACCENT_ORANGE
                                        } else {
                                            theme::btn_neutral()
                                        };
                                        let pause_label =
                                            if group.paused { "PAUSED" } else { "||" };
                                        let pause_btn = egui::Button::new(
                                            egui::RichText::new(pause_label)
                                                .color(theme::on_fill_text(pause_color))
                                                .small(),
                                        )
                                        .fill(pause_color)
                                        .corner_radius(4.0);
                                        if ui
                                            .add_enabled(group.enabled, pause_btn)
                                            .on_hover_text(help(HelpKey::GangPause))
                                            .on_disabled_hover_text(help(HelpKey::GangNeedsEnable))
                                            .clicked()
                                        {
                                            to_pause = Some((group.id, !group.paused));
                                        }

                                        ui.add_space(4.0);

                                        let rel_btn = egui::Button::new(
                                            egui::RichText::new("Relative").small(),
                                        )
                                        .selected(group.mode == GangMode::Relative)
                                        .corner_radius(4.0);
                                        if ui
                                            .add_enabled(group.enabled, rel_btn)
                                            .on_hover_text(help(HelpKey::GangRelative))
                                            .on_disabled_hover_text(help(HelpKey::GangNeedsEnable))
                                            .clicked()
                                        {
                                            to_set_mode = Some((group.id, GangMode::Relative));
                                        }
                                        let abs_btn = egui::Button::new(
                                            egui::RichText::new("Absolute").small(),
                                        )
                                        .selected(group.mode == GangMode::Absolute)
                                        .corner_radius(4.0);
                                        if ui
                                            .add_enabled(group.enabled, abs_btn)
                                            .on_hover_text(help(HelpKey::GangAbsolute))
                                            .on_disabled_hover_text(help(HelpKey::GangNeedsEnable))
                                            .clicked()
                                        {
                                            to_set_mode = Some((group.id, GangMode::Absolute));
                                        }

                                        ui.add_space(8.0);

                                        // Pan link mode — OFF / ON / REV. REV
                                        // only offered for an exact pair.
                                        ui.label(
                                            egui::RichText::new("Pan")
                                                .small()
                                                .color(theme::label_weak()),
                                        );
                                        if let Some(m) = pan_mode_buttons(
                                            ui,
                                            group.effective_pan_mode(),
                                            group.enabled,
                                            group.members.len() == 2,
                                        ) {
                                            to_set_pan_mode = Some((group.id, m));
                                        }

                                        ui.add_space(8.0);

                                        ui.label(
                                            egui::RichText::new(&group.name)
                                                .strong()
                                                .color(theme::label_color()),
                                        );

                                        ui.add_space(8.0);

                                        let member_text = format_members(&group.members);
                                        let member_color = if !group.members.is_empty() {
                                            theme::channel_color(&group.members[0])
                                        } else {
                                            theme::btn_neutral()
                                        };
                                        theme::colored_badge(ui, &member_text, member_color);
                                    });

                                    // Section badges — manual wrap into rows
                                    // that fit within `row_w`, since egui's
                                    // built-in wrap layouts didn't break the
                                    // row reliably in this frame nesting.
                                    if !group.linked_sections.is_empty() {
                                        let mut sections: Vec<&ParameterSection> =
                                            group.linked_sections.iter().collect();
                                        sections.sort_by_key(|s| s.to_string());
                                        let badge_rows = wrap_badges(ui, row_w, &sections);
                                        for badge_row in &badge_rows {
                                            ui.horizontal(|ui| {
                                                for section in badge_row {
                                                    theme::colored_badge(
                                                        ui,
                                                        &gang_section_label(section),
                                                        theme::SCOPE_ACTIVE,
                                                    );
                                                }
                                            });
                                        }
                                    }

                                    // Action buttons row
                                    ui.horizontal(|ui| {
                                        ui.add_space(52.0);
                                        let edit_btn = theme::action_button(
                                            "Edit",
                                            theme::ACCENT_ORANGE,
                                            egui::Vec2::new(60.0, 24.0),
                                        );
                                        if ui
                                            .add(edit_btn)
                                            .on_hover_text(help(HelpKey::GangEdit))
                                            .clicked()
                                        {
                                            to_edit = Some(group.clone());
                                        }
                                        // Long-press to confirm — matches the
                                        // Setup-tab transport buttons and the
                                        // Macros-tab Delete.
                                        if theme::long_press_button(
                                            ui,
                                            "Delete",
                                            theme::ACCENT_RED,
                                            egui::Vec2::new(60.0, 24.0),
                                            true,
                                            theme::LONG_PRESS_DURATION_MS,
                                        ) {
                                            to_remove = Some(group.id);
                                        }
                                    });
                                });
                            ui.add_space(4.0);
                        }

                        if let Some(id) = to_remove {
                            let mgr_clone = gang_manager.clone();
                            runtime.spawn(async move {
                                mgr_clone.write().await.remove_group(id);
                            });
                            tab.status_message = Some("Gang removed".into());
                        }

                        if let Some((id, new_enabled)) = to_toggle {
                            let mgr_clone = gang_manager.clone();
                            runtime.spawn(async move {
                                let mut mgr = mgr_clone.write().await;
                                if let Some(group) = mgr.groups.get_mut(&id) {
                                    group.enabled = new_enabled;
                                    if !new_enabled {
                                        group.paused = false;
                                    }
                                }
                            });
                        }

                        if let Some((id, new_paused)) = to_pause {
                            let mgr_clone = gang_manager.clone();
                            runtime.spawn(async move {
                                let mut mgr = mgr_clone.write().await;
                                if let Some(group) = mgr.groups.get_mut(&id) {
                                    group.paused = new_paused;
                                }
                            });
                        }

                        if let Some((id, new_mode)) = to_set_mode {
                            let mgr_clone = gang_manager.clone();
                            runtime.spawn(async move {
                                let mut mgr = mgr_clone.write().await;
                                if let Some(group) = mgr.groups.get_mut(&id) {
                                    group.mode = new_mode;
                                }
                            });
                        }

                        if let Some((id, new_pan_mode)) = to_set_pan_mode {
                            let mgr_clone = gang_manager.clone();
                            runtime.spawn(async move {
                                let mut mgr = mgr_clone.write().await;
                                if let Some(group) = mgr.groups.get_mut(&id) {
                                    group.pan_mode = Some(new_pan_mode);
                                }
                            });
                        }

                        if let Some(group) = to_edit {
                            tab.editing_gang_id = Some(group.id);
                            tab.new_gang_name = group.name.clone();
                            tab.new_gang_members = group.members.clone();
                            tab.member_anchor.clear();
                            tab.new_gang_sections = group.linked_sections.clone();
                            tab.new_gang_pan_mode = group.effective_pan_mode();
                            // Expand the builder so the edited gang's channels and
                            // parameters are visible.
                            tab.form_collapsed = false;
                            tab.status_message = None;
                        }
                    }
                });
        });
    });
}

/// Format a list of channel members for display.
fn format_members(members: &[ChannelId]) -> String {
    if members.is_empty() {
        return String::new();
    }

    // Check if all members are the same type
    let all_same_type = members
        .windows(2)
        .all(|w| std::mem::discriminant(&w[0]) == std::mem::discriminant(&w[1]));

    if all_same_type {
        // Simple format: just the numbers with ranges
        let prefix = match members[0] {
            ChannelId::Input(_) => "Input",
            ChannelId::Aux(_) => "Aux",
            ChannelId::Group(_) => "Group",
            ChannelId::Matrix(_) => "Mtx",
            ChannelId::ControlGroup(_) => "CG",
            ChannelId::GraphicEq(_) => "GEQ",
            ChannelId::MatrixInput(_) => "MtxIn",
        };
        let numbers: Vec<u8> = members
            .iter()
            .map(|m| match m {
                ChannelId::Input(n)
                | ChannelId::Aux(n)
                | ChannelId::Group(n)
                | ChannelId::Matrix(n)
                | ChannelId::ControlGroup(n)
                | ChannelId::GraphicEq(n)
                | ChannelId::MatrixInput(n) => *n,
            })
            .collect();
        format!("{} {}", prefix, format_ranges(&numbers))
    } else {
        // Mixed: use prefix notation
        let mut parts = Vec::new();
        for m in members {
            let (prefix, n) = match m {
                ChannelId::Input(n) => ("I", *n),
                ChannelId::Aux(n) => ("A", *n),
                ChannelId::Group(n) => ("G", *n),
                ChannelId::Matrix(n) => ("M", *n),
                ChannelId::ControlGroup(n) => ("CG", *n),
                ChannelId::GraphicEq(n) => ("GEQ", *n),
                ChannelId::MatrixInput(n) => ("MI", *n),
            };
            parts.push(format!("{prefix}{n}"));
        }
        parts.join(",")
    }
}

/// Compress a sorted list of numbers into range notation: [1,2,3,7,12] -> "1-3,7,12"
fn format_ranges(numbers: &[u8]) -> String {
    if numbers.is_empty() {
        return String::new();
    }

    let mut sorted = numbers.to_vec();
    sorted.sort();
    sorted.dedup();

    let mut parts = Vec::new();
    let mut start = sorted[0];
    let mut end = sorted[0];

    for &n in &sorted[1..] {
        if n == end + 1 {
            end = n;
        } else {
            if start == end {
                parts.push(start.to_string());
            } else {
                parts.push(format!("{start}-{end}"));
            }
            start = n;
            end = n;
        }
    }
    if start == end {
        parts.push(start.to_string());
    } else {
        parts.push(format!("{start}-{end}"));
    }

    parts.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_ranges_compresses() {
        assert_eq!(format_ranges(&[1, 2, 3, 7, 12]), "1-3,7,12");
        assert_eq!(format_ranges(&[5]), "5");
        assert_eq!(format_ranges(&[1, 3, 5]), "1,3,5");
    }

    #[test]
    fn format_members_same_type() {
        let members = vec![
            ChannelId::Input(1),
            ChannelId::Input(2),
            ChannelId::Input(3),
        ];
        assert_eq!(format_members(&members), "Input 1-3");
    }

    #[test]
    fn format_members_mixed() {
        let members = vec![ChannelId::Input(1), ChannelId::Aux(2), ChannelId::Group(5)];
        assert_eq!(format_members(&members), "I1,A2,G5");
    }
}
