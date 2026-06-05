use std::sync::atomic::{AtomicU8, Ordering};

use eframe::egui;

use super::help::{HelpKey, help};
use crate::model::channel::ChannelId;
use crate::model::state::ConnectionHealth;
use crate::model::ui_mode::ColorTheme;

// ─── Active colour theme ──────────────────────────────────────────────
//
// The whole palette below resolves at runtime through accessor functions
// so the UI can hot-switch between the dark and light themes. The active
// theme lives in a process-global atomic: the UI thread writes it once at
// the top of every frame (in `configure_style`), and the accessor
// functions — called from deep render code that has no `ColorTheme` to
// hand — read it back. Single writer + same-frame readers on one thread,
// so `Relaxed` is sufficient.

static ACTIVE_THEME: AtomicU8 = AtomicU8::new(0); // 0 = Dark, 1 = Light

/// Set the active colour theme. Called once per frame from
/// [`configure_style`] before any widget is built.
#[inline]
pub fn set_active_theme(theme: ColorTheme) {
    ACTIVE_THEME.store(matches!(theme, ColorTheme::Light) as u8, Ordering::Relaxed);
}

/// True when the light theme is active.
#[inline]
fn is_light() -> bool {
    ACTIVE_THEME.load(Ordering::Relaxed) == 1
}

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
pub const FONT_SIZE_SMALL: f32 = 14.0;
pub const FONT_SIZE_TINY: f32 = 11.0;

// ─── Background colors ────────────────────────────────────────────────
//
// Theme-dependent: the dark theme keeps its original near-black shades;
// the light theme makes every background level pure white and relies on
// the (now black) borders/separators for visual separation.

#[inline]
pub fn bg_dark() -> egui::Color32 {
    if is_light() {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_rgb(0x1A, 0x1A, 0x1E)
    }
}
#[inline]
pub fn bg_panel() -> egui::Color32 {
    if is_light() {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_rgb(0x25, 0x25, 0x28)
    }
}
#[inline]
pub fn bg_elevated() -> egui::Color32 {
    if is_light() {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_rgb(0x2E, 0x2E, 0x32)
    }
}
#[inline]
pub fn bg_input() -> egui::Color32 {
    if is_light() {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_rgb(0x1E, 0x1E, 0x22)
    }
}
#[inline]
pub fn border_subtle() -> egui::Color32 {
    if is_light() {
        egui::Color32::BLACK
    } else {
        egui::Color32::from_rgb(0x3A, 0x3A, 0x3E)
    }
}
#[inline]
pub fn border_focus() -> egui::Color32 {
    if is_light() {
        egui::Color32::BLACK
    } else {
        egui::Color32::from_rgb(0x5A, 0x5A, 0x60)
    }
}

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
//
// Theme-dependent: the dark theme uses a dark maroon row tint; the light
// theme uses a pale red so the current-cue row's (now dark) text reads.

#[inline]
pub fn cue_current_bg() -> egui::Color32 {
    if is_light() {
        egui::Color32::from_rgb(0xFB, 0xE3, 0xE3)
    } else {
        egui::Color32::from_rgb(0x3A, 0x1A, 0x1A)
    }
}
#[inline]
pub fn cue_current_border() -> egui::Color32 {
    if is_light() {
        egui::Color32::from_rgb(0xC0, 0x40, 0x40)
    } else {
        egui::Color32::from_rgb(0x6A, 0x2A, 0x2A)
    }
}

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

// ─── Body-text colors (theme-dependent) ───────────────────────────────
//
// The `TEXT_*` constants above stay white/grey: they are the "on-accent"
// glyph colours painted on coloured button/badge/toggle fills, which look
// identical in both themes. These accessors are for *general body text*
// sitting on a theme background — they go dark in the light theme so
// labels and headings stay readable on white.

/// Primary body/heading text on a theme background.
#[inline]
pub fn label_color() -> egui::Color32 {
    if is_light() {
        egui::Color32::from_rgb(0x1A, 0x1A, 0x1E)
    } else {
        TEXT_PRIMARY
    }
}
/// Secondary/dimmed body text on a theme background.
#[inline]
pub fn label_weak() -> egui::Color32 {
    if is_light() {
        egui::Color32::from_rgb(0x55, 0x55, 0x55)
    } else {
        TEXT_SECONDARY
    }
}
/// Disabled/placeholder body text on a theme background.
#[inline]
pub fn label_disabled() -> egui::Color32 {
    if is_light() {
        egui::Color32::from_rgb(0x99, 0x99, 0x99)
    } else {
        TEXT_DISABLED
    }
}

/// Always-dark background for self-drawn "dimmed" coloured tiles (Monitor
/// / Pan Link unselected tiles, order badges, inactive pan tracks). These
/// blend a channel colour toward a dark base and paint white glyph text on
/// top, so the base must stay dark in both themes — otherwise they'd turn
/// pale-on-white and the white text would vanish in the light theme.
#[inline]
pub fn tile_dim_bg() -> egui::Color32 {
    egui::Color32::from_rgb(0x2E, 0x2E, 0x32)
}

