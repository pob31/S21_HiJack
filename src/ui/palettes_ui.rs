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
use super::help::{HelpHover, HelpKey, help};
use super::status::StatusMessage;
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

    /// Whether `ch`'s type variant matches this choice (ignoring the number).
    fn matches_channel(&self, ch: &ChannelId) -> bool {
        matches!(
            (self, ch),
            (Self::Input, ChannelId::Input(_))
                | (Self::Aux, ChannelId::Aux(_))
                | (Self::Group, ChannelId::Group(_))
                | (Self::Matrix, ChannelId::Matrix(_))
        )
    }
}

/// Whether a palette should appear in the list given the capture-form inputs.
/// Cumulative AND: the channel TYPE always applies (the combo is never empty);
/// a parsed channel NUMBER narrows to that exact channel; a non-empty NAME
/// narrows by case-insensitive substring (independent of the number).
fn palette_matches_filter(
    p: &ChannelPalette,
    type_choice: ChannelTypeChoice,
    number: Option<u8>,
    name_query: &str,
) -> bool {
    if !type_choice.matches_channel(&p.channel) {
        return false;
    }
    if let Some(num) = number {
        if p.channel != type_choice.to_channel_id(num) {
            return false;
        }
    }
    let q = name_query.trim().to_lowercase();
    if !q.is_empty() && !p.name.to_lowercase().contains(&q) {
        return false;
    }
    true
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
    /// One-shot flag set when the channel-number commit just auto-filled the
    /// palette name: on the next frame the name field grabs focus and selects
    /// all its text so the operator can type over it or keep it. Cleared once
    /// applied.
    pub autofill_select: bool,
    /// `(type, number, suggestion)` of the last name auto-fill. Used to avoid
    /// re-firing on a plain re-focus of the same channel number, and — crucially
    /// — to avoid clobbering a name the operator has since typed (we only
    /// overwrite when the field is empty or still exactly our last suggestion).
    pub last_autofill: Option<(ChannelTypeChoice, u8, String)>,
    pub status_message: Option<StatusMessage>,
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
            autofill_select: false,
            last_autofill: None,
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
            let combo = egui::ComboBox::from_id_salt("palette_capture_ch_type")
                .selected_text(state.capture_channel_type.label())
                .width(70.0)
                .show_ui(ui, |ui| {
                    for ch in ChannelTypeChoice::ALL {
                        ui.selectable_value(&mut state.capture_channel_type, ch, ch.label());
                    }
                });
            combo
                .response
                .on_hover_text(help(HelpKey::PaletteChannelType));
        });
        let num_resp = theme::padded_text_edit_sized(
            ui,
            &mut state.capture_channel_number,
            40.0,
            theme::ROW_H,
            true,
            "",
        )
        .on_hover_text(help(HelpKey::PaletteChannelNumber));
        // On commit (Enter / focus-loss), auto-fill the name with
        // "<number> <channel name>" and mark it for pre-selection so the
        // operator can type over it or keep it. Only fires when this channel
        // number wasn't just filled AND the name field is empty or still our
        // last suggestion — so a typed custom name is never clobbered (e.g. when
        // the number field loses focus to the Capture button).
        if num_resp.lost_focus() {
            if let Ok(num) = state.capture_channel_number.parse::<u8>() {
                let already = matches!(
                    &state.last_autofill,
                    Some((t, n, _)) if *t == state.capture_channel_type && *n == num
                );
                let name_is_ours = state.new_palette_name.trim().is_empty()
                    || state
                        .last_autofill
                        .as_ref()
                        .is_some_and(|(_, _, s)| *s == state.new_palette_name);
                if !already && name_is_ours {
                    let channel = state.capture_channel_type.to_channel_id(num);
                    let name = console_state.try_read().ok().and_then(|s| {
                        s.channel_names_for(std::slice::from_ref(&channel))
                            .get(&channel)
                            .cloned()
                    });
                    let suggestion = match name {
                        Some(n) if !n.trim().is_empty() => format!("{num} {n}"),
                        _ => format!("{num}"),
                    };
                    state.new_palette_name = suggestion.clone();
                    state.last_autofill = Some((state.capture_channel_type, num, suggestion));
                    state.autofill_select = true;
                }
            }
        }

        theme::row_label(ui, "Name:", theme::label_color());
        let name_resp = theme::padded_text_edit_sized(
            ui,
            &mut state.new_palette_name,
            140.0,
            theme::ROW_H,
            true,
            "",
        )
        .on_hover_text(help(HelpKey::PaletteName));
        // Apply the one-shot pre-selection: focus the name field and select all
        // its text so the auto-filled suggestion is ready to type over.
        if state.autofill_select {
            state.autofill_select = false;
            name_resp.request_focus();
            let len = state.new_palette_name.chars().count();
            let mut ts =
                egui::text_edit::TextEditState::load(ui.ctx(), name_resp.id).unwrap_or_default();
            ts.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(len),
            )));
            ts.store(ui.ctx(), name_resp.id);
        }
    });

    ui.horizontal(|ui| {
        theme::row_label(ui, "Include:", theme::label_color());
        for (i, kind) in PaletteKind::all().iter().enumerate() {
            ui.checkbox(&mut state.capture_kinds[i], kind.label())
                .on_hover_text(help(HelpKey::PaletteInclude));
        }

        let any_kind = state.capture_kinds.iter().any(|on| *on);
        let can_capture = is_connected && !state.new_palette_name.is_empty() && any_kind;
        if theme::row_action_button(
            ui,
            "Capture",
            theme::ACCENT_GREEN,
            90.0,
            can_capture,
            help(HelpKey::PaletteCapture),
        ) {
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

    // Master "make all rippled palette adjustments permanent" — global (lives
    // outside the per-palette panel). Enabled when any palette has un-stored
    // in-session changes; the end-of-show "save everything" affordance.
    let working_total = palette_manager
        .try_read()
        .map(|m| m.palettes_with_working_changes())
        .unwrap_or(0);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if theme::row_action_button(
            ui,
            "Store all palette changes",
            theme::ACCENT_GREEN,
            180.0,
            working_total > 0,
            help(HelpKey::PaletteStoreAll),
        ) {
            store_all_palette_changes(palette_manager, runtime);
            state.status_message =
                Some(format!("Stored changes to {working_total} palette(s)").into());
        }
        if working_total > 0 {
            ui.label(
                egui::RichText::new(format!("● {working_total} with unsaved changes"))
                    .color(theme::TEXT_WARNING)
                    .small(),
            );
        }
    });

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
                // Filter the list to the capture form's channel type / number /
                // name so the operator sees only palettes relevant to what they
                // are building (see `palette_matches_filter`).
                let number = state.capture_channel_number.parse::<u8>().ok();
                let type_choice = state.capture_channel_type;
                let name_query = state.new_palette_name.clone();
                let total = mgr.palettes.len();
                let palettes: Vec<&ChannelPalette> = mgr
                    .sorted_palettes()
                    .into_iter()
                    .filter(|p| palette_matches_filter(p, type_choice, number, &name_query))
                    .collect();
                if palettes.is_empty() {
                    let msg = if total == 0 {
                        "No palettes yet. Capture one above."
                    } else {
                        "No palettes match the current channel / name."
                    };
                    ui.label(egui::RichText::new(msg).color(theme::label_weak()))
                        .on_hover_help_inline(HelpKey::PaletteInfoEmpty);
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
                                // Name — left-justified.
                                let r_name = ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&palette.name)
                                            .strong()
                                            .color(theme::label_color()),
                                    )
                                    .sense(egui::Sense::click()),
                                );
                                // Metadata + unsaved marker — right-justified,
                                // like the cue list. A click-sensing strip fills
                                // the gap so clicks anywhere on the row select.
                                let mut r_meta: Option<egui::Response> = None;
                                let mut r_work: Option<egui::Response> = None;
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if palette.has_working_changes() {
                                            r_work = Some(
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(format!(
                                                            "● {} unsaved",
                                                            palette.working_count()
                                                        ))
                                                        .color(theme::TEXT_WARNING)
                                                        .small(),
                                                    )
                                                    .sense(egui::Sense::click()),
                                                ),
                                            );
                                        }
                                        r_meta = Some(
                                            ui.add(
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
                                            ),
                                        );
                                        // Fill the gap between name and meta with
                                        // a click-sensing strip.
                                        let fill_w = ui.available_width().max(1.0);
                                        let (_, r_fill) = ui.allocate_exact_size(
                                            egui::Vec2::new(fill_w, 1.0),
                                            egui::Sense::click(),
                                        );
                                        if r_fill.clicked() {
                                            clicked = true;
                                        }
                                    },
                                );
                                if r_name.clicked()
                                    || r_meta.is_some_and(|r| r.clicked())
                                    || r_work.is_some_and(|r| r.clicked())
                                {
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
        let resp = ui.colored_label(theme::TEXT_WARNING, &msg.text);
        if let Some(key) = msg.help {
            resp.on_hover_help_inline(key);
        }
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
                let _ = theme::row_long_press_button_hover(
                    ui,
                    "Re-capture",
                    theme::ACCENT_BLUE,
                    90.0,
                    false,
                    theme::long_press_hover(help(HelpKey::PaletteRecapture)),
                );
                let _ = theme::row_long_press_button_hover(
                    ui,
                    "Delete",
                    theme::ACCENT_RED,
                    70.0,
                    false,
                    theme::long_press_hover(help(HelpKey::PaletteDelete)),
                );
            });
            // Placeholder "Store changes" / "Revert changes" row so the layout
            // (and the palette list below) doesn't shift when a palette becomes
            // selected.
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let _ = theme::row_action_button(
                    ui,
                    "Store changes",
                    theme::ACCENT_GREEN,
                    110.0,
                    false,
                    help(HelpKey::PaletteStoreChanges),
                );
                let _ = theme::row_action_button(
                    ui,
                    "Revert changes",
                    theme::ACCENT_AMBER,
                    110.0,
                    false,
                    help(HelpKey::PaletteRevert),
                );
            });
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "Select a palette below to edit it and assign it to snapshots.",
                )
                .color(theme::label_weak()),
            )
            .on_hover_help_inline(HelpKey::PaletteInfoSelectHint);
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
            let resp = theme::padded_text_edit_sized(ui, &mut buf, 160.0, theme::ROW_H, true, "")
                .on_hover_text(help(HelpKey::PaletteRename));
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

            if theme::row_long_press_button_hover(
                ui,
                "Re-capture",
                theme::ACCENT_BLUE,
                90.0,
                is_connected,
                theme::long_press_hover(help(HelpKey::PaletteRecapture)),
            ) {
                recapture_palette(pid, console_state, palette_manager, runtime, ui_tx);
            }
            if theme::row_long_press_button_hover(
                ui,
                "Delete",
                theme::ACCENT_RED,
                70.0,
                true,
                theme::long_press_hover(help(HelpKey::PaletteDelete)),
            ) {
                delete_palette(pid, cue_manager, palette_manager, runtime);
                state.selected_palette_id = None;
                state.rename_draft = None;
                state.status_message = Some("Palette deleted".into());
            }
        });

        // Second action row: commit this palette's in-session adjustments into
        // its permanent values (so they save with the show), or revert them by
        // reloading the last-captured values onto the channel. Enabled only
        // while the live overlay has changes.
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            if theme::row_action_button(
                ui,
                "Store changes",
                theme::ACCENT_GREEN,
                110.0,
                palette_info.has_working,
                help(HelpKey::PaletteStoreChanges),
            ) {
                // Same effect as Re-capture: read the channel's current console
                // state into the palette (and clear the overlay). Reading the
                // live mirror — rather than draining the ~150 ms-lagging overlay
                // — guarantees Store captures exactly what's on the desk now, so
                // the stored state can't lag behind the operator's last tweak.
                recapture_palette(pid, console_state, palette_manager, runtime, ui_tx);
                state.status_message =
                    Some(format!("Stored changes to '{}'", palette_info.name).into());
            }
            if theme::row_action_button(
                ui,
                "Revert changes",
                theme::ACCENT_AMBER,
                110.0,
                palette_info.has_working,
                help(HelpKey::PaletteRevert),
            ) {
                let _ = ui_tx.send(UiEvent::PaletteRevert { palette_id: pid });
                state.status_message = Some(
                    format!("Reverted '{}' to last captured values", palette_info.name).into(),
                );
            }
            if palette_info.has_working {
                ui.label(
                    egui::RichText::new(format!("● {} unsaved", palette_info.working_count))
                        .color(theme::TEXT_WARNING)
                        .small(),
                );
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
    /// Overlay content width.
    const CONTENT_W: f32 = 300.0;
    /// Fixed width of each kind (Eq/Dyn1/Dyn2) column, so its header label and
    /// checkbox share one centred column.
    const KIND_COL_W: f32 = 44.0;
    /// Height of a centred grid cell (comfortable click target).
    const CELL_H: f32 = 22.0;

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
                    ui.label(egui::RichText::new("No snapshots yet.").color(theme::label_weak()))
                        .on_hover_help_inline(HelpKey::PaletteInfoNoSnapshots);
                    return;
                }

                // Header and rows share one Grid so the kind checkbox columns
                // and the snapshot-name column line up. (The name column used to
                // be drawn with `add_sized`, which centres the label in the
                // remaining width and floated names far right of the header.)
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // The default stripe colour (`faint_bg_color`) is
                        // `bg_elevated`, the same grey as an unchecked checkbox's
                        // fill — so toggles vanish on striped rows. Use the panel
                        // grey instead: still a clear stripe over the popup's
                        // darker base, but distinct from the toggle grey.
                        ui.visuals_mut().faint_bg_color = theme::bg_panel();
                        egui::Grid::new("palette_assign_grid")
                            .num_columns(info.kinds.len() + 1)
                            .spacing([8.0, 4.0])
                            .striped(true)
                            .show(ui, |ui| {
                                // Header row: a clickable kind label per checkbox
                                // column (click = toggle that column for every
                                // snapshot), then a clickable "Snapshot" header
                                // (click = toggle every checkbox). Header label
                                // and checkboxes share a fixed-width centred cell
                                // so they line up.
                                for k in &info.kinds {
                                    let resp = ui
                                        .allocate_ui_with_layout(
                                            egui::vec2(KIND_COL_W, CELL_H),
                                            egui::Layout::top_down(egui::Align::Center),
                                            |ui| {
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(k.label())
                                                            .small()
                                                            .strong()
                                                            .color(theme::label_weak()),
                                                    )
                                                    .sense(egui::Sense::click()),
                                                )
                                            },
                                        )
                                        .inner
                                        .on_hover_text(
                                            "Click to assign / clear this column for all snapshots",
                                        );
                                    if resp.clicked() {
                                        let all_on = rows.iter().all(|r| {
                                            r.cells.iter().any(|c| {
                                                c.kind == *k
                                                    && matches!(c.state, CellState::LinkedToThis)
                                            })
                                        });
                                        let targets: Vec<(Uuid, PaletteKind)> =
                                            rows.iter().map(|r| (r.snapshot_id, *k)).collect();
                                        bulk_assign(
                                            !all_on,
                                            pid,
                                            targets,
                                            info.channel.clone(),
                                            cue_manager,
                                            palette_manager,
                                            runtime,
                                            ui_tx,
                                        );
                                    }
                                }
                                let snap_hdr = ui
                                    .add(
                                        egui::Label::new(
                                            egui::RichText::new("Snapshot")
                                                .small()
                                                .color(theme::label_weak()),
                                        )
                                        .sense(egui::Sense::click()),
                                    )
                                    .on_hover_text(
                                        "Click to assign / clear every checkbox for all snapshots",
                                    );
                                if snap_hdr.clicked() {
                                    let all_on = rows.iter().all(|r| {
                                        r.cells
                                            .iter()
                                            .all(|c| matches!(c.state, CellState::LinkedToThis))
                                    });
                                    let targets: Vec<(Uuid, PaletteKind)> = rows
                                        .iter()
                                        .flat_map(|r| {
                                            info.kinds.iter().map(move |k| (r.snapshot_id, *k))
                                        })
                                        .collect();
                                    bulk_assign(
                                        !all_on,
                                        pid,
                                        targets,
                                        info.channel.clone(),
                                        cue_manager,
                                        palette_manager,
                                        runtime,
                                        ui_tx,
                                    );
                                }
                                ui.end_row();

                                // One row per snapshot; `cells` is built 1:1 from
                                // `info.kinds` (see read_membership_rows).
                                for row in &rows {
                                    for cell in &row.cells {
                                        let mut on = matches!(cell.state, CellState::LinkedToThis);
                                        // Centre the checkbox under its column
                                        // header (item 4).
                                        let resp = ui
                                            .allocate_ui_with_layout(
                                                egui::vec2(KIND_COL_W, CELL_H),
                                                egui::Layout::top_down(egui::Align::Center),
                                                |ui| ui.checkbox(&mut on, ""),
                                            )
                                            .inner;
                                        if let CellState::LinkedToOther { other_name } = &cell.state
                                        {
                                            resp.clone().on_hover_text(
                                                help(HelpKey::PaletteLinkedOther)
                                                    .replace("{name}", other_name),
                                            );
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
                                    }
                                    // Clicking the snapshot name toggles all of
                                    // that row's kinds (item 5).
                                    let name_resp = ui
                                        .add(
                                            egui::Label::new(
                                                egui::RichText::new(&row.snapshot_name)
                                                    .color(theme::label_color()),
                                            )
                                            .truncate()
                                            .sense(egui::Sense::click()),
                                        )
                                        .on_hover_text(
                                            "Click to assign / clear all kinds for this snapshot",
                                        );
                                    if name_resp.clicked() {
                                        let all_on = row
                                            .cells
                                            .iter()
                                            .all(|c| matches!(c.state, CellState::LinkedToThis));
                                        let targets: Vec<(Uuid, PaletteKind)> = info
                                            .kinds
                                            .iter()
                                            .map(|k| (row.snapshot_id, *k))
                                            .collect();
                                        bulk_assign(
                                            !all_on,
                                            pid,
                                            targets,
                                            info.channel.clone(),
                                            cue_manager,
                                            palette_manager,
                                            runtime,
                                            ui_tx,
                                        );
                                    }
                                    ui.end_row();
                                }
                            });
                    });
            });
        });
}

