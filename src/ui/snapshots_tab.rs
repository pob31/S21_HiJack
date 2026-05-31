use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::UiEvent;
use super::help::{HelpKey, help};
use super::palettes_ui::{PalettesUiState, draw_palettes_section};
use super::scope_editor::ScopeEditorState;
use super::theme;
use crate::console::cue_manager::CueManager;
use crate::console::palette_manager::PaletteManager;
use crate::console::snapshot_engine::SnapshotEngine;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::parameter::{ParameterAddress, ParameterValue};
use crate::model::snapshot::{Cue, Snapshot, SnapshotKind};
use crate::model::state::ConsoleState;
use crate::osc::qlab_client::QLabClient;
use crate::osc::qlab_cue_builder::{build_snapshot_cues, build_snapshot_load_cue};

/// Space (px) left below the snapshot list so the status line under the card
/// (and an eventual footer) stay visible when the list fills the column.
const LIST_BOTTOM_RESERVE: f32 = 40.0;
/// Minimum snapshot-list height on short windows; the outer column ScrollArea
/// takes over scrolling below this.
const LIST_MIN_HEIGHT: f32 = 120.0;

/// State for the Snapshots tab.
pub struct SnapshotsTabState {
    // Cue management
    pub selected_cue_id: Option<Uuid>,
    pub new_cue_number: String,
    pub new_cue_name: String,
    pub selected_snapshot_for_cue: Option<Uuid>,
    /// Type-to-filter text for the Add Cue snapshot picker dropdown.
    pub add_cue_snap_filter: String,
    /// Type-to-filter text for the Cue Editor snapshot picker dropdown.
    pub cue_editor_snap_filter: String,

    // Cue editor
    pub last_edited_cue_id: Option<Uuid>,
    pub editing_cue_number: String,
    pub editing_local_snapshot: Option<Uuid>,
    pub editing_console_snapshot: String,
    pub editing_scope_override_enabled: bool,
    pub editing_scope_template_id: Option<Uuid>,
    pub editing_cue_notes: String,

    // Snapshot management
    pub new_snapshot_name: String,
    pub selected_snapshot_id: Option<Uuid>,
    /// Snapshot kind picked at capture time. Defaults to ApplyOnSave so the
    /// behaviour matches v7.
    pub pending_kind: SnapshotKind,

    // Scope
    pub scope_editor: ScopeEditorState,
    pub selected_scope_template_id: Option<Uuid>,

    // Feedback
    pub status_message: Option<String>,

    // Console row for the Add Cue form (empty = no row link).
    pub new_cue_console_row: String,

    // Shift console refs modal state.
    pub shift_modal_open: bool,
    pub shift_from_row: String,
    pub shift_delta: String,
    pub shift_status: Option<String>,
}

impl Default for SnapshotsTabState {
    fn default() -> Self {
        Self {
            selected_cue_id: None,
            new_cue_number: String::new(),
            new_cue_name: String::new(),
            selected_snapshot_for_cue: None,
            add_cue_snap_filter: String::new(),
            cue_editor_snap_filter: String::new(),
            last_edited_cue_id: None,
            editing_cue_number: String::new(),
            editing_local_snapshot: None,
            editing_console_snapshot: String::new(),
            editing_scope_override_enabled: false,
            editing_scope_template_id: None,
            editing_cue_notes: String::new(),
            new_snapshot_name: String::new(),
            selected_snapshot_id: None,
            pending_kind: SnapshotKind::default(),
            scope_editor: ScopeEditorState::default(),
            selected_scope_template_id: None,
            status_message: None,
            new_cue_console_row: String::new(),
            shift_modal_open: false,
            shift_from_row: "1".into(),
            shift_delta: "1".into(),
            shift_status: None,
        }
    }
}

