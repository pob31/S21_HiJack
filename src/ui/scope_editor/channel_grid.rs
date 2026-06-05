//! Matrix rendering for the scope editor — the channel × parameter grid that
//! drives every selection / pre-wait / fade interaction. Pure egui code; all
//! state lives in [`super::state::ScopeEditorState`] and is mutated via
//! public methods.
//!
//! Each group's matrix is a frozen-pane table: the first column (parameter /
//! section / timing labels) stays put under horizontal scroll, and the header
//! row (channel numbers) stays put under vertical scroll. egui has no
//! `position: sticky`, so this is built from four regions whose scroll offsets
//! are synchronized — the body grid is the master, and the frozen header strip
//! (x) and label column (y) follow it (one frame behind, invisible at 50 Hz).

use std::collections::{HashMap, HashSet};

use eframe::egui;

use super::state::{
    ChannelGroup, ScopeEditMode, ScopeEditorState, cell_available, group_paths_by_section,
};
use crate::model::channel::ChannelId;
use crate::model::config::ConsoleConfig;
use crate::model::parameter::{ParameterPath, ParameterSection, TimingCategory};
use crate::ui::help::{HelpKey, help};
use crate::ui::theme;

/// Per-group data assembled once per frame, before drawing. Constructed by
/// [`super::draw_scope_window`] and consumed by the matrix renderer here.
pub(super) struct GroupRenderData {
    pub group: ChannelGroup,
    pub channels: Vec<ChannelId>,
    pub paths: Vec<ParameterPath>,
    pub available: HashMap<ChannelId, HashSet<ParameterPath>>,
    pub channel_names: HashMap<ChannelId, String>,
    /// Cloned snapshot of the live console config, used by
    /// `ParameterPath::label_with_config` to render bus rows with their
    /// current "Aux N" / "Group N" labels.
    pub config: ConsoleConfig,
    /// Phase C: per-channel dirty path set, sliced from the global dirty
    /// tracker. Empty when the dirty tracker is unavailable. Used by the
    /// matrix cell renderer to draw the golden earmark.
    pub dirty: HashMap<ChannelId, HashSet<ParameterPath>>,
}

/// Draw one channel-type group: collapsible header + (when expanded) matrix.
/// Returns the matrix frame's screen rect when expanded (so the caller can
/// decide whether to overlay a sticky channel header), or `None` when collapsed.
pub(super) fn draw_channel_group_block(
    ui: &mut egui::Ui,
    state: &mut ScopeEditorState,
    data: &GroupRenderData,
) -> Option<egui::Rect> {
    // Group header row: triangle + label + tristate cell + count.
    let group_expanded = state.expanded_groups.contains(&data.group);
    let group_all = state.is_all_selected(&data.channels, &data.paths, &data.available);
    let group_any = state.is_any_selected(&data.channels, &data.paths, &data.available);

    egui::Frame::new()
        .fill(theme::bg_elevated())
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Triangle: expand/collapse the group.
                let triangle = if group_expanded { "▼" } else { "▶" };
                let tri_resp = ui.add(
                    egui::Label::new(
                        egui::RichText::new(triangle)
                            .color(theme::label_color())
                            .size(theme::FONT_SIZE_BODY),
                    )
                    .sense(egui::Sense::click()),
                );
                if tri_resp.clicked() {
                    if group_expanded {
                        state.expanded_groups.remove(&data.group);
                    } else {
                        state.expanded_groups.insert(data.group);
                    }
                }
                ui.add_space(4.0);

                // Group label: clicking it bulk-toggles every cell in the group.
                let label_resp = ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "{} ({})",
                            data.group.label(),
                            data.channels.len()
                        ))
                        .strong()
                        .color(theme::label_color())
                        .size(theme::FONT_SIZE_SECTION),
                    )
                    .sense(egui::Sense::click()),
                );
                if label_resp.clicked() {
                    state.toggle_all(&data.channels, &data.paths, &data.available);
                }

                ui.add_space(8.0);

                // Group tristate cell.
                let tri_cell = matrix_cell(
                    ui,
                    egui::Vec2::new(28.0, 20.0),
                    group_all,
                    group_any,
                    true,
                    /* dirty */ false,
                    "",
                );
                if tri_cell.clicked() {
                    state.toggle_all(&data.channels, &data.paths, &data.available);
                }
            });
        });

    // Body: matrix only if expanded. Return its frame rect for the sticky
    // header overlay.
    if group_expanded {
        let inner = egui::Frame::new()
            .fill(theme::bg_panel())
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 6))
            .show(ui, |ui| {
                draw_group_matrix(ui, state, data);
            });
        Some(inner.response.rect)
    } else {
        None
    }
}

