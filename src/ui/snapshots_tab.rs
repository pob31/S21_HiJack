use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::console::cue_manager::CueManager;
use crate::model::snapshot::{Cue, Snapshot};
use crate::model::state::ConsoleState;
use super::scope_editor::{ScopeEditorState, draw_scope_editor};
use super::UiEvent;

/// State for the Snapshots tab.
pub struct SnapshotsTabState {
    // Cue management
    pub selected_cue_id: Option<Uuid>,
    pub new_cue_number: String,
    pub new_cue_name: String,
    pub selected_snapshot_for_cue: Option<Uuid>,

    // Snapshot management
    pub new_snapshot_name: String,
    pub selected_snapshot_id: Option<Uuid>,

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
            new_snapshot_name: String::new(),
            selected_snapshot_id: None,
            scope_editor: ScopeEditorState::default(),
            new_template_name: String::new(),
            selected_scope_template_id: None,
            status_message: None,
        }
    }
}

/// Draw the Snapshots tab.
pub fn draw_snapshots_tab(
    ui: &mut egui::Ui,
    snap_state: &mut SnapshotsTabState,
    console_state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    connected: &Arc<AtomicBool>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let is_connected = connected.load(Ordering::Relaxed);

    // Read channel counts for scope editor
    let (input_count, aux_count, group_count) = if let Ok(st) = console_state.try_read() {
        (
            st.config.input_channel_count,
            st.config.aux_output_count,
            st.config.group_output_count,
        )
    } else {
        (48, 8, 16) // defaults
    };

    // Two-column layout
    let available = ui.available_size();
    let left_width = (available.x * 0.4).min(400.0);

    ui.horizontal(|ui| {
        // Left panel: Scope templates + editor
        ui.vertical(|ui| {
            ui.set_width(left_width);

            // Scope template list
            ui.heading("Scope Templates");
            if let Ok(mgr) = cue_manager.try_read() {
                let mut templates: Vec<_> = mgr.scope_templates.values().collect();
                templates.sort_by(|a, b| a.name.cmp(&b.name));
                for tmpl in templates {
                    let selected = snap_state.selected_scope_template_id == Some(tmpl.id);
                    if ui.selectable_label(selected, format!("{} ({} ch)", tmpl.name, tmpl.channel_scopes.len())).clicked() {
                        snap_state.selected_scope_template_id = Some(tmpl.id);
                        snap_state.scope_editor = ScopeEditorState::from_scope_template(tmpl);
                    }
                }
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut snap_state.new_template_name);
                if ui.button("Save Template").clicked() && !snap_state.new_template_name.is_empty() {
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

            ui.separator();

            // Scope editor widget
            draw_scope_editor(
                ui,
                &mut snap_state.scope_editor,
                input_count,
                aux_count,
                group_count,
            );
        });

        ui.separator();

        // Right panel: Cue list + snapshots
        ui.vertical(|ui| {
            // Cue list
            ui.heading("Cue List");

            egui::ScrollArea::vertical()
                .id_salt("cue_list_scroll")
                .max_height(available.y * 0.4)
                .show(ui, |ui| {
                    if let Ok(mgr) = cue_manager.try_read() {
                        // Header
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("#").strong().monospace());
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("Name").strong());
                            ui.add_space(60.0);
                            ui.label(egui::RichText::new("Snapshot").strong());
                        });
                        ui.separator();

                        for cue in &mgr.cue_list.cues {
                            let selected = snap_state.selected_cue_id == Some(cue.id);
                            let snap_name = mgr
                                .snapshots
                                .get(&cue.snapshot_id)
                                .map(|s| s.name.as_str())
                                .unwrap_or("?");

                            let text = format!("{:<6.1}  {:<20}  {}", cue.cue_number, cue.name, snap_name);
                            if ui.selectable_label(selected, egui::RichText::new(&text).monospace()).clicked() {
                                snap_state.selected_cue_id = Some(cue.id);
                            }
                        }

                        if mgr.cue_list.cues.is_empty() {
                            ui.weak("No cues yet. Add one below.");
                        }
                    }
                });

            ui.add_space(4.0);

            // Add/delete cue controls
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

                if ui.button("Add Cue").clicked() {
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

                if ui.button("Delete Cue").clicked() {
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

            ui.add_space(12.0);
            ui.separator();

            // Snapshot management
            ui.heading("Snapshots");

            // Capture controls
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.add(egui::TextEdit::singleline(&mut snap_state.new_snapshot_name).desired_width(150.0));

                let can_capture = is_connected && !snap_state.new_snapshot_name.is_empty()
                    && snap_state.scope_editor.selection_count() > 0;

                if ui.add_enabled(can_capture, egui::Button::new("Capture Now")).clicked() {
                    capture_snapshot(
                        snap_state,
                        console_state,
                        cue_manager,
                        runtime,
                        ui_tx,
                    );
                }
            });

            if !is_connected {
                ui.weak("Connect to console to capture snapshots.");
            } else if snap_state.scope_editor.selection_count() == 0 {
                ui.weak("Select scope channels/sections to capture.");
            }

            ui.add_space(8.0);

            // Snapshot list
            egui::ScrollArea::vertical()
                .id_salt("snapshot_list_scroll")
                .show(ui, |ui| {
                    if let Ok(mgr) = cue_manager.try_read() {
                        let mut snapshots: Vec<_> = mgr.snapshots.values().collect();
                        snapshots.sort_by(|a, b| b.modified_at.cmp(&a.modified_at));

                        for snap in snapshots {
                            let selected = snap_state.selected_snapshot_id == Some(snap.id);
                            let label = format!(
                                "{} ({} params, {})",
                                snap.name,
                                snap.data.parameter_count(),
                                snap.modified_at.format("%H:%M:%S"),
                            );
                            if ui.selectable_label(selected, &label).clicked() {
                                snap_state.selected_snapshot_id = Some(snap.id);
                            }
                        }

                        if mgr.snapshots.is_empty() {
                            ui.weak("No snapshots yet.");
                        }
                    }
                });

            // Re-capture and delete snapshot buttons
            ui.horizontal(|ui| {
                let has_selection = snap_state.selected_snapshot_id.is_some();
                if ui.add_enabled(has_selection && is_connected, egui::Button::new("Re-capture")).clicked() {
                    recapture_snapshot(snap_state, console_state, cue_manager, runtime, ui_tx);
                }
                if ui.add_enabled(has_selection, egui::Button::new("Delete Snapshot")).clicked() {
                    if let Some(id) = snap_state.selected_snapshot_id {
                        let cue_mgr = cue_manager.clone();
                        runtime.spawn(async move {
                            cue_mgr.write().await.remove_snapshot(id);
                        });
                        snap_state.selected_snapshot_id = None;
                        snap_state.status_message = Some("Snapshot deleted".into());
                    }
                }
            });

            // Status message
            if let Some(msg) = &snap_state.status_message {
                ui.add_space(4.0);
                ui.colored_label(egui::Color32::YELLOW, msg);
            }
        });
    });
}