/// Draw the Snapshots tab.
#[allow(clippy::too_many_arguments)]
pub fn draw_snapshots_tab(
    ui: &mut egui::Ui,
    snap_state: &mut SnapshotsTabState,
    palettes_ui: &mut PalettesUiState,
    console_state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    snapshot_engine: &Option<Arc<SnapshotEngine>>,
    dirty_tracker: &Arc<RwLock<DirtyTracker>>,
    auto_update_on_recall: &Arc<AtomicBool>,
    console_snapshot_follow: &Arc<AtomicBool>,
    operating_mode: crate::model::operating_mode::OperatingMode,
    qlab_ip: &str,
    qlab_port: u16,
    connected: &Arc<AtomicBool>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let is_connected = connected.load(Ordering::Relaxed);

    // Read channel counts for scope template loading + migration.
    let (aux_count, group_count, matrix_count) = if let Ok(st) = console_state.try_read() {
        (
            st.config.aux_output_count,
            st.config.group_output_count,
            st.config.matrix_input_count,
        )
    } else {
        (8, 8, 10) // defaults
    };

    // Read current cue ID for highlighting
    let current_cue_id = cue_manager
        .try_read()
        .ok()
        .and_then(|mgr| mgr.current_cue().map(|c| c.id));

    // Three-column layout. Each column scrolls independently; lists inside
    // cards have their own bounded ScrollArea so trailing buttons never get
    // pushed off-screen by a long list.
    ui.columns(3, |cols| {
        // ── Column 1: Scope + Palettes ──
        egui::ScrollArea::vertical()
            .id_salt("snapshot_col1_scroll")
            .auto_shrink([false, false])
            .show(&mut cols[0], |ui| {
                theme::card_frame().show(ui, |ui| {
                    theme::section_heading(ui, "Scope");
                    let count = snap_state.scope_editor.selection_count();
                    let channel_count = snap_state.scope_editor.channel_paths.len();
                    ui.label(
                        egui::RichText::new(format!(
                            "{count} parameter{} selected across {channel_count} channel{}",
                            if count == 1 { "" } else { "s" },
                            if channel_count == 1 { "" } else { "s" },
                        ))
                        .color(theme::label_weak()),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let edit_btn = theme::action_button(
                            "Edit Scope…",
                            theme::ACCENT_BLUE,
                            egui::Vec2::new(120.0, 30.0),
                        );
                        if ui
                            .add(edit_btn)
                            .on_hover_text(help(HelpKey::SnapshotEditScope))
                            .clicked()
                        {
                            let template = snap_state
                                .scope_editor
                                .to_scope_template("Editing".into());
                            snap_state.scope_editor.open(
                                &template,
                                aux_count,
                                group_count,
                                matrix_count,
                            );
                        }
                        ui.add_space(8.0);
                        let clear_btn = theme::action_button(
                            "Clear",
                            theme::btn_neutral(),
                            egui::Vec2::new(70.0, 30.0),
                        );
                        if ui
                            .add(clear_btn)
                            .on_hover_text(help(HelpKey::SnapshotClearScope))
                            .clicked()
                        {
                            snap_state.scope_editor.clear();
                        }
                    });
                });

                ui.add_space(8.0);

                // Palettes section
                theme::card_frame().show(ui, |ui| {
                    draw_palettes_section(
                        ui,
                        palettes_ui,
                        console_state,
                        cue_manager,
                        palette_manager,
                        is_connected,
                        runtime,
                        ui_tx,
                    );
                });
            });

        // ── Column 2: Snapshots ──
        egui::ScrollArea::vertical()
            .id_salt("snapshot_col2_scroll")
            .auto_shrink([false, false])
            .show(&mut cols[1], |ui| {
                    // ── Snapshots card ──
                    theme::card_frame().show(ui, |ui| {
                        theme::section_heading(ui, "Snapshots");

                        // Workflow toggles + console-snapshot tools.
                        let uses_ipad = operating_mode.uses_ipad_protocol();
                        ui.horizontal(|ui| {
                            theme::row_spacer(ui);
                            let mut auto = auto_update_on_recall.load(Ordering::Relaxed);
                            if ui.checkbox(&mut auto, "Auto-save previous on recall")
                                .on_hover_text(help(HelpKey::SnapshotAutoUpdate))
                                .changed()
                            {
                                auto_update_on_recall.store(auto, Ordering::Relaxed);
                            }
                        });
                        ui.horizontal(|ui| {
                            theme::row_spacer(ui);
                            let mut follow = console_snapshot_follow.load(Ordering::Relaxed);
                            let resp = ui.add_enabled(
                                uses_ipad,
                                egui::Checkbox::new(&mut follow, "Follow desk recalls"),
                            );
                            if resp.clicked() && uses_ipad {
                                console_snapshot_follow.store(follow, Ordering::Relaxed);
                            }
                            if !uses_ipad {
                                let _ = resp.on_hover_text(help(HelpKey::SnapshotConsoleFollowReq));
                            }
                        });
                        ui.add_space(4.0);

                        // Snapshot kind picker — controls whether the scope is
                        // applied at SAVE time (only in-scope params stored,
                        // current behaviour) or at RECALL time (all params
                        // captured; scope filters at recall, with the option
                        // to "Recall without scope" to restore the entire
                        // saved state in one shot).
                        ui.horizontal(|ui| {
                            theme::row_label(ui, "Apply scope:", theme::label_color());
                            ui.radio_value(
                                &mut snap_state.pending_kind,
                                SnapshotKind::ApplyOnSave,
                                "On save",
                            );
                            ui.radio_value(
                                &mut snap_state.pending_kind,
                                SnapshotKind::ApplyOnRecall,
                                "On recall",
                            );
                        });

                        // Capture controls. ApplyOnRecall doesn't need a
                        // populated scope (the whole state is captured) so
                        // the selection-count gate only applies to ApplyOnSave.
                        ui.horizontal(|ui| {
                            theme::row_label(ui, "Name:", theme::label_color());
                            theme::padded_text_edit_sized(
                                ui,
                                &mut snap_state.new_snapshot_name,
                                220.0,
                                theme::ROW_H,
                                true,
                                "",
                            );

                            let scope_required = matches!(
                                snap_state.pending_kind,
                                SnapshotKind::ApplyOnSave
                            );
                            let scope_ok = !scope_required
                                || snap_state.scope_editor.selection_count() > 0;
                            let can_capture = is_connected
                                && !snap_state.new_snapshot_name.is_empty()
                                && scope_ok;

                            if theme::row_action_button(
                                ui,
                                "Capture Now",
                                theme::ACCENT_GREEN,
                                110.0,
                                can_capture,
                                help(HelpKey::SnapshotCaptureNow),
                            ) {
                                capture_snapshot(
                                    snap_state,
                                    console_state,
                                    cue_manager,
                                    dirty_tracker,
                                    runtime,
                                    ui_tx,
                                );
                            }
                        });

                        // Recall / Re-capture / Delete buttons.
                        // The "Recall (no scope)" button is only meaningful
                        // for ApplyOnRecall snapshots — ApplyOnSave snapshots
                        // already filtered at capture time, so the stored
                        // data IS the scope and there's nothing extra to
                        // recall. The button is greyed out (with a tooltip)
                        // when the selected snapshot is ApplyOnSave.
                        let selected_kind: Option<SnapshotKind> = snap_state
                            .selected_snapshot_id
                            .and_then(|id| {
                                cue_manager
                                    .try_read()
                                    .ok()
                                    .and_then(|mgr| mgr.snapshots.get(&id).map(|s| s.kind))
                            });
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let has_selection = snap_state.selected_snapshot_id.is_some();
                            let engine_ready = snapshot_engine.is_some() && is_connected;

                            if theme::row_action_button(
                                ui,
                                "Recall",
                                theme::ACCENT_GREEN,
                                70.0,
                                has_selection && engine_ready,
                                help(HelpKey::SnapshotRecall),
                            ) {
                                recall_selected_snapshot(
                                    snap_state,
                                    cue_manager,
                                    palette_manager,
                                    snapshot_engine,
                                    runtime,
                                    ui_tx,
                                    /* ignore_scope */ false,
                                );
                            }

                            let can_recall_no_scope = matches!(selected_kind, Some(SnapshotKind::ApplyOnRecall));
                            // Kept as an explicit scoped button (not row_action_button)
                            // because we need the Response for the tooltip below.
                            let recall_no_scope_resp = ui
                                .scope(|ui| {
                                    ui.spacing_mut().button_padding = egui::Vec2::new(12.0, 4.0);
                                    ui.add_enabled(
                                        has_selection && engine_ready && can_recall_no_scope,
                                        theme::action_button(
                                            "Recall full",
                                            theme::ACCENT_AMBER,
                                            egui::Vec2::new(90.0, theme::ROW_H),
                                        ),
                                    )
                                })
                                .inner;
                            if !can_recall_no_scope && has_selection {
                                let _ = recall_no_scope_resp
                                    .clone()
                                    .on_hover_text(help(HelpKey::SnapshotRecallNoScopeReq));
                            }
                            if recall_no_scope_resp.clicked() {
                                recall_selected_snapshot(
                                    snap_state,
                                    cue_manager,
                                    palette_manager,
                                    snapshot_engine,
                                    runtime,
                                    ui_tx,
                                    /* ignore_scope */ true,
                                );
                            }

                            if theme::row_action_button(
                                ui,
                                "Re-capture",
                                theme::ACCENT_BLUE,
                                85.0,
                                has_selection && is_connected,
                                help(HelpKey::SnapshotRecapture),
                            ) {
                                recapture_snapshot(snap_state, console_state, cue_manager, dirty_tracker, runtime, ui_tx);
                            }
                            if theme::row_long_press_button(
                                ui,
                                "Delete",
                                theme::ACCENT_RED,
                                70.0,
                                has_selection,
                            ) {
                                if let Some(id) = snap_state.selected_snapshot_id {
                                    let cue_mgr = cue_manager.clone();
                                    let pmgr = palette_manager.clone();
                                    runtime.spawn(async move {
                                        cue_mgr.write().await.remove_snapshot(id);
                                        // Drop back-references from every palette so the
                                        // "Linked Snapshots" UI count stays accurate. Recall
                                        // doesn't depend on this list (the forward direction
                                        // is on the snapshot itself, which is now gone), but
                                        // the palette detail pane reads it for display.
                                        pmgr.write().await.unlink_all_from_snapshot(id);
                                    });
                                    snap_state.selected_snapshot_id = None;
                                    snap_state.status_message = Some("Snapshot deleted".into());
                                }
                            }
                        });

                        // ── Undo + pacing ──
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            let engine_ready = snapshot_engine.is_some() && is_connected;
                            let has_undo = snapshot_engine.as_ref().map(|e| e.has_undo()).unwrap_or(false);
                            let undo_label = snapshot_engine
                                .as_ref()
                                .and_then(|e| e.undo_label())
                                .unwrap_or_else(|| "Undo".to_string());

                            if theme::row_action_button(
                                ui,
                                &undo_label,
                                theme::ACCENT_AMBER,
                                160.0,
                                has_undo && engine_ready,
                                help(HelpKey::SnapshotUndo),
                            ) {
                                if let Some(engine) = snapshot_engine.clone() {
                                    let tx = ui_tx.clone();
                                    runtime.spawn(async move {
                                        if let Some(result) = engine.undo_recall().await {
                                            let _ = tx.send(UiEvent::SnapshotCaptured {
                                                name: format!("Undo complete ({} params sent)", result.parameters_sent),
                                                param_count: result.parameters_sent,
                                            });
                                        }
                                    });
                                    snap_state.status_message = Some("Undoing...".into());
                                }
                            }

                            ui.add_space(16.0);
                            // Pacing now lives in Setup → Advanced…
                            // (single shared setting that also paces
                            // macro OSC sends). Surface the live value
                            // here so operators who used to tune it
                            // from this tab can still see it.
                            let pace = snapshot_engine
                                .as_ref()
                                .map(|e| e.pace_us())
                                .unwrap_or(0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "Pacing: {pace} μs (Setup → Advanced…)"
                                ))
                                .small()
                                .color(theme::label_weak()),
                            );
                        });

                        // ── Phase D: QLab export buttons ──
                        // Two flavours mirroring WFS-DIY's snapshot exports:
                        //
                        // - "Create Trigger Cue in QLab" — single network cue
                        //   whose customString is /snapshot/recall <name>.
                        //   Best for large snapshots: QLab fires one cue, the
                        //   daemon does the heavy lifting via Phase E listener.
                        //
                        // - "Export to QLab" — one network cue per stored
                        //   parameter, all in a group. QLab holds the data
                        //   and fires each cue directly to the console. Best
                        //   for small snapshots where the operator wants the
                        //   cues visible inside QLab itself.
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            let has_selection = snap_state.selected_snapshot_id.is_some();
                            let qlab_ip_owned = qlab_ip.to_string();
                            let qlab_port_local = qlab_port;

                            // Kept as explicit scoped buttons (not row_action_button)
                            // because we need each Response for its tooltip.
                            let trigger_resp = ui
                                .scope(|ui| {
                                    ui.spacing_mut().button_padding = egui::Vec2::new(12.0, 4.0);
                                    ui.add_enabled(
                                        has_selection,
                                        theme::action_button(
                                            "QLab trigger cue",
                                            theme::ACCENT_BLUE,
                                            egui::Vec2::new(140.0, theme::ROW_H),
                                        ),
                                    )
                                })
                                .inner
                                .on_hover_text(format!(
                                    "Create a single network cue in QLab whose customString \
                                     fires `/snapshot/recall <name>`. Sent to {qlab_ip}:{qlab_port}."
                                ));
                            if trigger_resp.clicked() {
                                qlab_create_trigger_cue(
                                    snap_state,
                                    cue_manager,
                                    qlab_ip_owned.clone(),
                                    qlab_port_local,
                                    runtime,
                                    ui_tx,
                                );
                            }

                            let export_resp = ui
                                .scope(|ui| {
                                    ui.spacing_mut().button_padding = egui::Vec2::new(12.0, 4.0);
                                    ui.add_enabled(
                                        has_selection,
                                        theme::action_button(
                                            "QLab export",
                                            theme::ACCENT_BLUE,
                                            egui::Vec2::new(110.0, theme::ROW_H),
                                        ),
                                    )
                                })
                                .inner
                                .on_hover_text(format!(
                                    "Export one network cue per parameter to QLab (grouped). \
                                     Sent to {qlab_ip}:{qlab_port}."
                                ));
                            if export_resp.clicked() {
                                qlab_export_full_snapshot(
                                    snap_state,
                                    cue_manager,
                                    palette_manager,
                                    qlab_ip_owned,
                                    qlab_port_local,
                                    runtime,
                                    ui_tx,
                                );
                            }
                        });

                        // Scope hint — rendered just above the list (the last
                        // thing in the card) so when it appears it only pushes
                        // the list down, never the capture / recall controls.
                        // (The disconnected hint itself is a bottom-anchored
                        // banner in `app.rs`, same pattern as the Gangs tab.)
                        if is_connected
                            && matches!(snap_state.pending_kind, SnapshotKind::ApplyOnSave)
                            && snap_state.scope_editor.selection_count() == 0
                        {
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("Select scope parameters to capture (or switch to 'On recall').").color(theme::label_weak()));
                        }

                        ui.add_space(8.0);

                        // Snapshot list — kept at the bottom of the card so
                        // adding or removing snapshots never shifts the
                        // capture / recall controls above it.
                        // The list is the last item in the card; fill the
                        // remaining column height, reserving room for the
                        // status line below the card / a future footer.
                        let list_height = (ui.available_height() - LIST_BOTTOM_RESERVE)
                            .max(LIST_MIN_HEIGHT);
                        egui::ScrollArea::vertical()
                            .id_salt("snapshot_list_scroll")
                            .max_height(list_height)
                            .show(ui, |ui| {
                                if let Ok(mgr) = cue_manager.try_read() {
                                    let mut snapshots: Vec<_> = mgr.snapshots.values().collect();
                                    snapshots.sort_by_key(|s| std::cmp::Reverse(s.modified_at));

                                    for snap in snapshots {
                                        let selected = snap_state.selected_snapshot_id == Some(snap.id);
                                        let bg = if selected { theme::bg_elevated() } else { theme::bg_panel() };

                                        let kind_label = match snap.kind {
                                            SnapshotKind::ApplyOnSave => "scope: save",
                                            SnapshotKind::ApplyOnRecall => "scope: recall",
                                        };
                                        let kind_color = match snap.kind {
                                            SnapshotKind::ApplyOnSave => theme::btn_neutral(),
                                            SnapshotKind::ApplyOnRecall => theme::ACCENT_BLUE,
                                        };
                                        let meta = format!(
                                            "{} params  {}",
                                            snap.data.parameter_count(),
                                            snap.modified_at.format("%H:%M:%S"),
                                        );
                                        // Whole row in one fixed-height band:
                                        // meta + badge pinned right, name fills
                                        // the rest (truncated, so a long name
                                        // can't wrap and break the baseline).
                                        let row_resp = egui::Frame::new()
                                            .fill(bg)
                                            .stroke(if selected {
                                                egui::Stroke::new(1.0, theme::ACCENT_BLUE)
                                            } else {
                                                egui::Stroke::NONE
                                            })
                                            .corner_radius(4.0)
                                            .inner_margin(egui::Margin::symmetric(8, 4))
                                            .show(ui, |ui| {
                                                let w = ui.available_width();
                                                ui.allocate_ui_with_layout(
                                                    egui::vec2(w, 22.0),
                                                    egui::Layout::right_to_left(egui::Align::Center),
                                                    |ui| {
                                                        ui.spacing_mut().item_spacing.x = 8.0;
                                                        // Meta + badge pinned right; the name fills the
                                                        // remaining width on the LEFT (explicit cell so
                                                        // it doesn't hug the badge). Name and meta use
                                                        // the same body font size, centred in the band,
                                                        // so they share one baseline.
                                                        ui.label(
                                                            egui::RichText::new(meta)
                                                                .color(theme::label_weak()),
                                                        );
                                                        theme::colored_badge(ui, kind_label, kind_color);
                                                        let name_w = ui.available_width();
                                                        ui.allocate_ui_with_layout(
                                                            egui::vec2(name_w, 22.0),
                                                            egui::Layout::left_to_right(egui::Align::Center),
                                                            |ui| {
                                                                ui.add(
                                                                    egui::Label::new(
                                                                        egui::RichText::new(&snap.name)
                                                                            .strong()
                                                                            .color(theme::label_color()),
                                                                    )
                                                                    .truncate(),
                                                                );
                                                            },
                                                        );
                                                    },
                                                );
                                            })
                                            .response;

                                        if row_resp.interact(egui::Sense::click()).clicked() {
                                            // Toggle: clicking the selected row deselects it.
                                            snap_state.selected_snapshot_id =
                                                if snap_state.selected_snapshot_id == Some(snap.id) {
                                                    None
                                                } else {
                                                    Some(snap.id)
                                                };
                                        }
                                        ui.add_space(1.0);
                                    }

                                    if mgr.snapshots.is_empty() {
                                        ui.label(egui::RichText::new("No snapshots yet.").color(theme::label_weak()));
                                    }
                                }
                            });
                    });

                    // Status message
                    if let Some(msg) = &snap_state.status_message {
                        ui.add_space(4.0);
                        ui.colored_label(theme::TEXT_WARNING, msg);
                    }
            });

        // ── Column 3: Cue List + reserved Cue Editor slot ──
        egui::ScrollArea::vertical()
            .id_salt("snapshot_col3_scroll")
            .auto_shrink([false, false])
            .show(&mut cols[2], |ui| {
                // Add Cue card — cue-creation controls (was titled "Cue
                // List"; the list itself now lives in its own card at the
                // bottom of the column).
                theme::card_frame().show(ui, |ui| {
                    // A cue needs a valid number plus at least one of a
                    // console-snapshot row or a local snapshot. The Add Cue
                    // button only appears once those are satisfied.
                    let cs_trim = snap_state.new_cue_console_row.trim();
                    let cs_row = if cs_trim.is_empty() {
                        None
                    } else {
                        cs_trim.parse::<i32>().ok()
                    };
                    let cs_valid = cs_trim.is_empty() || cs_row.is_some();
                    let num_ok = snap_state.new_cue_number.trim().parse::<f32>().is_ok();
                    let can_add = num_ok
                        && cs_valid
                        && (cs_row.is_some() || snap_state.selected_snapshot_for_cue.is_some());

                    // Heading row: title + Add Cue button (only when ready).
                    // row_spacer pins the row to ROW_H so the taller button
                    // appearing/disappearing doesn't shift the form below.
                    let mut add_clicked = false;
                    ui.horizontal(|ui| {
                        theme::row_spacer(ui);
                        ui.label(
                            egui::RichText::new("Add Cue")
                                .size(theme::FONT_SIZE_SECTION)
                                .strong()
                                .color(theme::label_color()),
                        );
                        if can_add {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    add_clicked = theme::row_action_button(
                                        ui,
                                        "Add Cue",
                                        theme::ACCENT_GREEN,
                                        90.0,
                                        true,
                                        help(HelpKey::CueAdd),
                                    );
                                },
                            );
                        }
                    });
                    ui.add_space(2.0);
                    let add_underline_w = ui.available_width();
                    let (add_rect, _) = ui
                        .allocate_exact_size(egui::vec2(add_underline_w, 1.0), egui::Sense::hover());
                    ui.painter().rect_filled(add_rect, 0.0, theme::border_subtle());
                    ui.add_space(6.0);

                    // Add Cue form
                    ui.horizontal(|ui| {
                        theme::row_label(ui, "Cue #:", theme::label_color());
                        theme::padded_text_edit_sized(ui, &mut snap_state.new_cue_number, 60.0, theme::ROW_H, true, "").on_hover_text(help(HelpKey::CueNumber));
                        theme::row_label(ui, "Name:", theme::label_color());
                        theme::padded_text_edit_sized(ui, &mut snap_state.new_cue_name, 130.0, theme::ROW_H, true, "").on_hover_text(help(HelpKey::CueName));
                        theme::row_label(ui, "CS:", theme::label_color());
                        theme::padded_text_edit_sized(ui, &mut snap_state.new_cue_console_row, 50.0, theme::ROW_H, true, "").on_hover_text(help(HelpKey::CueConsoleRow));
                    });
                    ui.horizontal(|ui| {
                        theme::row_label(ui, "Local snapshot:", theme::label_color());
                        if let Ok(mgr) = cue_manager.try_read() {
                            let snaps = sorted_snapshot_list(&mgr);
                            theme::row_combo(ui, 0, |ui| {
                                filtered_snapshot_combo(
                                    ui,
                                    "snapshot_selector",
                                    120.0,
                                    &mut snap_state.selected_snapshot_for_cue,
                                    &mut snap_state.add_cue_snap_filter,
                                    &snaps,
                                );
                            });
                        }
                    });

                    if add_clicked {
                        let num = snap_state.new_cue_number.trim().parse::<f32>().unwrap_or(0.0);
                        let row = cs_row;
                        let snap_id = snap_state.selected_snapshot_for_cue;
                        let name = if snap_state.new_cue_name.is_empty() {
                            format!("Cue {num}")
                        } else {
                            snap_state.new_cue_name.clone()
                        };
                        let mut cue = Cue::new(num, name);
                        if let Some(r) = row {
                            cue.console_snapshot = Some(r);
                        }
                        if let Some(id) = snap_id {
                            cue.snapshot_id = Some(id);
                        }
                        let cue_mgr = cue_manager.clone();
                        runtime.spawn(async move {
                            cue_mgr.write().await.add_cue(cue);
                        });
                        snap_state.new_cue_number.clear();
                        snap_state.new_cue_name.clear();
                        snap_state.new_cue_console_row.clear();
                        snap_state.status_message = Some(format!("Added cue {num}"));
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let shift_resp = ui
                            .scope(|ui| {
                                ui.spacing_mut().button_padding = egui::Vec2::new(12.0, 4.0);
                                ui.add(theme::action_button(
                                    "Shift console snapshots…",
                                    theme::btn_neutral(),
                                    egui::Vec2::new(190.0, theme::ROW_H),
                                ))
                            })
                            .inner
                            .on_hover_text(help(HelpKey::SnapshotBulkShift));
                        if shift_resp.clicked() {
                            snap_state.shift_modal_open = true;
                            snap_state.shift_status = None;
                        }
                    });
                });

                ui.add_space(8.0);

                // Cue Editor card — sits between the Add Cue controls and the
                // Cue List so its controls stay put while the cue list (at the
                // bottom of the column) grows and shrinks. Always rendered;
                // disabled when no cue is selected.
                theme::card_frame().show(ui, |ui| {
                    let has_selection = snap_state.selected_cue_id.is_some();
                    if let Ok(mgr) = cue_manager.try_read() {
                        // (Re)load the editing buffers when the selection changes.
                        if let Some(cue_id) = snap_state.selected_cue_id {
                            if let Some(cue) = mgr.cue_list.cues.iter().find(|c| c.id == cue_id) {
                                if snap_state.last_edited_cue_id != Some(cue_id) {
                                    snap_state.editing_cue_number = format!("{}", cue.cue_number);
                                    snap_state.editing_local_snapshot = cue.snapshot_id;
                                    snap_state.editing_console_snapshot = cue
                                        .console_snapshot
                                        .map(|n| n.to_string())
                                        .unwrap_or_default();
                                    snap_state.editing_scope_override_enabled = cue.scope_override.is_some();
                                    snap_state.editing_scope_template_id = cue.scope_override.as_ref().map(|s| s.id);
                                    snap_state.editing_cue_notes = cue.notes.clone();
                                    snap_state.last_edited_cue_id = Some(cue_id);
                                }
                            }
                        }

                        // True when the editing buffer differs from the stored
                        // cue — drives whether the heading shows a Save button.
                        let dirty = snap_state
                            .selected_cue_id
                            .and_then(|id| mgr.cue_list.cues.iter().find(|c| c.id == id))
                            .map(|cue| {
                                snap_state.editing_cue_number != format!("{}", cue.cue_number)
                                    || snap_state.editing_local_snapshot != cue.snapshot_id
                                    || snap_state.editing_console_snapshot
                                        != cue
                                            .console_snapshot
                                            .map(|n| n.to_string())
                                            .unwrap_or_default()
                                    || snap_state.editing_scope_override_enabled
                                        != cue.scope_override.is_some()
                                    || snap_state.editing_scope_template_id
                                        != cue.scope_override.as_ref().map(|s| s.id)
                                    || snap_state.editing_cue_notes != cue.notes
                            })
                            .unwrap_or(false);

                        // Heading row: title, then a right-aligned action for
                        // the selected cue — Save while there are unsaved edits,
                        // otherwise a long-press Delete. Nothing when no cue is
                        // selected.
                        let mut save_clicked = false;
                        let mut delete_clicked = false;
                        // row_spacer pins the row to ROW_H so the action
                        // button appearing/disappearing never shifts the form
                        // below. The right side carries the action for the
                        // selected cue — Save while dirty, otherwise Delete —
                        // or, with no selection, the "select a cue" hint (so it
                        // occupies the same slot instead of a separate line).
                        ui.horizontal(|ui| {
                            theme::row_spacer(ui);
                            ui.label(
                                egui::RichText::new("Cue Editor")
                                    .size(theme::FONT_SIZE_SECTION)
                                    .strong()
                                    .color(theme::label_color()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if has_selection && dirty {
                                        save_clicked = theme::row_action_button(
                                            ui,
                                            "Save Cue Changes",
                                            theme::ACCENT_GREEN,
                                            150.0,
                                            true,
                                            help(HelpKey::CueSaveChanges),
                                        );
                                    } else {
                                        // Delete is always shown but dimmed when
                                        // no cue is selected (the editor form
                                        // below is dimmed too), matching the
                                        // snapshot list's Delete.
                                        delete_clicked = theme::row_long_press_button(
                                            ui,
                                            "Delete",
                                            theme::ACCENT_RED,
                                            70.0,
                                            has_selection,
                                        );
                                    }
                                },
                            );
                        });
                        ui.add_space(2.0);
                        let underline_w = ui.available_width();
                        let (rect, _) = ui
                            .allocate_exact_size(egui::vec2(underline_w, 1.0), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 0.0, theme::border_subtle());
                        ui.add_space(6.0);

                        ui.add_enabled_ui(has_selection, |ui| {
                            // 2-column label|control form. row_label sizes each
                            // label cell to ROW_H (centred); the combo cells use
                            // row_combo so they match.
                            egui::Grid::new("cue_editor_grid")
                                .num_columns(2)
                                .spacing([12.0, 10.0])
                                .show(ui, |ui| {
                                    theme::row_label(ui, "Cue #:", theme::label_color());
                                    theme::padded_text_edit_sized(
                                        ui,
                                        &mut snap_state.editing_cue_number,
                                        70.0,
                                        theme::ROW_H,
                                        true,
                                        "",
                                    );
                                    ui.end_row();

                                    theme::row_label(ui, "Local snapshot:", theme::label_color());
                                    let snaps = sorted_snapshot_list(&mgr);
                                    theme::row_combo(ui, 0, |ui| {
                                        filtered_snapshot_combo(
                                            ui,
                                            "cue_editor_local_snapshot",
                                            140.0,
                                            &mut snap_state.editing_local_snapshot,
                                            &mut snap_state.cue_editor_snap_filter,
                                            &snaps,
                                        );
                                    });
                                    ui.end_row();

                                    theme::row_label(ui, "Console snapshot:", theme::label_color());
                                    theme::padded_text_edit_sized(
                                        ui,
                                        &mut snap_state.editing_console_snapshot,
                                        70.0,
                                        theme::ROW_H,
                                        true,
                                        "none",
                                    );
                                    ui.end_row();
                                });

                            // Checkbox + template selector on one row. The
                            // row_spacer pins it to ROW_H so toggling the
                            // override (which shows/hides the combo) doesn't
                            // shift the Notes field below it.
                            ui.horizontal(|ui| {
                                theme::row_spacer(ui);
                                ui.checkbox(&mut snap_state.editing_scope_override_enabled, "Scope Override").on_hover_text(help(HelpKey::SnapshotScopeOverride));
                                if snap_state.editing_scope_override_enabled {
                                    ui.add_space(8.0);
                                    let current_name = snap_state.editing_scope_template_id
                                        .and_then(|id| mgr.scope_templates.get(&id))
                                        .map(|t| t.name.clone())
                                        .unwrap_or_else(|| "(select)".into());
                                    theme::row_combo(ui, 0, |ui| {
                                        let combo = egui::ComboBox::from_id_salt("scope_override_selector")
                                            .selected_text(&current_name)
                                            .width(140.0)
                                            .height(320.0)
                                            .show_ui(ui, |ui| {
                                                for tmpl in mgr.scope_templates.values() {
                                                    if ui.selectable_label(
                                                        snap_state.editing_scope_template_id == Some(tmpl.id),
                                                        &tmpl.name,
                                                    ).clicked() {
                                                        snap_state.editing_scope_template_id = Some(tmpl.id);
                                                    }
                                                }
                                            });
                                        combo
                                            .response
                                            .on_hover_text(help(HelpKey::SnapshotScopeOverrideTemplate));
                                    });
                                }
                            });
                            ui.label("Notes:");
                            ui.add(
                                egui::TextEdit::multiline(&mut snap_state.editing_cue_notes)
                                    .desired_rows(2)
                                    .desired_width(f32::INFINITY),
                            );
                        });

                        if save_clicked {
                            if let Some(cue_id) = snap_state.selected_cue_id {
                                let parsed_num = snap_state.editing_cue_number.trim().parse::<f32>().ok();
                                let parsed_row: Option<i32> = if snap_state.editing_console_snapshot.trim().is_empty() {
                                    None
                                } else {
                                    snap_state.editing_console_snapshot.trim().parse().ok()
                                };
                                let local = snap_state.editing_local_snapshot;
                                let scope_override = if snap_state.editing_scope_override_enabled {
                                    snap_state.editing_scope_template_id
                                        .and_then(|id| mgr.scope_templates.get(&id).cloned())
                                } else {
                                    None
                                };
                                let notes = snap_state.editing_cue_notes.clone();
                                if local.is_none() && parsed_row.is_none() {
                                    snap_state.status_message = Some("Cue needs a Local snapshot, a Console snapshot, or both".into());
                                } else {
                                    let cue_mgr = cue_manager.clone();
                                    runtime.spawn(async move {
                                        cue_mgr.write().await.update_cue(
                                            cue_id,
                                            parsed_num,
                                            local,
                                            parsed_row,
                                            scope_override,
                                            notes,
                                        );
                                    });
                                    snap_state.status_message = Some("Cue updated".into());
                                    snap_state.last_edited_cue_id = None;
                                }
                            }
                        }

                        if delete_clicked {
                            if let Some(cue_id) = snap_state.selected_cue_id {
                                let cue_mgr = cue_manager.clone();
                                runtime.spawn(async move {
                                    cue_mgr.write().await.remove_cue(cue_id);
                                });
                                snap_state.selected_cue_id = None;
                                snap_state.last_edited_cue_id = None;
                                snap_state.status_message = Some("Cue deleted".into());
                            }
                        }
                    }
                });

                ui.add_space(8.0);

                // Cue List card — the cue list itself, kept at the bottom of
                // the column so adding or removing cues never shifts the Add
                // Cue or Cue Editor controls above it.
                theme::card_frame().show(ui, |ui| {
                    theme::section_heading(ui, "Cue List");

                    // Fixed column widths so the cue number, name, CS row and
                    // snapshot line up down the list. `LEAD` matches the row
                    // frame's left inner margin so the header lines up with the
                    // rows below it.
                    const COL_NUM_W: f32 = 44.0;
                    const COL_NAME_W: f32 = 110.0;
                    const COL_CS_W: f32 = 56.0;
                    const COL_SNAP_W: f32 = 120.0;
                    const COL_GAP: f32 = 6.0;
                    const LEAD: f32 = 8.0;
                    const CELL_H: f32 = 20.0;

                    // Fixed-width text cell (left-aligned, truncated to fit).
                    let text_cell = |ui: &mut egui::Ui, w: f32, rich: egui::RichText| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(w, CELL_H),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.add(egui::Label::new(rich).truncate());
                            },
                        );
                    };

                    // Column header.
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = COL_GAP;
                        ui.add_space(LEAD);
                        for (w, title) in [
                            (COL_NUM_W, "Cue"),
                            (COL_NAME_W, "Name"),
                            (COL_CS_W, "CS"),
                            (COL_SNAP_W, "Snapshot"),
                        ] {
                            text_cell(
                                ui,
                                w,
                                egui::RichText::new(title).small().color(theme::label_weak()),
                            );
                        }
                    });
                    ui.add_space(2.0);

                    // Cue list is the last item in the last card of the column;
                    // fill the remaining column height (same as the snapshot list).
                    let cue_list_height =
                        (ui.available_height() - LIST_BOTTOM_RESERVE).max(LIST_MIN_HEIGHT);
                    egui::ScrollArea::vertical()
                        .id_salt("cue_list_scroll")
                        .max_height(cue_list_height)
                        .show(ui, |ui| {
                            if let Ok(mgr) = cue_manager.try_read() {
                                for cue in &mgr.cue_list.cues {
                                    let selected = snap_state.selected_cue_id == Some(cue.id);
                                    let is_current = current_cue_id == Some(cue.id);
                                    let snap_name = cue
                                        .snapshot_id
                                        .and_then(|id| mgr.snapshots.get(&id))
                                        .map(|s| s.name.as_str())
                                        .unwrap_or("(no overlay)");
                                    let bg = if is_current {
                                        theme::cue_current_bg()
                                    } else if selected {
                                        theme::bg_elevated()
                                    } else {
                                        theme::bg_panel()
                                    };
                                    let border = if is_current {
                                        egui::Stroke::new(1.0, theme::cue_current_border())
                                    } else if selected {
                                        egui::Stroke::new(1.0, theme::ACCENT_BLUE)
                                    } else {
                                        egui::Stroke::NONE
                                    };
                                    let row_resp = egui::Frame::new()
                                        .fill(bg)
                                        .stroke(border)
                                        .corner_radius(4.0)
                                        .inner_margin(egui::Margin::symmetric(LEAD as i8, 4))
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = COL_GAP;
                                                // Fill the row so the whole line
                                                // is a click target.
                                                ui.set_min_width(ui.available_width());
                                                // Cue number — fixed-width badge.
                                                theme::colored_badge_sized(
                                                    ui,
                                                    &format!("{:.1}", cue.cue_number),
                                                    if is_current { theme::ACCENT_RED } else { theme::btn_neutral() },
                                                    COL_NUM_W,
                                                );
                                                // Name.
                                                text_cell(
                                                    ui,
                                                    COL_NAME_W,
                                                    egui::RichText::new(&cue.name)
                                                        .strong()
                                                        .color(theme::label_color()),
                                                );
                                                // Console-snapshot row.
                                                ui.allocate_ui_with_layout(
                                                    egui::vec2(COL_CS_W, CELL_H),
                                                    egui::Layout::left_to_right(egui::Align::Center),
                                                    |ui| {
                                                        if let Some(row) = cue.console_snapshot {
                                                            theme::colored_badge(
                                                                ui,
                                                                &format!("CS {row}"),
                                                                theme::ACCENT_AMBER,
                                                            );
                                                        }
                                                    },
                                                );
                                                // Snapshot overlay name.
                                                text_cell(
                                                    ui,
                                                    COL_SNAP_W,
                                                    egui::RichText::new(snap_name)
                                                        .color(theme::label_weak()),
                                                );
                                                // Scope-override flag (trailing).
                                                if cue.scope_override.is_some() {
                                                    theme::colored_badge(ui, "S", theme::ACCENT_BLUE);
                                                }
                                            });
                                        })
                                        .response;
                                    if row_resp.interact(egui::Sense::click()).clicked() {
                                        // Toggle: clicking the selected row deselects it.
                                        snap_state.selected_cue_id =
                                            if snap_state.selected_cue_id == Some(cue.id) {
                                                None
                                            } else {
                                                Some(cue.id)
                                            };
                                    }
                                    ui.add_space(2.0);
                                }
                                if mgr.cue_list.cues.is_empty() {
                                    ui.label(egui::RichText::new("No cues yet. Add one below.").color(theme::label_weak()));
                                }
                            }
                        });
                });
            });
    });

    // ── Shift console refs modal ──
    if snap_state.shift_modal_open {
        let mut open = true;
        egui::Window::new("Shift console memory refs")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_size([420.0, 180.0])
            .show(ui.ctx(), |ui| {
                ui.label(
                    egui::RichText::new(
                        "Bulk-shift the console-memory row reference on every snapshot. \
                         Use this after inserting or deleting a snapshot on the console \
                         so all linked rows still point to the right place.",
                    )
                    .color(theme::label_weak())
                    .small(),
                );
                ui.add_space(8.0);
                egui::Grid::new("shift_grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Starting row (inclusive):");
                        theme::padded_text_edit(ui, &mut snap_state.shift_from_row, 90.0, true, "")
                            .on_hover_text(help(HelpKey::ShiftFromRow));
                        ui.end_row();
                        ui.label("Delta (e.g. +1 / -1):");
                        theme::padded_text_edit(ui, &mut snap_state.shift_delta, 90.0, true, "")
                            .on_hover_text(help(HelpKey::ShiftDelta));
                        ui.end_row();
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .button("Apply")
                        .on_hover_text(help(HelpKey::ShiftApply))
                        .clicked()
                    {
                        let from = snap_state.shift_from_row.trim().parse::<i32>();
                        let delta = snap_state.shift_delta.trim().parse::<i32>();
                        match (from, delta) {
                            (Ok(from), Ok(delta)) => {
                                let cue_mgr = cue_manager.clone();
                                let (tx_status, rx_status) =
                                    std::sync::mpsc::sync_channel::<(usize, usize)>(1);
                                runtime.spawn(async move {
                                    let mut mgr = cue_mgr.write().await;
                                    let mut shifted = 0usize;
                                    let mut cleared = 0usize;
                                    for cue in mgr.cue_list.cues.iter_mut() {
                                        if let Some(row) = cue.console_snapshot {
                                            if row >= from {
                                                let new_row = row + delta;
                                                if new_row <= 0 {
                                                    cue.console_snapshot = None;
                                                    cleared += 1;
                                                } else {
                                                    cue.console_snapshot = Some(new_row);
                                                    shifted += 1;
                                                }
                                            }
                                        }
                                    }
                                    let _ = tx_status.send((shifted, cleared));
                                });
                                if let Ok((shifted, cleared)) =
                                    rx_status.recv_timeout(std::time::Duration::from_millis(500))
                                {
                                    snap_state.shift_status =
                                        Some(format!("Shifted {shifted} refs ({cleared} cleared)"));
                                } else {
                                    snap_state.shift_status = Some("Shift in progress…".into());
                                }
                            }
                            _ => {
                                snap_state.shift_status =
                                    Some("Both fields must be valid integers".into());
                            }
                        }
                    }
                    if ui
                        .button("Close")
                        .on_hover_text(help(HelpKey::ShiftClose))
                        .clicked()
                    {
                        snap_state.shift_modal_open = false;
                    }
                });
                if let Some(msg) = &snap_state.shift_status {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(msg).color(theme::TEXT_WARNING).small());
                }
            });
        if !open {
            snap_state.shift_modal_open = false;
        }
    }
}

