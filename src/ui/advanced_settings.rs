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
//! - **Display scale** — manual UI size multiplier.
//! - **Appearance** — colour theme and help-bubble language.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use eframe::egui;

use super::help::{HelpKey, help};
use super::setup_tab::{SetupTabState, save_app_preferences};
use super::theme;
use crate::model::ui_mode::ColorTheme;

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
                    .color(theme::label_color()),
            );
            ui.label(
                egui::RichText::new(
                    "Delay between OSC messages — prevents flooding the console \
                     during long sequences. Applies to snapshot recall and macros.",
                )
                .small()
                .color(theme::label_weak()),
            )
            .on_hover_text(help(HelpKey::AdvancedPacing));
            ui.add_space(2.0);
            let mut pace = setup.send_pace_us as f32;
            // Widen the slider track to fill the panel, and show the value in a
            // fixed-width box so the track length doesn't jump as the number
            // grows (egui sizes the track from `spacing.slider_width`, not the
            // allocated width, and its built-in value box auto-sizes). Both
            // controls pinned to ROW_H.
            const VALUE_W: f32 = 84.0;
            let resp = ui
                .horizontal(|ui| {
                    let gap = ui.spacing().item_spacing.x;
                    let track_w = (ui.available_width() - VALUE_W - gap).max(120.0);
                    ui.spacing_mut().slider_width = track_w;
                    // Make the slider rail visible — the light theme's white
                    // widget fill would otherwise hide the groove on the white
                    // panel. Scoped so only the slider is affected.
                    let slider = ui
                        .scope(|ui| {
                            let t = theme::slider_track();
                            ui.visuals_mut().widgets.inactive.bg_fill = t;
                            ui.visuals_mut().widgets.inactive.weak_bg_fill = t;
                            ui.visuals_mut().extreme_bg_color = t;
                            ui.add_sized(
                                [track_w, theme::ROW_H],
                                egui::Slider::new(&mut pace, 0.0..=5000.0)
                                    .step_by(100.0)
                                    .show_value(false),
                            )
                        })
                        .inner;
                    let value = ui.add_sized(
                        [VALUE_W, theme::ROW_H],
                        egui::DragValue::new(&mut pace)
                            .speed(100.0)
                            .range(0.0..=5000.0)
                            .fixed_decimals(0)
                            .suffix(" μs"),
                    );
                    slider.union(value)
                })
                .inner
                .on_hover_text(help(HelpKey::AdvancedPacing));
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
                    .color(theme::label_color()),
            );
            let diag_resp = ui
                .checkbox(&mut setup.show_diagnostics, "Show diagnostic tabs")
                .on_hover_text(help(HelpKey::AdvancedDiagnostics));
            if diag_resp.changed() {
                save_app_preferences(setup);
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // ── Display scale ──
            ui.label(
                egui::RichText::new("Display scale")
                    .strong()
                    .color(theme::label_color()),
            );
            ui.label(
                egui::RichText::new(
                    "Manual UI size, on top of the automatic display scaling. Leave at \
                     100% unless the UI looks too large or small — e.g. raise it when \
                     demoing on a big screen viewed from a distance.",
                )
                .small()
                .color(theme::label_weak()),
            )
            .on_hover_text(help(HelpKey::AdvancedScale));
            ui.add_space(2.0);
            // Slider/value work in percent; the model stores a 0.5–2.5 multiplier.
            let mut pct = (setup.ui_scale * 100.0).round();
            const SCALE_VALUE_W: f32 = 84.0;
            let scale_resp = ui
                .horizontal(|ui| {
                    let gap = ui.spacing().item_spacing.x;
                    let track_w = (ui.available_width() - SCALE_VALUE_W - gap).max(120.0);
                    ui.spacing_mut().slider_width = track_w;
                    // Visible rail in the light theme (see Pacing slider above).
                    let slider = ui
                        .scope(|ui| {
                            let t = theme::slider_track();
                            ui.visuals_mut().widgets.inactive.bg_fill = t;
                            ui.visuals_mut().widgets.inactive.weak_bg_fill = t;
                            ui.visuals_mut().extreme_bg_color = t;
                            ui.add_sized(
                                [track_w, theme::ROW_H],
                                egui::Slider::new(&mut pct, 50.0..=250.0)
                                    .step_by(5.0)
                                    .show_value(false),
                            )
                        })
                        .inner;
                    let value = ui.add_sized(
                        [SCALE_VALUE_W, theme::ROW_H],
                        egui::DragValue::new(&mut pct)
                            .speed(1.0)
                            .range(50.0..=250.0)
                            .fixed_decimals(0)
                            .suffix(" %"),
                    );
                    slider.union(value)
                })
                .inner
                .on_hover_text(help(HelpKey::AdvancedScale));
            if scale_resp.changed() {
                setup.ui_scale = pct / 100.0;
                save_app_preferences(setup);
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // ── Appearance ──
            ui.label(
                egui::RichText::new("Appearance")
                    .strong()
                    .color(theme::label_color()),
            );
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                theme::row_label(ui, "Color theme:", theme::label_color());
                let before = setup.color_theme;
                // The active choice is filled with the accent so it reads as
                // selected; the other stays neutral.
                let dark_fill = if setup.color_theme == ColorTheme::Dark {
                    theme::ACCENT_BLUE
                } else {
                    theme::btn_neutral()
                };
                let light_fill = if setup.color_theme == ColorTheme::Light {
                    theme::ACCENT_BLUE
                } else {
                    theme::btn_neutral()
                };
                if theme::row_action_button(
                    ui,
                    "Dark",
                    dark_fill,
                    80.0,
                    true,
                    help(HelpKey::ThemeDark),
                ) {
                    setup.color_theme = ColorTheme::Dark;
                }
                if theme::row_action_button(
                    ui,
                    "Light",
                    light_fill,
                    80.0,
                    true,
                    help(HelpKey::ThemeLight),
                ) {
                    setup.color_theme = ColorTheme::Light;
                }
                // configure_style reads setup.color_theme next frame, so the
                // switch is immediate — just persist the new choice.
                if setup.color_theme != before {
                    save_app_preferences(setup);
                }
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // ── Help bubbles ──
            ui.label(
                egui::RichText::new("Help bubbles")
                    .strong()
                    .color(theme::label_color()),
            );
            ui.label(
                egui::RichText::new(
                    "Tooltips localise into the chosen language; the rest of the UI \
                     stays English. Languages are JSON files in the app's locales \
                     folder — drop one in to add a language.",
                )
                .small()
                .color(theme::label_weak()),
            )
            .on_hover_text(help(HelpKey::AdvancedLanguage));
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                theme::row_label(ui, "Help-bubble language:", theme::label_color());
                let before = setup.help_language.clone();
                let current = super::help::language_name(&setup.help_language);
                egui::ComboBox::from_id_salt("help_language_combo")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (code, name) in super::help::available_languages() {
                            let selected = setup.help_language.eq_ignore_ascii_case(&code)
                                || (setup.help_language.is_empty()
                                    && code == super::help::ENGLISH_CODE);
                            if ui.selectable_label(selected, name).clicked() {
                                setup.help_language = code;
                            }
                        }
                    });
                // `help::set_active_language` publishes the choice for the next
                // frame's tooltips; persist so it survives restarts.
                if setup.help_language != before {
                    super::help::set_active_language(&setup.help_language);
                    save_app_preferences(setup);
                }
            });
        });
}
