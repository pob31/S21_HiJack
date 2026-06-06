//! Shared coloured channel-tile renderer.
//!
//! Extracted from the Monitor channel picker so other pickers (the Gangs
//! member picker) can reuse the exact same tile look. One uniform tile: a
//! coloured rounded rect with a title line, an optional name line, an optional
//! stereo bar down the right edge, an optional click-order badge, and an
//! optional ripple overlay.

use eframe::egui;

use super::theme;

/// What kind of ripple-mode visual marker a tile should carry. Layered on
/// top of the standard tile fill / selected outline so existing selection
/// state is still visible while the operator picks a ripple range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RippleHighlight {
    /// No ripple decoration.
    None,
    /// One of the two range endpoints — bright orange outline on top of the
    /// tile so the operator can see which two tiles they've picked.
    Endpoint,
    /// Sits between the two endpoints during `Confirming` — translucent
    /// orange overlay previewing what's about to be added.
    InRange,
}

/// One coloured channel tile. Returns the click response so the caller can
/// toggle selection state.
///
/// `order` doubles as the selected flag *and* the click-order badge:
/// - `None` → unselected (dimmed fill, subtle border).
/// - `Some(n)` with `n >= 1` → selected, full-saturation fill + white border,
///   with a small badge in the bottom-right showing `n`.
/// - `Some(0)` → the selected look **without** a badge. Used by unordered
///   pickers (the Gangs member picker) that only care about membership, not
///   click order. Ordered pickers (the Monitor profile picker) always pass a
///   1-based position so their badges keep showing.
///
/// When `stereo` is true a darker vertical bar is painted along the right edge
/// — the convention used by the desk's picker. `ripple_highlight` overlays an
/// orange marker for endpoints / range-preview when a ripple is in progress.
///
/// `enabled == false` renders the tile **disabled**: an extra-dim fill, weak
/// title text, no hover tint, and a hover-only `Sense` so the returned
/// response never reports `clicked()`. Disabled tiles are treated as
/// unselected (the selected stroke / badge are skipped). Used by pickers that
/// lock out incompatible channel types.
pub fn draw_tile(
    ui: &mut egui::Ui,
    tile_size: egui::Vec2,
    title: &str,
    name: &str,
    base_color: egui::Color32,
    order: Option<usize>,
    stereo: bool,
    ripple_highlight: RippleHighlight,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(tile_size, sense);

    let painter = ui.painter_at(rect);
    // Disabled tiles are never "selected" — drop the highlight so a locked-out
    // type can't show a white border / badge.
    let selected = enabled && order.is_some();

    // Tile fill: full-saturation when selected, dimmed when not, extra-dim when
    // disabled. Hover gets a slight tint (enabled tiles only) so the operator
    // sees what's about to be clicked.
    let fill = if !enabled {
        // Mix further toward the dark base so a locked-out tile reads clearly
        // inert next to the merely-unselected ones.
        blend(base_color, theme::tile_dim_bg(), 0.25)
    } else if selected {
        base_color
    } else {
        // Mix base_color towards a dark base at ~30% strength. Theme-
        // independent (tile_dim_bg) so the white glyph text stays readable
        // on the dimmed tile in the light theme too.
        blend(base_color, theme::tile_dim_bg(), 0.7)
    };
    let hover_fill = if enabled && response.hovered() {
        blend(fill, theme::TEXT_PRIMARY, 0.85)
    } else {
        fill
    };
    painter.rect_filled(rect, 4.0, hover_fill);

    // Outer stroke: bright when selected, subtle otherwise.
    let stroke = if selected {
        egui::Stroke::new(2.0, theme::TEXT_PRIMARY)
    } else {
        egui::Stroke::new(1.0, theme::border_subtle())
    };
    painter.rect_stroke(rect, 4.0, stroke, egui::StrokeKind::Inside);

    let text_color = if enabled {
        theme::TEXT_PRIMARY
    } else {
        theme::label_weak()
    };

    // Stereo bar: a darker rectangle pinned to the right edge.
    if stereo {
        let bar_rect = egui::Rect::from_min_max(
            egui::pos2(rect.max.x - 6.0, rect.min.y + 4.0),
            egui::pos2(rect.max.x - 2.0, rect.max.y - 4.0),
        );
        let bar_fill = blend(fill, egui::Color32::BLACK, 0.55);
        painter.rect_filled(bar_rect, 2.0, bar_fill);
    }

    // Ripple decoration — drawn on top of the standard tile so existing
    // selection state (the white outline + order badge) is still visible
    // while the operator picks a range.
    match ripple_highlight {
        RippleHighlight::None => {}
        RippleHighlight::Endpoint => {
            painter.rect_stroke(
                rect,
                4.0,
                egui::Stroke::new(3.0, theme::ACCENT_ORANGE),
                egui::StrokeKind::Inside,
            );
        }
        RippleHighlight::InRange => {
            // Translucent orange wash over the tile to preview the range.
            let overlay = egui::Color32::from_rgba_premultiplied(
                theme::ACCENT_ORANGE.r() / 3,
                theme::ACCENT_ORANGE.g() / 3,
                theme::ACCENT_ORANGE.b() / 3,
                90,
            );
            painter.rect_filled(rect, 4.0, overlay);
        }
    }

    // Title (top line) and name (bottom line). Name falls back to nothing
    // when offline / the console hasn't reported it.
    let title_pos = egui::pos2(rect.center().x, rect.min.y + 14.0);
    painter.text(
        title_pos,
        egui::Align2::CENTER_CENTER,
        title,
        egui::FontId::proportional(13.0),
        text_color,
    );
    if !name.is_empty() {
        let name_pos = egui::pos2(rect.center().x, rect.min.y + 34.0);
        painter.text(
            name_pos,
            egui::Align2::CENTER_CENTER,
            name,
            egui::FontId::proportional(11.0),
            text_color,
        );
    }

    // Order badge: small dark circle in the bottom-right corner of the tile
    // showing the click order (1, 2, 3, …). Visible only on selected tiles
    // that carry a real (1-based) position — `Some(0)` selects without a
    // badge (see the doc comment). Bottom-right placement keeps it clear of
    // the title and name text. When the tile also has the stereo bar on the
    // right edge, the badge nudges slightly further left so the two don't
    // fight for the same pixels.
    if let Some(n) = order {
        if n >= 1 {
            let right_inset = if stereo { 16.0 } else { 11.0 };
            let badge_center = egui::pos2(rect.max.x - right_inset, rect.max.y - 11.0);
            let badge_radius = 9.0;
            painter.circle_filled(badge_center, badge_radius, theme::tile_dim_bg());
            painter.circle_stroke(
                badge_center,
                badge_radius,
                egui::Stroke::new(1.0, theme::TEXT_PRIMARY),
            );
            painter.text(
                badge_center,
                egui::Align2::CENTER_CENTER,
                n.to_string(),
                egui::FontId::proportional(10.0),
                theme::TEXT_PRIMARY,
            );
        }
    }

    response.on_hover_text(if name.is_empty() {
        title.to_string()
    } else {
        format!("{title} — {name}")
    })
}

/// Linear blend `t` of `a` towards `b`. t=1.0 returns a, t=0.0 returns b.
///
/// NOTE the direction: this is the *opposite* of `theme::mix` (which returns
/// `a` at t=0.0). Don't substitute one for the other.
pub(crate) fn blend(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let lerp = |x: u8, y: u8| ((x as f32) * t + (y as f32) * (1.0 - t)).round() as u8;
    egui::Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}