/// Format a snapshot's captured values into sorted `(label, value)` display
/// pairs for the post-capture confirmation popup. Channel uses its `Display`;
/// the parameter path has no `Display`, so `{:?}` (matches the Inspector tab).
/// Floats are shown to one decimal; everything else via its natural string.
fn format_captured_params(
    values: &HashMap<ParameterAddress, ParameterValue>,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = values
        .iter()
        .map(|(addr, value)| {
            let label = format!("{} · {:?}", addr.channel, addr.parameter);
            let val = match value {
                ParameterValue::Float(f) => format!("{f:.1}"),
                ParameterValue::Bool(b) => b.to_string(),
                ParameterValue::Int(i) => i.to_string(),
                ParameterValue::String(s) => s.clone(),
            };
            (label, val)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A snapshot picker combobox with a type-to-filter search field at the top of
/// the dropdown. `snapshots` must be sorted for display. Typing filters the
/// list (case-insensitive substring); when exactly one entry matches, pressing
/// Tab confirms it. Selecting an item (or Tab-confirming) clears the filter and
/// closes the popup. Writes the chosen id (or `None` for "(none)") into `selected`.
fn filtered_snapshot_combo(
    ui: &mut egui::Ui,
    id_salt: &str,
    width: f32,
    selected: &mut Option<Uuid>,
    filter: &mut String,
    snapshots: &[(Uuid, String)],
) {
    let current_name = selected
        .and_then(|id| snapshots.iter().find(|(sid, _)| *sid == id))
        .map(|(_, name)| name.clone())
        .unwrap_or_else(|| "(none)".into());

    // Popup id egui uses for this combo (button id + "popup"), so we can close
    // it ourselves on selection / Tab. CloseOnClickOutside keeps the popup open
    // while the operator clicks/types in the search field.
    let popup_id = ui.make_persistent_id(id_salt).with("popup");

    let resp = egui::ComboBox::from_id_salt(id_salt)
        .selected_text(current_name)
        .width(width)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            // Search field, auto-focused when the popup first opens.
            let te = ui.add(
                egui::TextEdit::singleline(filter)
                    .hint_text("type to filter")
                    .desired_width(width),
            );
            if ui.memory(|m| m.focused().is_none()) {
                te.request_focus();
            }

            let needle = filter.to_lowercase();
            let matches: Vec<(Uuid, &str)> = snapshots
                .iter()
                .filter(|(_, name)| needle.is_empty() || name.to_lowercase().contains(&needle))
                .map(|(id, name)| (*id, name.as_str()))
                .collect();

            // Tab confirms when exactly one snapshot matches. consume_key stops
            // egui from also using Tab for focus navigation.
            let tab = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
            if tab && matches.len() == 1 {
                *selected = Some(matches[0].0);
                filter.clear();
                egui::Popup::close_id(ui.ctx(), popup_id);
                return;
            }

            ui.separator();

            if ui.selectable_label(selected.is_none(), "(none)").clicked() {
                *selected = None;
                filter.clear();
                egui::Popup::close_id(ui.ctx(), popup_id);
            }
            for (id, name) in &matches {
                if ui.selectable_label(*selected == Some(*id), *name).clicked() {
                    *selected = Some(*id);
                    filter.clear();
                    egui::Popup::close_id(ui.ctx(), popup_id);
                }
            }
            if matches.is_empty() && !needle.is_empty() {
                ui.label(egui::RichText::new("No match").color(theme::label_weak()));
            }
        });

    resp.response
        .clone()
        .on_hover_text(help(HelpKey::SnapshotPicker));

    // Popup closed (clicked away / Escape): drop any stale filter text.
    if resp.inner.is_none() && !filter.is_empty() {
        filter.clear();
    }
}

/// Collect snapshots as `(id, name)` sorted by name (case-insensitive) for the
/// filterable pickers.
fn sorted_snapshot_list(mgr: &CueManager) -> Vec<(Uuid, String)> {
    let mut out: Vec<(Uuid, String)> = mgr
        .snapshots
        .values()
        .map(|s| (s.id, s.name.clone()))
        .collect();
    out.sort_by_key(|a| a.1.to_lowercase());
    out
}

fn capture_snapshot(
    snap_state: &mut SnapshotsTabState,
    console_state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    dirty_tracker: &Arc<RwLock<DirtyTracker>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let scope = snap_state
        .scope_editor
        .to_scope_template(snap_state.new_snapshot_name.clone());
    let name = snap_state.new_snapshot_name.clone();
    let kind = snap_state.pending_kind;
    let st = console_state.clone();
    let cue_mgr = cue_manager.clone();
    let dirty = dirty_tracker.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        // The live mirror already tracks the surface (the console echoes every
        // fader/parameter move into it), so we capture it directly. We do NOT
        // request a /console/resend here — that is a multi-second full dump
        // whose flood races the capture and clobbers the live values.
        let state_guard = st.read().await;
        let data = state_guard.capture(&scope, kind);
        drop(state_guard);

        // Format for the confirmation popup before `data` moves into the snapshot.
        let params = format_captured_params(&data.values);
        let snapshot = Snapshot::new(name.clone(), scope, data, kind);
        let snapshot_id = snapshot.id;
        cue_mgr.write().await.add_snapshot(snapshot);

        // Phase C: capture establishes a new baseline — anything that
        // changes from now on is "modified since the last snapshot".
        // Mirrors WFS-DIY's clear-on-store behaviour.
        dirty.write().await.clear();

        let _ = tx.send(UiEvent::SnapshotCaptureConfirm {
            snapshot_id,
            name,
            params,
        });
    });

    snap_state.status_message = Some(format!("Capturing '{}'...", snap_state.new_snapshot_name));
    snap_state.new_snapshot_name.clear();
}

