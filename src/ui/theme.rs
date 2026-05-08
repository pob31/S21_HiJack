use eframe::egui;

use crate::model::channel::ChannelId;

// ─── Touch-friendly sizing constants ───────────────────────────────────

pub const GO_BUTTON_SIZE: egui::Vec2 = egui::Vec2::new(200.0, 100.0);
pub const PREV_BUTTON_SIZE: egui::Vec2 = egui::Vec2::new(120.0, 80.0);
pub const MACRO_BUTTON_SIZE: egui::Vec2 = egui::Vec2::new(120.0, 60.0);

// ─── Font sizes ────────────────────────────────────────────────────────

pub const FONT_SIZE_HEADING: f32 = 28.0;
pub const FONT_SIZE_BODY: f32 = 16.0;
pub const FONT_SIZE_CUE_CURRENT: f32 = 48.0;
pub const FONT_SIZE_CUE_NEXT: f32 = 24.0;
pub const FONT_SIZE_GO_BUTTON: f32 = 36.0;
pub const FONT_SIZE_SECTION: f32 = 20.0;
pub const FONT_SIZE_BADGE: f32 = 13.0;

// ─── Background colors ────────────────────────────────────────────────

pub const BG_DARK: egui::Color32 = egui::Color32::from_rgb(0x1A, 0x1A, 0x1E);
pub const BG_PANEL: egui::Color32 = egui::Color32::from_rgb(0x25, 0x25, 0x28);
pub const BG_ELEVATED: egui::Color32 = egui::Color32::from_rgb(0x2E, 0x2E, 0x32);
pub const BG_INPUT: egui::Color32 = egui::Color32::from_rgb(0x1E, 0x1E, 0x22);
pub const BORDER_SUBTLE: egui::Color32 = egui::Color32::from_rgb(0x3A, 0x3A, 0x3E);
pub const BORDER_FOCUS: egui::Color32 = egui::Color32::from_rgb(0x5A, 0x5A, 0x60);

// ─── Channel type colors (from DiGiCo channel selector) ───────────────

pub const CH_INPUT: egui::Color32 = egui::Color32::from_rgb(0x2D, 0x6E, 0x8E);
pub const CH_AUX: egui::Color32 = egui::Color32::from_rgb(0x8E, 0x3A, 0x8E);
pub const CH_GROUP: egui::Color32 = egui::Color32::from_rgb(0x8E, 0x3A, 0x3A);
pub const CH_MATRIX: egui::Color32 = egui::Color32::from_rgb(0x2E, 0x7D, 0x4F);
pub const CH_CG: egui::Color32 = egui::Color32::from_rgb(0x6B, 0x5D, 0x35);

// ─── UI accent colors ─────────────────────────────────────────────────

pub const ACCENT_GREEN: egui::Color32 = egui::Color32::from_rgb(0x00, 0xB4, 0x00);
pub const ACCENT_RED: egui::Color32 = egui::Color32::from_rgb(0xB4, 0x00, 0x00);
pub const ACCENT_AMBER: egui::Color32 = egui::Color32::from_rgb(0xDC, 0xA8, 0x00);
pub const ACCENT_BLUE: egui::Color32 = egui::Color32::from_rgb(0x2D, 0x8B, 0xC9);
pub const ACCENT_ORANGE: egui::Color32 = egui::Color32::from_rgb(0xC8, 0x75, 0x32);

// ─── Status colors (backward-compatible aliases) ──────────────────────

pub const COLOR_CONNECTED: egui::Color32 = ACCENT_GREEN;
pub const COLOR_CONNECTING: egui::Color32 = ACCENT_AMBER;
pub const COLOR_DISCONNECTED: egui::Color32 = ACCENT_RED;
pub const COLOR_GO_BUTTON: egui::Color32 = ACCENT_GREEN;
pub const COLOR_PREV_BUTTON: egui::Color32 = ACCENT_AMBER;
pub const COLOR_RECORDING: egui::Color32 = egui::Color32::from_rgb(220, 0, 0);
pub const COLOR_RECORDING_BG: egui::Color32 = egui::Color32::from_rgb(60, 0, 0);

// ─── Cue highlight colors ─────────────────────────────────────────────

