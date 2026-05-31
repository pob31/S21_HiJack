//! Palettes UI section embedded in the Snapshots tab.
//!
//! A palette can hold EQ, Dyn1, and/or Dyn2 values for one channel — the
//! capture form lets the operator pick which processes to include. Selecting a
//! palette opens a temporary "Assign to snapshots" overlay (floating right, over
//! the middle column): each snapshot row has one checkbox per kind the palette
//! covers, before its (truncated) name. Ticking links the palette on that
//! `(channel, kind)`; unticking unlinks. When another palette currently occupies
//! a slot the cell hints "currently: <other>" so the operator sees the swap.

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
use crate::model::parameter::{PaletteKind, ParameterSection};
use crate::model::state::ConsoleState;

/// Space (px) left below the palette list so the status line stays visible
/// when the list fills the column. Mirrors the snapshots/cue lists.
const PALETTE_LIST_BOTTOM_RESERVE: f32 = 40.0;
/// Minimum palette-list height on short windows; the outer column ScrollArea
/// takes over scrolling below this.
const PALETTE_LIST_MIN_HEIGHT: f32 = 120.0;

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
        theme::row_label(ui, "Channel:", theme::label_color());
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

        theme::row_label(ui, "Name:", theme::label_color());
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
        theme::row_label(ui, "Include:", theme::label_color());
        for (i, kind) in PaletteKind::all().iter().enumerate() {
            ui.checkbox(&mut state.capture_kinds[i], kind.label());
        }

        let any_kind = state.capture_kinds.iter().any(|on| *on);
        let can_capture = is_connected && !state.new_palette_name.is_empty() && any_kind;
        if theme::row_action_button(ui, "Capture", theme::ACCENT_GREEN, 90.0, can_capture) {
            capture_palette(state, console_state, palette_manager, runtime, ui_tx);
        }
    });

    ui.add_space(6.0);

    // Resolve the selected palette once (owned — no manager lock held across
    // the render closures). Clear a stale selection if it vanished.
    let selected = state
        .selected_palette_id
        .and_then(|pid| read_palette_info(palette_manager, pid).map(|info| (pid, info)));
    if state.selected_palette_id.is_some() && selected.is_none() {
        state.selected_palette_id = None;
    }

    // Action row for the selected palette (rename / Re-capture / Delete),
    // dimmed when nothing is selected — kept on top.
    draw_palette_actions(
        ui,
        state,
        selected.as_ref(),
        console_state,
        cue_manager,
        palette_manager,
        is_connected,
        runtime,
        ui_tx,
    );

    ui.add_space(8.0);

    // Anchor for the temporary assign overlay: just right of column 1, at the
    // palette list's top, so it floats over the middle column.
    let overlay_pos = egui::pos2(ui.max_rect().right() + 8.0, ui.cursor().top());

    // ── Palette list (fills the remaining column height) ──
    let list_height =
        (ui.available_height() - PALETTE_LIST_BOTTOM_RESERVE).max(PALETTE_LIST_MIN_HEIGHT);
    egui::ScrollArea::vertical()
        .id_salt("palette_list_scroll")
        .max_height(list_height)
        .show(ui, |ui| {
            if let Ok(mgr) = palette_manager.try_read() {
                let palettes = mgr.sorted_palettes();
                if palettes.is_empty() {
                    ui.label(
                        egui::RichText::new("No palettes yet. Capture one above.")
                            .color(theme::label_weak()),
                    );
                }
                for palette in palettes {
                    let selected = state.selected_palette_id == Some(palette.id);
                    let bg = if selected {
                        theme::bg_elevated()
                    } else {
                        theme::bg_panel()
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
                                            .color(theme::label_color()),
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
                                        .color(theme::label_weak())
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
                    if clicked {
                        // Toggle: re-clicking the selected palette deselects it.
                        state.selected_palette_id = if state.selected_palette_id == Some(palette.id)
                        {
                            None
                        } else {
                            Some(palette.id)
                        };
                        state.rename_draft = None;
                    }
                    ui.add_space(1.0);
                }
            }
        });

    // Temporary "Assign to snapshots" overlay while a palette is selected —
    // floats right, overlapping the middle column.
    if let Some((pid, info)) = &selected {
        draw_assign_overlay(
            ui.ctx(),
            overlay_pos,
            *pid,
            info,
            cue_manager,
            palette_manager,
            runtime,
            ui_tx,
        );
    }

    // Status
    if let Some(msg) = &state.status_message {
        ui.add_space(2.0);
        ui.colored_label(theme::TEXT_WARNING, msg);
    }
}

