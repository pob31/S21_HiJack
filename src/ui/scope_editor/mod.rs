//! Phase A: dedicated scope editor window with two-level collapsing.
//!
//! The editor presents one matrix per channel-type group (Inputs, Aux, Groups,
//! Matrix, Control Groups, Graphic EQ, Matrix Inputs). Each group is a
//! collapsible top-level section. Inside an expanded group, parameter sections
//! are themselves collapsible. Rows = individual `ParameterPath` variants
//! (maximum granularity), columns = channels in that group.
//!
//! Every header (corner / channel header / section header label / path row
//! label) is a one-click bulk toggle. Cells with no live data on the console
//! render greyed and ignore clicks. Channel column tooltips show the live
//! channel name from the console.
//!
//! Window state lives in [`state::ScopeEditorState`]. Open/Cancel/OK semantics:
//! opening takes a backup of the current selections; Cancel restores the
//! backup, OK commits the changes.
//!
//! See [Documentation/DiGiCo S OSC Commandset_OSCpaths.csv] for the source of
//! truth that drives `ParameterPath::available_for_channel`.
//!
//! ## Module layout (audit M5 — split from a 2,040-LOC monolith)
//!
//! - [`state`] — selection / filter / lifecycle state. Pure logic, no egui.
//!   All 18 unit tests live here.
//! - [`channel_grid`] — matrix rendering. Reads / mutates `ScopeEditorState`
//!   via public methods. Pure egui code.
//! - This `mod.rs` — public surface (`draw_scope_window`, `ScopeWindowOutcome`,
//!   `ScopeWindowResult`) plus per-frame `GroupRenderData` assembly that the
//!   matrix renderer consumes.

mod channel_grid;
mod state;

pub use state::{ChannelGroup, ScopeEditMode, ScopeEditorState};

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use eframe::egui;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::console::cue_manager::CueManager;
use crate::model::channel::ChannelId;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::parameter::{ParameterPath, TimingCategory};
use crate::model::snapshot::SnapshotKind;
use crate::model::state::ConsoleState;
use crate::ui::help::{HelpKey, help};
use crate::ui::recall_scope_popup::{RecallPopupKind, draw_recall_popup};
use crate::ui::theme;
use channel_grid::{
    CHANNEL_HEADER_HEIGHT, GroupRenderData, MATRIX_INNER_MARGIN, draw_channel_group_block,
    draw_channel_header_row, timing_keys_in_order,
};

// ── Window-rendering entrypoint ─────────────────────────────────────

/// Outcome of a single `draw_scope_window` frame. Carries the window
/// open/close result plus any deferred side effects the caller needs to
/// dispatch on the dirty tracker (which the editor only has read access to
/// during render).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScopeWindowOutcome {
    pub status: ScopeWindowResult,
    /// True if the operator clicked "Clear changes" — the caller should
    /// acquire a write lock on the dirty tracker and call `clear()`.
    pub clear_dirty_requested: bool,
    /// True if the operator clicked "Apply to selected" — the caller should
    /// write the current scope (`to_scope_template`) into the selected
    /// snapshot's scope (data untouched). The window stays open.
    pub apply_to_selected_requested: bool,
}

/// The snapshot the editor's "Apply Scope to Snapshot" action targets — the
/// currently-selected snapshot. Assembled by the caller (app.rs) from one
/// `cue_manager` read per frame so the editor closure never holds the guard.
pub struct ScopeApplyTarget {
    pub name: String,
    pub kind: SnapshotKind,
    /// How many cues recall this snapshot with a `scope_override` (which masks
    /// snapshot-scope edits at recall). Zero when none — the warning is hidden.
    pub override_cue_count: usize,
}

/// Result of a single `draw_scope_window` frame.
#[derive(Debug, Default, PartialEq, Eq)]
pub enum ScopeWindowResult {
    /// Window is still open and waiting for user input.
    #[default]
    StillOpen,
    /// User clicked OK; caller should read `to_scope_template`.
    Committed,
    /// User clicked Cancel or closed the window; selections were rolled back.
    Cancelled,
}