pub const CUE_CURRENT_BG: egui::Color32 = egui::Color32::from_rgb(0x3A, 0x1A, 0x1A);
pub const CUE_CURRENT_BORDER: egui::Color32 = egui::Color32::from_rgb(0x6A, 0x2A, 0x2A);

// ─── Scope toggle block colors ────────────────────────────────────────

pub const SCOPE_ACTIVE: egui::Color32 = ACCENT_GREEN;
pub const SCOPE_INACTIVE: egui::Color32 = egui::Color32::from_rgb(0x3A, 0x3A, 0x3E);
pub const SCOPE_PARTIAL: egui::Color32 = egui::Color32::from_rgb(0x00, 0x5A, 0x00);
/// Cell color for (channel, parameter) pairs that have no live data on the
/// console — they cannot be captured, so the editor renders them as
/// non-interactive grey outlines.
pub const SCOPE_UNAVAILABLE: egui::Color32 = egui::Color32::from_rgb(0x22, 0x22, 0x24);
/// Slightly lighter grey for unselected cells the console will already recall
/// (in session scope AND not channel-safed). Earmark, not a warning — the
/// user can still add these to the app's scope template.
pub const SCOPE_INACTIVE_RECALLED: egui::Color32 = egui::Color32::from_rgb(0x52, 0x52, 0x58);
/// Earmark color used by Phase C's dirty tracker (golden triangle in cell corner).
pub const SCOPE_DIRTY: egui::Color32 = egui::Color32::from_rgb(0xE6, 0xB8, 0x00);

// ─── Text colors ──────────────────────────────────────────────────────

pub const TEXT_PRIMARY: egui::Color32 = egui::Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub const TEXT_SECONDARY: egui::Color32 = egui::Color32::from_rgb(0x88, 0x88, 0x88);
pub const TEXT_DISABLED: egui::Color32 = egui::Color32::from_rgb(0x55, 0x55, 0x55);
pub const TEXT_WARNING: egui::Color32 = ACCENT_AMBER;

// ─── Style configuration ──────────────────────────────────────────────

/// Configure egui style with the DiGiCo-inspired dark theme.
pub fn configure_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    // Spacing
    style.spacing.button_padding = egui::Vec2::new(12.0, 8.0);
    style.spacing.item_spacing = egui::Vec2::new(10.0, 8.0);

    // Font sizes
    style.text_styles.insert(
        egui::TextStyle::Body,
        egui::FontId::proportional(FONT_SIZE_BODY),
    );
    style.text_styles.insert(
        egui::TextStyle::Heading,
        egui::FontId::proportional(FONT_SIZE_HEADING),
    );
    style.text_styles.insert(
        egui::TextStyle::Button,
        egui::FontId::proportional(FONT_SIZE_BODY),
    );
    style.text_styles.insert(
        egui::TextStyle::Small,
        egui::FontId::proportional(FONT_SIZE_BADGE),
    );

    // Dark visuals
    let mut visuals = egui::Visuals::dark();

    visuals.panel_fill = BG_PANEL;
    visuals.window_fill = BG_DARK;
    visuals.extreme_bg_color = BG_DARK;
    visuals.faint_bg_color = BG_ELEVATED;
    visuals.code_bg_color = BG_INPUT;

    // Selection. `selection.stroke.color` is what egui copies into
    // `WidgetVisuals::fg_stroke.color` for selected `selectable_label`
    // items — combobox dropdowns, scope toggles, etc. Use white instead
    // of the accent so the selected item's text reads against the
    // translucent-blue selection background instead of merging into it.
    visuals.selection.bg_fill = egui::Color32::from_rgba_premultiplied(0x2D, 0x8B, 0xC9, 80);
    visuals.selection.stroke = egui::Stroke::new(1.0, TEXT_PRIMARY);

    // Hyperlinks
    visuals.hyperlink_color = ACCENT_BLUE;

    // Widget visuals — inactive state
    visuals.widgets.inactive.bg_fill = BG_ELEVATED;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, BORDER_SUBTLE);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT_SECONDARY);
    visuals.widgets.inactive.weak_bg_fill = BG_ELEVATED;

    // Widget visuals — hovered state
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(0x38, 0x38, 0x3E);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, BORDER_FOCUS);
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(0x38, 0x38, 0x3E);

    // Widget visuals — active (clicked) state
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(0x40, 0x40, 0x48);
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT_BLUE);
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.active.weak_bg_fill = egui::Color32::from_rgb(0x40, 0x40, 0x48);

    // Widget visuals — open (expanded ComboBox, etc.)
    visuals.widgets.open.bg_fill = BG_ELEVATED;
    visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0, ACCENT_BLUE);
    visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.open.weak_bg_fill = BG_ELEVATED;

    // Widget visuals — non-interactive
    visuals.widgets.noninteractive.bg_fill = BG_PANEL;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(0.5, BORDER_SUBTLE);
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT_SECONDARY);
    visuals.widgets.noninteractive.weak_bg_fill = BG_PANEL;

    // Window appearance
    visuals.window_stroke = egui::Stroke::new(1.0, BORDER_SUBTLE);

    // Striped table rows
    visuals.striped = true;

    style.visuals = visuals;
    ctx.set_style(style);
}

