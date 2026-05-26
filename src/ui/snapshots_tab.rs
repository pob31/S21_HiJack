use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::UiEvent;
use super::palettes_ui::{PalettesUiState, draw_palettes_section};
use super::scope_editor::ScopeEditorState;
use super::theme;
use crate::console::cue_manager::CueManager;
use crate::console::palette_manager::PaletteManager;
use crate::console::snapshot_engine::SnapshotEngine;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::snapshot::{Cue, Snapshot, SnapshotKind};
use crate::model::state::ConsoleState;
use crate::osc::qlab_client::QLabClient;
use crate::osc::qlab_cue_builder::{build_snapshot_cues, build_snapshot_load_cue};

/// State for the Snapshots tab.
pub struct SnapshotsTabState {
    // Cue management
    pub selected_cue_id: Option<Uuid>,
    pub new_cue_number: String,
    pub new_cue_name: String,
    pub selected_snapshot_for_cue: Option<Uuid>,

    // Cue editor
    pub last_edited_cue_id: Option<Uuid>,
    pub editing_cue_number: String,
    pub editing_local_snapshot: Option<Uuid>,
    pub editing_console_snapshot: String,
    pub editing_fade_time: f32,
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
            last_edited_cue_id: None,
            editing_cue_number: String::new(),
            editing_local_snapshot: None,
            editing_console_snapshot: String::new(),
            editing_fade_time: 0.0,
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
    let available = ui.available_size();

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
                        .color(theme::TEXT_SECONDARY),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let edit_btn = theme::action_button(
                            "Edit Scope…",
                            theme::ACCENT_BLUE,
                            egui::Vec2::new(120.0, 30.0),
                        );
                        if ui.add(edit_btn).clicked() {
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
                            theme::BG_ELEVATED,
                            egui::Vec2::new(70.0, 30.0),
                        );
                        if ui.add(clear_btn).clicked() {
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
                                .on_hover_text(
                                    "When recalling a snapshot, write any dirty parameters \
                                     within the previous snapshot's scope back into it. \
                                     Captures mid-show tweaks automatically.",
                                )
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
                                let _ = resp.on_hover_text(
                                    "Requires Mode 2 or Mode 3 (iPad protocol) — the console \
                                     snapshot list is only reachable via that protocol.",
                                );
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
                            theme::row_label(ui, "Apply scope:", theme::TEXT_PRIMARY);
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
                            theme::row_label(ui, "Name:", theme::TEXT_PRIMARY);
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
                                let _ = recall_no_scope_resp.clone().on_hover_text(
                                    "Only available for snapshots captured with 'Apply scope on recall' \
                                     — ApplyOnSave snapshots already filtered at capture time, so there \
                                     is nothing outside the saved scope to recall.",
                                );
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
                                .color(theme::TEXT_SECONDARY),
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
                            ui.label(egui::RichText::new("Select scope parameters to capture (or switch to 'On recall').").color(theme::TEXT_SECONDARY));
                        }

                        ui.add_space(8.0);

                        // Snapshot list — kept at the bottom of the card so
                        // adding or removing snapshots never shifts the
                        // capture / recall controls above it.
                        egui::ScrollArea::vertical()
                            .id_salt("snapshot_list_scroll")
                            .max_height(180.0)
                            .show(ui, |ui| {
                                if let Ok(mgr) = cue_manager.try_read() {
                                    let mut snapshots: Vec<_> = mgr.snapshots.values().collect();
                                    snapshots.sort_by_key(|s| std::cmp::Reverse(s.modified_at));

                                    for snap in snapshots {
                                        let selected = snap_state.selected_snapshot_id == Some(snap.id);
                                        let bg = if selected { theme::BG_ELEVATED } else { theme::BG_PANEL };

                                        egui::Frame::new()
                                            .fill(bg)
                                            .stroke(if selected {
                                                egui::Stroke::new(1.0, theme::ACCENT_BLUE)
                                            } else {
                                                egui::Stroke::NONE
                                            })
                                            .corner_radius(4.0)
                                            .inner_margin(egui::Margin::symmetric(8, 3))
                                            .show(ui, |ui| {
                                                let response = ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(&snap.name)
                                                            .strong()
                                                            .color(theme::TEXT_PRIMARY),
                                                    );
                                                    let kind_label = match snap.kind {
                                                        SnapshotKind::ApplyOnSave => "scope: save",
                                                        SnapshotKind::ApplyOnRecall => "scope: recall",
                                                    };
                                                    theme::colored_badge(
                                                        ui,
                                                        kind_label,
                                                        match snap.kind {
                                                            SnapshotKind::ApplyOnSave => theme::BG_ELEVATED,
                                                            SnapshotKind::ApplyOnRecall => theme::ACCENT_BLUE,
                                                        },
                                                    );
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "{} params  {}",
                                                            snap.data.parameter_count(),
                                                            snap.modified_at.format("%H:%M:%S"),
                                                        ))
                                                        .color(theme::TEXT_SECONDARY)
                                                        .small(),
                                                    );
                                                }).response;