// ─── Render-time data snapshots ────────────────────────────────

struct PaletteInfo {
    name: String,
    channel: ChannelId,
    kinds: Vec<PaletteKind>,
    /// Un-stored in-session adjustments present on this palette.
    has_working: bool,
    /// Number of adjusted parameters in the working overlay.
    working_count: usize,
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
        has_working: palette.has_working_changes(),
        working_count: palette.working_count(),
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
        state.status_message = Some(StatusMessage::with_help(
            "Invalid channel number",
            HelpKey::PaletteWarnInvalidChannel,
        ));
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
        state.status_message = Some(StatusMessage::with_help(
            "Select at least one kind to capture",
            HelpKey::PaletteWarnNoKind,
        ));
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

    state.status_message = Some(format!("Capturing '{}'...", state.new_palette_name).into());
    state.new_palette_name.clear();
    // Allow the next commit of the same channel number to auto-fill again.
    state.last_autofill = None;
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
            // The freshly-captured values ARE the new baseline, so any
            // in-session overlay is now stale — drop it. (The live-absorb loop
            // keeps it empty afterwards since live == the captured values.)
            palette.discard_working();
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

/// Commit every palette's in-session overlay (end-of-show "make it all
/// permanent" affordance).
fn store_all_palette_changes(
    palette_manager: &Arc<RwLock<PaletteManager>>,
    runtime: &tokio::runtime::Handle,
) {
    let pmgr = palette_manager.clone();
    runtime.spawn(async move {
        pmgr.write().await.store_all_changes();
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
        // `palette_refs` is a direct snapshot-field write that changes what
        // a recall resolves to — invalidate the look-ahead cache.
        mgr.bump_model_gen();
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
        // `palette_refs` is a direct snapshot-field write that changes what
        // a recall resolves to — invalidate the look-ahead cache.
        mgr.bump_model_gen();
        drop(mgr);

        if !still_referenced {
            pmgr.write()
                .await
                .unlink_from_snapshot(palette_id, snapshot_id);
        }
    });
}

/// Bulk assign (`link = true`) or clear (`link = false`) `palette_id` across
/// many `(snapshot, kind)` slots on `channel` in a single task — powers the
/// assign-overlay header / snapshot-name click toggles (item 5). Assigning a
/// slot replaces any other palette already there (same replace semantics as a
/// single checkbox click); clearing only removes slots that currently point at
/// this palette. All mutations happen under one cue-manager write with a single
/// cache invalidation.
#[allow(clippy::too_many_arguments)]
fn bulk_assign(
    link: bool,
    palette_id: Uuid,
    targets: Vec<(Uuid, PaletteKind)>,
    channel: ChannelId,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    if targets.is_empty() {
        return;
    }
    let cue_mgr = cue_manager.clone();
    let pmgr = palette_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        // Lock order (see auto_save_previous_snapshot): cue_manager before
        // palette_manager. Both held across the loop so the bulk edit is atomic.
        let mut mgr = cue_mgr.write().await;
        let mut pal = pmgr.write().await;
        let mut changed = 0usize;
        for (snapshot_id, kind) in targets {
            let Some(snapshot) = mgr.snapshots.get_mut(&snapshot_id) else {
                continue;
            };
            let key = (channel.clone(), kind);
            if link {
                match snapshot.palette_refs.insert(key, palette_id) {
                    // Already linked to this palette — nothing to do.
                    Some(old) if old == palette_id => {}
                    // Slot stolen from another palette: drop its back-ref if this
                    // snapshot no longer references it anywhere, add ours.
                    Some(old) => {
                        if !snapshot.palette_refs.values().any(|p| *p == old) {
                            pal.unlink_from_snapshot(old, snapshot_id);
                        }
                        pal.link_to_snapshot(palette_id, snapshot_id);
                        changed += 1;
                    }
                    None => {
                        pal.link_to_snapshot(palette_id, snapshot_id);
                        changed += 1;
                    }
                }
            } else if snapshot.palette_refs.get(&key) == Some(&palette_id) {
                snapshot.palette_refs.remove(&key);
                if !snapshot.palette_refs.values().any(|p| *p == palette_id) {
                    pal.unlink_from_snapshot(palette_id, snapshot_id);
                }
                changed += 1;
            }
        }
        // Direct `palette_refs` writes change what recalls resolve to.
        mgr.bump_model_gen();
        let palette_name = pal
            .get_palette(&palette_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "?".into());
        drop(pal);
        drop(mgr);

        if changed > 0 {
            let _ = tx.send(UiEvent::PaletteBulkAssigned {
                palette_name,
                linked: link,
                count: changed,
            });
        }
    });
}