// ─── Helper functions ──────────────────────────────────────────────────

/// Standard dark card frame with border and rounding.
pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BG_PANEL)
        .stroke(egui::Stroke::new(1.0, BORDER_SUBTLE))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(12))
        .outer_margin(egui::Margin::symmetric(4, 0))
}

/// Slightly brighter card for nested/elevated content.
pub fn elevated_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(BG_ELEVATED)
        .stroke(egui::Stroke::new(1.0, BORDER_SUBTLE))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(10))
}

/// Styled section header with underline.
pub fn section_heading(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(FONT_SIZE_SECTION)
            .strong()
            .color(TEXT_PRIMARY),
    );
    ui.add_space(2.0);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(width, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, BORDER_SUBTLE);
    ui.add_space(6.0);
}

/// Small colored circle status indicator.
pub fn status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let size = 10.0;
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), size / 2.0, color);
}

/// Colored badge with text (number badge, channel type badge, etc.).
pub fn colored_badge(ui: &mut egui::Ui, text: &str, bg_color: egui::Color32) {
    let padding = egui::Vec2::new(8.0, 4.0);
    let text_galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(FONT_SIZE_BADGE),
        TEXT_PRIMARY,
    );
    let desired_size = text_galley.size() + padding * 2.0;
    let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

    ui.painter().rect_filled(rect, 4.0, bg_color);
    let text_pos = rect.center() - text_galley.size() / 2.0;
    ui.painter().galley(text_pos, text_galley, TEXT_PRIMARY);
}

/// Same as [`colored_badge`] but with an explicit width — useful for
/// laying out N evenly-sized badges per row. Height is derived from the
/// font + standard vertical padding so badges in the same row line up.
pub fn colored_badge_sized(ui: &mut egui::Ui, text: &str, bg_color: egui::Color32, width: f32) {
    let padding_y = 4.0;
    let text_galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(FONT_SIZE_BADGE),
        TEXT_PRIMARY,
    );
    let height = text_galley.size().y + padding_y * 2.0;
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(width, height), egui::Sense::hover());

    ui.painter().rect_filled(rect, 4.0, bg_color);
    let text_pos = rect.center() - text_galley.size() / 2.0;
    ui.painter().galley(text_pos, text_galley, TEXT_PRIMARY);
}

/// DiGiCo-style action button with colored fill.
pub fn action_button(text: &str, color: egui::Color32, size: egui::Vec2) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).color(TEXT_PRIMARY).strong())
        .fill(color)
        .min_size(size)
        .corner_radius(6.0)
}

// ─── Text-edit sizing primitives ──────────────────────────────────────
//
// Forces a consistent height + inner margin on single-line text edits
// so they line up cleanly across forms (Setup ports / IPs / show file,
// Gangs name + members, Snapshots fields…).

/// Standard rendered height for a single-line `TextEdit`.
pub const TEXT_EDIT_HEIGHT: f32 = 26.0;

/// Inner margin (text-to-border padding) for `TextEdit`s — gives the
/// text a bit of breathing room inside the box.
pub const TEXT_EDIT_MARGIN: egui::Margin = egui::Margin::symmetric(6, 4);

