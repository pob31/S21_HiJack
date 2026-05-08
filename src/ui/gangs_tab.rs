use std::collections::HashSet;
use std::sync::Arc;

use eframe::egui;
use tokio::sync::RwLock;

use super::theme;
use crate::console::gang_manager::GangManager;
use crate::model::channel::ChannelId;
use crate::model::gang::{GangGroup, GangMode};
use crate::model::parameter::ParameterSection;
use uuid::Uuid;

/// Channel type selector for the Add Gang form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelTypeSelection {
    Input,
    Aux,
    Group,
    Matrix,
    ControlGroup,
    GraphicEq,
    MatrixInput,
    Mixed,
}

impl ChannelTypeSelection {
    fn label(&self) -> &'static str {
        match self {
            Self::Input => "Input",
            Self::Aux => "Aux",
            Self::Group => "Group",
            Self::Matrix => "Matrix",
            Self::ControlGroup => "Control Group",
            Self::GraphicEq => "Graphic EQ",
            Self::MatrixInput => "Matrix Input",
            Self::Mixed => "Mixed",
        }
    }

    /// Variants offered in the Channel Type dropdown. `Mixed`,
    /// `GraphicEq` and `MatrixInput` are kept in the enum (so the
    /// underlying parser still recognises GEQn / MIn prefixes if they
    /// ever appear in a saved show file) but excluded from the UI
    /// picker — operators only build gangs on the five common types.
    const ALL: [Self; 5] = [
        Self::Input,
        Self::Aux,
        Self::Group,
        Self::Matrix,
        Self::ControlGroup,
    ];

    /// Sections this channel type can sensibly gang. Returned in display order.
    /// `Mixed` returns the union of all sections so the user can compose
    /// across types and let the engine sort out per-pair applicability.
    fn applicable_sections(&self) -> Vec<ParameterSection> {
        match self {
            Self::Input => ParameterSection::applicable_to(&ChannelId::Input(1)),
            Self::Aux => ParameterSection::applicable_to(&ChannelId::Aux(1)),
            Self::Group => ParameterSection::applicable_to(&ChannelId::Group(1)),
            Self::Matrix => ParameterSection::applicable_to(&ChannelId::Matrix(1)),
            Self::ControlGroup => ParameterSection::applicable_to(&ChannelId::ControlGroup(1)),
            Self::GraphicEq => ParameterSection::applicable_to(&ChannelId::GraphicEq(1)),
            Self::MatrixInput => ParameterSection::applicable_to(&ChannelId::MatrixInput(1)),
            Self::Mixed => ParameterSection::all_variants().to_vec(),
        }
    }
}

/// One-line tooltip explaining what a `ParameterSection` actually links
/// when included in a gang. Surfaces in the UI on hover so the operator
/// doesn't have to guess at non-obvious sections (Matrix Sends, Graphic
/// EQ, CG Membership).
fn section_tooltip(section: &ParameterSection) -> &'static str {
    match section {
        ParameterSection::FaderMutePan => "Channel fader level, mute and pan/balance.",
        ParameterSection::Name => "Channel name string.",
        ParameterSection::InputGain => {
            "Head-amp gain, total/computed gain, gain tracking, phantom \
             power, main/alt input switch, stereo mode (input channels only)."
        }
        ParameterSection::Trim => {
            "Channel trim level (-40 dB to +40 dB). Available on input, \
             aux, group and matrix channels."
        }
        ParameterSection::Polarity => {
            "Phase / polarity invert. Available on input, aux, group and \
             matrix channels."
        }
        ParameterSection::BalanceWidth => {
            "Stereo balance (-1 to +1) and width — applies to stereo \
             input channels only (GP OSC, input-only per the S21 chart)."
        }
        ParameterSection::Delay => "Channel delay time and enable.",
        ParameterSection::Digitube => "Digitube drive, bias and enable.",
        ParameterSection::Eq => {
            "Parametric EQ — band gains, Q, freq, dynamic-EQ \
             settings, EQ on/off."
        }
        ParameterSection::Dyn1 => "Dynamics 1 — compressor / gate parameters and on/off.",
        ParameterSection::Dyn2 => "Dynamics 2 — second processor parameters and on/off.",
        ParameterSection::Sends => {
            "Aux send levels and on/off across all aux buses. \
             Only propagates between members of the same channel type."
        }
        ParameterSection::GroupRouting => {
            "Group routing — which group buses the channel feeds. \
             Only propagates between same-type members."
        }
        ParameterSection::Inserts => "Insert A / B send / return enable.",
        ParameterSection::CgMembership => {
            "Control Group membership — which CGs the channel belongs \
             to. Settable via the iPad protocol only (Mode 2 / Mode 3) \
             using CGs_level / CGs_mute bitmasks; GP OSC has no \
             CG-membership writer. Only propagates between same-type \
             members."
        }
        ParameterSection::GraphicEq => {
            "Graphic EQ band gains. Only meaningful when ganging \
             Graphic EQ channels (GEQ1, GEQ2, …)."
        }
        ParameterSection::MatrixSends => {
            "Matrix-send levels and on/off. Only meaningful when \
             ganging Matrix Input channels (MI1, MI2, …)."
        }
    }
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
        let label = section.to_string();
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
        let label = section.to_string();
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
    pub new_gang_channel_type: ChannelTypeSelection,
    /// Range notation: "1-4,7,12" or for Mixed: "I1-4,A1-2,G5"
    pub new_gang_members: String,
    pub new_gang_sections: HashSet<ParameterSection>,
    pub editing_gang_id: Option<uuid::Uuid>,
    pub status_message: Option<String>,
}

