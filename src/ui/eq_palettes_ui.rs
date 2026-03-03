use std::sync::Arc;

use eframe::egui;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::console::cue_manager::CueManager;
use crate::console::eq_palette_manager::EqPaletteManager;
use crate::model::channel::ChannelId;
use crate::model::eq_palette::EqPalette;
use crate::model::state::ConsoleState;
use super::theme;
use super::UiEvent;

/// Channel type selector reused from macros_tab pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelTypeChoice {
    Input,
    Aux,
    Group,
    Matrix,
}

impl ChannelTypeChoice {
    const ALL: [Self; 4] = [Self::Input, Self::Aux, Self::Group, Self::Matrix];

    fn label(&self) -> &'static str {
        match self {
            Self::Input => "Input",
            Self::Aux => "Aux",
            Self::Group => "Group",
            Self::Matrix => "Matrix",
        }
    }

    fn to_channel_id(&self, num: u8) -> ChannelId {
        match self {
            Self::Input => ChannelId::Input(num),
            Self::Aux => ChannelId::Aux(num),
            Self::Group => ChannelId::Group(num),
            Self::Matrix => ChannelId::Matrix(num),
        }
    }
}

/// State for the EQ Palettes UI section within the Snapshots tab.
pub struct EqPalettesUiState {
    pub selected_palette_id: Option<Uuid>,
    pub new_palette_name: String,
    pub capture_channel_type: ChannelTypeChoice,
    pub capture_channel_number: String,
    pub link_snapshot_id: Option<Uuid>,
    pub link_channel_type: ChannelTypeChoice,
    pub link_channel_number: String,
    pub status_message: Option<String>,
}

impl Default for EqPalettesUiState {
    fn default() -> Self {
        Self {
            selected_palette_id: None,
            new_palette_name: String::new(),
            capture_channel_type: ChannelTypeChoice::Input,
            capture_channel_number: "1".into(),
            link_snapshot_id: None,
            link_channel_type: ChannelTypeChoice::Input,
            link_channel_number: "1".into(),
            status_message: None,
        }
    }
}

