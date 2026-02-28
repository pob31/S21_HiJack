use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tokio::sync::RwLock;

use crate::console::cue_manager::CueManager;
use crate::console::eq_palette_manager::EqPaletteManager;
use crate::console::macro_engine::MacroEngine;
use crate::console::macro_manager::MacroManager;
use crate::console::snapshot_engine::SnapshotEngine;
use super::theme;
use super::UiEvent;

/// State for the Live tab.
pub struct LiveTabState {
    pub last_recall_info: Option<String>,
}

impl Default for LiveTabState {
    fn default() -> Self {
        Self {
            last_recall_info: None,
        }
    }
}

/// Draw the Live tab.
pub fn draw_live_tab(
    ui: &mut egui::Ui,
    live: &mut LiveTabState,
    cue_manager: &Arc<RwLock<CueManager>>,
    macro_manager: &Arc<RwLock<MacroManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
    snapshot_engine: &Option<Arc<SnapshotEngine>>,
    macro_engine: &Option<Arc<MacroEngine>>,
    connected: &Arc<AtomicBool>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let is_connected = connected.load(Ordering::Relaxed);

    // Top bar: title + connection status
    ui.horizontal(|ui| {
        ui.heading("S21 HiJack — Live");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (color, text) = if is_connected {
                (theme::COLOR_CONNECTED, "Connected")
            } else {
                (theme::COLOR_DISCONNECTED, "Disconnected")
            };
            ui.colored_label(color, text);
            let circle_size = 12.0;
            let (rect, _) = ui.allocate_exact_size(
                egui::Vec2::splat(circle_size),
                egui::Sense::hover(),
            );
            ui.painter().circle_filled(rect.center(), circle_size / 2.0, color);
        });
    });

    ui.add_space(20.0);

    // Current cue
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new("CURRENT CUE").strong().size(theme::FONT_SIZE_CUE_NEXT));

        let current_text = if let Ok(mgr) = cue_manager.try_read() {
            if let Some(cue) = mgr.current_cue() {
                format!("{:.1}  —  {}", cue.cue_number, cue.name)
            } else {
                "—".to_string()
            }
        } else {
            "...".to_string()
        };

        egui::Frame::new()
            .fill(ui.style().visuals.extreme_bg_color)
            .inner_margin(egui::Margin::symmetric(20, 16))
            .corner_radius(8.0)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(&current_text)
                        .size(theme::FONT_SIZE_CUE_CURRENT)
                        .strong()
                        .monospace(),
                );
            });

        ui.add_space(16.0);

        // Next cue
        ui.label(egui::RichText::new("NEXT CUE").size(theme::FONT_SIZE_BODY));

        let next_text = if let Ok(mgr) = cue_manager.try_read() {
            if let Some(cue) = mgr.next_cue() {
                format!("{:.1}  —  {}", cue.cue_number, cue.name)
            } else {
                "—".to_string()
            }
        } else {
            "...".to_string()
        };

        egui::Frame::new()
            .fill(ui.style().visuals.faint_bg_color)
            .inner_margin(egui::Margin::symmetric(20, 12))
            .corner_radius(8.0)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(&next_text)
                        .size(theme::FONT_SIZE_CUE_NEXT)
                        .monospace(),
                );
            });

        ui.add_space(24.0);

        // GO and PREV buttons
        let has_cues = cue_manager
            .try_read()
            .map(|mgr| !mgr.cue_list.cues.is_empty())
            .unwrap_or(false);
        let buttons_enabled = is_connected && has_cues;

        ui.horizontal(|ui| {
            ui.add_space((ui.available_width() - theme::GO_BUTTON_SIZE.x - theme::PREV_BUTTON_SIZE.x - 20.0).max(0.0) / 2.0);

            // GO button
            let go_button = egui::Button::new(
                egui::RichText::new("GO")
                    .size(theme::FONT_SIZE_GO_BUTTON)
                    .strong()
                    .color(egui::Color32::WHITE),
            )
            .fill(if buttons_enabled { theme::COLOR_GO_BUTTON } else { egui::Color32::DARK_GRAY })
            .min_size(theme::GO_BUTTON_SIZE);

            if ui.add_enabled(buttons_enabled, go_button).clicked() {
                fire_go(cue_manager, eq_palette_manager, snapshot_engine, runtime, ui_tx);
            }

            ui.add_space(20.0);

            // PREV button
            let prev_button = egui::Button::new(
                egui::RichText::new("PREV")
                    .size(theme::FONT_SIZE_BODY)
                    .strong()
                    .color(egui::Color32::WHITE),
            )
            .fill(if buttons_enabled { theme::COLOR_PREV_BUTTON } else { egui::Color32::DARK_GRAY })
            .min_size(theme::PREV_BUTTON_SIZE);

            if ui.add_enabled(buttons_enabled, prev_button).clicked() {
                fire_prev(cue_manager, eq_palette_manager, snapshot_engine, runtime, ui_tx);
            }
        });

        ui.add_space(16.0);

        // Last recall result
        if let Some(info) = &live.last_recall_info {
            ui.label(egui::RichText::new(info).weak());
        }

        // Macro quick-trigger buttons
        if let Ok(mgr) = macro_manager.try_read() {
            if !mgr.quick_trigger_ids.is_empty() {
                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);
                ui.label(egui::RichText::new("MACROS").strong().size(theme::FONT_SIZE_BODY));
                ui.add_space(4.0);

                // Collect quick-trigger macro info (id, name)
                let qt_macros: Vec<_> = mgr.quick_trigger_ids.iter()
                    .filter_map(|id| mgr.get_macro(id).map(|m| (m.id, m.name.clone())))
                    .collect();
                drop(mgr);

                ui.horizontal_wrapped(|ui| {
                    for (id, name) in &qt_macros {
                        let button = egui::Button::new(
                            egui::RichText::new(name)
                                .color(egui::Color32::WHITE)
                                .strong(),
                        )
                        .fill(if is_connected { theme::COLOR_MACRO_BUTTON } else { egui::Color32::DARK_GRAY })
                        .min_size(theme::MACRO_BUTTON_SIZE);

                        if ui.add_enabled(is_connected, button).clicked() {
                            super::macros_tab::fire_macro_by_id(
                                *id, macro_manager, macro_engine, runtime, ui_tx,
                            );
                        }
                    }
                });
            }
        }

        if !is_connected {
            ui.add_space(8.0);
            ui.colored_label(theme::COLOR_DISCONNECTED, "No console connected");
        } else if !has_cues {
            ui.add_space(8.0);
            ui.weak("No cues loaded");
        }
    });
}

