//! Embedded fonts.
//!
//! Registers `NotoSansSymbols-Regular` as a fallback for both the Proportional
//! and Monospace font families. Primary text still uses egui's bundled
//! Ubuntu-Light; glyphs that egui's defaults don't carry (Unicode arrows in
//! the U+2190..U+21FF block, plus most symbol blocks) fall through to
//! NotoSansSymbols rather than rendering as tofu.
//!
//! NotoSansSymbols was chosen over the larger NotoSans family because the
//! primary font already covers Latin / punctuation; we only need the
//! coverage gap. ~220 KB.

use std::sync::Arc;

use eframe::egui;

const NOTO_SANS_SYMBOLS: &[u8] = include_bytes!("../../assets/fonts/NotoSansSymbols-Regular.ttf");

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "noto_sans_symbols".into(),
        Arc::new(egui::FontData::from_static(NOTO_SANS_SYMBOLS)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("noto_sans_symbols".into());
    }
    ctx.set_fonts(fonts);
}