impl Default for GangsTabState {
    fn default() -> Self {
        Self {
            new_gang_name: String::new(),
            new_gang_channel_type: ChannelTypeSelection::Input,
            new_gang_members: String::new(),
            new_gang_sections: HashSet::from([ParameterSection::FaderMutePan]),
            editing_gang_id: None,
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
                    .color(theme::TEXT_PRIMARY),
            );
            ui.add_space(12.0);
            theme::colored_badge(
                ui,
                &format!("{active_count} active / {total_count} total"),
                theme::BG_ELEVATED,
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
            // both retain the user's selection across channel-type flips
            // and pass the set into the right-column closure cheaply.
            let applicable: HashSet<ParameterSection> = tab
                .new_gang_channel_type
                .applicable_sections()
                .into_iter()
                .collect();
            tab.new_gang_sections.retain(|s| applicable.contains(s));

            ui.horizontal_top(|ui| {
                // Left column — fixed width form fields.
                ui.allocate_ui_with_layout(
                    egui::Vec2::new(LEFT_COL_W, ui.available_height()),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_width(LEFT_COL_W);
                        ui.set_max_width(LEFT_COL_W);
                        // Each row's label needs the cell to be the
                        // full row height (26 px) AND the text painted
                        // at the cell's vertical centre — otherwise
                        // egui Grid top-aligns the natural-height
                        // (~18 px) label cell against the 26 px input
                        // cell and the input visibly drops below the
                        // label's centreline.
                        //
                        // `add_sized` doesn't help here: it reports the
                        // child's `min_rect` (label's natural ~18 px)
                        // back to the parent, so the cell never grows
                        // past 18 px. Instead we explicitly allocate
                        // `label_w × 26` and paint the galley with a
                        // y offset that centres it.
                        const ROW_H: f32 = 26.0;
                        let row_label = |ui: &mut egui::Ui, text: &str| {
                            let galley = ui.painter().layout_no_wrap(
                                text.to_string(),
                                egui::FontId::proportional(theme::FONT_SIZE_BODY),
                                theme::TEXT_SECONDARY,
                            );
                            let label_w = galley.size().x;
                            let label_h = galley.size().y;
                            let (rect, _) = ui.allocate_exact_size(
                                egui::Vec2::new(label_w, ROW_H),
                                egui::Sense::hover(),
                            );
                            let y = rect.min.y + (ROW_H - label_h) / 2.0;
                            ui.painter().galley(
                                egui::pos2(rect.min.x, y),
                                galley,
                                theme::TEXT_SECONDARY,
                            );
                        };
                        egui::Grid::new("add_gang_grid")
                            .num_columns(2)
                            .spacing([10.0, 6.0])
                            .show(ui, |ui| {
                                row_label(ui, "Name:");
                                theme::padded_text_edit(
                                    ui,
                                    &mut tab.new_gang_name,
                                    240.0,
                                    true,
                                    "",
                                );
                                ui.end_row();

                                row_label(ui, "Channel type:");
                                // Match the Name / Members text fields:
                                //  * width 240 (set via `.width(240.0)`)
                                //  * Combobox button height ~25 px so
                                //    its content centre lines up with
                                //    the label's content centre (both
                                //    sit in a 26-tall row driven by
                                //    the label cell). With pad_y=4 the
                                //    button was 27 tall, the row grew
                                //    to 27, the label cell got
                                //    half-pixel-shifted, and the
                                //    combobox text rendered off-centre.
                                //  * fill `BG_INPUT` so the combobox
                                //    interior reads as the same dark
                                //    field as the surrounding TextEdits
                                //    rather than a lighter button.
                                // Wrap the combobox in a Frame with a
                                // negative top inner_margin to nudge
                                // the button visually upward by 2 px
                                // without disturbing the row's height
                                // or the label cell's positioning —
                                // egui's auto-centring kept landing the
                                // combobox content slightly low even
                                // with matched cell heights, so we
                                // override the y offset directly.
                                egui::Frame::new()
                                    .inner_margin(egui::Margin {
                                        left: 0,
                                        right: 0,
                                        top: -2,
                                        bottom: 0,
                                    })
                                    .show(ui, |ui| {
                                        ui.spacing_mut().button_padding =
                                            egui::Vec2::new(12.0, 3.5);
                                        let visuals = ui.visuals_mut();
                                        visuals.widgets.inactive.bg_fill = theme::BG_INPUT;
                                        visuals.widgets.inactive.weak_bg_fill = theme::BG_INPUT;
                                        visuals.widgets.hovered.bg_fill = theme::BG_INPUT;
                                        visuals.widgets.hovered.weak_bg_fill = theme::BG_INPUT;
                                        visuals.widgets.open.bg_fill = theme::BG_INPUT;
                                        visuals.widgets.open.weak_bg_fill = theme::BG_INPUT;
                                        egui::ComboBox::from_id_salt("gang_channel_type")
                                            .width(240.0)
                                            .selected_text(tab.new_gang_channel_type.label())
                                            .show_ui(ui, |ui| {
                                                for ct in &ChannelTypeSelection::ALL {
                                                    ui.selectable_value(
                                                        &mut tab.new_gang_channel_type,
                                                        *ct,
                                                        ct.label(),
                                                    );
                                                }
                                            });
                                    });
                                ui.end_row();

                                row_label(ui, "Members:");
                                let hint =
                                    if tab.new_gang_channel_type == ChannelTypeSelection::Mixed {
                                        "I1-4,A1-2,G5"
                                    } else {
                                        "1-4,7,12"
                                    };
                                theme::padded_text_edit(
                                    ui,
                                    &mut tab.new_gang_members,
                                    240.0,
                                    true,
                                    hint,
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
                                .color(theme::TEXT_PRIMARY),
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
                                            theme::toggle_block(ui, &section.to_string(), active)
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
                if ui.add(save_btn).clicked() && !tab.new_gang_name.trim().is_empty() {
                    let members =
                        parse_channel_members(tab.new_gang_channel_type, &tab.new_gang_members);

                    if members.is_empty() {
                        tab.status_message = Some("No valid members parsed".into());
                    } else if tab.new_gang_sections.is_empty() {
                        tab.status_message = Some("Select at least one section".into());
                    } else if members.len() < 2 {
                        tab.status_message = Some("A gang needs at least 2 members".into());
                    } else {
                        let name = tab.new_gang_name.trim().to_string();
                        let sections = tab.new_gang_sections.clone();
                        let mgr_clone = gang_manager.clone();

                        if let Some(edit_id) = tab.editing_gang_id.take() {
                            runtime.spawn(async move {
                                let mut mgr = mgr_clone.write().await;
                                if let Some(group) = mgr.groups.get_mut(&edit_id) {
                                    group.name = name;
                                    group.members = members;
                                    group.linked_sections = sections;
                                }
                            });
                            tab.status_message = Some("Gang updated".into());
                        } else {
                            let group = GangGroup::new(name.clone(), members, sections);
                            runtime.spawn(async move {
                                mgr_clone.write().await.add_group(group);
                            });
                            tab.status_message = Some(format!("Added gang '{name}'"));
                        }

                        tab.new_gang_name.clear();
                        tab.new_gang_members.clear();
                        tab.new_gang_sections = HashSet::from([ParameterSection::FaderMutePan]);
                    }
                }

                if editing {
                    let cancel_btn = theme::action_button(
                        "Cancel",
                        theme::BG_ELEVATED,
                        egui::Vec2::new(80.0, 32.0),
                    );
                    if ui.add(cancel_btn).clicked() {
                        tab.editing_gang_id = None;
                        tab.new_gang_name.clear();
                        tab.new_gang_members.clear();
                        tab.new_gang_sections = HashSet::from([ParameterSection::FaderMutePan]);
                        tab.status_message = None;
                    }
                }
            });

            // Status message
            if let Some(ref msg) = tab.status_message {
                ui.add_space(4.0);
                ui.colored_label(theme::TEXT_WARNING, msg.as_str());
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
                                .color(theme::TEXT_SECONDARY),
                        );
                    } else {
                        let mut to_remove = None;
                        let mut to_edit = None;
                        let mut to_toggle = None;
                        let mut to_pause: Option<(Uuid, bool)> = None;
                        let mut to_set_mode: Option<(Uuid, GangMode)> = None;

                        for group in &groups {
                            let bg = if !group.enabled || group.paused {
                                theme::BG_PANEL // dimmed when disabled or paused
                            } else {
                                theme::BG_ELEVATED
                            };
                            egui::Frame::new()
                                .fill(bg)
                                .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE))
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
                                            theme::BG_ELEVATED
                                        };
                                        let toggle_label = if group.enabled { "ON" } else { "OFF" };
                                        let toggle_btn = egui::Button::new(
                                            egui::RichText::new(toggle_label)
                                                .color(theme::TEXT_PRIMARY)
                                                .strong()
                                                .small(),
                                        )
                                        .fill(toggle_color)
                                        .corner_radius(4.0);
                                        if ui.add(toggle_btn).clicked() {
                                            to_toggle = Some((group.id, !group.enabled));
                                        }

                                        // Pause button
                                        let pause_color = if group.paused {
                                            theme::ACCENT_ORANGE
                                        } else {
                                            theme::BG_ELEVATED
                                        };
                                        let pause_label =
                                            if group.paused { "PAUSED" } else { "||" };
                                        let pause_btn = egui::Button::new(
                                            egui::RichText::new(pause_label)
                                                .color(theme::TEXT_PRIMARY)
                                                .small(),
                                        )
                                        .fill(pause_color)
                                        .corner_radius(4.0);
                                        if ui.add_enabled(group.enabled, pause_btn).clicked() {
                                            to_pause = Some((group.id, !group.paused));
                                        }

                                        ui.add_space(4.0);

                                        let rel_btn = egui::Button::new(
                                            egui::RichText::new("Relative").small(),
                                        )
                                        .selected(group.mode == GangMode::Relative)
                                        .corner_radius(4.0);
                                        if ui.add_enabled(group.enabled, rel_btn).clicked() {
                                            to_set_mode = Some((group.id, GangMode::Relative));
                                        }
                                        let abs_btn = egui::Button::new(
                                            egui::RichText::new("Absolute").small(),
                                        )
                                        .selected(group.mode == GangMode::Absolute)
                                        .corner_radius(4.0);
                                        if ui.add_enabled(group.enabled, abs_btn).clicked() {
                                            to_set_mode = Some((group.id, GangMode::Absolute));
                                        }

                                        ui.add_space(8.0);

                                        ui.label(
                                            egui::RichText::new(&group.name)
                                                .strong()
                                                .color(theme::TEXT_PRIMARY),
                                        );

                                        ui.add_space(8.0);

                                        let member_text = format_members(&group.members);
                                        let member_color = if !group.members.is_empty() {
                                            theme::channel_color(&group.members[0])
                                        } else {
                                            theme::BG_ELEVATED
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
                                                        &section.to_string(),
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
                                        if ui.add(edit_btn).clicked() {
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

                        if let Some(group) = to_edit {
                            tab.editing_gang_id = Some(group.id);
                            tab.new_gang_name = group.name.clone();
                            tab.new_gang_members = format_members(&group.members);
                            tab.new_gang_sections = group.linked_sections.clone();
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

/// Parse channel members from text input.
///
/// For single-type modes: "1-4,7,12" -> vec of that type.
/// For Mixed mode: "I1-4,A1-2,G5" -> mixed vec.
pub fn parse_channel_members(channel_type: ChannelTypeSelection, input: &str) -> Vec<ChannelId> {
    let input = input.trim();
    if input.is_empty() {
        return Vec::new();
    }

    if channel_type == ChannelTypeSelection::Mixed {
        parse_mixed_members(input)
    } else {
        let numbers = parse_number_ranges(input);
        let constructor: fn(u8) -> ChannelId = match channel_type {
            ChannelTypeSelection::Input => ChannelId::Input,
            ChannelTypeSelection::Aux => ChannelId::Aux,
            ChannelTypeSelection::Group => ChannelId::Group,
            ChannelTypeSelection::Matrix => ChannelId::Matrix,
            ChannelTypeSelection::ControlGroup => ChannelId::ControlGroup,
            ChannelTypeSelection::GraphicEq => ChannelId::GraphicEq,
            ChannelTypeSelection::MatrixInput => ChannelId::MatrixInput,
            ChannelTypeSelection::Mixed => unreachable!(),
        };
        numbers.into_iter().map(constructor).collect()
    }
}

/// Parse "I1-4,A1-2,G5" into mixed channel IDs.
fn parse_mixed_members(input: &str) -> Vec<ChannelId> {
    let mut result = Vec::new();

    for token in input.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }

        // Determine prefix and rest
        let (constructor, rest): (fn(u8) -> ChannelId, &str) =
            if let Some(r) = token.strip_prefix("CG") {
                (ChannelId::ControlGroup, r)
            } else if let Some(r) = token.strip_prefix("GEQ") {
                (ChannelId::GraphicEq, r)
            } else if let Some(r) = token.strip_prefix("MI") {
                (ChannelId::MatrixInput, r)
            } else if let Some(r) = token.strip_prefix('I') {
                (ChannelId::Input, r)
            } else if let Some(r) = token.strip_prefix('A') {
                (ChannelId::Aux, r)
            } else if let Some(r) = token.strip_prefix('G') {
                (ChannelId::Group, r)
            } else if let Some(r) = token.strip_prefix('M') {
                (ChannelId::Matrix, r)
            } else {
                continue; // Unknown prefix, skip
            };

        let numbers = parse_number_ranges(rest);
        result.extend(numbers.into_iter().map(constructor));
    }

    result
}

/// Parse "1-4,7,12" into a vec of numbers.
fn parse_number_ranges(input: &str) -> Vec<u8> {
    let mut result = Vec::new();

    for part in input.split([',', ' ']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        if let Some((start_str, end_str)) = part.split_once('-') {
            if let (Ok(start), Ok(end)) =
                (start_str.trim().parse::<u8>(), end_str.trim().parse::<u8>())
            {
                for n in start..=end {
                    result.push(n);
                }
            }
        } else if let Ok(n) = part.parse::<u8>() {
            result.push(n);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_range() {
        let result = parse_channel_members(ChannelTypeSelection::Input, "1-4,7,12");
        assert_eq!(result.len(), 6);
        assert_eq!(result[0], ChannelId::Input(1));
        assert_eq!(result[3], ChannelId::Input(4));
        assert_eq!(result[4], ChannelId::Input(7));
        assert_eq!(result[5], ChannelId::Input(12));
    }

    #[test]
    fn parse_aux_single() {
        let result = parse_channel_members(ChannelTypeSelection::Aux, "3");
        assert_eq!(result, vec![ChannelId::Aux(3)]);
    }

    #[test]
    fn parse_mixed_members_notation() {
        let result = parse_channel_members(ChannelTypeSelection::Mixed, "I1-3,A1,G5");
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], ChannelId::Input(1));
        assert_eq!(result[1], ChannelId::Input(2));
        assert_eq!(result[2], ChannelId::Input(3));
        assert_eq!(result[3], ChannelId::Aux(1));
        assert_eq!(result[4], ChannelId::Group(5));
    }

    #[test]
    fn parse_empty_returns_empty() {
        let result = parse_channel_members(ChannelTypeSelection::Input, "");
        assert!(result.is_empty());
    }

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

    #[test]
    fn parse_mixed_control_group() {
        let result = parse_channel_members(ChannelTypeSelection::Mixed, "CG1-3,I5");
        assert_eq!(result.len(), 4);
        assert_eq!(result[0], ChannelId::ControlGroup(1));
        assert_eq!(result[1], ChannelId::ControlGroup(2));
        assert_eq!(result[2], ChannelId::ControlGroup(3));
        assert_eq!(result[3], ChannelId::Input(5));
    }
}