/// Neutral fill for un-accented "grey buttons" / inactive segmented buttons /
/// tabs (the controls that aren't a distinctive DiGiCo accent colour). The
/// dark theme uses the original elevated grey; the light theme makes them a
/// light grey so they follow the theme instead of staying black on white.
/// Their glyph text is chosen by [`on_fill_text`] so it flips to dark on the
/// light fill automatically.
#[inline]
pub fn btn_neutral() -> egui::Color32 {
    if is_light() {
        egui::Color32::from_rgb(0xE2, 0xE2, 0xE2)
    } else {
        egui::Color32::from_rgb(0x2E, 0x2E, 0x32)
    }
}

/// Pick a readable glyph colour (text / icon) for content drawn on top of an
/// arbitrary `fill`. Returns pure black on light fills and white on dark
/// fills, keyed off the fill's perceived luminance. This keeps accent buttons
/// (green/red/blue/amber — all dark enough) showing white glyphs in *both*
/// themes — so the dark theme is unchanged — while neutral buttons, whose
/// fill goes light in the light theme, automatically switch to black glyphs.
/// Black (not just dark grey) is deliberate: the light theme targets bright
/// outdoor use where maximum contrast reads best.
#[inline]
pub fn on_fill_text(fill: egui::Color32) -> egui::Color32 {
    // Perceived luminance (Rec. 601). Threshold sits above the brightest
    // accent (amber ≈ 164) and below the light neutral fill (≈ 226) so only
    // genuinely light fills get dark text.
    let luminance = 0.299 * fill.r() as f32 + 0.587 * fill.g() as f32 + 0.114 * fill.b() as f32;
    if luminance > 175.0 {
        egui::Color32::BLACK
    } else {
        TEXT_PRIMARY
    }
}

/// Glyph colour for the *inactive / unselected* option of a neutral segmented
/// button, tab, or mode selector (whose fill is [`btn_neutral`]). The dark
/// theme keeps the original dim secondary grey so it reads as "not selected";
/// the high-contrast light theme uses pure black for outdoor readability. The
/// *active* option sits on an accent fill and keeps [`TEXT_PRIMARY`].
#[inline]
pub fn neutral_inactive_text() -> egui::Color32 {
    if is_light() {
        egui::Color32::BLACK
    } else {
        TEXT_SECONDARY
    }
}

/// Near-black text painted *on a bright accent fill* (amber / red warning
/// strips, etc.) where the dark theme used `BG_DARK` as the glyph colour
/// for contrast. Theme-independent: the fill stays bright in both themes,
/// so this text must stay dark — using `bg_dark()` would turn it white in
/// the light theme and lose the contrast.
pub const TEXT_ON_BRIGHT: egui::Color32 = egui::Color32::from_rgb(0x1A, 0x1A, 0x1E);

/// Fill colour for a slider's rail (groove). egui draws the rail from
/// `widgets.inactive.bg_fill`, which the light theme sets to white — making
/// the rail vanish on the white panel. Callers scope this onto the slider so
/// the track stays visible: a clear mid-grey in the light theme, the original
/// elevated grey in the dark theme.
#[inline]
pub fn slider_track() -> egui::Color32 {
    if is_light() {
        egui::Color32::from_rgb(0xC4, 0xC4, 0xCC)
    } else {
        egui::Color32::from_rgb(0x2E, 0x2E, 0x32)
    }
}

/// Paint a "skip to next" glyph (right-pointing triangle + vertical bar)
/// centred in `rect`, in `color`. Drawn by hand rather than composed from
/// font glyphs (`▶` + `|`) so the two parts always share a baseline and read
/// as one icon — the font composite looked disjointed, especially with the
/// light theme's heavier face.
pub fn paint_skip_glyph(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let c = rect.center();
    let h = 5.0; // half-height of the icon
    let tri_left = c.x - 6.0;
    let tri = vec![
        egui::pos2(tri_left, c.y - h),
        egui::pos2(tri_left, c.y + h),
        egui::pos2(c.x + 1.0, c.y),
    ];
    painter.add(egui::Shape::convex_polygon(tri, color, egui::Stroke::NONE));
    let bar = egui::Rect::from_min_max(
        egui::pos2(c.x + 3.0, c.y - h),
        egui::pos2(c.x + 5.5, c.y + h),
    );
    painter.rect_filled(bar, 0.5, color);
}

// ─── Style configuration ──────────────────────────────────────────────

