//! Palettes UI section embedded in the Snapshots tab.
//!
//! A palette can hold EQ, Dyn1, and/or Dyn2 values for one channel — the
//! capture form lets the operator pick which processes to include. Linkage to
//! snapshots is expressed as a per-kind membership grid on the selected
//! palette: each snapshot row shows one checkbox per kind the palette covers.
//! Ticking links the palette on that `(channel, kind)`; unticking unlinks.
//! When another palette currently occupies a slot the cell hints "currently:
//! <other>" so the operator sees the swap before it happens.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::UiEvent;
use super::theme;
use crate::console::cue_manager::CueManager;
use crate::console::palette_manager::PaletteManager;
use crate::model::channel::ChannelId;
use crate::model::palette::ChannelPalette;
use crate::model::parameter::{PaletteKind, ParameterPath, ParameterSection, ParameterValue};
use crate::model::state::ConsoleState;

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

    #[allow(clippy::wrong_self_convention)] // Copy enum; &self/self equivalent here.
    fn to_channel_id(&self, num: u8) -> ChannelId {
        match self {
            Self::Input => ChannelId::Input(num),
            Self::Aux => ChannelId::Aux(num),
            Self::Group => ChannelId::Group(num),
            Self::Matrix => ChannelId::Matrix(num),
        }
    }
}

/// State for the Palettes UI section within the Snapshots tab.
pub struct PalettesUiState {
    pub selected_palette_id: Option<Uuid>,
    pub new_palette_name: String,
    pub capture_channel_type: ChannelTypeChoice,
    pub capture_channel_number: String,
    /// Which kinds to include in the next captured palette, indexed by
    /// `PaletteKind::all()` order (Eq, Dyn1, Dyn2). Default: all on.
    pub capture_kinds: [bool; 3],
    /// `(palette_id, draft_name)` while the user is editing the selected
    /// palette's name. Cleared on commit (Enter / focus loss), cancel
    /// (Escape), or selection change.
    pub rename_draft: Option<(Uuid, String)>,
    pub status_message: Option<String>,
}

impl Default for PalettesUiState {
    fn default() -> Self {
        Self {
            selected_palette_id: None,
            new_palette_name: String::new(),
            capture_channel_type: ChannelTypeChoice::Input,
            capture_channel_number: "1".into(),
            capture_kinds: [true, true, true],
            rename_draft: None,
            status_message: None,
        }
    }
}

/// Format a palette's kinds as a compact chip, e.g. "Eq·Dyn1·Dyn2".
fn kinds_chip(palette: &ChannelPalette) -> String {
    let kinds = palette.kinds();
    if kinds.is_empty() {
        "(empty)".into()
    } else {
        kinds
            .iter()
            .map(|k| k.label())
            .collect::<Vec<_>>()
            .join("·")
    }
}