const CELL_SIZE: egui::Vec2 = egui::Vec2 { x: 22.0, y: 18.0 };
const CELL_SPACING: f32 = 3.0;
const ROW_LABEL_WIDTH: f32 = 170.0;
/// Height of the channel-number header row, used by the sticky-overlay backing.
pub(super) const CHANNEL_HEADER_HEIGHT: f32 = CELL_SIZE.y + 4.0;
/// Inner margin of the matrix frame (see `draw_channel_group_block`); the caller
/// needs it to place the sticky header at the corner's left edge.
pub(super) const MATRIX_INNER_MARGIN: f32 = 6.0;

/// One row of a group's matrix. Built once per frame so the frozen label column
/// and the scrolling body iterate the *same* list — identical row count and
/// order, which is what keeps them vertically aligned as sections expand or
/// collapse and timing rows appear.
enum MatrixRow {
    /// Section header: triangle + section name (label side) and per-channel
    /// section tristate cells (body side).
    SectionHeader {
        section: ParameterSection,
        paths: Vec<ParameterPath>,
        expanded: bool,
    },
    /// Per-category timing row (PreWait / Fade modes only): category label
    /// (label side) and per-channel draggable timing inputs (body side).
    Timing { cat: TimingCategory },
    /// Parameter path row (when its section is expanded): path label (label
    /// side) and per-channel toggle cells (body side).
    Path { path: ParameterPath },
}

/// Build the ordered row list for one group's matrix. Same ordering rules as the
/// legacy inline loop — sections in signal-flow order, timing rows in timing
/// modes (skipping fade-less categories in Fade mode), path rows when the
/// section is expanded.
fn build_matrix_rows(
    state: &ScopeEditorState,
    data: &GroupRenderData,
    paths_by_section: &[(ParameterSection, Vec<ParameterPath>)],
) -> Vec<MatrixRow> {
    let mut rows = Vec::new();
    for (section, section_paths) in paths_by_section {
        let key = (data.group, section.clone());
        let expanded = state.expanded_sections.contains(&key);
        rows.push(MatrixRow::SectionHeader {
            section: section.clone(),
            paths: section_paths.clone(),
            expanded,
        });

        if state.edit_mode != ScopeEditMode::Scope {
            for cat in TimingCategory::for_section(section) {
                // Skip Mute in Fade mode (Mute has no fade).
                if state.edit_mode == ScopeEditMode::Fade && !cat.supports_fade() {
                    continue;
                }
                rows.push(MatrixRow::Timing { cat: *cat });
            }
        }

        if expanded {
            for path in section_paths {
                rows.push(MatrixRow::Path { path: path.clone() });
            }
        }
    }
    rows
}

