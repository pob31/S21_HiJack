//! Advanced Settings popup window.
//!
//! Lives in a single `egui::Window` opened from the Setup tab's
//! bottom-left "Advanced…" button. Holds application-level preferences
//! that don't belong with the per-show connection settings:
//!
//! - **Pacing** — inter-message OSC delay shared between snapshot
//!   recall and macro execution. Read by `SnapshotEngine` and
//!   `MacroEngine` from the same `Arc<AtomicU64>` so the slider here
//!   immediately affects both.
//! - **Show diagnostic tabs** — toggle for the OSC Log + Inspector tabs.
//!
//! Future controls (color theme, translated help bubbles) slot into
//! the placeholder section at the bottom.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use eframe::egui;

use super::setup_tab::{SetupTabState, save_app_preferences};
use super::theme;

/// Render the Advanced Settings window. `open` is the standard
/// egui pattern: passing `&mut bool` lets the window manage its own
/// close button via the title-bar `×`.
pub fn draw_advanced_settings_window(
    ctx: &egui::Context,
    open: &mut bool,
    setup: &mut SetupTabState,
    send_pace_us: &Arc<AtomicU64>,
) {
    if !*open {
        return;
    }
    egui::Window::new("Advanced Settings")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(380.0)
        .show(ctx, |ui| {
            // ── Pacing ──
            ui.label(
                egui::RichText::new("Pacing")
                    .strong()
                    .color(theme::TEXT_PRIMARY),
            );
            ui.label(
                egui::RichText::new(
                    "Delay between OSC messages — prevents flooding the console \
                     during long sequences. Applies to snapshot recall and macros.",
                )
                .small()
                .color(theme::TEXT_SECONDARY),
            );
            ui.add_space(2.0);
            let mut pace = setup.send_pace_us as f32;
            let resp = ui.add(
                egui::Slider::new(&mut pace, 0.0..=5000.0)
                    .step_by(100.0)
                    .suffix(" μs"),
            );
            if resp.changed() {
                let new = pace as u64;
                setup.send_pace_us = new;
                send_pace_us.store(new, Ordering::Relaxed);
                save_app_preferences(setup);
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // ── Diagnostics ──
            ui.label(
                egui::RichText::new("Diagnostics")
                    .strong()
                    .color(theme::TEXT_PRIMARY),
            );
            let diag_resp = ui
                .checkbox(&mut setup.show_diagnostics, "Show diagnostic tabs")
                .on_hover_text("Adds OSC Log and Inspector tabs to the main tab bar.");
            if diag_resp.changed() {
                save_app_preferences(setup);
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // ── Coming soon ──
            // Reserved slots for future app-level preferences. Greyed
            // out so the user knows they exist; not yet wired to any
            // backing state.
            ui.label(
                egui::RichText::new("Coming soon")
                    .strong()
                    .color(theme::TEXT_SECONDARY),
            );
            ui.add_enabled_ui(false, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Color theme:");
                    let _ = ui.button("Default");
                });
                ui.horizontal(|ui| {
                    ui.label("Help-bubble language:");
                    let _ = ui.button("English");
                });
            });
        });
}
