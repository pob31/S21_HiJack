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
pub const COLOR_MACRO_BUTTON: egui::Color32 = egui::Color32::from_rgb(100, 60, 180);
pub const COLOR_RECORDING: egui::Color32 = egui::Color32::from_rgb(220, 0, 0);
pub const COLOR_RECORDING_BG: egui::Color32 = egui::Color32::from_rgb(60, 0, 0);

// ─── Cue highlight colors ─────────────────────────────────────────────

pub const CUE_CURRENT_BG: egui::Color32 = egui::Color32::from_rgb(0x3A, 0x1A, 0x1A);
pub const CUE_CURRENT_BORDER: egui::Color32 = egui::Color32::from_rgb(0x6A, 0x2A, 0x2A);

// ─── Scope toggle block colors ────────────────────────────────────────

pub const SCOPE_ACTIVE: egui::Color32 = ACCENT_GREEN;
pub const SCOPE_INACTIVE: egui::Color32 = egui::Color32::from_rgb(0x3A, 0x3A, 0x3E);
pub const SCOPE_PARTIAL: egui::Color32 = egui::Color32::from_rgb(0x00, 0x5A, 0x00);

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

    // Selection
    visuals.selection.bg_fill = egui::Color32::from_rgba_premultiplied(0x2D, 0x8B, 0xC9, 80);
    visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT_BLUE);

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
    let (rect, _) = ui.allocate_exact_size(
        egui::Vec2::new(width, 1.0),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(rect, 0.0, BORDER_SUBTLE);
    ui.add_space(6.0);
}

/// Small colored circle status indicator.
pub fn status_dot(ui: &mut egui::Ui, color: egui::Color32) {
    let size = 10.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::Vec2::splat(size),
        egui::Sense::hover(),
    );
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

/// DiGiCo-style action button with colored fill.
pub fn action_button(text: &str, color: egui::Color32, size: egui::Vec2) -> egui::Button<'_> {
    egui::Button::new(
        egui::RichText::new(text)
            .color(TEXT_PRIMARY)
            .strong(),
    )
    .fill(color)
    .min_size(size)
    .corner_radius(6.0)
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
fn lighten(color: egui::Color32, amount: u8) -> egui::Color32 {
    egui::Color32::from_rgb(
        color.r().saturating_add(amount),
        color.g().saturating_add(amount),
        color.b().saturating_add(amount),
    )
}