/// Draw the matrix for one expanded channel-type group as a frozen-pane table.
fn draw_group_matrix(ui: &mut egui::Ui, state: &mut ScopeEditorState, data: &GroupRenderData) {
    if data.paths.is_empty() {
        ui.label(
            egui::RichText::new("(no parameters)")
                .color(theme::label_weak())
                .small(),
        );
        return;
    }

    // Group paths by section, in section enum order. Rely on
    // ParameterPath::applicable_to returning paths in signal-flow order;
    // walking it once and grouping consecutive same-section paths gives a
    // stable ordering.
    let paths_by_section = group_paths_by_section(&data.paths);
    let rows = build_matrix_rows(state, data, &paths_by_section);

    // The group renders at its full content height and pushes the groups below
    // it down; the scope window's outer scroll handles overall vertical
    // navigation. Only the horizontal offset is synced (body → channel header),
    // so the channel numbers track the cell columns; the label column is a
    // plain unscrolled strip beside the body, keeping the parameter names put
    // while the channels scroll horizontally (only needed when the window is too
    // narrow to fit every channel).
    let prev_x = state
        .matrix_scroll_offset
        .get(&data.group)
        .map(|(x, _)| *x)
        .unwrap_or(0.0);

    // ── Top row: corner + channel header (in-flow). When this row scrolls
    // above the viewport, the caller overlays a sticky copy at the top. ──
    draw_channel_header_row(ui, state, data, prev_x, false);

    // ── Body row: frozen label column + full-height grid (horizontal scroll) ──
    // NOTE: a ScrollArea's content UI *inherits the parent layout*, and these
    // panes live inside `ui.horizontal(...)`, so the body re-establishes a
    // vertical layout explicitly — otherwise the rows render left-to-right.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            draw_label_column(ui, state, data, &rows);
        });

        ui.add_space(CELL_SPACING);

        let out = egui::ScrollArea::horizontal()
            .id_salt(("scope_body", data.group))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    draw_body_grid(ui, state, data, &rows);
                });
            });

        // Feed the body's horizontal offset to the channel header next frame.
        state
            .matrix_scroll_offset
            .insert(data.group, (out.state.offset.x, 0.0));
    });
}

/// Render the corner "All" cell + the per-channel header strip as one row.
/// Used in-flow at the top of each group's matrix and re-used by the sticky
/// overlay header. `sticky = true` gives it a distinct scroll id so the two
/// copies don't collide. `h_offset` is the body's horizontal scroll offset so
/// the channel numbers track the columns below.
pub(super) fn draw_channel_header_row(
    ui: &mut egui::Ui,
    state: &mut ScopeEditorState,
    data: &GroupRenderData,
    h_offset: f32,
    sticky: bool,
) {
    let salt = if sticky {
        ("scope_header_sticky", data.group)
    } else {
        ("scope_header", data.group)
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        draw_corner(ui, state, data);
        ui.add_space(CELL_SPACING);
        egui::ScrollArea::horizontal()
            .id_salt(salt)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .horizontal_scroll_offset(h_offset)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    draw_header_cells(ui, state, data);
                });
            });
    });
}

/// Top-left corner "All" cell — selects/deselects the whole group.
fn draw_corner(ui: &mut egui::Ui, state: &mut ScopeEditorState, data: &GroupRenderData) {
    let corner_all = state.is_all_selected(&data.channels, &data.paths, &data.available);
    let corner_any = state.is_any_selected(&data.channels, &data.paths, &data.available);
    let corner_resp = matrix_cell(
        ui,
        egui::Vec2::new(ROW_LABEL_WIDTH, CELL_SIZE.y),
        corner_all,
        corner_any,
        true,
        /* dirty */ false,
        "All",
    );
    if corner_resp.clicked() {
        state.toggle_all(&data.channels, &data.paths, &data.available);
    }
}

/// Per-channel column headers (the frozen header strip's contents).
fn draw_header_cells(ui: &mut egui::Ui, state: &mut ScopeEditorState, data: &GroupRenderData) {
    for ch in &data.channels {
        let col_all = state.is_column_all_selected(ch, &data.paths, &data.available);
        let col_any = state.is_column_any_selected(ch, &data.paths, &data.available);
        let label = channel_short_label(ch);
        // Channel header is "dirty" if any cell under it is dirty.
        let col_dirty = data.dirty.get(ch).is_some_and(|s| !s.is_empty());
        let resp = matrix_cell(ui, CELL_SIZE, col_all, col_any, true, col_dirty, &label);
        if resp.clicked() {
            state.toggle_column(ch, &data.paths, &data.available);
        }
        let tooltip = match data.channel_names.get(ch) {
            Some(n) => format!("{ch} — {n}"),
            None => format!("{ch}"),
        };
        let _ = resp.on_hover_text(tooltip);
        ui.add_space(CELL_SPACING);
    }
}

