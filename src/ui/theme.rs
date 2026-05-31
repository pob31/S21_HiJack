use std::sync::atomic::{AtomicU8, Ordering};

use eframe::egui;

use crate::model::channel::ChannelId;
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
    let luminance =
        0.299 * fill.r() as f32 + 0.587 * fill.g() as f32 + 0.114 * fill.b() as f32;
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

/// Small colored circle status indicator. Returns the (hover-sensing) response
/// so callers can attach a tooltip when the dot stands alone without a label.
pub fn status_dot(ui: &mut egui::Ui, color: egui::Color32) -> egui::Response {
    let size = 10.0;
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), size / 2.0, color);
    response
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
    ui.painter().galley(text_pos, text_galley, on_fill_text(bg_color));
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
    ui.painter().galley(text_pos, text_galley, on_fill_text(bg_color));
}

/// DiGiCo-style action button with colored fill.
pub fn action_button(text: &str, color: egui::Color32, size: egui::Vec2) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).color(on_fill_text(color)).strong())
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
) -> bool {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = egui::Vec2::new(12.0, 4.0);
        ui.add_enabled(
            enabled,
            action_button(text, color, egui::Vec2::new(width, ROW_H)),
        )
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
    let text_color = if active { TEXT_PRIMARY } else { neutral_inactive_text() };

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
            .color(if enabled { on_fill_text(color) } else { TEXT_DISABLED })
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
