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
    pub fade_progress: Option<(f32, f32)>, // (cue_number, progress 0.0..1.0)
}

impl Default for LiveTabState {
    fn default() -> Self {
        Self {
            last_recall_info: None,
            fade_progress: None,
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

    ui.vertical_centered(|ui| {
        ui.add_space(12.0);

        // ── Current Cue card (DiGiCo dark maroon) ──
        let current_text = if let Ok(mgr) = cue_manager.try_read() {
            if let Some(cue) = mgr.current_cue() {
                format!("{:.1}  —  {}", cue.cue_number, cue.name)
            } else {
                "—".to_string()
            }
        } else {
            "...".to_string()
        };

        ui.label(
            egui::RichText::new("CURRENT CUE")
                .strong()
                .size(theme::FONT_SIZE_BODY)
                .color(theme::TEXT_SECONDARY),
        );
        ui.add_space(4.0);

        egui::Frame::new()
            .fill(theme::CUE_CURRENT_BG)
            .stroke(egui::Stroke::new(1.0, theme::CUE_CURRENT_BORDER))
            .inner_margin(egui::Margin::symmetric(24, 20))
            .corner_radius(8.0)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(&current_text)
                        .size(theme::FONT_SIZE_CUE_CURRENT)
                        .strong()
                        .monospace()
                        .color(theme::TEXT_PRIMARY),
                );
            });

        ui.add_space(16.0);

        // ── Next Cue card ──
        let next_text = if let Ok(mgr) = cue_manager.try_read() {
            if let Some(cue) = mgr.next_cue() {
                format!("{:.1}  —  {}", cue.cue_number, cue.name)
            } else {
                "—".to_string()
            }
        } else {
            "...".to_string()
        };

        ui.label(
            egui::RichText::new("NEXT CUE")
                .size(theme::FONT_SIZE_BADGE)
                .color(theme::TEXT_SECONDARY),
        );
        ui.add_space(2.0);

        egui::Frame::new()
            .fill(theme::BG_ELEVATED)
            .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE))
            .inner_margin(egui::Margin::symmetric(20, 12))
            .corner_radius(8.0)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(&next_text)
                        .size(theme::FONT_SIZE_CUE_NEXT)
                        .monospace()
                        .color(theme::TEXT_PRIMARY),
                );
            });

        ui.add_space(24.0);

        // ── GO and PREV buttons ──
        let has_cues = cue_manager
            .try_read()
            .map(|mgr| !mgr.cue_list.cues.is_empty())
            .unwrap_or(false);
        let buttons_enabled = is_connected && has_cues;

        ui.horizontal(|ui| {
            ui.add_space((ui.available_width() - theme::GO_BUTTON_SIZE.x - theme::PREV_BUTTON_SIZE.x - 20.0).max(0.0) / 2.0);

            // GO button
            let go_color = if buttons_enabled { theme::COLOR_GO_BUTTON } else { theme::BG_ELEVATED };
            let go_button = egui::Button::new(
                egui::RichText::new("GO")
                    .size(theme::FONT_SIZE_GO_BUTTON)
                    .strong()
                    .color(theme::TEXT_PRIMARY),
            )
            .fill(go_color)
            .min_size(theme::GO_BUTTON_SIZE)
            .corner_radius(8.0);

            if ui.add_enabled(buttons_enabled, go_button).clicked() {
                fire_go(cue_manager, eq_palette_manager, snapshot_engine, runtime, ui_tx);
            }

            ui.add_space(20.0);

            // PREV button
            let prev_color = if buttons_enabled { theme::COLOR_PREV_BUTTON } else { theme::BG_ELEVATED };
            let prev_button = egui::Button::new(
                egui::RichText::new("PREV")
                    .size(theme::FONT_SIZE_BODY)
                    .strong()
                    .color(theme::TEXT_PRIMARY),
            )
            .fill(prev_color)
            .min_size(theme::PREV_BUTTON_SIZE)
            .corner_radius(8.0);

            if ui.add_enabled(buttons_enabled, prev_button).clicked() {
                fire_prev(cue_manager, eq_palette_manager, snapshot_engine, runtime, ui_tx);
            }
        });

        ui.add_space(12.0);

        // Last recall result
        if let Some(info) = &live.last_recall_info {
            ui.label(egui::RichText::new(info).color(theme::TEXT_SECONDARY));
        }

        // Fade progress bar
        if let Some((cue_num, progress)) = &live.fade_progress {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("Fade {cue_num:.1}:"))
                        .color(theme::TEXT_SECONDARY),
                );
                ui.add(
                    egui::ProgressBar::new(*progress)
                        .text(format!("{:.0}%", progress * 100.0))
                        .fill(theme::ACCENT_GREEN),
                );
            });
        }

        // ── Macro Quick-Trigger section ──
        if let Ok(mgr) = macro_manager.try_read() {
            if !mgr.quick_trigger_ids.is_empty() {
                ui.add_space(16.0);

                theme::card_frame().show(ui, |ui| {
                    theme::section_heading(ui, "Quick Macros");

                    // Collect quick-trigger macro info (id, name)
                    let qt_macros: Vec<_> = mgr.quick_trigger_ids.iter()
                        .filter_map(|id| mgr.get_macro(id).map(|m| (m.id, m.name.clone())))
                        .collect();
                    drop(mgr);

                    ui.horizontal_wrapped(|ui| {
                        for (id, name) in &qt_macros {
                            let btn_color = if is_connected { theme::COLOR_MACRO_BUTTON } else { theme::BG_ELEVATED };
                            let button = egui::Button::new(
                                egui::RichText::new(name)
                                    .color(theme::TEXT_PRIMARY)
                                    .strong(),
                            )
                            .fill(btn_color)
                            .min_size(theme::MACRO_BUTTON_SIZE)
                            .corner_radius(6.0);

                            if ui.add_enabled(is_connected, button).clicked() {
                                super::macros_tab::fire_macro_by_id(
                                    *id, macro_manager, macro_engine, runtime, ui_tx,
                                );
                            }
                        }
                    });
                });
            }
        }

        // Status hints
        if !is_connected {
            ui.add_space(8.0);
            ui.colored_label(theme::COLOR_DISCONNECTED, "No console connected");
        } else if !has_cues {
            ui.add_space(8.0);
            ui.label(egui::RichText::new("No cues loaded").color(theme::TEXT_SECONDARY));
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
