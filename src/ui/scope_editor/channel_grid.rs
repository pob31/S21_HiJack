//! Matrix rendering for the scope editor — the channel × parameter grid that
//! drives every selection / pre-wait / fade interaction. Pure egui code; all
//! state lives in [`super::state::ScopeEditorState`] and is mutated via
//! public methods.

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
pub(super) fn draw_channel_group_block(
    ui: &mut egui::Ui,
    state: &mut ScopeEditorState,
    data: &GroupRenderData,
) {
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

    // Body: matrix only if expanded.
    if group_expanded {
        egui::Frame::new()
            .fill(theme::bg_panel())
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 6))
            .show(ui, |ui| {
                draw_group_matrix(ui, state, data);
            });
    }
}

const CELL_SIZE: egui::Vec2 = egui::Vec2 { x: 22.0, y: 18.0 };
const CELL_SPACING: f32 = 3.0;
const ROW_LABEL_WIDTH: f32 = 170.0;

/// Draw the matrix for one expanded channel-type group.
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

    egui::ScrollArea::horizontal()
        .id_salt(("scope_group_hscroll", data.group))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Zero out horizontal item spacing — all spacing is manual via add_space.
            ui.spacing_mut().item_spacing.x = 0.0;

            // ── Header row: corner [All] + per-channel header cells ──
            ui.horizontal(|ui| {
                // Corner [All] cell — width = row label + spacing.
                let corner_all =
                    state.is_all_selected(&data.channels, &data.paths, &data.available);
                let corner_any =
                    state.is_any_selected(&data.channels, &data.paths, &data.available);
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
                ui.add_space(CELL_SPACING);

                // Per-channel column headers.
                for ch in &data.channels {
                    let col_all = state.is_column_all_selected(ch, &data.paths, &data.available);
                    let col_any = state.is_column_any_selected(ch, &data.paths, &data.available);
                    let label = channel_short_label(ch);
                    // Channel header is "dirty" if any cell under it is dirty.
                    let col_dirty = data.dirty.get(ch).is_some_and(|s| !s.is_empty());
                    let resp =
                        matrix_cell(ui, CELL_SIZE, col_all, col_any, true, col_dirty, &label);
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
            });

            // ── Section blocks ──
            for (section, section_paths) in &paths_by_section {
                let key = (data.group, section.clone());
                let expanded = state.expanded_sections.contains(&key);

                let sec_all =
                    state.is_section_all_selected(section_paths, &data.channels, &data.available);
                let sec_any =
                    state.is_section_any_selected(section_paths, &data.channels, &data.available);

                // Section header row: triangle + label + per-channel section cells.
                ui.horizontal(|ui| {
                    let row = section_header_row(
                        ui,
                        if expanded { "▼" } else { "▶" },
                        &section_display_name(section),
                        sec_all,
                        sec_any,
                    );
                    match row {
                        SectionHeaderClick::Triangle => {
                            if expanded {
                                state.expanded_sections.remove(&key);
                            } else {
                                state.expanded_sections.insert(key.clone());
                            }
                        }
                        SectionHeaderClick::Label => {
                            state.toggle_section_row(
                                section_paths,
                                &data.channels,
                                &data.available,
                            );
                        }
                        SectionHeaderClick::None => {}
                    }
                    ui.add_space(CELL_SPACING);

                    // Per-channel cells for the section header row.
                    if state.edit_mode == ScopeEditMode::Scope {
                        // Scope mode: tristate toggle cells (existing behavior)
                        for ch in &data.channels {
                            let mut any_available = false;
                            let mut all_on = true;
                            let mut any_on = false;
                            let mut all_recalled = true;
                            for path in section_paths {
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
                                .is_some_and(|set| section_paths.iter().any(|p| set.contains(p)));
                            // Check if this section+channel has any timing configured
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
                                state.toggle_section_column(section_paths, ch, &data.available);
                            }
                            ui.add_space(CELL_SPACING);
                        }
                    } else {
                        // PreWait / Fade mode: render dimmed placeholder cells
                        // to keep column alignment with the timing rows.
                        for ch in &data.channels {
                            let mut any_available = false;
                            let mut all_on = true;
                            let mut any_on = false;
                            for path in section_paths {
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
                });

                // Per-category timing rows (only in PreWait/Fade modes).
                if state.edit_mode != ScopeEditMode::Scope {
                    let categories = TimingCategory::for_section(section);
                    for cat in categories {
                        // Skip Mute in Fade mode (Mute has no fade)
                        if state.edit_mode == ScopeEditMode::Fade && !cat.supports_fade() {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            // Row label: indented, showing category + mode.
                            // Uses allocate_exact_size + manual paint to match
                            // path_row_label alignment exactly.
                            let label_text = match state.edit_mode {
                                ScopeEditMode::PreWait => format!("  {} pre-wait (s)", cat.label()),
                                ScopeEditMode::Fade => format!("  {} fade (s)", cat.label()),
                                _ => unreachable!(),
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
                            let text_pos = egui::pos2(
                                rect.left() + 4.0,
                                rect.center().y - galley.size().y / 2.0,
                            );
                            ui.painter().galley(text_pos, galley, theme::label_weak());
                            ui.add_space(CELL_SPACING);

                            // Per-channel timing inputs — paint value text inside
                            // allocate_exact_size cells to match matrix_cell alignment.
                            for ch in &data.channels {
                                let key = (ch.clone(), *cat);
                                let timing = state.channel_timings.entry(key.clone()).or_default();
                                let val = match state.edit_mode {
                                    ScopeEditMode::PreWait => &mut timing.pre_wait_secs,
                                    ScopeEditMode::Fade => &mut timing.fade_time_secs,
                                    _ => unreachable!(),
                                };

                                let (rect, resp) = ui
                                    .allocate_exact_size(CELL_SIZE, egui::Sense::click_and_drag());

                                // Background
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

                                // Display value
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

                                // Drag interaction
                                if resp.dragged() {
                                    let delta = resp.drag_delta().x * 0.05;
                                    *val = (*val + delta).clamp(0.0, 30.0);
                                }

                                ui.add_space(CELL_SPACING);
                            }
                        });
                    }
                }

                // Path rows (only if expanded, and only in Scope mode or when section is expanded).
                if expanded {
                    for path in section_paths {
                        ui.horizontal(|ui| {
                            // Row label (clickable: bulk-toggle this path
                            // across the whole group).
                            let row_all =
                                state.is_row_all_selected(path, &data.channels, &data.available);
                            let row_any =
                                state.is_row_any_selected(path, &data.channels, &data.available);
                            let row_resp = path_row_label(
                                ui,
                                &path.label_with_config(&data.config),
                                row_all,
                                row_any,
                            );
                            if row_resp.clicked() {
                                state.toggle_row(path, &data.channels, &data.available);
                            }
                            ui.add_space(CELL_SPACING);

                            // Per-channel cells.
                            for ch in &data.channels {
                                let avail = cell_available(&data.available, ch, path);
                                let sel = state.is_cell_selected(ch, path);
                                let cell_dirty =
                                    data.dirty.get(ch).is_some_and(|s| s.contains(path));
                                if state.edit_mode == ScopeEditMode::Scope {
                                    let conflict =
                                        state.console_recall.is_console_recalled(ch, path);
                                    let resp = matrix_cell_ex(
                                        ui, CELL_SIZE, sel, sel, avail, cell_dirty, conflict, "",
                                    );
                                    if resp.clicked() && avail {
                                        state.toggle_cell(ch, path);
                                    }
                                    if !avail {
                                        let _ = resp.on_hover_text(help(HelpKey::ScopeNoLiveParam));
                                    } else if conflict {
                                        let _ =
                                            resp.on_hover_text(help(HelpKey::ScopeConsoleConflict));
                                    }
                                } else {
                                    // In timing modes, path cells are dimmed/read-only
                                    matrix_cell(
                                        ui, CELL_SIZE, sel, sel,
                                        false, // not available = greyed
                                        false, "",
                                    );
                                }
                                ui.add_space(CELL_SPACING);
                            }
                        });
                    }
                }
            }
        });
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