/// Configure egui style with the DiGiCo-inspired colour theme. Called once
/// per frame so changing the theme in Advanced Settings hot-switches the UI
/// without a restart.
pub fn configure_style(ctx: &egui::Context, theme: ColorTheme) {
    // Publish the active theme first so every palette accessor below — and
    // all render code this frame — resolves to the right colours.
    set_active_theme(theme);
    let light = matches!(theme, ColorTheme::Light);

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

    // Base visuals: egui's light/dark template gets shadows, scrollbars and
    // selection contrast right for the mode; we then override the palette.
    let mut visuals = if light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };

    visuals.panel_fill = bg_panel();
    visuals.window_fill = bg_dark();
    visuals.extreme_bg_color = bg_dark();
    visuals.faint_bg_color = bg_elevated();
    visuals.code_bg_color = bg_input();

    // Neutral-widget hover/active fills. Dark theme nudges *brighter* than
    // the elevated base; on white we nudge *darker* so press/hover state
    // stays visible.
    let hover_fill = if light {
        egui::Color32::from_rgb(0xE8, 0xE8, 0xE8)
    } else {
        egui::Color32::from_rgb(0x38, 0x38, 0x3E)
    };
    let active_fill = if light {
        egui::Color32::from_rgb(0xD8, 0xD8, 0xD8)
    } else {
        egui::Color32::from_rgb(0x40, 0x40, 0x48)
    };

    // Selection. `selection.stroke.color` is what egui copies into
    // `WidgetVisuals::fg_stroke.color` for selected `selectable_label`
    // items — combobox dropdowns, scope toggles, etc. Use white instead
    // of the accent so the selected item's text reads against the
    // translucent-blue selection background instead of merging into it.
    visuals.selection.bg_fill = egui::Color32::from_rgba_premultiplied(0x2D, 0x8B, 0xC9, 80);
    visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);

    // Hyperlinks
    visuals.hyperlink_color = ACCENT_BLUE;

    // Widget visuals — inactive state
    visuals.widgets.inactive.bg_fill = bg_elevated();
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, border_subtle());
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, label_weak());
    visuals.widgets.inactive.weak_bg_fill = bg_elevated();

    // Widget visuals — hovered state
    visuals.widgets.hovered.bg_fill = hover_fill;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, border_focus());
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, label_color());
    visuals.widgets.hovered.weak_bg_fill = hover_fill;

    // Widget visuals — active (clicked) state
    visuals.widgets.active.bg_fill = active_fill;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT_BLUE);
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, label_color());
    visuals.widgets.active.weak_bg_fill = active_fill;

    // Widget visuals — open (expanded ComboBox, etc.)
    visuals.widgets.open.bg_fill = bg_elevated();
    visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0, ACCENT_BLUE);
    visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0, label_color());
    visuals.widgets.open.weak_bg_fill = bg_elevated();

    // Widget visuals — non-interactive
    visuals.widgets.noninteractive.bg_fill = bg_panel();
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(0.5, border_subtle());
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, label_weak());
    visuals.widgets.noninteractive.weak_bg_fill = bg_panel();

    // Window appearance
    visuals.window_stroke = egui::Stroke::new(1.0, border_subtle());

    // Striped table rows
    visuals.striped = true;

    style.visuals = visuals;
    ctx.set_style(style);
}

// ─── Helper functions ──────────────────────────────────────────────────

/// Standard card frame with border and rounding.
pub fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(bg_panel())
        .stroke(egui::Stroke::new(1.0, border_subtle()))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(12))
        .outer_margin(egui::Margin::symmetric(4, 0))
}

/// Slightly elevated card for nested content.
pub fn elevated_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(bg_elevated())
        .stroke(egui::Stroke::new(1.0, border_subtle()))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(10))
}

/// Styled section header with underline.
pub fn section_heading(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(FONT_SIZE_SECTION)
            .strong()
            .color(label_color()),
    );
    ui.add_space(2.0);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(width, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, border_subtle());
    ui.add_space(6.0);
}

/// Like [`section_heading`] but renders `right` (e.g. a small control) flush to
/// the right edge of the heading row, above the underline rule. Used to place
/// a per-section toggle/combobox next to its title.
pub fn section_heading_with(ui: &mut egui::Ui, text: &str, right: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(text)
                .size(FONT_SIZE_SECTION)
                .strong()
                .color(label_color()),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Compact the right-side control (combobox/checkbox) so it doesn't
            // make the heading row taller than a plain section heading. Zero the
            // vertical button padding to drop the height, but keep a real
            // minimum interact height equal to the body text — egui needs that
            // to vertically CENTER the combobox's text; zeroing it instead
            // dropped the text below the adjacent label's baseline. Net: short
            // row, text centered on the title/label baseline.
            let body_h = ui.text_style_height(&egui::TextStyle::Body);
            ui.spacing_mut().button_padding.y = 0.0;
            ui.spacing_mut().interact_size.y = body_h;
            right(ui);
        });
    });
    ui.add_space(2.0);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(width, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, border_subtle());
    ui.add_space(6.0);
}

/// Small colored circle status indicator. Returns the (hover-sensing) response
/// so callers can attach a tooltip when the dot stands alone without a label.
pub fn status_dot(ui: &mut egui::Ui, color: egui::Color32) -> egui::Response {
    let size = 10.0;
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), size / 2.0, color);
    response
}

