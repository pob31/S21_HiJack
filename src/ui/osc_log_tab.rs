use eframe::egui;

use super::theme;
use crate::model::osc_log::{OscDirection, OscLog};

/// State for the OSC Log tab.
pub struct OscLogTabState {
    /// Text filter applied to path column.
    pub filter: String,
    /// Show incoming messages.
    pub show_in: bool,
    /// Show outgoing messages.
    pub show_out: bool,
    /// Last-seen revision, used to detect new entries for auto-scroll.
    last_revision: u64,
}

impl Default for OscLogTabState {
    fn default() -> Self {
        Self {
            filter: String::new(),
            show_in: true,
            show_out: true,
            last_revision: 0,
        }
    }
}

/// Draw the OSC Log tab.
pub fn draw_osc_log_tab(ui: &mut egui::Ui, tab: &mut OscLogTabState, log: &OscLog) {
    // ── Toolbar ──
    theme::card_frame().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.add(
                egui::TextEdit::singleline(&mut tab.filter)
                    .desired_width(200.0)
                    .hint_text("path contains..."),
            );

            ui.add_space(12.0);
            ui.checkbox(&mut tab.show_in, "IN");
            ui.checkbox(&mut tab.show_out, "OUT");

            ui.add_space(12.0);

            let paused = log.paused();
            let pause_label = if paused { "Resume" } else { "Pause" };
            let pause_color = if paused {
                theme::ACCENT_GREEN
            } else {
                theme::ACCENT_AMBER
            };
            if ui
                .add(theme::action_button(
                    pause_label,
                    pause_color,
                    egui::Vec2::new(80.0, 28.0),
                ))
                .clicked()
            {
                log.set_paused(!paused);
            }

            if ui
                .add(theme::action_button(
                    "Clear",
                    theme::ACCENT_RED,
                    egui::Vec2::new(70.0, 28.0),
                ))
                .clicked()
            {
                log.clear();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("{} entries", log.len()))
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
            });
        });
    });

    ui.add_space(4.0);

    // ── Log table ──
    let entries = log.snapshot();
    let filter_lower = tab.filter.to_lowercase();

    let filtered: Vec<_> = entries
        .iter()
        .filter(|e| {
            let dir_ok = match e.direction {
                OscDirection::In => tab.show_in,
                OscDirection::Out => tab.show_out,
            };
            if !dir_ok {
                return false;
            }
            if !filter_lower.is_empty() && !e.path.to_lowercase().contains(&filter_lower) {
                return false;
            }
            true
        })
        .collect();

    let text_height = ui.text_style_height(&egui::TextStyle::Body);
    let row_height = text_height + 4.0;

    let new_revision = log.revision();
    let should_scroll = log.auto_scroll() && new_revision != tab.last_revision;
    tab.last_revision = new_revision;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(should_scroll)
        .show(ui, |ui| {
            use egui_extras::{Column, TableBuilder};

            TableBuilder::new(ui)
                .striped(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(70.0)) // Time
                .column(Column::exact(40.0)) // Dir
                .column(Column::initial(300.0).at_least(150.0)) // Path
                .column(Column::remainder()) // Args
                .header(row_height, |mut header| {
                    header.col(|ui| {
                        ui.strong("Time");
                    });
                    header.col(|ui| {
                        ui.strong("Dir");
                    });
                    header.col(|ui| {
                        ui.strong("Path");
                    });
                    header.col(|ui| {
                        ui.strong("Args");
                    });
                })
                .body(|body| {
                    body.rows(row_height, filtered.len(), |mut row| {
                        let entry = &filtered[row.index()];
                        row.col(|ui| {
                            ui.label(
                                egui::RichText::new(entry.timestamp.format("%H:%M:%S").to_string())
                                    .color(theme::TEXT_SECONDARY)
                                    .small(),
                            );
                        });
                        row.col(|ui| {
                            let (color, label) = match entry.direction {
                                OscDirection::In => (theme::ACCENT_GREEN, "IN"),
                                OscDirection::Out => (theme::ACCENT_BLUE, "OUT"),
                            };
                            ui.colored_label(color, label);
                        });
                        row.col(|ui| {
                            ui.label(&entry.path);
                        });
                        row.col(|ui| {
                            ui.label(egui::RichText::new(&entry.args).color(theme::TEXT_SECONDARY));
                        });
                    });
                });
        });
}