/// Draw the Palettes section (embedded in Snapshots tab).
#[allow(clippy::too_many_arguments)]
pub fn draw_palettes_section(
    ui: &mut egui::Ui,
    state: &mut PalettesUiState,
    console_state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    is_connected: bool,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    theme::section_heading(ui, "Palettes");

    // ── Capture palette ─────────────────────────────────────────
    ui.horizontal(|ui| {
        theme::row_label(ui, "Channel:", theme::TEXT_PRIMARY);
        theme::row_combo(ui, 0, |ui| {
            egui::ComboBox::from_id_salt("palette_capture_ch_type")
                .selected_text(state.capture_channel_type.label())
                .width(70.0)
                .show_ui(ui, |ui| {
                    for ch in ChannelTypeChoice::ALL {
                        ui.selectable_value(&mut state.capture_channel_type, ch, ch.label());
                    }
                });
        });
        theme::padded_text_edit_sized(
            ui,
            &mut state.capture_channel_number,
            40.0,
            theme::ROW_H,
            true,
            "",
        );

        theme::row_label(ui, "Name:", theme::TEXT_PRIMARY);
        theme::padded_text_edit_sized(
            ui,
            &mut state.new_palette_name,
            140.0,
            theme::ROW_H,
            true,
            "",
        );
    });

    ui.horizontal(|ui| {
        theme::row_label(ui, "Include:", theme::TEXT_PRIMARY);
        for (i, kind) in PaletteKind::all().iter().enumerate() {
            ui.checkbox(&mut state.capture_kinds[i], kind.label());
        }

        let any_kind = state.capture_kinds.iter().any(|on| *on);
        let can_capture = is_connected && !state.new_palette_name.is_empty() && any_kind;
        if theme::row_action_button(ui, "Capture", theme::ACCENT_GREEN, 90.0, can_capture) {
            capture_palette(state, console_state, palette_manager, runtime, ui_tx);
        }
    });

    ui.add_space(4.0);

    // ── Palette list ────────────────────────────────────────────
    egui::ScrollArea::vertical()
        .id_salt("palette_list_scroll")
        .max_height(120.0)
        .show(ui, |ui| {
            if let Ok(mgr) = palette_manager.try_read() {
                let palettes = mgr.sorted_palettes();
                if palettes.is_empty() {
                    ui.label(
                        egui::RichText::new("No palettes yet. Capture one above.")
                            .color(theme::TEXT_SECONDARY),
                    );
                }
                for palette in palettes {
                    let selected = state.selected_palette_id == Some(palette.id);
                    let bg = if selected {
                        theme::BG_ELEVATED
                    } else {
                        theme::BG_PANEL
                    };

                    let mut clicked = false;
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
                            ui.horizontal(|ui| {
                                let r_name = ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&palette.name)
                                            .strong()
                                            .color(theme::TEXT_PRIMARY),
                                    )
                                    .sense(egui::Sense::click()),
                                );
                                let r_meta = ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(format!(
                                            "{} · {} · {} params · {} refs",
                                            palette.channel,
                                            kinds_chip(palette),
                                            palette.parameter_count(),
                                            palette.referencing_snapshots.len(),
                                        ))
                                        .color(theme::TEXT_SECONDARY)
                                        .small(),
                                    )
                                    .sense(egui::Sense::click()),
                                );
                                // Fill the rest of the row with a click-sensing
                                // strip so clicks to the right of the labels
                                // also select the palette.
                                let fill_w = ui.available_width().max(1.0);
                                let (_, r_fill) = ui.allocate_exact_size(
                                    egui::Vec2::new(fill_w, 1.0),
                                    egui::Sense::click(),
                                );
                                if r_name.clicked() || r_meta.clicked() || r_fill.clicked() {
                                    clicked = true;
                                }
                            });
                        });
                    if clicked && state.selected_palette_id != Some(palette.id) {
                        state.selected_palette_id = Some(palette.id);
                        state.rename_draft = None;
                    }
                    ui.add_space(1.0);
                }
            }
        });

    // ── Palette detail / actions ────────────────────────────────
    let Some(pid) = state.selected_palette_id else {
        if let Some(msg) = &state.status_message {
            ui.add_space(2.0);
            ui.colored_label(theme::TEXT_WARNING, msg);
        }
        return;
    };

    // Snapshot all data we need from the locked managers before rendering,
    // so we never hold guards across egui closures that may spawn work.
    let Some(palette_info) = read_palette_info(palette_manager, pid) else {
        // Selected palette disappeared (deletion mid-frame). Clear selection.
        state.selected_palette_id = None;
        return;
    };

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        theme::row_label(ui, "Name:", theme::TEXT_PRIMARY);
        // Draft buffer is only present while editing; fall back to the
        // stored name. We bind the TextEdit to a local string and write
        // through to the draft on change.
        let mut buf = match &state.rename_draft {
            Some((id, draft)) if *id == pid => draft.clone(),
            _ => palette_info.name.clone(),
        };
        let resp = theme::padded_text_edit_sized(ui, &mut buf, 180.0, theme::ROW_H, true, "");
        if resp.changed() {
            state.rename_draft = Some((pid, buf.clone()));
        }
        let enter_pressed = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let escape_pressed = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));
        if escape_pressed {
            state.rename_draft = None;
            resp.surrender_focus();
        } else if enter_pressed || (resp.lost_focus() && state.rename_draft.is_some()) {
            if let Some((id, draft)) = state.rename_draft.take() {
                if id == pid && !draft.trim().is_empty() && draft != palette_info.name {
                    rename_palette(pid, draft, palette_manager, runtime);
                }
            }
        }

        if theme::row_action_button(ui, "Re-capture", theme::ACCENT_BLUE, 90.0, is_connected) {
            recapture_palette(pid, console_state, palette_manager, runtime, ui_tx);
        }
        if theme::row_action_button(ui, "Delete Palette", theme::ACCENT_RED, 100.0, true) {
            delete_palette(pid, cue_manager, palette_manager, runtime);
            state.selected_palette_id = None;
            state.rename_draft = None;
            state.status_message = Some("Palette deleted".into());
        }
    });

    // Detail: stored values
    egui::CollapsingHeader::new(
        egui::RichText::new(format!(
            "Values ({} · {})",
            palette_info.parameter_count,
            kinds_chip_from(&palette_info.kinds),
        ))
        .color(theme::TEXT_SECONDARY),
    )
    .default_open(false)
    .show(ui, |ui| {
        let mut entries: Vec<_> = palette_info.values.iter().collect();
        entries.sort_by_key(|(path, _)| format!("{:?}", path));
        for (path, value) in entries {
            ui.horizontal(|ui| {
                ui.monospace(format!("{:?}", path));
                ui.label(egui::RichText::new(format!("= {}", value)).color(theme::TEXT_SECONDARY));
            });
        }
    });

    // ── Membership grid ─────────────────────────────────────────
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new("Assign to snapshots")
            .strong()
            .color(theme::TEXT_PRIMARY),
    );
    ui.label(
        egui::RichText::new(
            "Tick a checkbox to use this palette on the corresponding snapshot. \
             Each column is one process.",
        )
        .color(theme::TEXT_SECONDARY)
        .small(),
    );

    if palette_info.kinds.is_empty() {
        ui.label(
            egui::RichText::new("Palette has no values yet — re-capture to populate it.")
                .color(theme::TEXT_SECONDARY),
        );
    } else {
        let rows = read_membership_rows(
            cue_manager,
            palette_manager,
            pid,
            &palette_info.channel,
            &palette_info.kinds,
        );

        if rows.is_empty() {
            ui.label(egui::RichText::new("No snapshots yet.").color(theme::TEXT_SECONDARY));
        } else {
            egui::ScrollArea::vertical()
                .id_salt("palette_membership_scroll")
                .max_height(220.0)
                .show(ui, |ui| {
                    egui::Grid::new("palette_membership_grid")
                        .num_columns(1 + palette_info.kinds.len())
                        .striped(true)
                        .spacing(egui::Vec2::new(8.0, 4.0))
                        .show(ui, |ui| {
                            // Header row
                            ui.label("");
                            for k in &palette_info.kinds {
                                ui.label(
                                    egui::RichText::new(k.label())
                                        .strong()
                                        .color(theme::TEXT_SECONDARY),
                                );
                            }
                            ui.end_row();

                            for row in &rows {
                                ui.label(
                                    egui::RichText::new(&row.snapshot_name)
                                        .color(theme::TEXT_PRIMARY),
                                );
                                for cell in &row.cells {
                                    let mut on = matches!(cell.state, CellState::LinkedToThis);
                                    let resp = ui.checkbox(&mut on, "");
                                    if let CellState::LinkedToOther { other_name } = &cell.state {
                                        resp.clone()
                                            .on_hover_text(format!("Currently: \"{other_name}\""));
                                    }
                                    if resp.changed() {
                                        if on {
                                            link_palette(
                                                pid,
                                                row.snapshot_id,
                                                palette_info.channel.clone(),
                                                cell.kind,
                                                cue_manager,
                                                palette_manager,
                                                runtime,
                                                ui_tx,
                                            );
                                        } else {
                                            unlink_palette(
                                                pid,
                                                row.snapshot_id,
                                                palette_info.channel.clone(),
                                                cell.kind,
                                                cue_manager,
                                                palette_manager,
                                                runtime,
                                            );
                                        }
                                    }
                                }
                                ui.end_row();
                            }
                        });
                });
        }
    }

    // Status
    if let Some(msg) = &state.status_message {
        ui.add_space(2.0);
        ui.colored_label(theme::TEXT_WARNING, msg);
    }
}