/// Console GP-OSC link state → status-dot colour + hover/label text, shared by
/// the top bar, Setup tab, and Monitor tab so they read consistently:
/// green = healthy; **yellow "Connecting…" = a session is up but the link
/// isn't currently reachable** (pings unanswered — still retrying); red = no
/// session at all.
pub fn console_status(connected: bool, health: ConnectionHealth) -> (egui::Color32, &'static str) {
    if !connected {
        (COLOR_DISCONNECTED, "Disconnected")
    } else if matches!(health, ConnectionHealth::Connected | ConnectionHealth::Idle) {
        (COLOR_CONNECTED, "Connected")
    } else {
        (COLOR_CONNECTING, "Connecting…")
    }
}

// ─── Responsive channel-grid sizing ─────────────────────────────────────────
// Shared by the Monitor channel picker and the Pan Link tab: both lay out an
// Inputs grid (10 tiles/row) beside an Auxes grid (4 tiles/row). Rather than
// fixed-size tiles that clip or scroll when the window is narrow, the tiles
// stretch to fill the available width — a single uniform tile width makes the
// two grids together fill the panel, clamped so tiles stay legible but never
// cartoonish.

/// Fixed tile height (logical points). Only the width is responsive.
pub const TILE_H: f32 = 52.0;
/// Minimum responsive tile width — below this "Input 48" + a name stops fitting.
pub const TILE_W_MIN: f32 = 56.0;
/// Maximum responsive tile width — past this, growing tiles just looks silly.
pub const TILE_W_MAX: f32 = 120.0;
/// Gap between tiles within a grid.
pub const TILE_GAP: f32 = 8.0;
/// Gap between the Inputs panel and the Auxes panel.
pub const TILE_COLUMN_GAP: f32 = 20.0;

/// Uniform tile width so an `inputs_cols`-wide grid + a column gap + an
/// `aux_cols`-wide grid together fill `avail_w`, clamped to
/// `[TILE_W_MIN, TILE_W_MAX]`. Callers derive each panel's width with
/// [`panel_width`].
pub fn channel_tile_width(avail_w: f32, inputs_cols: u8, aux_cols: u8) -> f32 {
    let cols = (inputs_cols as f32 + aux_cols as f32).max(1.0);
    let inner_gaps = TILE_GAP * (inputs_cols.saturating_sub(1) + aux_cols.saturating_sub(1)) as f32;
    ((avail_w - inner_gaps - TILE_COLUMN_GAP) / cols).clamp(TILE_W_MIN, TILE_W_MAX)
}

/// Width of a grid panel holding `cols` tiles of width `tile_w` (with `TILE_GAP`
/// between them). Used to size the panel and its header so they line up.
pub fn panel_width(tile_w: f32, cols: u8) -> f32 {
    tile_w * cols as f32 + TILE_GAP * cols.saturating_sub(1) as f32
}

/// Minimum sensible content width for an inputs+auxes grid pair (tiles at
/// `TILE_W_MIN`). Windows hosting the grids use this as their `min_width` so the
/// tiles never shrink below legibility or scroll horizontally.
pub fn grids_min_width(inputs_cols: u8, aux_cols: u8) -> f32 {
    panel_width(TILE_W_MIN, inputs_cols) + TILE_COLUMN_GAP + panel_width(TILE_W_MIN, aux_cols)
}

/// Colored badge with text (number badge, channel type badge, etc.).
pub fn colored_badge(ui: &mut egui::Ui, text: &str, bg_color: egui::Color32) {
    let padding = egui::Vec2::new(8.0, 4.0);
    let text_galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(FONT_SIZE_BADGE),
        on_fill_text(bg_color),
    );
    let desired_size = text_galley.size() + padding * 2.0;
    let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

    ui.painter().rect_filled(rect, 4.0, bg_color);
    let text_pos = rect.center() - text_galley.size() / 2.0;
    ui.painter()
        .galley(text_pos, text_galley, on_fill_text(bg_color));
}

/// Same as [`colored_badge`] but with an explicit width — useful for
/// laying out N evenly-sized badges per row. Height is derived from the
/// font + standard vertical padding so badges in the same row line up.
pub fn colored_badge_sized(ui: &mut egui::Ui, text: &str, bg_color: egui::Color32, width: f32) {
    let padding_y = 4.0;
    let text_galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(FONT_SIZE_BADGE),
        on_fill_text(bg_color),
    );
    let height = text_galley.size().y + padding_y * 2.0;
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(width, height), egui::Sense::hover());

    ui.painter().rect_filled(rect, 4.0, bg_color);
    let text_pos = rect.center() - text_galley.size() / 2.0;
    ui.painter()
        .galley(text_pos, text_galley, on_fill_text(bg_color));
}