/// Draw the EQ Palettes section (embedded in Snapshots tab).
pub fn draw_eq_palettes_section(
    ui: &mut egui::Ui,
    state: &mut EqPalettesUiState,
    console_state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
    is_connected: bool,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    theme::section_heading(ui, "EQ Palettes");

    // ── Capture palette ─────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label("Channel:");
        egui::ComboBox::from_id_salt("palette_capture_ch_type")
            .selected_text(state.capture_channel_type.label())
            .width(70.0)
            .show_ui(ui, |ui| {
                for ch in ChannelTypeChoice::ALL {
                    ui.selectable_value(&mut state.capture_channel_type, ch, ch.label());
                }
            });
        ui.add(egui::TextEdit::singleline(&mut state.capture_channel_number).desired_width(40.0));

        ui.label("Name:");
        ui.add(egui::TextEdit::singleline(&mut state.new_palette_name).desired_width(120.0));

        let can_capture = is_connected && !state.new_palette_name.is_empty();
        let capture_btn = theme::action_button("Capture EQ", theme::ACCENT_GREEN, egui::Vec2::new(90.0, 28.0));
        if ui.add_enabled(can_capture, capture_btn).clicked() {
            capture_palette(state, console_state, eq_palette_manager, runtime, ui_tx);
        }
    });

    // ── Palette list ────────────────────────────────────────────
    ui.add_space(4.0);

    egui::ScrollArea::vertical()
        .id_salt("palette_list_scroll")
        .max_height(120.0)
        .show(ui, |ui| {
            if let Ok(mgr) = eq_palette_manager.try_read() {
                let palettes = mgr.sorted_palettes();
                if palettes.is_empty() {
                    ui.label(egui::RichText::new("No palettes yet. Capture one above.").color(theme::TEXT_SECONDARY));
                }
                for palette in palettes {
                    let selected = state.selected_palette_id == Some(palette.id);
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
                                    egui::RichText::new(&palette.name)
                                        .strong()
                                        .color(theme::TEXT_PRIMARY),
                                );
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} | {} params | {} refs",
                                        palette.channel,
                                        palette.parameter_count(),
                                        palette.referencing_snapshots.len(),
                                    ))
                                    .color(theme::TEXT_SECONDARY)
                                    .small(),
                                );
                            }).response;

                            if response.interact(egui::Sense::click()).clicked() {
                                state.selected_palette_id = Some(palette.id);
                            }
                        });
                    ui.add_space(1.0);
                }
            }
        });

    // ── Palette detail / actions ────────────────────────────────
    if let Some(pid) = state.selected_palette_id {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let recapture_btn = theme::action_button("Re-capture", theme::ACCENT_BLUE, egui::Vec2::new(90.0, 28.0));
            if ui.add_enabled(is_connected, recapture_btn).clicked() {
                recapture_palette(pid, console_state, eq_palette_manager, runtime, ui_tx);
            }
            let del_btn = theme::action_button("Delete Palette", theme::ACCENT_RED, egui::Vec2::new(100.0, 28.0));
            if ui.add(del_btn).clicked() {
                delete_palette(pid, cue_manager, eq_palette_manager, runtime);
                state.selected_palette_id = None;
                state.status_message = Some("Palette deleted".into());
            }
        });

        // Detail: EQ values
        if let Ok(mgr) = eq_palette_manager.try_read() {
            if let Some(palette) = mgr.get_palette(&pid) {
                egui::CollapsingHeader::new(
                    egui::RichText::new(format!("EQ Values ({})", palette.parameter_count()))
                        .color(theme::TEXT_SECONDARY),
                )
                .default_open(false)
                .show(ui, |ui| {
                    let mut entries: Vec<_> = palette.eq_values.iter().collect();
                    entries.sort_by_key(|(path, _)| format!("{:?}", path));
                    for (path, value) in entries {
                        ui.horizontal(|ui| {
                            ui.monospace(format!("{:?}", path));
                            ui.label(
                                egui::RichText::new(format!("= {}", value))
                                    .color(theme::TEXT_SECONDARY),
                            );
                        });
                    }
                });

                // Referencing snapshots
                if !palette.referencing_snapshots.is_empty() {
                    egui::CollapsingHeader::new(
                        egui::RichText::new(format!("Linked Snapshots ({})", palette.referencing_snapshots.len()))
                            .color(theme::TEXT_SECONDARY),
                    )
                    .default_open(false)
                    .show(ui, |ui| {
                        if let Ok(cue_mgr) = cue_manager.try_read() {
                            for snap_id in &palette.referencing_snapshots {
                                let name = cue_mgr.snapshots.get(snap_id)
                                    .map(|s| s.name.as_str())
                                    .unwrap_or("(unknown)");
                                ui.label(
                                    egui::RichText::new(format!("  {name}"))
                                        .color(theme::TEXT_PRIMARY),
                                );
                            }
                        }
                    });
                }
            }
        }
    }

    // ── Link / Unlink ───────────────────────────────────────────
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Link Palette to Snapshot").strong().color(theme::TEXT_PRIMARY));

    ui.horizontal(|ui| {
        // Snapshot dropdown
        ui.label("Snapshot:");
        if let Ok(mgr) = cue_manager.try_read() {
            let current_name = state.link_snapshot_id
                .and_then(|id| mgr.snapshots.get(&id))
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "(select)".into());

            egui::ComboBox::from_id_salt("palette_link_snapshot")
                .selected_text(&current_name)
                .width(140.0)
                .show_ui(ui, |ui| {
                    for snap in mgr.snapshots.values() {
                        if ui.selectable_label(
                            state.link_snapshot_id == Some(snap.id),
                            &snap.name,
                        ).clicked() {
                            state.link_snapshot_id = Some(snap.id);
                        }
                    }
                });
        }

        // Channel for link
        ui.label("Ch:");
        egui::ComboBox::from_id_salt("palette_link_ch_type")
            .selected_text(state.link_channel_type.label())
            .width(70.0)
            .show_ui(ui, |ui| {
                for ch in ChannelTypeChoice::ALL {
                    ui.selectable_value(&mut state.link_channel_type, ch, ch.label());
                }
            });
        ui.add(egui::TextEdit::singleline(&mut state.link_channel_number).desired_width(30.0));
    });

    ui.horizontal(|ui| {
        // Palette dropdown
        let selected_palette_name = state.selected_palette_id
            .and_then(|id| eq_palette_manager.try_read().ok()
                .and_then(|mgr| mgr.get_palette(&id).map(|p| p.name.clone())))
            .unwrap_or_else(|| "(select palette above)".into());
        ui.label(
            egui::RichText::new(format!("Palette: {selected_palette_name}"))
                .color(theme::TEXT_SECONDARY),
        );

        let can_link = state.selected_palette_id.is_some() && state.link_snapshot_id.is_some();

        let link_btn = theme::action_button("Link", theme::ACCENT_GREEN, egui::Vec2::new(60.0, 28.0));
        if ui.add_enabled(can_link, link_btn).clicked() {
            if let (Some(palette_id), Some(snap_id)) = (state.selected_palette_id, state.link_snapshot_id) {
                if let Ok(ch_num) = state.link_channel_number.parse::<u8>() {
                    let channel = state.link_channel_type.to_channel_id(ch_num);
                    link_palette(palette_id, snap_id, channel, cue_manager, eq_palette_manager, runtime, ui_tx);
                }
            }
        }

        let unlink_btn = theme::action_button("Unlink", theme::ACCENT_RED, egui::Vec2::new(60.0, 28.0));
        if ui.add_enabled(can_link, unlink_btn).clicked() {
            if let (Some(palette_id), Some(snap_id)) = (state.selected_palette_id, state.link_snapshot_id) {
                if let Ok(ch_num) = state.link_channel_number.parse::<u8>() {
                    let channel = state.link_channel_type.to_channel_id(ch_num);
                    unlink_palette(palette_id, snap_id, channel, cue_manager, eq_palette_manager, runtime);
                    state.status_message = Some("Palette unlinked".into());
                }
            }
        }
    });

    // Status
    if let Some(msg) = &state.status_message {
        ui.add_space(2.0);
        ui.colored_label(theme::TEXT_WARNING, msg);
    }
}