/// Frozen first column — renders the label half of every row in order.
fn draw_label_column(
    ui: &mut egui::Ui,
    state: &mut ScopeEditorState,
    data: &GroupRenderData,
    rows: &[MatrixRow],
) {
    for row in rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            match row {
                MatrixRow::SectionHeader {
                    section,
                    paths,
                    expanded,
                } => {
                    let sec_all =
                        state.is_section_all_selected(paths, &data.channels, &data.available);
                    let sec_any =
                        state.is_section_any_selected(paths, &data.channels, &data.available);
                    let click = section_header_row(
                        ui,
                        if *expanded { "▼" } else { "▶" },
                        &section_display_name(section),
                        sec_all,
                        sec_any,
                    );
                    match click {
                        SectionHeaderClick::Triangle => {
                            let key = (data.group, section.clone());
                            if *expanded {
                                state.expanded_sections.remove(&key);
                            } else {
                                state.expanded_sections.insert(key);
                            }
                        }
                        SectionHeaderClick::Label => {
                            state.toggle_section_row(paths, &data.channels, &data.available);
                        }
                        SectionHeaderClick::None => {}
                    }
                }
                MatrixRow::Timing { cat } => {
                    // Indented category label; matches path_row_label alignment.
                    let label_text = match state.edit_mode {
                        ScopeEditMode::PreWait => format!("  {} pre-wait (s)", cat.label()),
                        ScopeEditMode::Fade => format!("  {} fade (s)", cat.label()),
                        ScopeEditMode::Scope => String::new(),
                    };
                    let (rect, _resp) = ui.allocate_exact_size(
                        egui::Vec2::new(ROW_LABEL_WIDTH, CELL_SIZE.y),
                        egui::Sense::hover(),
                    );
                    let galley = ui.painter().layout_no_wrap(
                        label_text,
                        egui::FontId::proportional(theme::FONT_SIZE_BADGE),
                        theme::label_weak(),
                    );
                    let text_pos =
                        egui::pos2(rect.left() + 4.0, rect.center().y - galley.size().y / 2.0);
                    ui.painter().galley(text_pos, galley, theme::label_weak());
                }
                MatrixRow::Path { path } => {
                    let row_all = state.is_row_all_selected(path, &data.channels, &data.available);
                    let row_any = state.is_row_any_selected(path, &data.channels, &data.available);
                    let row_resp =
                        path_row_label(ui, &path.label_with_config(&data.config), row_all, row_any);
                    if row_resp.clicked() {
                        state.toggle_row(path, &data.channels, &data.available);
                    }
                }
            }
        });
    }
}