                                                if response.interact(egui::Sense::click()).clicked() {
                                                    snap_state.selected_snapshot_id = Some(snap.id);
                                                }
                                            });
                                        ui.add_space(1.0);
                                    }

                                    if mgr.snapshots.is_empty() {
                                        ui.label(egui::RichText::new("No snapshots yet.").color(theme::TEXT_SECONDARY));
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
                    theme::section_heading(ui, "Add Cue");

                    // Add Cue form
                    ui.horizontal(|ui| {
                        theme::row_label(ui, "Cue #:", theme::TEXT_PRIMARY);
                        theme::padded_text_edit_sized(ui, &mut snap_state.new_cue_number, 60.0, theme::ROW_H, true, "");
                        theme::row_label(ui, "Name:", theme::TEXT_PRIMARY);
                        theme::padded_text_edit_sized(ui, &mut snap_state.new_cue_name, 130.0, theme::ROW_H, true, "");
                        theme::row_label(ui, "CS:", theme::TEXT_PRIMARY);
                        theme::padded_text_edit_sized(ui, &mut snap_state.new_cue_console_row, 50.0, theme::ROW_H, true, "");
                    });
                    ui.horizontal(|ui| {
                        theme::row_label(ui, "Local snapshot:", theme::TEXT_PRIMARY);
                        if let Ok(mgr) = cue_manager.try_read() {
                            let current_name = snap_state
                                .selected_snapshot_for_cue
                                .and_then(|id| mgr.snapshots.get(&id))
                                .map(|s| s.name.clone())
                                .unwrap_or_else(|| "(none)".into());
                            theme::row_combo(ui, 0, |ui| {
                                egui::ComboBox::from_id_salt("snapshot_selector")
                                    .selected_text(&current_name)
                                    .width(120.0)
                                    .show_ui(ui, |ui| {
                                        if ui
                                            .selectable_label(
                                                snap_state.selected_snapshot_for_cue.is_none(),
                                                "(none)",
                                            )
                                            .clicked()
                                        {
                                            snap_state.selected_snapshot_for_cue = None;
                                        }
                                        for snap in mgr.snapshots.values() {
                                            if ui.selectable_label(
                                                snap_state.selected_snapshot_for_cue == Some(snap.id),
                                                &snap.name,
                                            ).clicked() {
                                                snap_state.selected_snapshot_for_cue = Some(snap.id);
                                            }
                                        }
                                    });
                            });
                        }
                        if theme::row_action_button(ui, "Add Cue", theme::ACCENT_GREEN, 75.0, true) {
                            let parsed_num = snap_state.new_cue_number.parse::<f32>();
                            let parsed_row = if snap_state.new_cue_console_row.trim().is_empty() {
                                Ok(None)
                            } else {
                                snap_state.new_cue_console_row.trim().parse::<i32>().map(Some)
                            };
                            let snap_id = snap_state.selected_snapshot_for_cue;
                            match (parsed_num, parsed_row) {
                                (Ok(num), Ok(row)) if row.is_some() || snap_id.is_some() => {
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
                                (Ok(_), Ok(_)) => {
                                    snap_state.status_message = Some("Cue needs a Local snapshot, a Console snapshot, or both".into());
                                }
                                (Err(_), _) => {
                                    snap_state.status_message = Some("Enter a valid cue number".into());
                                }
                                (_, Err(_)) => {
                                    snap_state.status_message = Some("Row must be a whole number (or blank)".into());
                                }
                            }
                        }
                        if theme::row_long_press_button(
                            ui,
                            "Delete",
                            theme::ACCENT_RED,
                            70.0,
                            snap_state.selected_cue_id.is_some(),
                        ) {
                            if let Some(cue_id) = snap_state.selected_cue_id {
                                let cue_mgr = cue_manager.clone();
                                runtime.spawn(async move {
                                    cue_mgr.write().await.remove_cue(cue_id);
                                });
                                snap_state.selected_cue_id = None;
                                snap_state.status_message = Some("Cue deleted".into());
                            }
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let shift_resp = ui
                            .scope(|ui| {
                                ui.spacing_mut().button_padding = egui::Vec2::new(12.0, 4.0);
                                ui.add(theme::action_button(
                                    "Shift console snapshots…",
                                    theme::BG_ELEVATED,
                                    egui::Vec2::new(190.0, theme::ROW_H),
                                ))
                            })
                            .inner
                            .on_hover_text(
                                "Bulk-shift every cue's console snapshot when you've \
                                 inserted or removed snapshots on the console.",
                            );
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
                    theme::section_heading(ui, "Cue Editor");
                    let has_selection = snap_state.selected_cue_id.is_some();
                    if !has_selection {
                        ui.label(
                            egui::RichText::new("Select a cue to edit.")
                                .color(theme::TEXT_SECONDARY),
                        );
                    }
                    if let Ok(mgr) = cue_manager.try_read() {
                        if let Some(cue_id) = snap_state.selected_cue_id {
                            if let Some(cue) = mgr.cue_list.cues.iter().find(|c| c.id == cue_id) {
                                if snap_state.last_edited_cue_id != Some(cue_id) {
                                    snap_state.editing_cue_number = format!("{}", cue.cue_number);
                                    snap_state.editing_local_snapshot = cue.snapshot_id;
                                    snap_state.editing_console_snapshot = cue
                                        .console_snapshot
                                        .map(|n| n.to_string())
                                        .unwrap_or_default();
                                    snap_state.editing_fade_time = cue.fade_time;
                                    snap_state.editing_scope_override_enabled = cue.scope_override.is_some();
                                    snap_state.editing_scope_template_id = cue.scope_override.as_ref().map(|s| s.id);
                                    snap_state.editing_cue_notes = cue.notes.clone();
                                    snap_state.last_edited_cue_id = Some(cue_id);
                                }
                            }
                        }
                        ui.add_enabled_ui(has_selection, |ui| {
                            // 2-column label|control form. row_label sizes each
                            // label cell to ROW_H (centred), and the combo cell
                            // uses the -7 nudge since Grid cells top-align.
                            egui::Grid::new("cue_editor_grid")
                                .num_columns(2)
                                .spacing([12.0, 10.0])
                                .show(ui, |ui| {
                                    theme::row_label(ui, "Cue #:", theme::TEXT_PRIMARY);
                                    theme::padded_text_edit_sized(
                                        ui,
                                        &mut snap_state.editing_cue_number,
                                        70.0,
                                        theme::ROW_H,
                                        true,
                                        "",
                                    );
                                    ui.end_row();

                                    theme::row_label(ui, "Local snapshot:", theme::TEXT_PRIMARY);
                                    let current_name = snap_state
                                        .editing_local_snapshot
                                        .and_then(|id| mgr.snapshots.get(&id))
                                        .map(|s| s.name.clone())
                                        .unwrap_or_else(|| "(none)".into());
                                    theme::row_combo(ui, 0, |ui| {
                                        egui::ComboBox::from_id_salt("cue_editor_local_snapshot")
                                            .selected_text(&current_name)
                                            .width(140.0)
                                            .show_ui(ui, |ui| {
                                                if ui
                                                    .selectable_label(
                                                        snap_state.editing_local_snapshot.is_none(),
                                                        "(none)",
                                                    )
                                                    .clicked()
                                                {
                                                    snap_state.editing_local_snapshot = None;
                                                }
                                                for s in mgr.snapshots.values() {
                                                    if ui
                                                        .selectable_label(
                                                            snap_state.editing_local_snapshot == Some(s.id),
                                                            &s.name,
                                                        )
                                                        .clicked()
                                                    {
                                                        snap_state.editing_local_snapshot = Some(s.id);
                                                    }
                                                }
                                            });
                                    });
                                    ui.end_row();

                                    theme::row_label(ui, "Console snapshot:", theme::TEXT_PRIMARY);
                                    theme::padded_text_edit_sized(
                                        ui,
                                        &mut snap_state.editing_console_snapshot,
                                        70.0,
                                        theme::ROW_H,
                                        true,
                                        "none",
                                    );
                                    ui.end_row();

                                    theme::row_label(ui, "Fade Time:", theme::TEXT_PRIMARY);
                                    ui.add(
                                        egui::Slider::new(&mut snap_state.editing_fade_time, 0.0..=60.0)
                                            .suffix(" s")
                                            .step_by(0.1),
                                    );
                                    ui.end_row();
                                });

                            ui.horizontal(|ui| {
                                theme::row_spacer(ui);
                                ui.checkbox(&mut snap_state.editing_scope_override_enabled, "Scope Override");
                            });
                            if snap_state.editing_scope_override_enabled {
                                ui.horizontal(|ui| {
                                    theme::row_label(ui, "Template:", theme::TEXT_PRIMARY);
                                    let current_name = snap_state.editing_scope_template_id
                                        .and_then(|id| mgr.scope_templates.get(&id))
                                        .map(|t| t.name.clone())
                                        .unwrap_or_else(|| "(select)".into());
                                    theme::row_combo(ui, 0, |ui| {
                                        egui::ComboBox::from_id_salt("scope_override_selector")
                                            .selected_text(&current_name)
                                            .width(140.0)
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
                                    });
                                });
                            }
                            ui.label("Notes:");
                            ui.add(
                                egui::TextEdit::multiline(&mut snap_state.editing_cue_notes)
                                    .desired_rows(2)
                                    .desired_width(f32::INFINITY),
                            );
                            let save_clicked = ui
                                .scope(|ui| {
                                    ui.spacing_mut().button_padding = egui::Vec2::new(12.0, 4.0);
                                    ui.add(theme::action_button(
                                        "Save Cue Changes",
                                        theme::ACCENT_GREEN,
                                        egui::Vec2::new(140.0, theme::ROW_H),
                                    ))
                                    .clicked()
                                })
                                .inner;
                            if save_clicked {
                                if let Some(cue_id) = snap_state.selected_cue_id {
                                    let parsed_num = snap_state.editing_cue_number.trim().parse::<f32>().ok();
                                    let parsed_row: Option<i32> = if snap_state.editing_console_snapshot.trim().is_empty() {
                                        None
                                    } else {
                                        snap_state.editing_console_snapshot.trim().parse().ok()
                                    };
                                    let local = snap_state.editing_local_snapshot;
                                    let fade_time = snap_state.editing_fade_time;
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
                                                fade_time,
                                                scope_override,
                                                notes,
                                            );
                                        });
                                        snap_state.status_message = Some("Cue updated".into());
                                        snap_state.last_edited_cue_id = None;
                                    }
                                }
                            }
                        });
                    }
                });

                ui.add_space(8.0);

                // Cue List card — the cue list itself, kept at the bottom of
                // the column so adding or removing cues never shifts the Add
                // Cue or Cue Editor controls above it.
                theme::card_frame().show(ui, |ui| {
                    theme::section_heading(ui, "Cue List");

                    egui::ScrollArea::vertical()
                        .id_salt("cue_list_scroll")
                        .max_height(available.y * 0.35)
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
                                        theme::CUE_CURRENT_BG
                                    } else if selected {
                                        theme::BG_ELEVATED
                                    } else {
                                        theme::BG_PANEL
                                    };
                                    let border = if is_current {
                                        egui::Stroke::new(1.0, theme::CUE_CURRENT_BORDER)
                                    } else if selected {
                                        egui::Stroke::new(1.0, theme::ACCENT_BLUE)
                                    } else {
                                        egui::Stroke::NONE
                                    };
                                    egui::Frame::new()
                                        .fill(bg)
                                        .stroke(border)
                                        .corner_radius(4.0)
                                        .inner_margin(egui::Margin::symmetric(8, 4))
                                        .show(ui, |ui| {
                                            let response = ui.horizontal(|ui| {
                                                theme::colored_badge(
                                                    ui,
                                                    &format!("{:.1}", cue.cue_number),
                                                    if is_current { theme::ACCENT_RED } else { theme::BG_ELEVATED },
                                                );
                                                ui.add_space(8.0);
                                                ui.label(
                                                    egui::RichText::new(&cue.name)
                                                        .strong()
                                                        .color(theme::TEXT_PRIMARY),
                                                );
                                                if let Some(row) = cue.console_snapshot {
                                                    theme::colored_badge(
                                                        ui,
                                                        &format!("CS {row}"),
                                                        theme::ACCENT_AMBER,
                                                    );
                                                }
                                                ui.label(
                                                    egui::RichText::new(snap_name)
                                                        .color(theme::TEXT_SECONDARY),
                                                );
                                                if cue.fade_time > 0.0 {
                                                    theme::colored_badge(
                                                        ui,
                                                        &format!("{:.1}s", cue.fade_time),
                                                        theme::ACCENT_AMBER,
                                                    );
                                                }
                                                if cue.scope_override.is_some() {
                                                    theme::colored_badge(ui, "S", theme::ACCENT_BLUE);
                                                }
                                            }).response;
                                            if response.interact(egui::Sense::click()).clicked() {
                                                snap_state.selected_cue_id = Some(cue.id);
                                            }
                                        });
                                    ui.add_space(2.0);
                                }
                                if mgr.cue_list.cues.is_empty() {
                                    ui.label(egui::RichText::new("No cues yet. Add one below.").color(theme::TEXT_SECONDARY));
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
                    .color(theme::TEXT_SECONDARY)
                    .small(),
                );
                ui.add_space(8.0);
                egui::Grid::new("shift_grid")
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Starting row (inclusive):");
                        theme::padded_text_edit(ui, &mut snap_state.shift_from_row, 90.0, true, "");
                        ui.end_row();
                        ui.label("Delta (e.g. +1 / -1):");
                        theme::padded_text_edit(ui, &mut snap_state.shift_delta, 90.0, true, "");
                        ui.end_row();
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Apply").clicked() {
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
                    if ui.button("Close").clicked() {
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
        let state_guard = st.read().await;
        let data = state_guard.capture(&scope, kind);
        let param_count = data.parameter_count();
        drop(state_guard);

        let snapshot = Snapshot::new(name.clone(), scope, data, kind);
        cue_mgr.write().await.add_snapshot(snapshot);

        // Phase C: capture establishes a new baseline — anything that
        // changes from now on is "modified since the last snapshot".
        // Mirrors WFS-DIY's clear-on-store behaviour.
        dirty.write().await.clear();

        let _ = tx.send(UiEvent::SnapshotCaptured { name, param_count });
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

        // Capture fresh data using the original kind.
        let state_guard = st.read().await;
        let data = state_guard.capture(&scope, kind);
        let param_count = data.parameter_count();
        drop(state_guard);

        // Update
        cue_mgr.write().await.update_snapshot(snap_id, data);

        // Phase C: re-capture also re-anchors the dirty baseline.
        dirty.write().await.clear();

        let _ = tx.send(UiEvent::SnapshotCaptured { name, param_count });
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

        let label = if ignore_scope {
            format!(
                "Recalled '{name}' without scope ({} params sent)",
                result.parameters_sent
            )
        } else {
            format!("Recalled '{name}' ({} params sent)", result.parameters_sent)
        };
        let _ = tx.send(UiEvent::SnapshotCaptured {
            name: label,
            param_count: result.parameters_sent,
        });
    });

    snap_state.status_message = Some(if ignore_scope {
        "Recalling without scope...".into()
    } else {
        "Recalling...".into()
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