/// Render a single-line `TextEdit` with the standard inner margin and
/// an explicit forced rect via `add_sized`. Useful when the edit lives
/// inside a `Grid` cell or other auto-sizing container that would
/// otherwise squeeze the box to fit its current text. Pass an empty
/// `hint` for none.
pub fn padded_text_edit(
    ui: &mut egui::Ui,
    value: &mut String,
    width: f32,
    enabled: bool,
    hint: &str,
) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| {
        let mut edit = egui::TextEdit::singleline(value).margin(TEXT_EDIT_MARGIN);
        if !hint.is_empty() {
            edit = edit.hint_text(hint);
        }
        ui.add_sized([width, TEXT_EDIT_HEIGHT], edit)
    })
    .inner
}

/// Scope/section toggle block — green when active, grey when inactive.
/// Returns the response for click detection.
pub fn toggle_block(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let fill = if active { SCOPE_ACTIVE } else { SCOPE_INACTIVE };
    let text_color = if active { TEXT_PRIMARY } else { TEXT_SECONDARY };

    let padding = egui::Vec2::new(10.0, 8.0);
    let text_galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(FONT_SIZE_BADGE),
        text_color,
    );
    let desired_size = egui::Vec2::new(
        (text_galley.size().x + padding.x * 2.0).max(80.0),
        text_galley.size().y + padding.y * 2.0,
    );
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());

    // Hover effect
    let fill = if response.hovered() {
        lighten(fill, 20)
    } else {
        fill
    };

    ui.painter().rect_filled(rect, 6.0, fill);

    // Center text in the block
    let text_pos = rect.center() - text_galley.size() / 2.0;
    ui.painter().galley(text_pos, text_galley, text_color);

    response
}

/// Toggle block with partial selection indicator (50% alpha green).
pub fn toggle_block_tristate(
    ui: &mut egui::Ui,
    label: &str,
    all_selected: bool,
    any_selected: bool,
) -> egui::Response {
    let fill = if all_selected {
        SCOPE_ACTIVE
    } else if any_selected {
        SCOPE_PARTIAL
    } else {
        SCOPE_INACTIVE
    };
    let text_color = if all_selected || any_selected {
        TEXT_PRIMARY
    } else {
        TEXT_SECONDARY
    };

    let padding = egui::Vec2::new(10.0, 8.0);
    let text_galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(FONT_SIZE_BADGE),
        text_color,
    );
    let desired_size = egui::Vec2::new(
        (text_galley.size().x + padding.x * 2.0).max(80.0),
        text_galley.size().y + padding.y * 2.0,
    );
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());

    let fill = if response.hovered() {
        lighten(fill, 20)
    } else {
        fill
    };

    ui.painter().rect_filled(rect, 6.0, fill);

    let text_pos = rect.center() - text_galley.size() / 2.0;
    ui.painter().galley(text_pos, text_galley, text_color);

    response
}

/// Returns the DiGiCo color for a channel type.
pub fn channel_color(channel: &ChannelId) -> egui::Color32 {
    match channel {
        ChannelId::Input(_) => CH_INPUT,
        ChannelId::Aux(_) => CH_AUX,
        ChannelId::Group(_) => CH_GROUP,
        ChannelId::Matrix(_) => CH_MATRIX,
        ChannelId::ControlGroup(_) => CH_CG,
        ChannelId::GraphicEq(_) => CH_MATRIX,
        ChannelId::MatrixInput(_) => CH_MATRIX,
    }
}

/// Lighten a color by adding a fixed amount to each channel.
pub fn lighten(color: egui::Color32, amount: u8) -> egui::Color32 {
    egui::Color32::from_rgb(
        color.r().saturating_add(amount),
        color.g().saturating_add(amount),
        color.b().saturating_add(amount),
    )
}

// ─── Long-press button ─────────────────────────────────────────────────

/// Standard long-press duration for "are you sure?" actions: hold for half
/// a second to confirm, release early or move off the button to cancel.
pub const LONG_PRESS_DURATION_MS: u64 = 500;

