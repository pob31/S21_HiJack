use std::sync::Arc;

use eframe::egui;
use tokio::sync::RwLock;

use super::theme;
use crate::model::channel::ChannelId;
use crate::model::state::ConsoleState;

/// State for the Inspector tab.
pub struct InspectorTabState {
    /// Text filter applied to any column.
    pub filter: String,
    /// Filter by channel type (None = show all).
    pub channel_type_filter: Option<ChannelTypeFilter>,
    /// Column to sort by.
    pub sort_column: SortColumn,
    /// Sort ascending.
    pub sort_ascending: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelTypeFilter {
    Input,
    Aux,
    Group,
    Matrix,
    ControlGroup,
    GraphicEq,
    MatrixInput,
}

impl ChannelTypeFilter {
    fn label(&self) -> &str {
        match self {
            Self::Input => "Input",
            Self::Aux => "Aux",
            Self::Group => "Group",
            Self::Matrix => "Matrix",
            Self::ControlGroup => "CG",
            Self::GraphicEq => "GEQ",
            Self::MatrixInput => "MtxIn",
        }
    }

    fn matches(&self, ch: &ChannelId) -> bool {
        matches!(
            (self, ch),
            (Self::Input, ChannelId::Input(_))
                | (Self::Aux, ChannelId::Aux(_))
                | (Self::Group, ChannelId::Group(_))
                | (Self::Matrix, ChannelId::Matrix(_))
                | (Self::ControlGroup, ChannelId::ControlGroup(_))
                | (Self::GraphicEq, ChannelId::GraphicEq(_))
                | (Self::MatrixInput, ChannelId::MatrixInput(_))
        )
    }

    const ALL: &[Self] = &[
        Self::Input,
        Self::Aux,
        Self::Group,
        Self::Matrix,
        Self::ControlGroup,
        Self::GraphicEq,
        Self::MatrixInput,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortColumn {
    Channel,
    Parameter,
    Value,
}

impl Default for InspectorTabState {
    fn default() -> Self {
        Self {
            filter: String::new(),
            channel_type_filter: None,
            sort_column: SortColumn::Channel,
            sort_ascending: true,
        }
    }
}

/// A single row in the inspector table.
struct InspectorRow {
    channel: String,
    parameter: String,
    value: String,
    channel_color: egui::Color32,
}

/// Draw the Inspector tab.
pub fn draw_inspector_tab(
    ui: &mut egui::Ui,
    tab: &mut InspectorTabState,
    state: &Arc<RwLock<ConsoleState>>,
) {
    // ── Toolbar ──
    theme::card_frame().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.add(
                egui::TextEdit::singleline(&mut tab.filter)
                    .desired_width(200.0)
                    .hint_text("channel, param, or value..."),
            );

            ui.add_space(12.0);
            ui.label("Type:");

            // "All" button
            let all_active = tab.channel_type_filter.is_none();
            let fill = if all_active {
                theme::ACCENT_BLUE
            } else {
                theme::BG_ELEVATED
            };
            let text_color = if all_active {
                theme::TEXT_PRIMARY
            } else {
                theme::TEXT_SECONDARY
            };
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("All").color(text_color))
                        .fill(fill)
                        .corner_radius(4.0),
                )
                .clicked()
            {
                tab.channel_type_filter = None;
            }