fn recapture_snapshot(
    snap_state: &mut SnapshotsTabState,
    console_state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    dirty_tracker: &Arc<RwLock<DirtyTracker>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let Some(snap_id) = snap_state.selected_snapshot_id else {
        return;
    };

    let st = console_state.clone();
    let cue_mgr = cue_manager.clone();
    let dirty = dirty_tracker.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        // Read the existing snapshot's scope AND its kind so re-capture
        // honours the original ApplyOnSave / ApplyOnRecall semantics.
        let mgr = cue_mgr.read().await;
        let Some(existing) = mgr.snapshots.get(&snap_id) else {
            return;
        };
        let scope = existing.scope.clone();
        let name = existing.name.clone();
        let kind = existing.kind;
        drop(mgr);

        // Capture the live mirror directly (no /console/resend — see
        // `capture_snapshot`).
        let state_guard = st.read().await;
        let data = state_guard.capture(&scope, kind);
        drop(state_guard);

        // Format for the confirmation popup before `data` moves into the store.
        let params = format_captured_params(&data.values);
        cue_mgr.write().await.update_snapshot(snap_id, data);

        // Phase C: re-capture also re-anchors the dirty baseline.
        dirty.write().await.clear();

        let _ = tx.send(UiEvent::SnapshotCaptureConfirm {
            snapshot_id: snap_id,
            name,
            params,
        });
    });

    snap_state.status_message = Some("Re-capturing...".into());
}