/// Per-button press tracking saved in egui's temporary data store. Keyed by
/// the button's auto-generated `Response::id` so each long-press button on
/// screen has its own independent timer state.
#[derive(Clone, Copy)]
struct LongPressData {
    /// `egui` input time (seconds) when the press began.
    start: f64,
    /// True if the pointer has left the button rect at any point during the
    /// press. Once cancelled, the press cannot trigger even if the operator
    /// drags back onto the button — they must release and start over.
    cancelled: bool,
}

/// A button that requires a sustained press to activate. The action only
/// fires when the operator releases the pointer **while still over the
/// button** AND the press has lasted at least `duration_ms`. Releasing
/// early or letting the pointer escape the button rect cancels the press
/// silently.
///
/// Returns `true` on the single frame the long-press completes — the
/// caller should treat it the same way they treat `Button::clicked()`.
///
/// While held with the pointer over the button, a thin progress bar fills
/// across the bottom edge of the button to show how close the press is to
/// triggering. The bar disappears (and the timer is marked cancelled) the
/// moment the pointer escapes the rect — the operator can still see the
/// dimmed armed state but releasing now is a no-op.
pub fn long_press_button(
    ui: &mut egui::Ui,
    text: &str,
    color: egui::Color32,
    size: egui::Vec2,
    enabled: bool,
    duration_ms: u64,
) -> bool {
    // Render through `egui::Button` so the size, frame, padding, and
    // press/hover tinting match `action_button` exactly. We just supply
    // the base fill and let egui's WidgetVisuals handle the rest.
    let now = ui.input(|i| i.time);
    let fill = if enabled {
        color
    } else {
        egui::Color32::from_rgba_premultiplied(color.r(), color.g(), color.b(), 80)
    };

    let button = egui::Button::new(
        egui::RichText::new(text)
            .color(if enabled { TEXT_PRIMARY } else { TEXT_DISABLED })
            .strong(),
    )
    .fill(fill)
    .min_size(size)
    .corner_radius(6.0)
    .sense(if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    });
    let response = ui.add(button);
    let rect = response.rect;
    let id = response.id;
    let pointer_down = enabled && response.is_pointer_button_down_on();
    let on_button = response.contains_pointer();

    let mut state: Option<LongPressData> = ui.data(|d| d.get_temp(id));

    // Press-down event: pointer was just put down on the button.
    if pointer_down && state.is_none() {
        state = Some(LongPressData {
            start: now,
            cancelled: false,
        });
    }

    // Escape: pointer left the rect while held → cancel for the rest of
    // this press. The state stays around so we don't fire on release; the
    // operator must let go and re-press to try again.
    if pointer_down && !on_button {
        if let Some(s) = state.as_mut() {
            s.cancelled = true;
        }
    }

    // Release: pointer button just came up.
    let mut triggered = false;
    if !pointer_down {
        if let Some(s) = state.take() {
            let elapsed_ms = ((now - s.start) * 1000.0) as u64;
            if !s.cancelled && on_button && elapsed_ms >= duration_ms {
                triggered = true;
            }
        }
    }

    // Persist (or clear) per-button state.
    match state {
        Some(s) => ui.data_mut(|d| d.insert_temp(id, s)),
        None => ui.data_mut(|d| d.remove::<LongPressData>(id)),
    }

    // While the press is active, request a repaint each frame so the
    // progress bar animates smoothly.
    if state.is_some() {
        ui.ctx().request_repaint();
    }

    // Progress bar overlay along the bottom edge — only while a press
    // is in progress AND the pointer is still on the button.
    if let Some(s) = state {
        if !s.cancelled && on_button {
            let elapsed = (now - s.start) as f32 * 1000.0;
            let progress = (elapsed / duration_ms as f32).clamp(0.0, 1.0);
            let bar_height = 4.0;
            let bar_rect = egui::Rect::from_min_max(
                egui::pos2(rect.min.x, rect.max.y - bar_height),
                egui::pos2(rect.min.x + rect.width() * progress, rect.max.y),
            );
            ui.painter_at(rect).rect_filled(bar_rect, 0.0, TEXT_PRIMARY);
        }
    }

    if enabled {
        response.on_hover_text(format!("Hold {duration_ms} ms to confirm"));
    }

    triggered
}