/// DiGiCo-style action button with colored fill.
pub fn action_button(text: &str, color: egui::Color32, size: egui::Vec2) -> egui::Button<'_> {
    egui::Button::new(
        egui::RichText::new(text)
            .color(on_fill_text(color))
            .strong(),
    )
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
    padded_text_edit_sized(ui, value, width, TEXT_EDIT_HEIGHT, enabled, hint)
}

/// Same as [`padded_text_edit`] but with an explicit `height` — used by the
/// row-alignment helpers below to force a TextEdit to a shared row height.
pub fn padded_text_edit_sized(
    ui: &mut egui::Ui,
    value: &mut String,
    width: f32,
    height: f32,
    enabled: bool,
    hint: &str,
) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| {
        let mut edit = egui::TextEdit::singleline(value).margin(TEXT_EDIT_MARGIN);
        if !hint.is_empty() {
            edit = edit.hint_text(hint);
        }
        ui.add_sized([width, height], edit)
    })
    .inner
}

// ─── Row-alignment primitives ──────────────────────────────────────────
//
// Mixing bare labels (~18 px), text edits (26 px) and buttons / comboboxes
// (~30-35 px under the global `button_padding = 12×8`) in one horizontal row
// reads as ragged — egui centres them vertically, but the differing heights
// put the text on different lines. The fix: pick one row height, size every
// widget in the row to it, and centre. `ROW_H` matches the action-button
// height already used across the Snapshots / Macros tabs.
pub const ROW_H: f32 = 28.0;

/// Paint `text` as a label whose galley is vertically centred within a
/// `ROW_H`-tall cell. Unlike `ui.add_sized([w, ROW_H], Label)` — which reports
/// the label's natural ~18 px `min_rect` back to the parent and so leaves a
/// Grid cell short and top-aligned — this allocates exactly `text_w × ROW_H`,
/// so the control beside it lines up on the same centreline.
pub fn row_label(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(FONT_SIZE_BODY),
        color,
    );
    let label_h = galley.size().y;
    let (rect, _) = ui.allocate_exact_size(
        egui::Vec2::new(galley.size().x, ROW_H),
        egui::Sense::hover(),
    );
    let y = rect.min.y + (ROW_H - label_h) / 2.0;
    ui.painter()
        .galley(egui::pos2(rect.min.x, y), galley, color);
}

/// `ROW_H`-tall action button. Compresses `button_padding.y` in a child scope
/// so the button renders at exactly `ROW_H` instead of overflowing under the
/// global `(12, 8)` padding. Returns `clicked()`; respects `enabled`.
pub fn row_action_button(
    ui: &mut egui::Ui,
    text: &str,
    color: egui::Color32,
    width: f32,
    enabled: bool,
    hover: impl Into<egui::WidgetText> + Clone,
) -> bool {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = egui::Vec2::new(12.0, 4.0);
        ui.add_enabled(
            enabled,
            action_button(text, color, egui::Vec2::new(width, ROW_H)),
        )
        // Show the help text whether the button is enabled or greyed out.
        .on_hover_text(hover.clone())
        .on_disabled_hover_text(hover)
        .clicked()
    })
    .inner
}

/// `ROW_H`-tall long-press button — see [`long_press_button`]. Returns true
/// the frame the press completes.
pub fn row_long_press_button(
    ui: &mut egui::Ui,
    text: &str,
    color: egui::Color32,
    width: f32,
    enabled: bool,
) -> bool {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = egui::Vec2::new(12.0, 4.0);
        long_press_button(
            ui,
            text,
            color,
            egui::Vec2::new(width, ROW_H),
            enabled,
            LONG_PRESS_DURATION_MS,
        )
    })
    .inner
}

/// `ROW_H`-tall long-press button with a caller-supplied hover tooltip (e.g. a
/// descriptive help string). See [`long_press_button_hover`].
pub fn row_long_press_button_hover(
    ui: &mut egui::Ui,
    text: &str,
    color: egui::Color32,
    width: f32,
    enabled: bool,
    hover: impl Into<egui::WidgetText>,
) -> bool {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = egui::Vec2::new(12.0, 4.0);
        long_press_button_hover(
            ui,
            text,
            color,
            egui::Vec2::new(width, ROW_H),
            enabled,
            LONG_PRESS_DURATION_MS,
            hover,
        )
    })
    .inner
}

/// Build a long-press tooltip: a description followed by the standard
/// "hold to confirm" hint, so a converted button still explains itself.
pub fn long_press_hover(desc: impl std::fmt::Display) -> String {
    format!(
        "{}\n\n{}",
        desc,
        help(HelpKey::LongPressConfirm).replace("{ms}", &LONG_PRESS_DURATION_MS.to_string())
    )
}