            for ct in ChannelTypeFilter::ALL {
                let is_active = tab.channel_type_filter == Some(*ct);
                let fill = if is_active {
                    theme::ACCENT_BLUE
                } else {
                    theme::BG_ELEVATED
                };
                let text_color = if is_active {
                    theme::TEXT_PRIMARY
                } else {
                    theme::TEXT_SECONDARY
                };
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new(ct.label()).color(text_color))
                            .fill(fill)
                            .corner_radius(4.0),
                    )
                    .clicked()
                {
                    tab.channel_type_filter = Some(*ct);
                }
            }
        });

        // Param count
        if let Ok(st) = state.try_read() {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(format!(
                    "{} parameters in state mirror",
                    st.parameter_count()
                ))
                .color(theme::TEXT_SECONDARY)
                .small(),
            );
        }
    });

    ui.add_space(4.0);

    // ── Build rows ──
    let rows = if let Ok(st) = state.try_read() {
        let filter_lower = tab.filter.to_lowercase();
        let mut rows: Vec<InspectorRow> = st
            .iter_parameters()
            .filter(|(addr, _)| {
                if let Some(ref ct) = tab.channel_type_filter {
                    if !ct.matches(&addr.channel) {
                        return false;
                    }
                }
                true
            })
            .map(|(addr, value)| InspectorRow {
                channel: format!("{}", addr.channel),
                parameter: format!("{:?}", addr.parameter),
                value: format!("{}", value),
                channel_color: theme::channel_color(&addr.channel),
            })
            .filter(|row| {
                if filter_lower.is_empty() {
                    return true;
                }
                row.channel.to_lowercase().contains(&filter_lower)
                    || row.parameter.to_lowercase().contains(&filter_lower)
                    || row.value.to_lowercase().contains(&filter_lower)
            })
            .collect();

        // Sort
        match tab.sort_column {
            SortColumn::Channel => rows.sort_by(|a, b| {
                let cmp = a.channel.cmp(&b.channel);
                if tab.sort_ascending {
                    cmp
                } else {
                    cmp.reverse()
                }
            }),
            SortColumn::Parameter => rows.sort_by(|a, b| {
                let cmp = a.parameter.cmp(&b.parameter);
                if tab.sort_ascending {
                    cmp
                } else {
                    cmp.reverse()
                }
            }),
            SortColumn::Value => rows.sort_by(|a, b| {
                let cmp = a.value.cmp(&b.value);
                if tab.sort_ascending {
                    cmp
                } else {
                    cmp.reverse()
                }
            }),
        }

        rows
    } else {
        vec![]
    };

    // ── Table ──
    let text_height = ui.text_style_height(&egui::TextStyle::Body);
    let row_height = text_height + 4.0;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            use egui_extras::{Column, TableBuilder};

            TableBuilder::new(ui)
                .striped(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::initial(150.0).at_least(100.0)) // Channel
                .column(Column::initial(250.0).at_least(150.0)) // Parameter
                .column(Column::remainder()) // Value
                .header(row_height, |mut header| {
                    header.col(|ui| {
                        if ui
                            .selectable_label(tab.sort_column == SortColumn::Channel, "Channel")
                            .clicked()
                        {
                            if tab.sort_column == SortColumn::Channel {
                                tab.sort_ascending = !tab.sort_ascending;
                            } else {
                                tab.sort_column = SortColumn::Channel;
                                tab.sort_ascending = true;
                            }
                        }
                    });
                    header.col(|ui| {
                        if ui
                            .selectable_label(tab.sort_column == SortColumn::Parameter, "Parameter")
                            .clicked()
                        {
                            if tab.sort_column == SortColumn::Parameter {
                                tab.sort_ascending = !tab.sort_ascending;
                            } else {
                                tab.sort_column = SortColumn::Parameter;
                                tab.sort_ascending = true;
                            }
                        }
                    });
                    header.col(|ui| {
                        if ui
                            .selectable_label(tab.sort_column == SortColumn::Value, "Value")
                            .clicked()
                        {
                            if tab.sort_column == SortColumn::Value {
                                tab.sort_ascending = !tab.sort_ascending;
                            } else {
                                tab.sort_column = SortColumn::Value;
                                tab.sort_ascending = true;
                            }
                        }
                    });
                })
                .body(|body| {
                    body.rows(row_height, rows.len(), |mut row| {
                        let entry = &rows[row.index()];
                        row.col(|ui| {
                            ui.colored_label(entry.channel_color, &entry.channel);
                        });
                        row.col(|ui| {
                            ui.label(&entry.parameter);
                        });
                        row.col(|ui| {
                            ui.label(egui::RichText::new(&entry.value).color(theme::TEXT_PRIMARY));
                        });
                    });
                });
        });
}