/// Scrolling body — renders the per-channel cell half of every row in order.
fn draw_body_grid(
    ui: &mut egui::Ui,
    state: &mut ScopeEditorState,
    data: &GroupRenderData,
    rows: &[MatrixRow],
) {
    for row in rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            match row {
                MatrixRow::SectionHeader { section, paths, .. } => {
                    if state.edit_mode == ScopeEditMode::Scope {
                        // Scope mode: tristate toggle cells (existing behavior).
                        for ch in &data.channels {
                            let mut any_available = false;
                            let mut all_on = true;
                            let mut any_on = false;
                            let mut all_recalled = true;
                            for path in paths {
                                if !cell_available(&data.available, ch, path) {
                                    continue;
                                }
                                any_available = true;
                                if state.is_cell_selected(ch, path) {
                                    any_on = true;
                                } else {
                                    all_on = false;
                                }
                                if !state.console_recall.is_console_recalled(ch, path) {
                                    all_recalled = false;
                                }
                            }
                            let cell_all = any_available && all_on;
                            let cell_any = any_on;
                            let cell_recalled = any_available && all_recalled;
                            let cell_dirty = data
                                .dirty
                                .get(ch)
                                .is_some_and(|set| paths.iter().any(|p| set.contains(p)));
                            // Check if this section+channel has any timing configured.
                            let has_timing =
                                TimingCategory::for_section(section).iter().any(|cat| {
                                    let t = state
                                        .channel_timings
                                        .get(&(ch.clone(), *cat))
                                        .cloned()
                                        .unwrap_or_default();
                                    t.pre_wait_secs != 0.0 || t.fade_time_secs != 0.0
                                });
                            let resp = matrix_cell_ex(
                                ui,
                                CELL_SIZE,
                                cell_all,
                                cell_any,
                                any_available,
                                cell_dirty || has_timing, // show timing indicator via earmark
                                cell_recalled,
                                "",
                            );
                            if resp.clicked() && any_available {
                                state.toggle_section_column(paths, ch, &data.available);
                            }
                            ui.add_space(CELL_SPACING);
                        }
                    } else {
                        // PreWait / Fade mode: dimmed placeholder cells to keep
                        // column alignment with the timing rows.
                        for ch in &data.channels {
                            let mut any_available = false;
                            let mut all_on = true;
                            let mut any_on = false;
                            for path in paths {
                                if !cell_available(&data.available, ch, path) {
                                    continue;
                                }
                                any_available = true;
                                if state.is_cell_selected(ch, path) {
                                    any_on = true;
                                } else {
                                    all_on = false;
                                }
                            }
                            let cell_all = any_available && all_on;
                            let cell_any = any_on;
                            matrix_cell(
                                ui, CELL_SIZE, cell_all, cell_any,
                                false, // not interactive in timing mode
                                false, "",
                            );
                            ui.add_space(CELL_SPACING);
                        }
                    }
                }
                MatrixRow::Timing { cat } => {
                    for ch in &data.channels {
                        let key = (ch.clone(), *cat);
                        let timing = state.channel_timings.entry(key.clone()).or_default();
                        let val = match state.edit_mode {
                            ScopeEditMode::PreWait => &mut timing.pre_wait_secs,
                            ScopeEditMode::Fade => &mut timing.fade_time_secs,
                            ScopeEditMode::Scope => unreachable!(),
                        };

                        let (rect, resp) =
                            ui.allocate_exact_size(CELL_SIZE, egui::Sense::click_and_drag());

                        // Background.
                        let bg = if *val != 0.0 {
                            theme::SCOPE_ACTIVE
                        } else {
                            theme::SCOPE_INACTIVE
                        };
                        let bg = if resp.hovered() {
                            theme::lighten(bg, 25)
                        } else {
                            bg
                        };
                        ui.painter().rect_filled(rect, 3.0, bg);

                        // Display value.
                        let text = format!("{:.1}", val);
                        let galley = ui.painter().layout_no_wrap(
                            text,
                            egui::FontId::proportional(8.0),
                            theme::TEXT_PRIMARY,
                        );
                        let text_pos = egui::pos2(
                            rect.center().x - galley.size().x / 2.0,
                            rect.center().y - galley.size().y / 2.0,
                        );
                        ui.painter().galley(text_pos, galley, theme::TEXT_PRIMARY);

                        // Drag interaction.
                        if resp.dragged() {
                            let delta = resp.drag_delta().x * 0.05;
                            *val = (*val + delta).clamp(0.0, 30.0);
                        }

                        ui.add_space(CELL_SPACING);
                    }
                }
                MatrixRow::Path { path } => {
                    for ch in &data.channels {
                        let avail = cell_available(&data.available, ch, path);
                        let sel = state.is_cell_selected(ch, path);
                        let cell_dirty = data.dirty.get(ch).is_some_and(|s| s.contains(path));
                        if state.edit_mode == ScopeEditMode::Scope {
                            let conflict = state.console_recall.is_console_recalled(ch, path);
                            let resp = matrix_cell_ex(
                                ui, CELL_SIZE, sel, sel, avail, cell_dirty, conflict, "",
                            );
                            if resp.clicked() && avail {
                                state.toggle_cell(ch, path);
                            }
                            if !avail {
                                let _ = resp.on_hover_text(help(HelpKey::ScopeNoLiveParam));
                            } else if conflict {
                                let _ = resp.on_hover_text(help(HelpKey::ScopeConsoleConflict));
                            }
                        } else {
                            // In timing modes, path cells are dimmed/read-only.
                            matrix_cell(
                                ui, CELL_SIZE, sel, sel, false, // not available = greyed
                                false, "",
                            );
                        }
                        ui.add_space(CELL_SPACING);
                    }
                }
            }
        });
    }
}