/// Recall the currently-selected snapshot, optionally bypassing the scope.
///
/// `ignore_scope = false` is the standard recall: filter by `snapshot.scope`
/// (or, if the snapshot is `ApplyOnSave`, the scope filter is a no-op since
/// the stored data is already inside the scope).
///
/// `ignore_scope = true` only makes sense for `ApplyOnRecall` snapshots:
/// fire every stored parameter regardless of scope, so the operator can jump
/// into a cue list mid-show without dragging accumulated partial changes
/// along. The button gating in the UI prevents this from being clicked on
/// `ApplyOnSave` snapshots, but the engine handles either input gracefully.
#[allow(clippy::too_many_arguments)]
fn recall_selected_snapshot(
    snap_state: &mut SnapshotsTabState,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    snapshot_engine: &Option<Arc<SnapshotEngine>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
    ignore_scope: bool,
) {
    let Some(snap_id) = snap_state.selected_snapshot_id else {
        return;
    };
    recall_snapshot_by_id(
        snap_id,
        cue_manager,
        palette_manager,
        snapshot_engine,
        runtime,
        ui_tx,
        ignore_scope,
    );
    snap_state.status_message = Some(if ignore_scope {
        "Recalling without scope...".into()
    } else {
        "Recalling...".into()
    });
}