/// Render the scope editor window. Early-returns `StillOpen` (without drawing)
/// when `state.window_open` is false. The caller should invoke this from the
/// App-level `update()` so the window floats above the central panel.
///
/// `dirty` is the live dirty tracker borrow (from `try_read()`). When `None`
/// the toolbar's three "modified" controls render greyed out — there's no
/// data to show.
pub fn draw_scope_window(
    ctx: &egui::Context,
    state: &mut ScopeEditorState,
    console_state: &ConsoleState,
    dirty: Option<&DirtyTracker>,
    cue_manager: &Arc<RwLock<CueManager>>,
    runtime: &tokio::runtime::Handle,
    window_title: &str,
    // The currently-selected snapshot, if any. Enables the "Apply Scope to
    // Snapshot" action (which writes this scope into that snapshot) and drives
    // its name/kind badge and scope-override warning.
    apply_target: Option<&ScopeApplyTarget>,
) -> ScopeWindowOutcome {
    if !state.window_open {
        return ScopeWindowOutcome::default();
    }

    // Snapshot templates (id, name) up front so the egui closure doesn't
    // hold the cue_manager read guard while it renders.
    let templates: Vec<(Uuid, String)> = cue_manager
        .try_read()
        .map(|mgr| {
            let mut v: Vec<_> = mgr
                .scope_templates
                .values()
                .map(|t| (t.id, t.name.clone()))
                .collect();
            v.sort_by(|a, b| a.1.cmp(&b.1));
            v
        })
        .unwrap_or_default();
    // Pull the full template for Load (if one is selected) so we don't
    // need to reach back into cue_manager inside the closure.
    let selected_template_full = state.selected_template_id.and_then(|id| {
        cue_manager
            .try_read()
            .ok()
            .and_then(|mgr| mgr.scope_templates.get(&id).cloned())
    });

    // Phase C: if auto-preselect is on AND the dirty tracker's generation
    // has changed since we last looked, replace the selections with the
    // dirty set. Done BEFORE rendering so the matrix shows the new state.
    if state.auto_preselect_modified
        && let Some(d) = dirty
        && d.generation() != state.last_dirty_generation
    {
        state.apply_dirty_set(d.dirty_set());
        state.last_dirty_generation = d.generation();
    }

    // Snapshot config + channel names once per frame, BEFORE entering the
    // egui::Window closure that holds an exclusive borrow of state.
    let config = console_state.config.clone();
    let aux_count = config.aux_output_count;
    let group_count = config.group_output_count;
    let matrix_count = config.matrix_input_count;

    // Build per-group data: channels, applicable paths, availability map,
    // channel-name lookup. Done up front so the inner closures don't need
    // ConsoleState anymore.
    //
    // Availability is **static** — sourced from ParameterPath::available_for_channel
    // (the CSV table). Every cell whose path is applicable to the channel type
    // is selectable, regardless of whether the live console has pushed a value
    // for it yet. This lets the operator build a scope BEFORE connecting, and
    // it lets aux/group/matrix/CG cells be selectable from the moment the
    // window opens (the previous live-data check left them all greyed until GP
    // OSC discovery populated the parameter mirror, which made it look like
    // those channel types were missing from the scope).
    //
    // Capturing a (channel, path) cell that has no live value just produces no
    // entry in the snapshot — ConsoleState::capture skips missing parameters
    // silently. No harm.
    let groups_data: Vec<GroupRenderData> = ChannelGroup::all()
        .iter()
        .map(|g| {
            let channels = g.channels_from(&config);
            let paths = ParameterPath::applicable_to(
                &g.representative_channel(),
                aux_count,
                group_count,
                matrix_count,
            );
            // Build the availability map: every (channel, path) pair where the
            // path is statically applicable to the channel type. Within a
            // group every channel has the same type, so every channel gets the
            // same set of available paths. ParameterPath::applicable_to has
            // already filtered the rows, so the set is just `paths` cloned.
            let path_set: HashSet<ParameterPath> = paths.iter().cloned().collect();
            let available: HashMap<ChannelId, HashSet<ParameterPath>> = channels
                .iter()
                .map(|ch| (ch.clone(), path_set.clone()))
                .collect();
            let channel_names = console_state.channel_names_for(&channels);
            // Per-group dirty subset (only the channels in this group).
            let mut dirty_subset: HashMap<ChannelId, HashSet<ParameterPath>> = HashMap::new();
            if let Some(d) = dirty {
                let full = d.dirty_set();
                for ch in &channels {
                    if let Some(paths) = full.get(ch)
                        && !paths.is_empty()
                    {
                        dirty_subset.insert(ch.clone(), paths.clone());
                    }
                }
            }
            GroupRenderData {
                group: *g,
                channels,
                paths,
                available,
                channel_names,
                config: config.clone(),
                dirty: dirty_subset,
            }
        })
        .collect();

    // Global render-order key list for the timing selection — lets the numeric
    // entry box resolve a deterministic "first selected" cell. Empty in Scope
    // mode (the selection only exists in timing modes).
    let timing_order: Vec<(ChannelId, TimingCategory)> =
        timing_keys_in_order(&groups_data, state.edit_mode);

    let mut outcome = ScopeWindowOutcome::default();
    let mut still_open = state.window_open;
    let dirty_has_any = dirty.map(|d| d.has_any()).unwrap_or(false);

    // Open spanning the full application window so the maximum number of
    // channel columns is visible at once. The screen-based default applies on
    // first show for this id; the operator can still resize/move afterward.
    let screen = ctx.content_rect();
    egui::Window::new(window_title)
        // Pin a stable id so a changing title (e.g. "Editing scope: <name>")
        // doesn't reset the window's size/position every frame. The "_full"
        // suffix re-applies the new full-width default once for sessions that
        // persisted the older fixed size.
        .id(egui::Id::new("snapshot_scope_window_full"))
        .collapsible(false)
        .resizable(true)
        .default_pos(screen.left_top() + egui::vec2(8.0, 40.0))
        .default_size([screen.width() - 16.0, (screen.height() - 80.0).max(400.0)])
        .open(&mut still_open)
        .show(ctx, |ui| {
            // ─ Toolbar ─
            ui.horizontal(|ui| {
                // ── Mode toggle ──
                let mode_btn_size = egui::Vec2::new(70.0, 28.0);
                if ui
                    .add(
                        egui::Button::new("Scope")
                            .selected(state.edit_mode == ScopeEditMode::Scope)
                            .min_size(mode_btn_size),
                    )
                    .on_hover_text(help(HelpKey::ScopeModeScope))
                    .clicked()
                {
                    state.set_edit_mode(ScopeEditMode::Scope);
                }
                if ui
                    .add(
                        egui::Button::new("Pre-wait")
                            .selected(state.edit_mode == ScopeEditMode::PreWait)
                            .min_size(mode_btn_size),
                    )
                    .on_hover_text(help(HelpKey::ScopeModePreWait))
                    .clicked()
                {
                    state.set_edit_mode(ScopeEditMode::PreWait);
                }
                if ui
                    .add(
                        egui::Button::new("Fade")
                            .selected(state.edit_mode == ScopeEditMode::Fade)
                            .min_size(mode_btn_size),
                    )
                    .on_hover_text(help(HelpKey::ScopeModeFade))
                    .clicked()
                {
                    state.set_edit_mode(ScopeEditMode::Fade);
                }
                ui.separator();
                ui.add_space(8.0);

                if state.edit_mode == ScopeEditMode::Scope {
                    let clear_btn = theme::action_button(
                        "Clear All",
                        theme::btn_neutral(),
                        egui::Vec2::new(80.0, 28.0),
                    );
                    if ui
                        .add(clear_btn)
                        .on_hover_text(help(HelpKey::ScopeClearAll))
                        .clicked()
                    {
                        state.clear();
                    }
                    ui.add_space(8.0);
                    let sel_count = state.selection_count();
                    // Fixed width so the badge doesn't grow with the count and
                    // shove the controls after it along the row. Singular
                    // "Selection" for 0 and 1, plural otherwise.
                    theme::colored_badge_sized(
                        ui,
                        &format!(
                            "{sel_count} Selection{}",
                            if sel_count > 1 { "s" } else { "" }
                        ),
                        theme::ACCENT_BLUE,
                        120.0,
                    );
                    ui.add_space(16.0);

                    // ── Phase C: dirty tracker controls ──
                    ui.separator();
                    ui.add_space(8.0);
                    let dirty_available = dirty.is_some();

                    // "Auto-preselect modified" toggle.
                    let auto_resp = ui.add_enabled(
                        dirty_available,
                        egui::Checkbox::new(
                            &mut state.auto_preselect_modified,
                            "Auto-preselect modified",
                        ),
                    );
                    if auto_resp.changed()
                        && state.auto_preselect_modified
                        && let Some(d) = dirty
                    {
                        state.apply_dirty_set(d.dirty_set());
                        state.last_dirty_generation = d.generation();
                    }

                    // "Select modified" one-shot button (hidden when auto is on,
                    // following WFS-DIY behaviour — the auto toggle subsumes it).
                    if !state.auto_preselect_modified {
                        let select_btn = theme::action_button(
                            "Select modified",
                            theme::ACCENT_BLUE,
                            egui::Vec2::new(130.0, 28.0),
                        );
                        if ui
                            .add_enabled(dirty_available && dirty_has_any, select_btn)
                            .clicked()
                            && let Some(d) = dirty
                        {
                            state.apply_dirty_set(d.dirty_set());
                            state.last_dirty_generation = d.generation();
                        }
                    }

                    // "Clear changes" — wipe the dirty set without sending
                    // anything. The actual clear happens in the caller after
                    // this frame returns (we only have a borrow of the tracker).
                    let clear_changes_btn = theme::action_button(
                        "Clear changes",
                        theme::btn_neutral(),
                        egui::Vec2::new(120.0, 28.0),
                    );
                    if ui
                        .add_enabled(dirty_available && dirty_has_any, clear_changes_btn)
                        .on_hover_text(help(HelpKey::ScopeClearChanges))
                        .on_disabled_hover_text(help(HelpKey::ScopeClearChangesDisabled))
                        .clicked()
                    {
                        outcome.clear_dirty_requested = true;
                        state.last_dirty_generation = 0;
                    }

                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("Click any header to bulk toggle.")
                            .color(theme::label_weak())
                            .size(theme::FONT_SIZE_BADGE),
                    );
                } else {
                    // ── Timing entry panel (Pre-wait / Fade modes) ──
                    // Replaces the scope-only "modified" controls so the strip
                    // stays one row tall and the table keeps its full height.
                    let sel = state.timing_selection_count();
                    theme::colored_badge_sized(
                        ui,
                        &format!("{sel} cell{}", if sel == 1 { "" } else { "s" }),
                        theme::ACCENT_BLUE,
                        80.0,
                    );
                    ui.add_space(8.0);

                    if sel == 0 {
                        ui.label(
                            egui::RichText::new(
                                "Click a timing cell to select; Shift-click for a rectangle.",
                            )
                            .color(theme::label_weak())
                            .size(theme::FONT_SIZE_BADGE),
                        );
                    } else {
                        let unit = match state.edit_mode {
                            ScopeEditMode::PreWait => "pre-wait (s):",
                            ScopeEditMode::Fade => "fade (s):",
                            ScopeEditMode::Scope => "",
                        };
                        theme::row_label(ui, unit, theme::label_color());

                        let resp = theme::padded_text_edit_sized(
                            ui,
                            &mut state.timing_value_draft,
                            70.0,
                            theme::ROW_H,
                            true,
                            "0.0",
                        )
                        .on_hover_text(
                            "Type seconds to set all selected cells. \
                             Or nudge the selection: Up/Down ±0.1 s, \
                             Alt+Up/Down ±1 s, or drag a selected cell.",
                        );

                        let enter =
                            resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let esc_in_box =
                            resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));

                        if esc_in_box {
                            // Cancel the edit: revert the box, keep the selection.
                            state.timing_value_draft = fmt_first_timing(state, &timing_order);
                            resp.surrender_focus();
                        } else if enter {
                            apply_timing_draft(state);
                        } else if !resp.has_focus() && !resp.changed() {
                            // Keep the box showing the first selected cell's
                            // value as the selection changes, without clobbering
                            // what the operator is currently typing.
                            state.timing_value_draft = fmt_first_timing(state, &timing_order);
                        }

                        ui.add_space(8.0);
                        if theme::row_action_button(
                            ui,
                            "OK",
                            theme::ACCENT_GREEN,
                            56.0,
                            true,
                            "Apply the value to every selected cell",
                        ) {
                            apply_timing_draft(state);
                        }
                        if theme::row_action_button(
                            ui,
                            "Cancel",
                            theme::btn_neutral(),
                            70.0,
                            true,
                            "Revert the entered value",
                        ) {
                            state.timing_value_draft = fmt_first_timing(state, &timing_order);
                        }
                        if theme::row_action_button(
                            ui,
                            "Clear selection",
                            theme::ACCENT_RED,
                            120.0,
                            true,
                            "Deselect all cells",
                        ) {
                            state.clear_timing_selection();
                        }
                    }
                }
            });

            // Esc with a timing selection active but no text field focused
            // clears the selection (Esc inside the numeric box cancels the edit
            // instead — handled above via `esc_in_box`).
            if state.edit_mode != ScopeEditMode::Scope
                && !state.timing_selection.is_empty()
                && ui.ctx().memory(|m| m.focused().is_none())
                && ui.input(|i| i.key_pressed(egui::Key::Escape))
            {
                state.clear_timing_selection();
            }

            // Arrow keys nudge the whole selection by a relative amount:
            // Up/Down = ±0.1 s, Alt+Up/Down = ±1.0 s. Only when a selection is
            // active and no text field has focus (so typing in the numeric box
            // is unaffected). `consume_key` stops the keys from also scrolling
            // the group list below.
            if state.edit_mode != ScopeEditMode::Scope
                && !state.timing_selection.is_empty()
                && ui.ctx().memory(|m| m.focused().is_none())
            {
                let delta = ui.input_mut(|i| {
                    let mut d = 0.0_f32;
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                        d += 0.1;
                    }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                        d -= 0.1;
                    }
                    if i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowUp) {
                        d += 1.0;
                    }
                    if i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowDown) {
                        d -= 1.0;
                    }
                    d
                });
                if delta != 0.0 {
                    state.nudge_timing_selection(state.edit_mode, delta);
                    state.timing_value_draft = fmt_first_timing(state, &timing_order);
                }
            }

            // ── Templates row ─────────────────────────────────────────
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                theme::row_label(ui, "Templates:", theme::label_color());
                let current_name = state
                    .selected_template_id
                    .and_then(|id| templates.iter().find(|(tid, _)| *tid == id))
                    .map(|(_, n)| n.clone())
                    .unwrap_or_else(|| "(select)".into());
                theme::row_combo(ui, 0, |ui| {
                    let combo = egui::ComboBox::from_id_salt("scope_editor_templates_combo")
                        .selected_text(&current_name)
                        .width(180.0)
                        .height(320.0)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(state.selected_template_id.is_none(), "(select)")
                                .clicked()
                            {
                                state.selected_template_id = None;
                            }
                            for (id, name) in &templates {
                                if ui
                                    .selectable_label(state.selected_template_id == Some(*id), name)
                                    .clicked()
                                {
                                    state.selected_template_id = Some(*id);
                                }
                            }
                        });
                    combo
                        .response
                        .on_hover_text(help(HelpKey::ScopeTemplateCombo));
                });

                if theme::row_action_button(
                    ui,
                    "Load",
                    theme::ACCENT_BLUE,
                    70.0,
                    selected_template_full.is_some(),
                    help(HelpKey::ScopeTemplateLoad),
                ) {
                    if let Some(tmpl) = selected_template_full.as_ref() {
                        state.load_template(tmpl, aux_count, group_count, matrix_count);
                    }
                }

                ui.separator();
                ui.add_space(4.0);

                theme::row_label(ui, "Save as:", theme::label_color());
                theme::padded_text_edit_sized(
                    ui,
                    &mut state.template_name_buf,
                    140.0,
                    theme::ROW_H,
                    true,
                    "",
                );
                let can_save = !state.template_name_buf.trim().is_empty();
                if theme::row_action_button(
                    ui,
                    "Save",
                    theme::ACCENT_GREEN,
                    70.0,
                    can_save,
                    help(HelpKey::ScopeTemplateSave),
                ) {
                    let name = state.template_name_buf.trim().to_string();
                    let new_tmpl = state.to_scope_template(name);
                    state.template_name_buf.clear();
                    // Adopt the new template as the selection so a follow-up
                    // Update/Delete targets it without a second click.
                    state.selected_template_id = Some(new_tmpl.id);
                    let cue_mgr = cue_manager.clone();
                    runtime.spawn(async move {
                        cue_mgr.write().await.add_scope_template(new_tmpl);
                    });
                }

                ui.separator();
                ui.add_space(4.0);

                if theme::row_long_press_button(
                    ui,
                    "Update",
                    theme::ACCENT_AMBER,
                    80.0,
                    state.selected_template_id.is_some(),
                ) {
                    if let Some(id) = state.selected_template_id {
                        let existing_name = templates
                            .iter()
                            .find(|(tid, _)| *tid == id)
                            .map(|(_, n)| n.clone())
                            .unwrap_or_else(|| "Untitled".into());
                        let replacement = state.to_scope_template(existing_name);
                        let cue_mgr = cue_manager.clone();
                        runtime.spawn(async move {
                            cue_mgr.write().await.update_scope_template(id, replacement);
                        });
                    }
                }

                if theme::row_long_press_button(
                    ui,
                    "Delete",
                    theme::ACCENT_RED,
                    80.0,
                    state.selected_template_id.is_some(),
                ) {
                    if let Some(id) = state.selected_template_id {
                        let cue_mgr = cue_manager.clone();
                        runtime.spawn(async move {
                            cue_mgr.write().await.remove_scope_template(id);
                        });
                        state.selected_template_id = None;
                    }
                }
            });

            ui.add_space(6.0);
            ui.separator();

            // ─ Per-group matrices (scroll area leaves room for footer) ─
            let footer_height = 60.0; // footer row + separator + margins
            let available = ui.available_height() - footer_height;
            // Vertical-only: each group renders at its full content height and
            // the groups stack, so this outer area scrolls the whole stack.
            // (Horizontal scrolling for channels lives inside each group's body,
            // with the parameter-name column frozen.)
            let mut group_rects: Vec<(ChannelGroup, egui::Rect)> = Vec::new();
            let scroll_out = egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(available.max(100.0))
                .show(ui, |ui| {
                    for data in &groups_data {
                        if data.channels.is_empty() {
                            continue;
                        }
                        if let Some(rect) = draw_channel_group_block(ui, state, data) {
                            group_rects.push((data.group, rect));
                        }
                        ui.add_space(8.0);
                    }
                });

            // Sticky channel-number header: when an expanded group is scrolled so
            // its own header row sits above the viewport, re-pin a copy at the
            // top of the scroll viewport so the channel numbers stay visible
            // while paging through that group's parameters.
            let viewport = scroll_out.inner_rect;
            let vtop = viewport.top();
            let active = group_rects
                .iter()
                .find(|(_, r)| r.top() < vtop && r.bottom() > vtop + CHANNEL_HEADER_HEIGHT + 8.0)
                .copied();
            if let Some((group, rect)) = active
                && let Some(data) = groups_data.iter().find(|d| d.group == group)
            {
                let h_offset = state
                    .matrix_scroll_offset
                    .get(&group)
                    .map(|(x, _)| *x)
                    .unwrap_or(0.0);
                let pos = egui::pos2(rect.left() + MATRIX_INNER_MARGIN, vtop);
                egui::Area::new(egui::Id::new("scope_sticky_header"))
                    .order(egui::Order::Foreground)
                    .movable(false)
                    .fixed_pos(pos)
                    .constrain_to(viewport)
                    .show(ctx, |ui| {
                        ui.set_clip_rect(viewport);
                        // Match the body's channel-strip width so the same
                        // columns are visible at the same offset.
                        ui.set_max_width((viewport.right() - pos.x).max(0.0));
                        // Opaque backing so scrolled rows don't bleed through.
                        let bg = egui::Rect::from_min_size(
                            egui::pos2(rect.left(), vtop),
                            egui::vec2(rect.width(), CHANNEL_HEADER_HEIGHT),
                        );
                        ui.painter().rect_filled(bg, 0.0, theme::bg_panel());
                        draw_channel_header_row(ui, state, data, h_offset, true);
                    });
            }

            // ─ Footer: Console Recall + OK/Cancel ─
            ui.separator();
            ui.horizontal(|ui| {
                // Console Recall buttons (left side). The block layout mirrors
                // the S-series console's own recall-scope screen, so the whole
                // group is hidden on families that don't have it.
                if console_state
                    .config
                    .profile()
                    .supports(crate::model::family::AppFeature::RecallScopeUi)
                {
                    ui.label(
                        // Match the buttons' tiny font (not the slightly-smaller
                        // `.small()`), so the label baseline sits with the button
                        // text to its right rather than reading high.
                        egui::RichText::new("Console Recall:")
                            .color(theme::label_weak())
                            .size(theme::FONT_SIZE_TINY),
                    );
                    let btn_size = egui::Vec2::new(100.0, 24.0);
                    let scope_btn = egui::Button::new(
                        egui::RichText::new("Session Scope")
                            .size(theme::FONT_SIZE_TINY)
                            .color(
                                if state.console_recall.session_scope.active_blocks.is_empty() {
                                    theme::TEXT_SECONDARY
                                } else {
                                    egui::Color32::from_rgb(0, 180, 0)
                                },
                            ),
                    )
                    .fill(theme::btn_neutral())
                    .min_size(btn_size);
                    if ui.add(scope_btn).clicked() {
                        state.recall_popup.open = Some(RecallPopupKind::SessionScope);
                    }
                    for (label, kind) in [
                        ("Input Safe", RecallPopupKind::InputSafe),
                        ("Aux Safe", RecallPopupKind::AuxSafe),
                        ("Group Safe", RecallPopupKind::GroupSafe),
                        ("Matrix Safe", RecallPopupKind::MatrixSafe),
                        ("CG Safe", RecallPopupKind::CgSafe),
                    ] {
                        let btn = egui::Button::new(
                            egui::RichText::new(label).size(theme::FONT_SIZE_TINY),
                        )
                        .fill(theme::btn_neutral())
                        .min_size(egui::Vec2::new(72.0, 24.0));
                        if ui.add(btn).clicked() {
                            state.recall_popup.open = Some(kind);
                            state.recall_popup.selected_channel = 1;
                        }
                    }
                }

                // OK / Cancel (right side). OK commits the WORKING scope used
                // by the next capture — it never writes to a snapshot. The
                // Apply-to-snapshot action lives in its own group below so the
                // distinction is unmistakable.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok_btn = theme::action_button(
                        "OK",
                        theme::ACCENT_GREEN,
                        egui::Vec2::new(90.0, 28.0),
                    );
                    if ui
                        .add(ok_btn)
                        .on_hover_text(help(HelpKey::ScopeEditorOk))
                        .clicked()
                    {
                        state.commit();
                        outcome.status = ScopeWindowResult::Committed;
                    }
                    ui.add_space(8.0);
                    let cancel_btn = theme::action_button(
                        "Cancel",
                        theme::btn_neutral(),
                        egui::Vec2::new(90.0, 28.0),
                    );
                    if ui.add(cancel_btn).clicked() {
                        state.cancel();
                        outcome.status = ScopeWindowResult::Cancelled;
                    }
                });
            });

            // ─ Apply this (working) scope onto the selected snapshot ─
            // Scope only; captured data untouched. For editing a snapshot's
            // scope offline, with no live desk to re-capture from. The window
            // stays open. Kept in its own framed group, distinct from OK/Cancel,
            // because this is the ONLY footer action that changes a snapshot.
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| match apply_target {
                    Some(target) => {
                        ui.label(
                            egui::RichText::new("Apply scope to snapshot:")
                                .color(theme::label_weak())
                                .size(theme::FONT_SIZE_TINY),
                        );
                        ui.label(
                            egui::RichText::new(&target.name)
                                .strong()
                                .color(theme::label_color()),
                        );
                        let (kind_label, kind_color) = match target.kind {
                            SnapshotKind::ApplyOnSave => ("scope: save", theme::btn_neutral()),
                            SnapshotKind::ApplyOnRecall => ("scope: recall", theme::ACCENT_BLUE),
                        };
                        theme::colored_badge(ui, kind_label, kind_color);

                        let apply_label = format!("Apply Scope to '{}'", target.name);
                        let apply_btn = theme::action_button(
                            &apply_label,
                            theme::ACCENT_BLUE,
                            egui::Vec2::new(190.0, 28.0),
                        );
                        if ui
                            .add(apply_btn)
                            .on_hover_text(help(HelpKey::ScopeEditorApplyToSnapshot))
                            .clicked()
                        {
                            outcome.apply_to_selected_requested = true;
                        }

                        if target.override_cue_count > 0 {
                            ui.label(
                                egui::RichText::new(format!(
                                    "⚠ {} cue(s) recall this snapshot with a scope \
                                     override — this scope won't affect those recalls",
                                    target.override_cue_count
                                ))
                                .color(theme::ACCENT_AMBER)
                                .size(theme::FONT_SIZE_TINY),
                            )
                            .on_hover_text(help(HelpKey::ScopeOverrideMasksEdit));
                        }
                    }
                    None => {
                        let apply_btn = theme::action_button(
                            "Apply Scope to Snapshot",
                            theme::ACCENT_BLUE,
                            egui::Vec2::new(190.0, 28.0),
                        );
                        ui.add_enabled(false, apply_btn).on_hover_text(
                            "Select a snapshot in the Snapshots tab first to apply \
                             this scope to it",
                        );
                    }
                });
            });
        });

    // Honour the X close button (which flips `still_open` to false). Treat
    // closing-via-X as Cancel — discard pending changes.
    if !still_open && state.window_open && outcome.status == ScopeWindowResult::StillOpen {
        state.cancel();
        outcome.status = ScopeWindowResult::Cancelled;
    }

    // Render the recall scope/safe popup if open
    draw_recall_popup(
        ctx,
        &mut state.recall_popup,
        &mut state.console_recall,
        console_state,
    );

    outcome
}

/// Format the first-selected timing cell's value for the numeric box (one
/// decimal, matching the cells), or "" when the selection is empty.
fn fmt_first_timing(state: &ScopeEditorState, order: &[(ChannelId, TimingCategory)]) -> String {
    state
        .first_timing_value(state.edit_mode, order)
        .map(|v| format!("{v:.1}"))
        .unwrap_or_default()
}

/// Parse the numeric box and apply it to every selected timing cell, then
/// canonicalise the box text. A no-op on unparseable input — the operator keeps
/// their text so a typo can be fixed rather than silently lost.
fn apply_timing_draft(state: &mut ScopeEditorState) {
    if let Some(secs) = ScopeEditorState::parse_timing_secs(&state.timing_value_draft) {
        state.apply_timing_value_to_selection(state.edit_mode, secs);
        state.timing_value_draft = format!("{secs:.1}");
    }
}