fn capture_snapshot(
    snap_state: &mut SnapshotsTabState,
    console_state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let scope = snap_state.scope_editor.to_scope_template(
        snap_state.new_snapshot_name.clone(),
    );
    let name = snap_state.new_snapshot_name.clone();
    let st = console_state.clone();
    let cue_mgr = cue_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        let state_guard = st.read().await;
        let data = state_guard.capture(&scope);
        let param_count = data.parameter_count();
        drop(state_guard);

        let snapshot = Snapshot::new(name.clone(), scope, data);
        cue_mgr.write().await.add_snapshot(snapshot);

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
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let Some(snap_id) = snap_state.selected_snapshot_id else { return };

    let st = console_state.clone();
    let cue_mgr = cue_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        // Read the existing snapshot's scope
        let mgr = cue_mgr.read().await;
        let Some(existing) = mgr.snapshots.get(&snap_id) else { return };
        let scope = existing.scope.clone();
        let name = existing.name.clone();
        drop(mgr);

        // Capture fresh data
        let state_guard = st.read().await;
        let data = state_guard.capture(&scope);
        let param_count = data.parameter_count();
        drop(state_guard);

        // Update
        cue_mgr.write().await.update_snapshot(snap_id, data);

        let _ = tx.send(UiEvent::SnapshotCaptured {
            name,
            param_count,
        });
    });

    snap_state.status_message = Some("Re-capturing...".into());
}