/// Recall a snapshot by id (no `SnapshotsTabState` needed). Used by the tab's
/// Recall buttons (via `recall_selected_snapshot`) and by the post-capture
/// confirmation popup's "Reload to verify" button. Emits `SnapshotRecalled`
/// on completion. No-op if the engine isn't connected or the id is unknown.
#[allow(clippy::too_many_arguments)]
pub fn recall_snapshot_by_id(
    snap_id: Uuid,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    snapshot_engine: &Option<Arc<SnapshotEngine>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
    ignore_scope: bool,
) {
    let Some(engine) = snapshot_engine.clone() else {
        return;
    };
    let cue_mgr = cue_manager.clone();
    let pmgr = palette_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        // Read the snapshot + scope under the cue-manager lock.
        let mgr = cue_mgr.read().await;
        let Some(snapshot) = mgr.snapshots.get(&snap_id).cloned() else {
            return;
        };
        let scope = snapshot.scope.clone();
        let name = snapshot.name.clone();
        drop(mgr);

        // Read palettes under the palette-manager lock.
        let palettes_guard = pmgr.read().await;
        let result = engine
            .recall(&snapshot, &scope, &palettes_guard.palettes, ignore_scope)
            .await;
        drop(palettes_guard);

        // Emit a recall-specific event so the status reads "Recalled …",
        // not "Captured …". The handler adds the "(N params sent)" suffix.
        let display = if ignore_scope {
            format!("{name} (no scope)")
        } else {
            name.clone()
        };
        let _ = tx.send(UiEvent::SnapshotRecalled {
            name: display,
            params_sent: result.parameters_sent,
        });
    });
}