fn fire_go(
    cue_manager: &Arc<RwLock<CueManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
    snapshot_engine: &Option<Arc<SnapshotEngine>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let Some(engine) = snapshot_engine.clone() else { return };
    let cue_mgr = cue_manager.clone();
    let eq_mgr = eq_palette_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        let mut mgr = cue_mgr.write().await;
        if let Some(cue) = mgr.go_next() {
            let cue = cue.clone();
            if let Some(snapshot) = mgr.get_snapshot(&cue.snapshot_id).cloned() {
                drop(mgr);
                let pmgr = eq_mgr.read().await;
                let result = engine.recall_cue(&cue, &snapshot, &pmgr.palettes).await;
                let _ = tx.send(UiEvent::CueRecalled {
                    cue_number: cue.cue_number,
                    params_sent: result.parameters_sent,
                });
            }
        }
    });
}

fn fire_prev(
    cue_manager: &Arc<RwLock<CueManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
    snapshot_engine: &Option<Arc<SnapshotEngine>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let Some(engine) = snapshot_engine.clone() else { return };
    let cue_mgr = cue_manager.clone();
    let eq_mgr = eq_palette_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        let mut mgr = cue_mgr.write().await;
        if let Some(cue) = mgr.go_previous() {
            let cue = cue.clone();
            if let Some(snapshot) = mgr.get_snapshot(&cue.snapshot_id).cloned() {
                drop(mgr);
                let pmgr = eq_mgr.read().await;
                let result = engine.recall_cue(&cue, &snapshot, &pmgr.palettes).await;
                let _ = tx.send(UiEvent::CueRecalled {
                    cue_number: cue.cue_number,
                    params_sent: result.parameters_sent,
                });
            }
        }
    });
}