/// Friendly section name shown in the section header row.
fn section_display_name(s: &ParameterSection) -> String {
    match s {
        ParameterSection::Name => "Identity".into(),
        ParameterSection::FaderMutePan => "Fader / Mute / Pan".into(),
        ParameterSection::InputGain => "Input".into(),
        ParameterSection::Trim => "Trim".into(),
        ParameterSection::Polarity => "Polarity".into(),
        ParameterSection::BalanceWidth => "Balance / Width".into(),
        ParameterSection::Delay => "Delay".into(),
        ParameterSection::Digitube => "DiGiTube".into(),
        ParameterSection::Eq => "EQ".into(),
        ParameterSection::Dyn1 => "Dynamics 1".into(),
        ParameterSection::Dyn2 => "Dynamics 2".into(),
        ParameterSection::Sends => "Aux Sends".into(),
        ParameterSection::GroupRouting => "Group Routing".into(),
        ParameterSection::Inserts => "Inserts".into(),
        ParameterSection::CgMembership => "CG Membership".into(),
        ParameterSection::GraphicEq => "Graphic EQ".into(),
        ParameterSection::MatrixSends => "Matrix Sends".into(),
    }
}

/// Compact channel column-header text. Shows just the index ("1", "2"…).
fn channel_short_label(ch: &ChannelId) -> String {
    match ch {
        ChannelId::Input(n)
        | ChannelId::Aux(n)
        | ChannelId::Group(n)
        | ChannelId::Matrix(n)
        | ChannelId::ControlGroup(n)
        | ChannelId::GraphicEq(n)
        | ChannelId::MatrixInput(n) => n.to_string(),
    }
}

// ── Cell rendering primitives ──────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
enum SectionHeaderClick {
    None,
    Triangle,
    Label,
}

