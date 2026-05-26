//! Embedded fonts.
//!
//! Registers two symbol fallbacks for both the Proportional and Monospace
//! families. Primary text still uses egui's bundled Ubuntu-Light.
//!
//! - `NotoSansSymbols2` (subset, Geometric Shapes block) is inserted *ahead*
//!   of egui's emoji fallback. egui's emoji font only carries the play-button
//!   forms ▶/◀, so ▼ ▲ ► ◄ would otherwise render as tofu (notably the
//!   expanded disclosure triangles in the Scope editor). Giving this font
//!   priority for the U+25A0..U+25FF block keeps every geometric glyph
//!   (triangles, circles, squares) consistent. ~10 KB subset.
//! - `NotoSansSymbols` is appended last as a catch-all for Unicode arrows
//!   (U+2190..U+21FF) and the other symbol blocks egui's defaults miss.
//!   ~220 KB.

use std::sync::Arc;

use eframe::egui;

const NOTO_SANS_SYMBOLS: &[u8] = include_bytes!("../../assets/fonts/NotoSansSymbols-Regular.ttf");
const NOTO_SANS_SYMBOLS2_GEOMETRIC: &[u8] =
    include_bytes!("../../assets/fonts/NotoSansSymbols2-Geometric-subset.ttf");

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "noto_sans_symbols".into(),
        Arc::new(egui::FontData::from_static(NOTO_SANS_SYMBOLS)),
    );
    fonts.font_data.insert(
        "noto_sans_symbols2".into(),
        Arc::new(egui::FontData::from_static(NOTO_SANS_SYMBOLS2_GEOMETRIC)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let fam = fonts.families.entry(family).or_default();
        // After the primary font, before egui's emoji fallback, so geometric
        // shapes render from one consistent symbol font.
        let pos = fam.len().min(1);
        fam.insert(pos, "noto_sans_symbols2".into());
        // Lowest-priority catch-all for arrows and other symbol blocks.
        fam.push("noto_sans_symbols".into());
    }
    ctx.set_fonts(fonts);
}
