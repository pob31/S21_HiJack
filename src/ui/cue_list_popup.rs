//! Cue-list popup — a floating window opened from the top-bar transport that
//! lists every cue and lets the operator jump straight to any of them (instead
//! of stepping PREV/GO one at a time). Each row has a Fire button that makes
//! that cue current and recalls it via [`super::cue_transport::fire_cue_by_id`].

use std::sync::Arc;

use eframe::egui;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::UiEvent;
use super::theme;
use crate::console::cue_manager::CueManager;
use crate::console::palette_manager::PaletteManager;
use crate::console::snapshot_engine::SnapshotEngine;

/// One cue's display data, snapshotted out from under the cue-manager lock so
/// we don't hold the guard across the render closure.
struct CueRow {
    id: Uuid,
    number: f32,
    name: String,
    console_snapshot: Option<i32>,
    snapshot_name: Option<String>,
    is_current: bool,
}

/// Draw the cue-list popup if `open`. `open` is the standard egui pattern: the
/// window's title-bar `×` clears it.
#[allow(clippy::too_many_arguments)]
pub fn draw_cue_list_popup(
    ctx: &egui::Context,
    open: &mut bool,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    snapshot_engine: &Option<Arc<SnapshotEngine>>,
    is_connected: bool,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    if !*open {
        return;
    }

    // Snapshot the cue rows (cheap clone) so the render closure holds no lock.
    let Ok(mgr) = cue_manager.try_read() else {
        return; // lock busy this frame; redraw next frame
    };
    let current_id = mgr.current_cue().map(|c| c.id);
    let rows: Vec<CueRow> = mgr
        .cue_list
        .cues
        .iter()
        .map(|c| CueRow {
            id: c.id,
            number: c.cue_number,
            name: c.name.clone(),
            console_snapshot: c.console_snapshot,
            snapshot_name: c
                .snapshot_id
                .and_then(|sid| mgr.get_snapshot(&sid))
                .map(|s| s.name.clone()),
            is_current: Some(c.id) == current_id,
        })
        .collect();
    drop(mgr);

    let mut fire_id: Option<Uuid> = None;

    egui::Window::new("Cue List")
        .open(open)
        .collapsible(false)
        .resizable(true)
        .default_size([460.0, 420.0])
        .show(ctx, |ui| {
            if rows.is_empty() {
                ui.label(egui::RichText::new("No cues yet.").color(theme::TEXT_SECONDARY));
                return;
            }
            if !is_connected {
                ui.label(
                    egui::RichText::new("Connect to the console to fire cues.")
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
                ui.add_space(4.0);
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for row in &rows {
                        let bg = if row.is_current {
                            theme::CUE_CURRENT_BG
                        } else {
                            theme::BG_PANEL
                        };
                        egui::Frame::new()
                            .fill(bg)
                            .corner_radius(4.0)
                            .inner_margin(egui::Margin::symmetric(8, 4))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let fire = egui::Button::new(
                                        egui::RichText::new("Fire")
                                            .color(theme::TEXT_PRIMARY)
                                            .strong(),
                                    )
                                    .fill(theme::ACCENT_GREEN)
                                    .corner_radius(4.0)
                                    .min_size(egui::vec2(54.0, 22.0));
                                    if ui
                                        .add_enabled(is_connected, fire)
                                        .on_hover_text("Jump to this cue and recall it.")
                                        .clicked()
                                    {
                                        fire_id = Some(row.id);
                                    }

                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new(format!("Cue {:.1}", row.number))
                                            .strong()
                                            .color(if row.is_current {
                                                theme::ACCENT_BLUE
                                            } else {
                                                theme::TEXT_PRIMARY
                                            }),
                                    );
                                    if !row.name.is_empty() {
                                        ui.label(
                                            egui::RichText::new(&row.name)
                                                .color(theme::TEXT_PRIMARY),
                                        );
                                    }

                                    // Right-aligned meta: console row + overlay snapshot.
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let mut meta = String::new();
                                            if let Some(r) = row.console_snapshot {
                                                meta.push_str(&format!("row {r}"));
                                            }
                                            if let Some(n) = &row.snapshot_name {
                                                if !meta.is_empty() {
                                                    meta.push_str("  ·  ");
                                                }
                                                meta.push_str(n);
                                            }
                                            if !meta.is_empty() {
                                                ui.label(
                                                    egui::RichText::new(meta)
                                                        .color(theme::TEXT_SECONDARY)
                                                        .small(),
                                                );
                                            }
                                        },
                                    );
                                });
                            });
                        ui.add_space(2.0);
                    }
                });
        });

    if let Some(id) = fire_id {
        super::cue_transport::fire_cue_by_id(
            id,
            cue_manager,
            palette_manager,
            snapshot_engine,
            runtime,
            ui_tx,
        );
    }
}