/// Render a section header row's leading triangle + label as one combined
/// horizontal item. Returns which sub-area was clicked, if any.
fn section_header_row(
    ui: &mut egui::Ui,
    triangle: &str,
    label: &str,
    all_selected: bool,
    any_selected: bool,
) -> SectionHeaderClick {
    let mut click = SectionHeaderClick::None;

    let total_width = ROW_LABEL_WIDTH;
    let triangle_width: f32 = 16.0;
    let label_width = total_width - triangle_width;

    // Triangle area
    let (tri_rect, tri_resp) = ui.allocate_exact_size(
        egui::Vec2::new(triangle_width, CELL_SIZE.y),
        egui::Sense::click(),
    );
    let tri_galley = ui.painter().layout_no_wrap(
        triangle.to_string(),
        egui::FontId::proportional(theme::FONT_SIZE_BODY),
        theme::label_color(),
    );
    let tri_pos = egui::pos2(
        tri_rect.left() + 2.0,
        tri_rect.center().y - tri_galley.size().y / 2.0,
    );
    ui.painter()
        .galley(tri_pos, tri_galley, theme::label_color());
    if tri_resp.clicked() {
        click = SectionHeaderClick::Triangle;
    }

    // Label area (clickable bulk-toggle target).
    let (label_rect, label_resp) = ui.allocate_exact_size(
        egui::Vec2::new(label_width, CELL_SIZE.y),
        egui::Sense::click(),
    );

    // Background tint based on tristate selection.
    let bg = if all_selected {
        theme::SCOPE_ACTIVE
    } else if any_selected {
        theme::SCOPE_PARTIAL
    } else {
        theme::bg_elevated()
    };
    let bg = if label_resp.hovered() {
        theme::lighten(bg, 25)
    } else {
        bg
    };
    ui.painter().rect_filled(label_rect, 3.0, bg);

    let text_color = if all_selected || any_selected {
        theme::TEXT_PRIMARY
    } else {
        theme::label_weak()
    };
    let label_galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(theme::FONT_SIZE_BADGE),
        text_color,
    );
    let label_pos = egui::pos2(
        label_rect.left() + 6.0,
        label_rect.center().y - label_galley.size().y / 2.0,
    );
    ui.painter().galley(label_pos, label_galley, text_color);

    if label_resp.clicked() {
        click = SectionHeaderClick::Label;
    }

    click
}

/// Render a single ParameterPath row label as a clickable bulk-toggle target.
fn path_row_label(
    ui: &mut egui::Ui,
    label: &str,
    all_selected: bool,
    any_selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::Vec2::new(ROW_LABEL_WIDTH, CELL_SIZE.y),
        egui::Sense::click(),
    );
    let bg = if all_selected {
        theme::SCOPE_ACTIVE
    } else if any_selected {
        theme::SCOPE_PARTIAL
    } else {
        theme::bg_panel()
    };
    let bg = if response.hovered() {
        theme::lighten(bg, 25)
    } else {
        bg
    };
    ui.painter().rect_filled(rect, 3.0, bg);

    let text_color = if all_selected || any_selected {
        theme::TEXT_PRIMARY
    } else {
        theme::label_weak()
    };
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(11.0),
        text_color,
    );
    let pos = egui::pos2(rect.left() + 16.0, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, text_color);
    response
}

/// Render a single matrix cell. Available cells respond to clicks; unavailable
/// cells render dim and ignore interaction.
fn matrix_cell(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    all_selected: bool,
    any_selected: bool,
    available: bool,
    dirty: bool,
    label: &str,
) -> egui::Response {
    matrix_cell_ex(
        ui,
        size,
        all_selected,
        any_selected,
        available,
        dirty,
        false,
        label,
    )
}