// ─── Render-time data snapshots ────────────────────────────────

struct PaletteInfo {
    name: String,
    channel: ChannelId,
    kinds: Vec<PaletteKind>,
    parameter_count: usize,
    values: HashMap<ParameterPath, ParameterValue>,
}

fn read_palette_info(
    palette_manager: &Arc<RwLock<PaletteManager>>,
    pid: Uuid,
) -> Option<PaletteInfo> {
    let mgr = palette_manager.try_read().ok()?;
    let palette = mgr.get_palette(&pid)?;
    Some(PaletteInfo {
        name: palette.name.clone(),
        channel: palette.channel.clone(),
        kinds: palette.kinds(),
        parameter_count: palette.parameter_count(),
        values: palette.values.clone(),
    })
}

fn kinds_chip_from(kinds: &[PaletteKind]) -> String {
    if kinds.is_empty() {
        "(empty)".into()
    } else {
        kinds
            .iter()
            .map(|k| k.label())
            .collect::<Vec<_>>()
            .join("·")
    }
}

enum CellState {
    LinkedToThis,
    LinkedToOther { other_name: String },
    NotLinked,
}

struct MembershipCell {
    kind: PaletteKind,
    state: CellState,
}

struct MembershipRow {
    snapshot_id: Uuid,
    snapshot_name: String,
    cells: Vec<MembershipCell>,
}

