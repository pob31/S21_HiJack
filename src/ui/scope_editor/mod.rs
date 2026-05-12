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
use crate::model::parameter::ParameterPath;
use crate::model::state::ConsoleState;
use crate::ui::recall_scope_popup::{RecallPopupKind, draw_recall_popup};
use crate::ui::theme;
use channel_grid::{GroupRenderData, draw_channel_group_block};

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

    let mut outcome = ScopeWindowOutcome::default();
    let mut still_open = state.window_open;
    let dirty_has_any = dirty.map(|d| d.has_any()).unwrap_or(false);

    egui::Window::new("Snapshot Scope")
        .collapsible(false)
        .resizable(true)
        .default_size([1100.0, 700.0])
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
                    .clicked()
                {
                    state.edit_mode = ScopeEditMode::Scope;
                }
                if ui
                    .add(
                        egui::Button::new("Pre-wait")
                            .selected(state.edit_mode == ScopeEditMode::PreWait)
                            .min_size(mode_btn_size),
                    )
                    .clicked()
                {
                    state.edit_mode = ScopeEditMode::PreWait;
                }
                if ui
                    .add(
                        egui::Button::new("Fade")
                            .selected(state.edit_mode == ScopeEditMode::Fade)
                            .min_size(mode_btn_size),
                    )
                    .clicked()
                {
                    state.edit_mode = ScopeEditMode::Fade;
                }
                ui.separator();
                ui.add_space(8.0);

                let clear_btn = theme::action_button(
                    "Clear All",
                    theme::BG_ELEVATED,
                    egui::Vec2::new(80.0, 28.0),
                );
                if ui.add(clear_btn).clicked() {
                    state.clear();
                }
                ui.add_space(8.0);
                theme::colored_badge(
                    ui,
                    &format!("{} selections", state.selection_count()),
                    theme::ACCENT_BLUE,
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
                    theme::BG_ELEVATED,
                    egui::Vec2::new(120.0, 28.0),
                );
                if ui
                    .add_enabled(dirty_available && dirty_has_any, clear_changes_btn)
                    .clicked()
                {
                    outcome.clear_dirty_requested = true;
                    state.last_dirty_generation = 0;
                }

                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Click any header to bulk toggle.")
                        .color(theme::TEXT_SECONDARY)
                        .size(theme::FONT_SIZE_BADGE),
                );
            });

            // ── Templates row ─────────────────────────────────────────
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Templates:");
                let current_name = state
                    .selected_template_id
                    .and_then(|id| templates.iter().find(|(tid, _)| *tid == id))
                    .map(|(_, n)| n.clone())
                    .unwrap_or_else(|| "(select)".into());
                egui::ComboBox::from_id_salt("scope_editor_templates_combo")
                    .selected_text(&current_name)
                    .width(180.0)
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

                let load_btn = theme::action_button(
                    "Load",
                    theme::ACCENT_BLUE,
                    egui::Vec2::new(70.0, 28.0),
                );
                if ui
                    .add_enabled(selected_template_full.is_some(), load_btn)
                    .clicked()
                {
                    if let Some(tmpl) = selected_template_full.as_ref() {
                        state.load_template(tmpl, aux_count, group_count, matrix_count);
                    }
                }

                ui.separator();
                ui.add_space(4.0);

                ui.label("Save as:");
                ui.add(
                    egui::TextEdit::singleline(&mut state.template_name_buf)
                        .desired_width(140.0),
                );
                let save_btn = theme::action_button(
                    "Save",
                    theme::ACCENT_GREEN,
                    egui::Vec2::new(70.0, 28.0),
                );
                let can_save = !state.template_name_buf.trim().is_empty();
                if ui.add_enabled(can_save, save_btn).clicked() {
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

                if theme::long_press_button(
                    ui,
                    "Update",
                    theme::ACCENT_AMBER,
                    egui::Vec2::new(80.0, 28.0),
                    state.selected_template_id.is_some(),
                    theme::LONG_PRESS_DURATION_MS,
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

                if theme::long_press_button(
                    ui,
                    "Delete",
                    theme::ACCENT_RED,
                    egui::Vec2::new(80.0, 28.0),
                    state.selected_template_id.is_some(),
                    theme::LONG_PRESS_DURATION_MS,
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
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .max_height(available.max(100.0))
                .show(ui, |ui| {
                    for data in &groups_data {
                        if data.channels.is_empty() {
                            continue;
                        }
                        draw_channel_group_block(ui, state, data);
                        ui.add_space(8.0);
                    }
                });

            // ─ Footer: Console Recall + OK/Cancel ─
            ui.separator();
            ui.horizontal(|ui| {
                // Console Recall buttons (left side)
                ui.label(
                    egui::RichText::new("Console Recall:")
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
                let btn_size = egui::Vec2::new(100.0, 24.0);
                let scope_btn =
                    egui::Button::new(egui::RichText::new("Session Scope").size(10.0).color(
                        if state.console_recall.session_scope.active_blocks.is_empty() {
                            theme::TEXT_SECONDARY
                        } else {
                            egui::Color32::from_rgb(0, 180, 0)
                        },
                    ))
                    .fill(theme::BG_ELEVATED)
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
                    let btn = egui::Button::new(egui::RichText::new(label).size(10.0))
                        .fill(theme::BG_ELEVATED)
                        .min_size(egui::Vec2::new(72.0, 24.0));
                    if ui.add(btn).clicked() {
                        state.recall_popup.open = Some(kind);
                        state.recall_popup.selected_channel = 1;
                    }
                }

                // OK / Cancel (right side)
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok_btn = theme::action_button(
                        "OK",
                        theme::ACCENT_GREEN,
                        egui::Vec2::new(90.0, 28.0),
                    );
                    if ui.add(ok_btn).clicked() {
                        state.commit();
                        outcome.status = ScopeWindowResult::Committed;
                    }
                    ui.add_space(8.0);
                    let cancel_btn = theme::action_button(
                        "Cancel",
                        theme::BG_ELEVATED,
                        egui::Vec2::new(90.0, 28.0),
                    );
                    if ui.add(cancel_btn).clicked() {
                        state.cancel();
                        outcome.status = ScopeWindowResult::Cancelled;
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