fn matrix_cell_ex(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    all_selected: bool,
    any_selected: bool,
    available: bool,
    dirty: bool,
    conflict: bool,
    label: &str,
) -> egui::Response {
    let sense = if available {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);

    let scope_conflict = egui::Color32::from_rgb(50, 100, 200); // blue
    let base = if !available {
        theme::SCOPE_UNAVAILABLE
    } else if all_selected && conflict {
        scope_conflict
    } else if all_selected {
        theme::SCOPE_ACTIVE
    } else if any_selected {
        theme::SCOPE_PARTIAL
    } else if conflict {
        theme::SCOPE_INACTIVE_RECALLED
    } else {
        theme::SCOPE_INACTIVE
    };
    let fill = if available && response.hovered() {
        theme::lighten(base, 25)
    } else {
        base
    };
    ui.painter().rect_filled(rect, 3.0, fill);

    if !available {
        // Outline-only treatment for unavailable cells.
        ui.painter().rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.0, theme::TEXT_DISABLED),
            egui::StrokeKind::Inside,
        );
    }

    if !label.is_empty() {
        let color = if !available {
            theme::TEXT_DISABLED
        } else if all_selected || any_selected {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        };
        let galley =
            ui.painter()
                .layout_no_wrap(label.to_string(), egui::FontId::proportional(9.0), color);
        let pos = rect.center() - galley.size() / 2.0;
        ui.painter().galley(pos, galley, color);
    }

    // Phase C: golden earmark in the top-right corner when dirty.
    // Triangle ~35% of the cell width — visible without obscuring the label.
    if dirty {
        let earmark_size = (size.x.min(size.y) * 0.35).max(4.0);
        let cx = rect.right() - 1.0;
        let cy = rect.top() + 1.0;
        let points = vec![
            egui::pos2(cx - earmark_size, cy),
            egui::pos2(cx, cy),
            egui::pos2(cx, cy + earmark_size),
        ];
        ui.painter().add(egui::Shape::convex_polygon(
            points,
            theme::SCOPE_DIRTY,
            egui::Stroke::NONE,
        ));
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_data(group: ChannelGroup) -> GroupRenderData {
        GroupRenderData {
            group,
            channels: vec![],
            paths: vec![],
            available: HashMap::new(),
            channel_names: HashMap::new(),
            config: ConsoleConfig::default(),
            dirty: HashMap::new(),
        }
    }

    #[test]
    fn scope_mode_emits_path_rows_only_for_expanded_sections() {
        let mut state = ScopeEditorState::default(); // edit_mode = Scope → no timing rows
        let group = ChannelGroup::Inputs;
        state
            .expanded_sections
            .insert((group, ParameterSection::Eq));

        let pbs = vec![
            (
                ParameterSection::FaderMutePan,
                vec![ParameterPath::Fader, ParameterPath::Mute],
            ),
            (
                ParameterSection::Eq,
                vec![ParameterPath::EqBandGain(1), ParameterPath::EqBandGain(2)],
            ),
        ];

        let rows = build_matrix_rows(&state, &render_data(group), &pbs);

        // Two section headers; only the expanded Eq section contributes path rows.
        assert_eq!(rows.len(), 4);
        assert!(matches!(
            rows[0],
            MatrixRow::SectionHeader {
                expanded: false,
                ..
            }
        ));
        assert!(matches!(
            rows[1],
            MatrixRow::SectionHeader { expanded: true, .. }
        ));
        assert!(matches!(rows[2], MatrixRow::Path { .. }));
        assert!(matches!(rows[3], MatrixRow::Path { .. }));
    }

    #[test]
    fn timing_mode_inserts_timing_rows_after_each_section_header() {
        let mut state = ScopeEditorState::default();
        state.edit_mode = ScopeEditMode::PreWait;
        let group = ChannelGroup::Inputs;

        // FaderMutePan has two timing categories (Fader, Mute); section not
        // expanded, so no path rows follow.
        let pbs = vec![(
            ParameterSection::FaderMutePan,
            vec![ParameterPath::Fader, ParameterPath::Mute],
        )];

        let rows = build_matrix_rows(&state, &render_data(group), &pbs);

        assert_eq!(rows.len(), 3); // header + 2 timing rows
        assert!(matches!(rows[0], MatrixRow::SectionHeader { .. }));
        assert!(
            rows[1..]
                .iter()
                .all(|r| matches!(r, MatrixRow::Timing { .. }))
        );
    }

    #[test]
    fn fade_mode_skips_fadeless_timing_categories() {
        let mut state = ScopeEditorState::default();
        state.edit_mode = ScopeEditMode::Fade;
        let group = ChannelGroup::Inputs;

        let pbs = vec![(
            ParameterSection::FaderMutePan,
            vec![ParameterPath::Fader, ParameterPath::Mute],
        )];

        let rows = build_matrix_rows(&state, &render_data(group), &pbs);

        // Mute has no fade, so only the Fader timing row survives in Fade mode.
        let timing_rows = rows
            .iter()
            .filter(|r| matches!(r, MatrixRow::Timing { .. }))
            .count();
        assert_eq!(timing_rows, 1);
    }
}