fn read_membership_rows(
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    pid: Uuid,
    channel: &ChannelId,
    kinds: &[PaletteKind],
) -> Vec<MembershipRow> {
    let Ok(cue_mgr) = cue_manager.try_read() else {
        return Vec::new();
    };
    let pmgr = palette_manager.try_read().ok();

    let mut snaps: Vec<_> = cue_mgr.snapshots.values().collect();
    snaps.sort_by(|a, b| a.name.cmp(&b.name));

    snaps
        .into_iter()
        .map(|snap| {
            let cells = kinds
                .iter()
                .map(|&k| {
                    let key = (channel.clone(), k);
                    let state = match snap.palette_refs.get(&key) {
                        Some(other) if *other == pid => CellState::LinkedToThis,
                        Some(other) => {
                            let name = pmgr
                                .as_ref()
                                .and_then(|p| p.get_palette(other).map(|x| x.name.clone()))
                                .unwrap_or_else(|| "(unknown)".into());
                            CellState::LinkedToOther { other_name: name }
                        }
                        None => CellState::NotLinked,
                    };
                    MembershipCell { kind: k, state }
                })
                .collect();
            MembershipRow {
                snapshot_id: snap.id,
                snapshot_name: snap.name.clone(),
                cells,
            }
        })
        .collect()
}

// ─── Mutations ─────────────────────────────────────────────────

fn capture_palette(
    state: &mut PalettesUiState,
    console_state: &Arc<RwLock<ConsoleState>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let Ok(ch_num) = state.capture_channel_number.parse::<u8>() else {
        state.status_message = Some("Invalid channel number".into());
        return;
    };
    let channel = state.capture_channel_type.to_channel_id(ch_num);
    let kinds: Vec<PaletteKind> = PaletteKind::all()
        .iter()
        .copied()
        .enumerate()
        .filter_map(|(i, k)| {
            if state.capture_kinds[i] {
                Some(k)
            } else {
                None
            }
        })
        .collect();
    if kinds.is_empty() {
        state.status_message = Some("Select at least one kind to capture".into());
        return;
    }
    let name = state.new_palette_name.clone();
    let st = console_state.clone();
    let pmgr = palette_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        let state_guard = st.read().await;
        let mut merged = HashMap::new();
        for k in &kinds {
            let section_values = state_guard.capture_section(&channel, k.section());
            merged.extend(section_values);
        }
        let param_count = merged.len();
        drop(state_guard);

        let palette = ChannelPalette::new(name.clone(), channel, &kinds, merged);
        pmgr.write().await.add_palette(palette);

        let _ = tx.send(UiEvent::PaletteCaptured { name, param_count });
    });

    state.status_message = Some(format!("Capturing '{}'...", state.new_palette_name));
    state.new_palette_name.clear();
}