fn capture_palette(
    state: &mut EqPalettesUiState,
    console_state: &Arc<RwLock<ConsoleState>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let Ok(ch_num) = state.capture_channel_number.parse::<u8>() else {
        state.status_message = Some("Invalid channel number".into());
        return;
    };
    let channel = state.capture_channel_type.to_channel_id(ch_num);
    let name = state.new_palette_name.clone();
    let st = console_state.clone();
    let eq_mgr = eq_palette_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        let state_guard = st.read().await;
        let eq_values = state_guard.capture_eq(&channel);
        let param_count = eq_values.len();
        drop(state_guard);

        let palette = EqPalette::new(name.clone(), channel, eq_values);
        eq_mgr.write().await.add_palette(palette);

        let _ = tx.send(UiEvent::PaletteCaptured { name, param_count });
    });

    state.status_message = Some(format!("Capturing '{}'...", state.new_palette_name));
    state.new_palette_name.clear();
}

fn recapture_palette(
    palette_id: Uuid,
    console_state: &Arc<RwLock<ConsoleState>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let st = console_state.clone();
    let eq_mgr = eq_palette_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        let mgr = eq_mgr.read().await;
        let Some(palette) = mgr.get_palette(&palette_id) else { return };
        let channel = palette.channel.clone();
        let name = palette.name.clone();
        let affected_count = palette.referencing_snapshots.len();
        drop(mgr);

        let state_guard = st.read().await;
        let eq_values = state_guard.capture_eq(&channel);
        drop(state_guard);

        let mut mgr = eq_mgr.write().await;
        if let Some(palette) = mgr.get_palette_mut(&palette_id) {
            palette.eq_values = eq_values
                .into_iter()
                .filter(|(p, _)| p.section() == crate::model::parameter::ParameterSection::Eq)
                .collect();
            palette.touch();
        }

        let _ = tx.send(UiEvent::PaletteUpdated { name, affected_count });
    });
}

fn delete_palette(
    palette_id: Uuid,
    cue_manager: &Arc<RwLock<CueManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
    runtime: &tokio::runtime::Handle,
) {
    let eq_mgr = eq_palette_manager.clone();
    let cue_mgr = cue_manager.clone();

    runtime.spawn(async move {
        // Clear palette refs from all snapshots
        let mut mgr = cue_mgr.write().await;
        for snapshot in mgr.snapshots.values_mut() {
            snapshot.eq_palette_refs.retain(|_, pid| *pid != palette_id);
        }
        drop(mgr);

        eq_mgr.write().await.remove_palette(palette_id);
    });
}

fn link_palette(
    palette_id: Uuid,
    snapshot_id: Uuid,
    channel: ChannelId,
    cue_manager: &Arc<RwLock<CueManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let cue_mgr = cue_manager.clone();
    let eq_mgr = eq_palette_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        // Set eq_palette_refs on the snapshot
        let mut mgr = cue_mgr.write().await;
        let (palette_name, snapshot_name) = if let Some(snapshot) = mgr.snapshots.get_mut(&snapshot_id) {
            // If there was a previous palette for this channel, unlink it
            if let Some(old_pid) = snapshot.eq_palette_refs.insert(channel, palette_id) {
                if old_pid != palette_id {
                    let mut eq = eq_mgr.write().await;
                    eq.unlink_from_snapshot(old_pid, snapshot_id);
                    eq.link_to_snapshot(palette_id, snapshot_id);
                }
            } else {
                eq_mgr.write().await.link_to_snapshot(palette_id, snapshot_id);
            }

            let sname = snapshot.name.clone();
            let pname = eq_mgr.read().await
                .get_palette(&palette_id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "?".into());
            (pname, sname)
        } else {
            return;
        };
        drop(mgr);

        let _ = tx.send(UiEvent::PaletteLinked { palette_name, snapshot_name });
    });
}

fn unlink_palette(
    palette_id: Uuid,
    snapshot_id: Uuid,
    channel: ChannelId,
    cue_manager: &Arc<RwLock<CueManager>>,
    eq_palette_manager: &Arc<RwLock<EqPaletteManager>>,
    runtime: &tokio::runtime::Handle,
) {
    let cue_mgr = cue_manager.clone();
    let eq_mgr = eq_palette_manager.clone();

    runtime.spawn(async move {
        let mut mgr = cue_mgr.write().await;
        if let Some(snapshot) = mgr.snapshots.get_mut(&snapshot_id) {
            snapshot.eq_palette_refs.remove(&channel);
        }
        drop(mgr);

        eq_mgr.write().await.unlink_from_snapshot(palette_id, snapshot_id);
    });
}