/// Phase D: spawn a background task that builds the per-parameter QLab
/// export (one network cue per stored parameter, all in a group) and fires
/// it at the configured QLab address. Reports success/failure via
/// `UiEvent::SnapshotCaptured` (used here as a generic status message
/// channel — Phase D doesn't have a dedicated event variant).
fn qlab_export_full_snapshot(
    snap_state: &mut SnapshotsTabState,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    qlab_ip: String,
    qlab_port: u16,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let Some(snap_id) = snap_state.selected_snapshot_id else {
        return;
    };
    let cue_mgr = cue_manager.clone();
    let pal_mgr = palette_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        // Pull the snapshot under the cue-manager lock.
        let mgr = cue_mgr.read().await;
        let Some(snapshot) = mgr.snapshots.get(&snap_id).cloned() else {
            return;
        };
        drop(mgr);

        let pmgr = pal_mgr.read().await;
        let palettes = pmgr.palettes.clone();
        drop(pmgr);

        let sequence = build_snapshot_cues(&snapshot, &palettes, /* qlab_patch */ 1);
        let child_count = sequence.network_cues.len();

        match QLabClient::new(&qlab_ip, qlab_port).await {
            Ok(client) => match client.send_sequence(&sequence).await {
                Ok(sent) => {
                    let _ = tx.send(UiEvent::SnapshotCaptured {
                        name: format!(
                            "Exported '{}' to QLab: {child_count} cues, {sent} OSC messages",
                            snapshot.name
                        ),
                        param_count: child_count,
                    });
                }
                Err(e) => {
                    let _ = tx.send(UiEvent::SnapshotCaptured {
                        name: format!("QLab export failed: {e}"),
                        param_count: 0,
                    });
                }
            },
            Err(e) => {
                let _ = tx.send(UiEvent::SnapshotCaptured {
                    name: format!("QLab connect failed: {e}"),
                    param_count: 0,
                });
            }
        }
    });

    snap_state.status_message = Some("Exporting to QLab...".into());
}