fn recapture_palette(
    palette_id: Uuid,
    console_state: &Arc<RwLock<ConsoleState>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let st = console_state.clone();
    let pmgr = palette_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        let mgr = pmgr.read().await;
        let Some(palette) = mgr.get_palette(&palette_id) else {
            return;
        };
        let channel = palette.channel.clone();
        let name = palette.name.clone();
        let kinds = palette.kinds();
        let affected_count = palette.referencing_snapshots.len();
        drop(mgr);

        if kinds.is_empty() {
            return;
        }

        let state_guard = st.read().await;
        let mut merged = HashMap::new();
        for k in &kinds {
            let section_values = state_guard.capture_section(&channel, k.section());
            merged.extend(section_values);
        }
        drop(state_guard);

        let allowed: Vec<ParameterSection> = kinds.iter().map(|k| k.section()).collect();
        let filtered: HashMap<_, _> = merged
            .into_iter()
            .filter(|(p, _)| allowed.contains(&p.section()))
            .collect();

        let mut mgr = pmgr.write().await;
        if let Some(palette) = mgr.get_palette_mut(&palette_id) {
            palette.values = filtered;
            palette.touch();
        }

        let _ = tx.send(UiEvent::PaletteUpdated {
            name,
            affected_count,
        });
    });
}

fn rename_palette(
    palette_id: Uuid,
    new_name: String,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    runtime: &tokio::runtime::Handle,
) {
    let pmgr = palette_manager.clone();
    runtime.spawn(async move {
        let mut mgr = pmgr.write().await;
        if let Some(p) = mgr.get_palette_mut(&palette_id) {
            let trimmed = new_name.trim();
            if !trimmed.is_empty() && p.name != trimmed {
                p.name = trimmed.to_string();
                p.touch();
            }
        }
    });
}

fn delete_palette(
    palette_id: Uuid,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    runtime: &tokio::runtime::Handle,
) {
    let pmgr = palette_manager.clone();
    let cue_mgr = cue_manager.clone();

    runtime.spawn(async move {
        // Clear forward refs from every snapshot.
        let mut mgr = cue_mgr.write().await;
        for snapshot in mgr.snapshots.values_mut() {
            snapshot.palette_refs.retain(|_, pid| *pid != palette_id);
        }
        drop(mgr);

        pmgr.write().await.remove_palette(palette_id);
    });
}

#[allow(clippy::too_many_arguments)]
fn link_palette(
    palette_id: Uuid,
    snapshot_id: Uuid,
    channel: ChannelId,
    kind: PaletteKind,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let cue_mgr = cue_manager.clone();
    let pmgr = palette_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        // Set palette_refs on the snapshot, replacing any prior link for
        // the same (channel, kind).
        let mut mgr = cue_mgr.write().await;
        let (palette_name, snapshot_name) =
            if let Some(snapshot) = mgr.snapshots.get_mut(&snapshot_id) {
                // If there was a previous palette for this (channel, kind),
                // unlink the old back-reference before inserting the new.
                let key = (channel.clone(), kind);
                if let Some(old_pid) = snapshot.palette_refs.insert(key, palette_id) {
                    if old_pid != palette_id {
                        let mut p = pmgr.write().await;
                        p.unlink_from_snapshot(old_pid, snapshot_id);
                        p.link_to_snapshot(palette_id, snapshot_id);
                    }
                } else {
                    pmgr.write().await.link_to_snapshot(palette_id, snapshot_id);
                }

                let sname = snapshot.name.clone();
                let pname = pmgr
                    .read()
                    .await
                    .get_palette(&palette_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "?".into());
                (pname, sname)
            } else {
                return;
            };
        drop(mgr);

        let _ = tx.send(UiEvent::PaletteLinked {
            palette_name,
            snapshot_name,
        });
    });
}

fn unlink_palette(
    palette_id: Uuid,
    snapshot_id: Uuid,
    channel: ChannelId,
    kind: PaletteKind,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    runtime: &tokio::runtime::Handle,
) {
    let cue_mgr = cue_manager.clone();
    let pmgr = palette_manager.clone();

    runtime.spawn(async move {
        // Remove the forward ref for this (channel, kind). The palette's
        // back-ref only points to a snapshot ID, so we drop it only when
        // no remaining slot on the same snapshot still uses this palette.
        let mut mgr = cue_mgr.write().await;
        let still_referenced = if let Some(snapshot) = mgr.snapshots.get_mut(&snapshot_id) {
            snapshot.palette_refs.remove(&(channel, kind));
            snapshot.palette_refs.values().any(|pid| *pid == palette_id)
        } else {
            true
        };
        drop(mgr);

        if !still_referenced {
            pmgr.write()
                .await
                .unlink_from_snapshot(palette_id, snapshot_id);
        }
    });
}