/// Action row for the selected palette — rename, channel, Re-capture, Delete.
/// Dimmed (disabled) when no palette is selected, so it holds a stable position
/// on top of column 1. The snapshot-assign UI lives in `draw_assign_overlay`.
#[allow(clippy::too_many_arguments)]
fn draw_palette_actions(
    ui: &mut egui::Ui,
    state: &mut PalettesUiState,
    selected: Option<&(Uuid, PaletteInfo)>,
    console_state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    is_connected: bool,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    ui.add_enabled_ui(selected.is_some(), |ui| {
        let Some((pid, palette_info)) = selected else {
            // No palette selected — dimmed placeholder occupying the action
            // row so the layout reads the same as when one is selected.
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                theme::row_label(ui, "Name:", theme::label_color());
                let mut empty = String::new();
                let _ =
                    theme::padded_text_edit_sized(ui, &mut empty, 160.0, theme::ROW_H, false, "");
                let _ = theme::row_action_button(ui, "Re-capture", theme::ACCENT_BLUE, 90.0, false);
                let _ =
                    theme::row_action_button(ui, "Delete Palette", theme::ACCENT_RED, 100.0, false);
            });
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "Select a palette below to edit it and assign it to snapshots.",
                )
                .color(theme::label_weak()),
            );
            return;
        };
        let pid = *pid;

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            theme::row_label(ui, "Name:", theme::label_color());
            // Draft buffer is only present while editing; fall back to the
            // stored name. We bind the TextEdit to a local string and write
            // through to the draft on change.
            let mut buf = match &state.rename_draft {
                Some((id, draft)) if *id == pid => draft.clone(),
                _ => palette_info.name.clone(),
            };
            let resp = theme::padded_text_edit_sized(ui, &mut buf, 160.0, theme::ROW_H, true, "");
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

            // Channel the palette belongs to.
            ui.label(
                egui::RichText::new(format!("{}", palette_info.channel)).color(theme::label_weak()),
            );

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
    });
}

/// Temporary "Assign to snapshots" overlay shown while a palette is selected.
/// Floats just right of column 1 (overlapping the middle column). Each row is
/// one checkbox per palette kind, placed BEFORE the (truncated) snapshot name;
/// a small header labels the kind columns. Reuses the same membership read +
/// link/unlink as the old inline grid.
#[allow(clippy::too_many_arguments)]
fn draw_assign_overlay(
    ctx: &egui::Context,
    pos: egui::Pos2,
    pid: Uuid,
    info: &PaletteInfo,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    /// Width of one kind checkbox column (header label + checkbox align here).
    const CB_W: f32 = 30.0;
    /// Overlay content width.
    const CONTENT_W: f32 = 300.0;

    if info.kinds.is_empty() {
        return;
    }
    let rows = read_membership_rows(
        cue_manager,
        palette_manager,
        pid,
        &info.channel,
        &info.kinds,
    );

    egui::Area::new(egui::Id::new("palette_assign_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(CONTENT_W);
                ui.label(
                    egui::RichText::new(format!("Assign ‘{}’ to snapshots", info.name)).strong(),
                );
                ui.label(
                    egui::RichText::new(format!("{}", info.channel))
                        .color(theme::label_weak())
                        .small(),
                );
                ui.separator();

                if rows.is_empty() {
                    ui.label(egui::RichText::new("No snapshots yet.").color(theme::label_weak()));
                    return;
                }

                // Header: one kind label per checkbox column, then "Snapshot".
                ui.horizontal(|ui| {
                    for k in &info.kinds {
                        ui.allocate_ui_with_layout(
                            egui::vec2(CB_W, 16.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(k.label())
                                        .small()
                                        .strong()
                                        .color(theme::label_weak()),
                                );
                            },
                        );
                    }
                    ui.label(
                        egui::RichText::new("Snapshot")
                            .small()
                            .color(theme::label_weak()),
                    );
                });

                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for row in &rows {
                            ui.horizontal(|ui| {
                                for cell in &row.cells {
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(CB_W, 18.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            let mut on =
                                                matches!(cell.state, CellState::LinkedToThis);
                                            let resp = ui.checkbox(&mut on, "");
                                            if let CellState::LinkedToOther { other_name } =
                                                &cell.state
                                            {
                                                resp.clone().on_hover_text(format!(
                                                    "Currently: \"{other_name}\""
                                                ));
                                            }
                                            if resp.changed() {
                                                if on {
                                                    link_palette(
                                                        pid,
                                                        row.snapshot_id,
                                                        info.channel.clone(),
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
                                                        info.channel.clone(),
                                                        cell.kind,
                                                        cue_manager,
                                                        palette_manager,
                                                        runtime,
                                                    );
                                                }
                                            }
                                        },
                                    );
                                }
                                let name_w = ui.available_width().max(1.0);
                                ui.add_sized(
                                    [name_w, 18.0],
                                    egui::Label::new(
                                        egui::RichText::new(&row.snapshot_name)
                                            .color(theme::label_color()),
                                    )
                                    .truncate(),
                                );
                            });
                        }
                    });
            });
        });
}

// ─── Render-time data snapshots ────────────────────────────────

struct PaletteInfo {
    name: String,
    channel: ChannelId,
    kinds: Vec<PaletteKind>,
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
    })
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
