use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::console::cue_manager::CueManager;
use crate::console::palette_manager::PaletteManager;
use crate::console::snapshot_engine::SnapshotEngine;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::snapshot::{Cue, Snapshot, SnapshotKind};
use crate::model::state::ConsoleState;
use crate::osc::qlab_client::QLabClient;
use crate::osc::qlab_cue_builder::{build_snapshot_cues, build_snapshot_load_cue};
use super::palettes_ui::{PalettesUiState, draw_palettes_section};
use super::scope_editor::ScopeEditorState;
use super::theme;
use super::UiEvent;

/// State for the Snapshots tab.
pub struct SnapshotsTabState {
    // Cue management
    pub selected_cue_id: Option<Uuid>,
    pub new_cue_number: String,
    pub new_cue_name: String,
    pub selected_snapshot_for_cue: Option<Uuid>,

    // Cue editor
    pub last_edited_cue_id: Option<Uuid>,
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
    pub new_template_name: String,
    pub selected_scope_template_id: Option<Uuid>,

    // Feedback
    pub status_message: Option<String>,
}

impl Default for SnapshotsTabState {
    fn default() -> Self {
        Self {
            selected_cue_id: None,
            new_cue_number: String::new(),
            new_cue_name: String::new(),
            selected_snapshot_for_cue: None,
            last_edited_cue_id: None,
            editing_fade_time: 0.0,
            editing_scope_override_enabled: false,
            editing_scope_template_id: None,
            editing_cue_notes: String::new(),
            new_snapshot_name: String::new(),
            selected_snapshot_id: None,
            pending_kind: SnapshotKind::default(),
            scope_editor: ScopeEditorState::default(),
            new_template_name: String::new(),
            selected_scope_template_id: None,
            status_message: None,
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

    // Two-column layout
    let available = ui.available_size();
    let left_width = (available.x * 0.5).min(700.0);
    let panel_height = available.y;

    ui.horizontal(|ui| {
        // Left panel: Scope templates + editor
        ui.vertical(|ui| {
            ui.set_width(left_width);
            ui.set_min_height(panel_height);

            egui::ScrollArea::vertical()
                .id_salt("snapshot_left_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Scope template list
                    theme::card_frame().show(ui, |ui| {
                        theme::section_heading(ui, "Scope Templates");
                        if let Ok(mgr) = cue_manager.try_read() {
                            let mut templates: Vec<_> = mgr.scope_templates.values().collect();
                            templates.sort_by(|a, b| a.name.cmp(&b.name));
                            for tmpl in templates {
                                let selected = snap_state.selected_scope_template_id == Some(tmpl.id);
                                let text = format!("{} ({} ch)", tmpl.name, tmpl.channel_scopes.len());
                                let response = ui.selectable_label(selected, egui::RichText::new(&text).color(
                                    if selected { theme::TEXT_PRIMARY } else { theme::TEXT_SECONDARY }
                                ));
                                if response.clicked() {
                                    snap_state.selected_scope_template_id = Some(tmpl.id);
                                    snap_state.scope_editor.load_template(
                                        tmpl,
                                        aux_count,
                                        group_count,
                                        matrix_count,
                                    );
                                }
                            }
                        }

                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut snap_state.new_template_name).desired_width(150.0));
                            let save_btn = theme::action_button("Save Template", theme::ACCENT_GREEN, egui::Vec2::new(100.0, 28.0));
                            if ui.add(save_btn).clicked() && !snap_state.new_template_name.is_empty() {
                                let template = snap_state.scope_editor.to_scope_template(
                                    snap_state.new_template_name.clone(),
                                );
                                let cue_mgr = cue_manager.clone();
                                runtime.spawn(async move {
                                    cue_mgr.write().await.add_scope_template(template);
                                });
                                snap_state.status_message = Some(format!("Saved template: {}", snap_state.new_template_name));
                                snap_state.new_template_name.clear();
                            }
                        });
                    });

                    ui.add_space(8.0);

                    // Recall scope summary + Edit Scope button.
                    // The full editor lives in a dedicated window (rendered at App level)
                    // so the matrix has room for 48 channels and the two-level
                    // collapsing without competing with the snapshots tab layout.
                    theme::card_frame().show(ui, |ui| {
                        theme::section_heading(ui, "Recall Scope");
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
                });
        });

        ui.add_space(4.0);

        // Right panel: Cue list + snapshots
        ui.vertical(|ui| {
            ui.set_min_height(panel_height);

            egui::ScrollArea::vertical()
                .id_salt("snapshot_right_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // ── Cue List card ──
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
                                        let snap_name = mgr
                                            .snapshots
                                            .get(&cue.snapshot_id)
                                            .map(|s| s.name.as_str())
                                            .unwrap_or("?");

                                        // Cue row with DiGiCo-style maroon highlight for current
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
                                                    // Number badge
                                                    theme::colored_badge(
                                                        ui,
                                                        &format!("{:.1}", cue.cue_number),
                                                        if is_current { theme::ACCENT_RED } else { theme::BG_ELEVATED },
                                                    );

                                                    ui.add_space(8.0);

                                                    // Cue name
                                                    ui.label(
                                                        egui::RichText::new(&cue.name)
                                                            .strong()
                                                            .color(theme::TEXT_PRIMARY),
                                                    );

                                                    // Snapshot name (secondary)
                                                    ui.label(
                                                        egui::RichText::new(snap_name)
                                                            .color(theme::TEXT_SECONDARY),
                                                    );

                                                    // Fade time badge
                                                    if cue.fade_time > 0.0 {
                                                        theme::colored_badge(
                                                            ui,
                                                            &format!("{:.1}s", cue.fade_time),
                                                            theme::ACCENT_AMBER,
                                                        );
                                                    }

                                                    // Scope override indicator
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

                        ui.add_space(8.0);

                        // Add cue controls
                        ui.horizontal(|ui| {
                            ui.label("Cue #:");
                            ui.add(egui::TextEdit::singleline(&mut snap_state.new_cue_number).desired_width(50.0));
                            ui.label("Name:");
                            ui.add(egui::TextEdit::singleline(&mut snap_state.new_cue_name).desired_width(120.0));
                        });

                        // Snapshot selector for new cue
                        ui.horizontal(|ui| {
                            ui.label("Snapshot:");
                            if let Ok(mgr) = cue_manager.try_read() {
                                let current_name = snap_state
                                    .selected_snapshot_for_cue
                                    .and_then(|id| mgr.snapshots.get(&id))
                                    .map(|s| s.name.clone())
                                    .unwrap_or_else(|| "(select)".into());

                                egui::ComboBox::from_id_salt("snapshot_selector")
                                    .selected_text(&current_name)
                                    .show_ui(ui, |ui| {
                                        for snap in mgr.snapshots.values() {
                                            if ui.selectable_label(
                                                snap_state.selected_snapshot_for_cue == Some(snap.id),
                                                &snap.name,
                                            ).clicked() {
                                                snap_state.selected_snapshot_for_cue = Some(snap.id);
                                            }
                                        }
                                    });
                            }

                            let add_btn = theme::action_button("Add Cue", theme::ACCENT_GREEN, egui::Vec2::new(80.0, 28.0));
                            if ui.add(add_btn).clicked() {
                                if let (Ok(num), Some(snap_id)) = (
                                    snap_state.new_cue_number.parse::<f32>(),
                                    snap_state.selected_snapshot_for_cue,
                                ) {
                                    let name = if snap_state.new_cue_name.is_empty() {
                                        format!("Cue {num}")
                                    } else {
                                        snap_state.new_cue_name.clone()
                                    };
                                    let cue = Cue::new(num, name, snap_id);
                                    let cue_mgr = cue_manager.clone();
                                    runtime.spawn(async move {
                                        cue_mgr.write().await.add_cue(cue);
                                    });
                                    snap_state.new_cue_number.clear();
                                    snap_state.new_cue_name.clear();
                                    snap_state.status_message = Some(format!("Added cue {num}"));
                                } else {
                                    snap_state.status_message = Some("Enter a valid cue number and select a snapshot".into());
                                }
                            }

                            let del_btn = theme::action_button("Delete", theme::ACCENT_RED, egui::Vec2::new(70.0, 28.0));
                            if ui.add_enabled(snap_state.selected_cue_id.is_some(), del_btn).clicked() {
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
                    });

                    // ── Cue Editor card (when a cue is selected) ──
                    if let Some(cue_id) = snap_state.selected_cue_id {
                        ui.add_space(8.0);
                        theme::card_frame().show(ui, |ui| {
                            theme::section_heading(ui, "Cue Editor");

                            if let Ok(mgr) = cue_manager.try_read() {
                                if let Some(cue) = mgr.cue_list.cues.iter().find(|c| c.id == cue_id) {
                                    // Sync editor state when selection changes
                                    if snap_state.last_edited_cue_id != Some(cue_id) {
                                        snap_state.editing_fade_time = cue.fade_time;
                                        snap_state.editing_scope_override_enabled = cue.scope_override.is_some();
                                        snap_state.editing_scope_template_id = cue.scope_override.as_ref().map(|s| s.id);
                                        snap_state.editing_cue_notes = cue.notes.clone();
                                        snap_state.last_edited_cue_id = Some(cue_id);
                                    }

                                    ui.horizontal(|ui| {
                                        ui.label("Fade Time:");
                                        ui.add(
                                            egui::Slider::new(&mut snap_state.editing_fade_time, 0.0..=60.0)
                                                .suffix(" s")
                                                .step_by(0.1),
                                        );
                                    });

                                    ui.checkbox(&mut snap_state.editing_scope_override_enabled, "Scope Override");

                                    if snap_state.editing_scope_override_enabled {
                                        ui.horizontal(|ui| {
                                            ui.label("Template:");
                                            let current_name = snap_state.editing_scope_template_id
                                                .and_then(|id| mgr.scope_templates.get(&id))
                                                .map(|t| t.name.clone())
                                                .unwrap_or_else(|| "(select)".into());

                                            egui::ComboBox::from_id_salt("scope_override_selector")
                                                .selected_text(&current_name)
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
                                    }

                                    ui.label("Notes:");
                                    ui.add(
                                        egui::TextEdit::multiline(&mut snap_state.editing_cue_notes)
                                            .desired_rows(2)
                                            .desired_width(f32::INFINITY),
                                    );

                                    let save_btn = theme::action_button("Save Cue Changes", theme::ACCENT_GREEN, egui::Vec2::new(140.0, 28.0));
                                    if ui.add(save_btn).clicked() {
                                        let fade_time = snap_state.editing_fade_time;
                                        let scope_override = if snap_state.editing_scope_override_enabled {
                                            snap_state.editing_scope_template_id
                                                .and_then(|id| mgr.scope_templates.get(&id).cloned())
                                        } else {
                                            None
                                        };
                                        let notes = snap_state.editing_cue_notes.clone();
                                        let cue_mgr = cue_manager.clone();
                                        runtime.spawn(async move {
                                            cue_mgr.write().await.update_cue(cue_id, fade_time, scope_override, notes);
                                        });
                                        snap_state.status_message = Some("Cue updated".into());
                                    }
                                }
                            }
                        });
                    }

                    ui.add_space(8.0);

                    // ── Snapshots card ──
                    theme::card_frame().show(ui, |ui| {
                        theme::section_heading(ui, "Snapshots");

                        // Snapshot kind picker — controls whether the scope is
                        // applied at SAVE time (only in-scope params stored,
                        // current behaviour) or at RECALL time (all params
                        // captured; scope filters at recall, with the option
                        // to "Recall without scope" to restore the entire
                        // saved state in one shot).
                        ui.horizontal(|ui| {
                            ui.label("Apply scope:");
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
                            ui.label("Name:");
                            ui.add(egui::TextEdit::singleline(&mut snap_state.new_snapshot_name).desired_width(150.0));

                            let scope_required = matches!(
                                snap_state.pending_kind,
                                SnapshotKind::ApplyOnSave
                            );
                            let scope_ok = !scope_required
                                || snap_state.scope_editor.selection_count() > 0;
                            let can_capture = is_connected
                                && !snap_state.new_snapshot_name.is_empty()
                                && scope_ok;

                            let capture_btn = theme::action_button("Capture Now", theme::ACCENT_GREEN, egui::Vec2::new(110.0, 28.0));
                            if ui.add_enabled(can_capture, capture_btn).clicked() {
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

                        if !is_connected {
                            ui.label(egui::RichText::new("Connect to console to capture snapshots.").color(theme::TEXT_SECONDARY));
                        } else if matches!(snap_state.pending_kind, SnapshotKind::ApplyOnSave)
                            && snap_state.scope_editor.selection_count() == 0
                        {
                            ui.label(egui::RichText::new("Select scope parameters to capture (or switch to 'On recall').").color(theme::TEXT_SECONDARY));
                        }

                        ui.add_space(8.0);

                        // Snapshot list
                        egui::ScrollArea::vertical()
                            .id_salt("snapshot_list_scroll")
                            .max_height(180.0)
                            .show(ui, |ui| {
                                if let Ok(mgr) = cue_manager.try_read() {
                                    let mut snapshots: Vec<_> = mgr.snapshots.values().collect();
                                    snapshots.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));

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

                            let recall_btn = theme::action_button("Recall", theme::ACCENT_GREEN, egui::Vec2::new(80.0, 28.0));
                            if ui.add_enabled(has_selection && engine_ready, recall_btn).clicked() {
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
                            let recall_no_scope_btn = theme::action_button(
                                "Recall (no scope)",
                                theme::ACCENT_AMBER,
                                egui::Vec2::new(140.0, 28.0),
                            );
                            let recall_no_scope_resp = ui.add_enabled(
                                has_selection && engine_ready && can_recall_no_scope,
                                recall_no_scope_btn,
                            );
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

                            let recapture_btn = theme::action_button("Re-capture", theme::ACCENT_BLUE, egui::Vec2::new(100.0, 28.0));
                            if ui.add_enabled(has_selection && is_connected, recapture_btn).clicked() {
                                recapture_snapshot(snap_state, console_state, cue_manager, dirty_tracker, runtime, ui_tx);
                            }
                            let del_btn = theme::action_button("Delete", theme::ACCENT_RED, egui::Vec2::new(80.0, 28.0));
                            if ui.add_enabled(has_selection, del_btn).clicked() {
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

                            let undo_btn = theme::action_button(
                                &undo_label,
                                theme::ACCENT_AMBER,
                                egui::Vec2::new(160.0, 28.0),
                            );
                            if ui.add_enabled(has_undo && engine_ready, undo_btn).clicked() {
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
                            ui.label("Pace (μs):");
                            let mut pace_val = snapshot_engine
                                .as_ref()
                                .map(|e| e.pace_us() as f32)
                                .unwrap_or(0.0);
                            let slider = egui::Slider::new(&mut pace_val, 0.0..=5000.0)
                                .step_by(100.0)
                                .clamping(egui::SliderClamping::Always);
                            if ui.add_enabled(engine_ready, slider).changed() {
                                if let Some(engine) = snapshot_engine.as_ref() {
                                    engine.set_pace_us(pace_val as u64);
                                }
                            }
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

                            let trigger_btn = theme::action_button(
                                "Create Trigger Cue in QLab",
                                theme::ACCENT_BLUE,
                                egui::Vec2::new(200.0, 28.0),
                            );
                            if ui.add_enabled(has_selection, trigger_btn).clicked() {
                                qlab_create_trigger_cue(
                                    snap_state,
                                    cue_manager,
                                    qlab_ip_owned.clone(),
                                    qlab_port_local,
                                    runtime,
                                    ui_tx,
                                );
                            }

                            let export_btn = theme::action_button(
                                "Export to QLab",
                                theme::ACCENT_BLUE,
                                egui::Vec2::new(140.0, 28.0),
                            );
                            if ui.add_enabled(has_selection, export_btn).clicked() {
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

                            ui.label(
                                egui::RichText::new(format!("→ {qlab_ip}:{qlab_port}"))
                                    .color(theme::TEXT_SECONDARY)
                                    .small(),
                            );
                        });
                    });

                    // Status message
                    if let Some(msg) = &snap_state.status_message {
                        ui.add_space(4.0);
                        ui.colored_label(theme::TEXT_WARNING, msg);
                    }

                    ui.add_space(8.0);

                    // ── Palettes section (EQ / Compressor / Gate) ──
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
        });
    });
}

fn capture_snapshot(
    snap_state: &mut SnapshotsTabState,
    console_state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    dirty_tracker: &Arc<RwLock<DirtyTracker>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let scope = snap_state.scope_editor.to_scope_template(
        snap_state.new_snapshot_name.clone(),
    );
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

        let _ = tx.send(UiEvent::SnapshotCaptured {
            name,
            param_count,
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
    let Some(snap_id) = snap_state.selected_snapshot_id else { return };

    let st = console_state.clone();
    let cue_mgr = cue_manager.clone();
    let dirty = dirty_tracker.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        // Read the existing snapshot's scope AND its kind so re-capture
        // honours the original ApplyOnSave / ApplyOnRecall semantics.
        let mgr = cue_mgr.read().await;
        let Some(existing) = mgr.snapshots.get(&snap_id) else { return };
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

        let _ = tx.send(UiEvent::SnapshotCaptured {
            name,
            param_count,
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
    let Some(snap_id) = snap_state.selected_snapshot_id else { return };
    let Some(engine) = snapshot_engine.clone() else { return };
    let cue_mgr = cue_manager.clone();
    let pmgr = palette_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        // Read the snapshot + scope under the cue-manager lock.
        let mgr = cue_mgr.read().await;
        let Some(snapshot) = mgr.snapshots.get(&snap_id).cloned() else { return };
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
            format!("Recalled '{name}' without scope ({} params sent)", result.parameters_sent)
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
    let Some(snap_id) = snap_state.selected_snapshot_id else { return };
    let cue_mgr = cue_manager.clone();
    let pal_mgr = palette_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        // Pull the snapshot under the cue-manager lock.
        let mgr = cue_mgr.read().await;
        let Some(snapshot) = mgr.snapshots.get(&snap_id).cloned() else { return };
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
    let Some(snap_id) = snap_state.selected_snapshot_id else { return };
    let cue_mgr = cue_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        let mgr = cue_mgr.read().await;
        let Some(snapshot) = mgr.snapshots.get(&snap_id).cloned() else { return };
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