/// Phase D: spawn a background task that builds the single-trigger-cue
/// QLab sequence (one network cue whose customString is `/snapshot/recall
/// <name>`) and fires it. Used for large snapshots where pushing every
/// parameter as its own cue would stall QLab.
fn qlab_create_trigger_cue(
    snap_state: &mut SnapshotsTabState,
    cue_manager: &Arc<RwLock<CueManager>>,
    qlab_ip: String,
    qlab_port: u16,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let Some(snap_id) = snap_state.selected_snapshot_id else {
        return;
    };
    let cue_mgr = cue_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        let mgr = cue_mgr.read().await;
        let Some(snapshot) = mgr.snapshots.get(&snap_id).cloned() else {
            return;
        };
        drop(mgr);

        let sequence = build_snapshot_load_cue(&snapshot.name, /* qlab_patch */ 1);

        match QLabClient::new(&qlab_ip, qlab_port).await {
            Ok(client) => match client.send_sequence(&sequence).await {
                Ok(sent) => {
                    let _ = tx.send(UiEvent::SnapshotCaptured {
                        name: format!(
                            "Trigger cue created in QLab for '{}' ({sent} OSC messages)",
                            snapshot.name
                        ),
                        param_count: sent,
                    });
                }
                Err(e) => {
                    let _ = tx.send(UiEvent::SnapshotCaptured {
                        name: format!("QLab trigger cue failed: {e}"),
                        param_count: 0,
                    });
                }
            },
            Err(e) => {
                let _ = tx.send(UiEvent::SnapshotCaptured {
                    name: format!("QLab connect failed: {e}"),
                    param_count: 0,
                });
            }
        }
    });

    snap_state.status_message = Some("Creating QLab trigger cue...".into());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::parameter::ParameterPath;

    #[test]
    fn format_captured_params_sorts_and_formats() {
        let mut values: HashMap<ParameterAddress, ParameterValue> = HashMap::new();
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(41),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(0.04),
        );
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(41),
                parameter: ParameterPath::Mute,
            },
            ParameterValue::Bool(false),
        );
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(42),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(-10.04),
        );

        let out = format_captured_params(&values);

        // Sorted by label string.
        let labels: Vec<&str> = out.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(
            labels,
            vec!["Input 41 · Fader", "Input 41 · Mute", "Input 42 · Fader"]
        );
        // Floats rounded to one decimal; bool as text.
        assert_eq!(out[0].1, "0.0");
        assert_eq!(out[1].1, "false");
        assert_eq!(out[2].1, "-10.0");
    }
}
