use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::console::cue_manager::CueManager;
use crate::console::eq_palette_manager::EqPaletteManager;
use crate::model::snapshot::{Cue, Snapshot};
use crate::model::state::ConsoleState;
use super::eq_palettes_ui::{EqPalettesUiState, draw_eq_palettes_section};
use super::scope_editor::{ScopeEditorState, draw_scope_editor};
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
    eq_palettes_ui: &mut EqPalettesUiState,
    console_state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
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
                                    snap_state.scope_editor = ScopeEditorState::from_scope_template(tmpl);
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

                    // Scope editor widget
                    theme::card_frame().show(ui, |ui| {
                        theme::section_heading(ui, "Recall Scope");
                        draw_scope_editor(
                            ui,
                            &mut snap_state.scope_editor,
                            input_count,
                            aux_count,
                            group_count,
                        );
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

                        // Capture controls
                        ui.horizontal(|ui| {
                            ui.label("Name:");
                            ui.add(egui::TextEdit::singleline(&mut snap_state.new_snapshot_name).desired_width(150.0));

                            let can_capture = is_connected && !snap_state.new_snapshot_name.is_empty()
                                && snap_state.scope_editor.selection_count() > 0;

                            let capture_btn = theme::action_button("Capture Now", theme::ACCENT_GREEN, egui::Vec2::new(110.0, 28.0));
                            if ui.add_enabled(can_capture, capture_btn).clicked() {
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
                            ui.label(egui::RichText::new("Connect to console to capture snapshots.").color(theme::TEXT_SECONDARY));
                        } else if snap_state.scope_editor.selection_count() == 0 {
                            ui.label(egui::RichText::new("Select scope channels/sections to capture.").color(theme::TEXT_SECONDARY));
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

                        // Re-capture and delete snapshot buttons
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let has_selection = snap_state.selected_snapshot_id.is_some();
                            let recapture_btn = theme::action_button("Re-capture", theme::ACCENT_BLUE, egui::Vec2::new(100.0, 28.0));
                            if ui.add_enabled(has_selection && is_connected, recapture_btn).clicked() {
                                recapture_snapshot(snap_state, console_state, cue_manager, runtime, ui_tx);
                            }
                            let del_btn = theme::action_button("Delete", theme::ACCENT_RED, egui::Vec2::new(80.0, 28.0));
                            if ui.add_enabled(has_selection, del_btn).clicked() {
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
                    });

                    // Status message
                    if let Some(msg) = &snap_state.status_message {
                        ui.add_space(4.0);
                        ui.colored_label(theme::TEXT_WARNING, msg);
                    }

                    ui.add_space(8.0);

                    // ── EQ Palettes section ──
                    theme::card_frame().show(ui, |ui| {
                        draw_eq_palettes_section(
                            ui,
                            eq_palettes_ui,
                            console_state,
                            cue_manager,
                            eq_palette_manager,
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