/// Run `add_combo` inside a scope tuned so an `egui::ComboBox`'s closed box is
/// `ROW_H` tall — matching the sibling text edits / buttons — and centres on
/// the row's centreline. The caller builds the ComboBox itself (it owns the
/// id_salt, width, selected_text and items).
///
/// The closed combo's height is driven by `interact_size.y`, which we pin to
/// `ROW_H`; `button_padding` stays modest so it doesn't push past that. Once
/// the combo is full row height, `ui.horizontal`'s `Align::Center` (and a
/// Grid row whose label cell is also `ROW_H`) line it up without any nudge —
/// `nudge_top` remains as an optional fine-tune (negative = up).
pub fn row_combo<R>(
    ui: &mut egui::Ui,
    nudge_top: i8,
    add_combo: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 0,
            right: 0,
            top: nudge_top,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.spacing_mut().interact_size.y = ROW_H;
            ui.spacing_mut().button_padding = egui::Vec2::new(12.0, 4.0);
            let v = ui.visuals_mut();
            v.widgets.inactive.bg_fill = bg_input();
            v.widgets.inactive.weak_bg_fill = bg_input();
            v.widgets.hovered.bg_fill = bg_input();
            v.widgets.hovered.weak_bg_fill = bg_input();
            v.widgets.open.bg_fill = bg_input();
            v.widgets.open.weak_bg_fill = bg_input();
            add_combo(ui)
        })
        .inner
}

/// Pin the current horizontal row to `ROW_H` by allocating a zero-width,
/// `ROW_H`-tall invisible spacer. Use as the first thing in a checkbox- or
/// radio-only row so its content centres on the shared centreline.
pub fn row_spacer(ui: &mut egui::Ui) {
    ui.allocate_exact_size(egui::Vec2::new(0.0, ROW_H), egui::Sense::hover());
}

/// Scope/section toggle block — green when active, grey when inactive.
/// Returns the response for click detection.
pub fn toggle_block(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let fill = if active { SCOPE_ACTIVE } else { btn_neutral() };
    let text_color = if active {
        TEXT_PRIMARY
    } else {
        neutral_inactive_text()
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

    // Hover effect
    let fill = if response.hovered() {
        lighten(fill, 20)
    } else {
        fill
    };

    ui.painter().rect_filled(rect, 6.0, fill);
    ui.painter().rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(1.0, border_subtle()),
        egui::StrokeKind::Inside,
    );

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
        btn_neutral()
    };
    let text_color = if all_selected || any_selected {
        TEXT_PRIMARY
    } else {
        neutral_inactive_text()
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
    ui.painter().rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(1.0, border_subtle()),
        egui::StrokeKind::Inside,
    );

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

/// Darken a color by subtracting a fixed amount from each channel.
fn darken(color: egui::Color32, amount: u8) -> egui::Color32 {
    egui::Color32::from_rgb(
        color.r().saturating_sub(amount),
        color.g().saturating_sub(amount),
        color.b().saturating_sub(amount),
    )
}

/// Linearly blend `a` toward `b` by `t` (0.0 = `a`, 1.0 = `b`), per channel.
fn mix(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    egui::Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}

// ─── Long-press button ─────────────────────────────────────────────────

/// Standard long-press duration for "are you sure?" actions: hold for half
/// a second to confirm, release early or move off the button to cancel.
pub const LONG_PRESS_DURATION_MS: u64 = 500;

/// Sheen alpha (/255) at rest: 0, so an unpressed long-press button is flat —
/// just the crisp 45° cut line below — and sits in line with the rest of the
/// UI. The sheen fades in only while held, ramping toward
/// [`SEAM_HELD_GLINT_ALPHA`] as the press nears triggering. When shown it is
/// drawn in a tone that *contrasts with the label* (dark sheen under white
/// text, light under black) so it never hurts readability.
const SEAM_GLINT_ALPHA: u8 = 0;
/// Alpha (/255) of the crisp 45° "cut" line — the at-rest "slash" that marks
/// the button as long-press even while it's flat.
const SEAM_LINE_ALPHA: u8 = 120;
/// Peak alpha the sheen ramps **up to** while the button is held, so the
/// oblique separation visibly deepens as the press nears triggering — the
/// progress cue, with no separate progress bar.
const SEAM_HELD_GLINT_ALPHA: u8 = 200;
/// Alpha the crisp cut line ramps up to while held.
const SEAM_HELD_LINE_ALPHA: u8 = 235;

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
/// At rest the button is flat like every other button, save for a crisp 45°
/// cut line ("slash") across the face that marks its long-press identity.
/// Holding the pointer over it fades in a contrast-toned sheen along that cut
/// and deepens it as the press nears triggering — a bar-free progress cue. The
/// sheen relaxes away the moment the pointer escapes the rect (the press is
/// cancelled) or the button is released, leaving just the flat slash again.
pub fn long_press_button(
    ui: &mut egui::Ui,
    text: &str,
    color: egui::Color32,
    size: egui::Vec2,
    enabled: bool,
    duration_ms: u64,
) -> bool {
    let hint = help(HelpKey::LongPressConfirm).replace("{ms}", &duration_ms.to_string());
    long_press_button_hover(ui, text, color, size, enabled, duration_ms, hint)
}

