use std::collections::HashSet;
use std::sync::Arc;

use eframe::egui;
use tokio::sync::RwLock;

use super::gang_member_picker::{
    GangMemberPickerState, GangPickerOutcome, draw_gang_member_picker,
};
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
    /// Staged member list for the gang being created / edited. Picked via the
    /// tile picker modal ([`member_picker`]), not typed.
    pub new_gang_members: Vec<ChannelId>,
    pub new_gang_sections: HashSet<ParameterSection>,
    /// Pan link mode for the gang being created / edited (independent of the
    /// linked sections above).
    pub new_gang_pan_mode: GangPanMode,
    pub editing_gang_id: Option<uuid::Uuid>,
    /// `Some(_)` while the tile member picker window is open.
    pub member_picker: Option<GangMemberPickerState>,
    pub status_message: Option<StatusMessage>,
}

impl Default for GangsTabState {
    fn default() -> Self {
        Self {
            new_gang_name: String::new(),
            new_gang_members: Vec::new(),
            new_gang_sections: HashSet::from([ParameterSection::FaderMutePan]),
            new_gang_pan_mode: GangPanMode::On,
            editing_gang_id: None,
            member_picker: None,
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
        // Two-column body: Name / Channel Type / Members on the left in
        // a fixed-width column; the wrapped Linked Sections picker on
        // the right, taking the remaining width so its toggle buttons
        // reflow into more rows when the window is narrowed.
        const LEFT_COL_W: f32 = 360.0;
        let editing = tab.editing_gang_id.is_some();
        theme::card_frame().show(ui, |ui| {
            // Pin BOTH min and max width: without max_width, frames in
            // egui can grow past the window when their content (e.g. a
            // long horizontal row) exceeds available_width, which is
            // what was causing the form card to overflow off-screen.
            let card_w = ui.available_width();
            ui.set_min_width(card_w);
            ui.set_max_width(card_w);
            theme::section_heading(ui, if editing { "Edit Gang" } else { "New Gang" });

            // Compute the applicable section set once, here, so we can
            // both retain the user's selection as members change and pass
            // the set into the right-column closure cheaply. The set is the
            // union of the sections applicable to each distinct channel type
            // among the picked members (empty members → all variants).
            let applicable: HashSet<ParameterSection> =
                applicable_sections_for(&tab.new_gang_members);
            tab.new_gang_sections.retain(|s| applicable.contains(s));

            ui.horizontal_top(|ui| {
                // Left column — fixed width form fields.
                ui.allocate_ui_with_layout(
                    egui::Vec2::new(LEFT_COL_W, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_width(LEFT_COL_W);
                        ui.set_max_width(LEFT_COL_W);
                        // 2-column label|control form. `theme::row_label`
                        // sizes each label cell to ROW_H and centres it
                        // (Grid cells top-align, so a bare label would sit
                        // above the control's centreline); `theme::row_combo`
                        // sizes the ComboBox to the same ROW_H so it matches
                        // the text edits beside / above it.
                        egui::Grid::new("add_gang_grid")
                            .num_columns(2)
                            .spacing([10.0, 10.0])
                            .show(ui, |ui| {
                                theme::row_label(ui, "Name:", theme::label_weak());
                                theme::padded_text_edit_sized(
                                    ui,
                                    &mut tab.new_gang_name,
                                    240.0,
                                    theme::ROW_H,
                                    true,
                                    "",
                                )
                                .on_hover_text(help(HelpKey::GangNewName));
                                ui.end_row();

                                theme::row_label(ui, "Members:", theme::label_weak());
                                // Open the tile picker modal; show how many are
                                // currently picked. Centre the button + count in
                                // a ROW_H cell so they sit on the "Members:"
                                // centreline (Grid cells top-align by default).
                                ui.allocate_ui_with_layout(
                                    egui::Vec2::new(240.0, theme::ROW_H),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        let label = if tab.editing_gang_id.is_some() {
                                            "Edit members…"
                                        } else {
                                            "Pick members…"
                                        };
                                        if theme::row_action_button(
                                            ui,
                                            label,
                                            theme::ACCENT_BLUE,
                                            130.0,
                                            true,
                                            help(HelpKey::GangNewMembers),
                                        ) && tab.member_picker.is_none()
                                        {
                                            // Non-blocking read: skip opening this
                                            // frame if the state lock is busy.
                                            if let Ok(st) = state.try_read() {
                                                let editing = tab.editing_gang_id.is_some();
                                                tab.member_picker =
                                                    Some(GangMemberPickerState::new(
                                                        &tab.new_gang_members,
                                                        editing,
                                                        &st,
                                                    ));
                                            }
                                        }
                                        ui.add_space(8.0);
                                        let n = tab.new_gang_members.len();
                                        ui.label(
                                            egui::RichText::new(format!("{n} picked"))
                                                .small()
                                                .color(theme::label_weak()),
                                        );
                                    },
                                );
                                ui.end_row();

                                // Pan link mode — its own control, separate
                                // from the Fader/Mute section. REV is only
                                // offered for an exact pair.
                                theme::row_label(ui, "Pan:", theme::label_weak());
                                // Centre the OFF/ON/REV buttons in a ROW_H cell
                                // so they sit on the "Pan:" centreline and fill
                                // the striped row evenly — Grid cells top-align
                                // by default, which left the short buttons high
                                // with empty grey below them.
                                ui.allocate_ui_with_layout(
                                    egui::Vec2::new(240.0, theme::ROW_H),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        // Compress button padding so OFF/ON/REV stay
                                        // within ROW_H — the default (12, 8) made them
                                        // overflow it, pushing the row taller and the
                                        // buttons off the "Pan:" centreline.
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
                                    },
                                );
                                ui.end_row();
                            });
                    },
                );

                ui.add_space(16.0);

                // Right column — Linked Sections picker. Takes the rest
                // of the form's width; horizontal_wrapped inside reflows
                // the button rows when that width shrinks. Both
                // set_min_width and set_max_width are needed (matches
                // the monitor_tab pattern) so the wrapped row knows
                // exactly where to break — without max_width, egui can
                // let the child grow past the allocated rect and the
                // tiles never wrap.
                let right_w = ui.available_width();
                ui.allocate_ui_with_layout(
                    egui::Vec2::new(right_w, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_width(right_w);
                        ui.set_max_width(right_w);
                        ui.label(
                            egui::RichText::new("Linked Sections")
                                .strong()
                                .color(theme::label_color()),
                        );
                        ui.add_space(4.0);
                        // Section toggle blocks. Non-applicable sections
                        // for the current channel type stay in the
                        // layout via `UiBuilder::invisible` so the tile
                        // grid doesn't reshuffle when the operator flips
                        // channel type.
                        //
                        // We do row wrapping manually: pre-compute each
                        // tile's width using the same measurement logic
                        // as `theme::toggle_block`, group tiles into
                        // rows that fit within `right_w`, then emit one
                        // `ui.horizontal` per row. Both `horizontal_wrapped`
                        // and explicit `with_main_wrap(true)` layouts
                        // refused to break the row in this nesting
                        // (form card → horizontal_top → top_down →
                        // wrap), so the manual approach is the only one
                        // that reliably wraps the tiles.
                        let rows = wrap_section_tiles(ui, right_w);
                        for row in &rows {
                            ui.horizontal(|ui| {
                                for section in row {
                                    let is_applicable = applicable.contains(section);
                                    let active = tab.new_gang_sections.contains(section);
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
                                            tab.new_gang_sections.insert((*section).clone());
                                        }
                                    }
                                }
                            });
                        }
                    },
                );
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let btn_text = if editing { "Save" } else { "Add Gang" };
                let btn_color = if editing {
                    theme::ACCENT_BLUE
                } else {
                    theme::ACCENT_GREEN
                };
                let save_btn =
                    theme::action_button(btn_text, btn_color, egui::Vec2::new(100.0, 32.0));
                if ui
                    .add(save_btn)
                    .on_hover_text(help(HelpKey::GangSave))
                    .clicked()
                    && !tab.new_gang_name.trim().is_empty()
                {
                    let members = tab.new_gang_members.clone();

                    if members.is_empty() {
                        tab.status_message = Some(StatusMessage::with_help(
                            "No valid members parsed",
                            HelpKey::GangsWarnNoMembers,
                        ));
                    } else if tab.new_gang_sections.is_empty() {
                        tab.status_message = Some(StatusMessage::with_help(
                            "Select at least one section",
                            HelpKey::GangsWarnNoSection,
                        ));
                    } else if members.len() < 2 {
                        tab.status_message = Some(StatusMessage::with_help(
                            "A gang needs at least 2 members",
                            HelpKey::GangsWarnMinMembers,
                        ));
                    } else if tab.new_gang_pan_mode == GangPanMode::Reversed && members.len() != 2 {
                        tab.status_message = Some(StatusMessage::with_help(
                            "Reversed pan needs exactly 2 members",
                            HelpKey::GangsWarnReversedPan,
                        ));
                    } else {
                        let name = tab.new_gang_name.trim().to_string();
                        let sections = tab.new_gang_sections.clone();
                        let pan_mode = tab.new_gang_pan_mode;
                        let mgr_clone = gang_manager.clone();

                        // Heads-up if this gang links the fader and its members
                        // sit at very different audible levels: in Relative mode
                        // that offset is preserved, so they won't be matched. The
                        // engine already collapses the inaudible bottom of the
                        // track (gang_engine floor), so this flags only real,
                        // above-floor spreads. Non-blocking — the gang is created.
                        let spread = if sections.contains(&ParameterSection::FaderMutePan) {
                            // Non-blocking: skip the spread heads-up this frame
                            // if the state lock is busy rather than block the UI.
                            state
                                .try_read()
                                .ok()
                                .and_then(|st| {
                                    floored_fader_spread(&st, &members)
                                        .filter(|s| *s > GANG_SPREAD_WARN_DB)
                                })
                        } else {
                            None
                        };
                        let spread_note = |base: String| match spread {
                            Some(s) => StatusMessage::with_help(
                                format!(
                                    "{base} — faders span {s:.0} dB; in Relative mode they keep that offset"
                                ),
                                HelpKey::GangsWarnFaderSpread,
                            ),
                            None => base.into(),
                        };

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

                        tab.new_gang_name.clear();
                        tab.new_gang_members.clear();
                        tab.new_gang_sections = HashSet::from([ParameterSection::FaderMutePan]);
                        tab.new_gang_pan_mode = GangPanMode::On;
                    }
                }

                if editing {
                    let cancel_btn = theme::action_button(
                        "Cancel",
                        theme::btn_neutral(),
                        egui::Vec2::new(80.0, 32.0),
                    );
                    if ui
                        .add(cancel_btn)
                        .on_hover_text(help(HelpKey::GangCancelEdit))
                        .clicked()
                    {
                        tab.editing_gang_id = None;
                        tab.new_gang_name.clear();
                        tab.new_gang_members.clear();
                        tab.new_gang_sections = HashSet::from([ParameterSection::FaderMutePan]);
                        tab.new_gang_pan_mode = GangPanMode::On;
                        tab.member_picker = None;
                        tab.status_message = None;
                    }
                }
            });

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
                            tab.new_gang_sections = group.linked_sections.clone();
                            tab.new_gang_pan_mode = group.effective_pan_mode();
                            tab.status_message = None;
                        }
                    }
                });
        });

        // ── Member picker modal ── Driven here, after the form and list
        // cards, where no gang-manager read-guard is held. The picker touches
        // only `state` / the egui context, so it never contends with the
        // gang-manager lock. Save writes the chosen members straight back into
        // the form's staged list; the next frame recomputes the applicable
        // Linked Sections from them.
        let picker_outcome = tab
            .member_picker
            .as_mut()
            .and_then(|p| draw_gang_member_picker(ui.ctx(), p));
        match picker_outcome {
            Some(GangPickerOutcome::Save(members)) => {
                tab.new_gang_members = members;
                tab.member_picker = None;
            }
            Some(GangPickerOutcome::Cancel) => {
                tab.member_picker = None;
            }
            None => {}
        }
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
