//! Embedded fonts.
//!
//! Registers `NotoSans-Regular` as a fallback for both the Proportional and
//! Monospace font families. Primary text still uses egui's bundled fonts;
//! glyphs that egui's defaults don't carry (Unicode arrows, miscellaneous
//! symbols) fall through to NotoSans rather than rendering as tofu.

use std::sync::Arc;

use eframe::egui;

const NOTO_SANS_REGULAR: &[u8] = include_bytes!("../../assets/fonts/NotoSans-Regular.ttf");

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "noto_sans".into(),
        Arc::new(egui::FontData::from_static(NOTO_SANS_REGULAR)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("noto_sans".into());
    }
    ctx.set_fonts(fonts);
}