/// Like [`long_press_button`] but with a caller-supplied hover tooltip (e.g. a
/// descriptive help string) in place of the default "hold to confirm" hint —
/// the caller folds the hold hint into `hover` if they still want it shown.
#[allow(clippy::too_many_arguments)]
pub fn long_press_button_hover(
    ui: &mut egui::Ui,
    text: &str,
    color: egui::Color32,
    size: egui::Vec2,
    enabled: bool,
    duration_ms: u64,
    hover: impl Into<egui::WidgetText>,
) -> bool {
    // Custom-painted (rather than via `egui::Button`) so we control the draw
    // order precisely: fill → 45° seam → label. Drawing the seam *under* the
    // label keeps the text crisp. The long-press state machine further down is
    // unchanged from the old `Button` path — it only needs
    // `is_pointer_button_down_on()` / `contains_pointer()`, which the manually
    // allocated response provides identically.
    let now = ui.input(|i| i.time);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    // Treat `size` as a *minimum*, growing to fit the label + padding in
    // **both** dimensions like `egui::Button::min_size` did — otherwise a
    // label wider than the requested width (e.g. "Re-capture") gets clipped,
    // and a short requested height clips below its neighbours.
    let font_id = egui::TextStyle::Button.resolve(ui.style());
    let pad = ui.spacing().button_padding;
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_string(), font_id.clone(), egui::Color32::WHITE);
    let desired = egui::vec2(
        size.x.max(galley.size().x + pad.x * 2.0),
        size.y.max(galley.size().y + pad.y * 2.0),
    );
    let (rect, response) = ui.allocate_exact_size(desired, sense);
    let id = response.id;
    let pointer_down = enabled && response.is_pointer_button_down_on();
    let on_button = response.contains_pointer();
    // `any_down` (a pointer button is physically held anywhere) is robust
    // where `is_pointer_button_down_on()` can flicker false mid-hold for a
    // manually-allocated widget; `released` is the genuine release event we
    // trigger on. Both drive the state machine below so it never fires early.
    let any_down = ui.input(|i| i.pointer.any_down());
    let released = ui.input(|i| i.pointer.primary_released());

    // Hold progress in [0,1], peeked from the press timer (the state machine
    // below owns the canonical update). Non-zero only while actively held on
    // the button — it deepens the 45° cut as a bar-free progress cue. `armed`
    // means held past the threshold *and still on the button*: the action is
    // ready but only fires on **release**, so the fill lights up to say
    // "release to confirm" (slide off to cancel even now).
    let hold = match ui.data(|d| d.get_temp::<LongPressData>(id)) {
        Some(s) if any_down && !s.cancelled && on_button => {
            (((now - s.start) as f32 * 1000.0) / duration_ms as f32).clamp(0.0, 1.0)
        }
        _ => 0.0,
    };
    let armed = hold >= 1.0;

    // ── Base fill with hover/press feedback (replaces egui's WidgetVisuals) ──
    let base_fill = if !enabled {
        // Muted: desaturate the accent toward the surface so a disabled Delete
        // recedes as clearly inactive rather than reading as a strong red.
        mix(color, bg_elevated(), 0.72)
    } else if armed {
        // Held long enough — light up to invite release (the trigger).
        lighten(color, 28)
    } else if pointer_down {
        darken(color, 15)
    } else if response.hovered() {
        lighten(color, 20)
    } else {
        color
    };
    // `mark` is the label colour. The gradient sheen uses the *opposite* tone
    // (dark sheen under white text, light under black) so it never hurts
    // readability; the cut line is bevelled with both a light and a dark
    // stroke so the separation keeps contrast on dark *and* bright fills.
    // `dim` fades the affordance when disabled; `chan` builds a translucent
    // colour of a given channel tone at a given alpha.
    let mark = on_fill_text(color);
    let seam_rgb = if mark == egui::Color32::BLACK {
        egui::Color32::WHITE
    } else {
        egui::Color32::BLACK
    };
    let dim = |a: u8| if enabled { a } else { (a as f32 * 0.3) as u8 };
    let chan = |rgb: egui::Color32, a: u8| {
        egui::Color32::from_rgba_unmultiplied(rgb.r(), rgb.g(), rgb.b(), dim(a))
    };

    let ramp = |rest: u8, held: u8| rest + ((held - rest) as f32 * hold) as u8;
    let seam_alpha = ramp(SEAM_GLINT_ALPHA, SEAM_HELD_GLINT_ALPHA);
    let line_alpha = ramp(SEAM_LINE_ALPHA, SEAM_HELD_LINE_ALPHA);

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, base_fill);

    // ── 45° oblique separation through the middle: the long-press identity ──
    // A true 45° "/" cut across the centre. At rest only the crisp bevelled cut
    // line shows, so the button reads flat like the rest of the UI. While held,
    // a contrast-coloured sheen fades in — brightest along the cut, fading out
    // to the left and right edges (two gradients running opposite ways) and
    // deepening as the press nears triggering. The sheen fades to nothing at
    // the left/right edges, so the square mesh clip never shows where the
    // fill's rounded corners are.
    {
        let cx = rect.center().x;
        // 45° ⇒ the cut's horizontal half-span equals half the height.
        let half = (rect.height() * 0.5).min(rect.width() * 0.5);
        let seam_top = egui::pos2(cx + half, rect.top());
        let seam_bot = egui::pos2(cx - half, rect.bottom());
        // Sheen mesh — absent at rest (seam_alpha == 0), fading in only as the
        // hold deepens. Skipped entirely while transparent so the unpressed
        // button is genuinely flat.
        if seam_alpha > 0 {
            let vert = |x: f32, y: f32, a: u8| egui::epaint::Vertex {
                pos: egui::pos2(x, y),
                uv: egui::epaint::WHITE_UV,
                color: chan(seam_rgb, a),
            };
            let mut mesh = egui::epaint::Mesh::default();
            mesh.vertices.push(vert(rect.left(), rect.top(), 0)); // 0 top-left
            mesh.vertices.push(vert(rect.right(), rect.top(), 0)); // 1 top-right
            mesh.vertices.push(vert(rect.left(), rect.bottom(), 0)); // 2 bottom-left
            mesh.vertices.push(vert(rect.right(), rect.bottom(), 0)); // 3 bottom-right
            mesh.vertices.push(vert(seam_top.x, seam_top.y, seam_alpha)); // 4 cut-top
            mesh.vertices.push(vert(seam_bot.x, seam_bot.y, seam_alpha)); // 5 cut-bottom
            // Left region (cut→left edge) then right region (cut→right edge).
            mesh.indices
                .extend_from_slice(&[0, 4, 5, 0, 5, 2, 4, 1, 3, 4, 3, 5]);
            painter.add(egui::Shape::mesh(mesh));
        }
        // Bevel the cut: a dark shadow line and a light highlight line offset
        // across it, so the separation reads on dark fills (highlight shows)
        // and bright fills (shadow shows) alike. Always drawn — this is the
        // at-rest "slash" that identifies the button.
        let off = egui::vec2(0.8, 0.0);
        painter.line_segment(
            [seam_top + off, seam_bot + off],
            egui::Stroke::new(1.0, chan(egui::Color32::BLACK, line_alpha)),
        );
        painter.line_segment(
            [seam_top - off, seam_bot - off],
            egui::Stroke::new(1.0, chan(egui::Color32::WHITE, line_alpha)),
        );
    }

    // ── Label, painted above the seam so the text stays crisp ──
    let text_color = if enabled { mark } else { TEXT_DISABLED };
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        font_id,
        text_color,
    );

    // ── Long-press state machine ──
    let mut state: Option<LongPressData> = ui.data(|d| d.get_temp(id));

    // Press-down: start the timer when the pointer goes down on the button.
    // (`is_pointer_button_down_on()` is reliable on the press frame; once the
    // state exists we keep it across any later flicker — see cleanup below.)
    if pointer_down && state.is_none() {
        state = Some(LongPressData {
            start: now,
            cancelled: false,
        });
    }

    // Escape: while a press is active, the pointer being off the button
    // latches a cancel (the operator must lift and re-press). Keyed off
    // `on_button` only, so it's immune to the down-flag flicker.
    if state.is_some() && !on_button {
        if let Some(s) = state.as_mut() {
            s.cancelled = true;
        }
    }

    // Fire ONLY on a genuine release over the button, after a long enough,
    // uncancelled hold. A release event cannot occur mid-hold, so this never
    // fires while held — not even the instant the timer expires. The operator
    // must lift the click/finger while still over the button.
    let mut triggered = false;
    if released {
        if let Some(s) = state.take() {
            let elapsed_ms = ((now - s.start) * 1000.0) as u64;
            if !s.cancelled && on_button && elapsed_ms >= duration_ms {
                triggered = true;
            }
        }
    }
    // Drop a stale press once no pointer button is held (e.g. released
    // off-window). Using `any_down` — not the flicker-prone down-on flag —
    // keeps the timer alive throughout a real hold.
    if !any_down {
        state = None;
    }

    // Persist (or clear) per-button state.
    match state {
        Some(s) => ui.data_mut(|d| d.insert_temp(id, s)),
        None => ui.data_mut(|d| d.remove::<LongPressData>(id)),
    }

    // While the press is active, repaint each frame so the 45° cut animates
    // (deepening toward triggering).
    if state.is_some() {
        ui.ctx().request_repaint();
    }

    // Show the tooltip in both states (an enabled button gets the hold hint /
    // description; a disabled one still explains why — matching the app's
    // `on_disabled_hover_text` convention elsewhere).
    response.on_hover_text(hover);

    triggered
}
