//! Embedded fonts.
//!
//! Primary body text is the bundled Noto Sans Regular (a normal weight, heavier
//! than egui's default Ubuntu-Light) — used in both themes for stronger, more
//! legible text. Two symbol fallbacks are registered for both the Proportional
//! and Monospace families.
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
/// Bundled regular-weight body font. Heavier than egui's default Ubuntu-Light,
/// so it serves as the primary proportional face in both themes for stronger,
/// more legible text.
const NOTO_SANS_REGULAR: &[u8] = include_bytes!("../../assets/fonts/NotoSans-Regular.ttf");
/// Subset of Noto Sans CJK SC (Simplified-Chinese regional Han shapes) covering
/// only the Han / kana / hangul glyphs used by the bundled zh/ja/ko help-bubble
/// locales (~1 k glyphs, ~160 KB vs the 16 MB full font). Registered as a
/// fallback so Chinese/Japanese/Korean tooltips render; Latin text still uses
/// Noto Sans Regular. CFF outlines — ab_glyph / ttf-parser rasterize them.
///
/// Regenerate after editing the zh/ja/ko locales (the source
/// `NotoSansCJKsc-Regular.otf` is committed alongside, so this works on any
/// machine; needs `fonttools`):
///   union the characters in assets/locales/{zh,ja,ko}.json into a text file, then
///   `python -m fontTools.subset assets/fonts/NotoSansCJKsc-Regular.otf \
///     --text-file=<chars> --output-file=assets/fonts/NotoSansCJKsc-subset.otf \
///     --desubroutinize --no-hinting --layout-features=''`
const NOTO_SANS_CJK: &[u8] = include_bytes!("../../assets/fonts/NotoSansCJKsc-subset.otf");

/// Install the UI fonts. Noto Sans Regular is the primary proportional face
/// (ahead of egui's lighter Ubuntu-Light) so body text renders with more
/// weight; the two Noto symbol fonts are registered as fallbacks. Called once
/// at startup — the font is theme-independent, so no re-install on theme change.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "noto_sans_symbols".into(),
        Arc::new(egui::FontData::from_static(NOTO_SANS_SYMBOLS)),
    );
    fonts.font_data.insert(
        "noto_sans_regular".into(),
        Arc::new(egui::FontData::from_static(NOTO_SANS_REGULAR)),
    );
    // Make Noto Sans Regular the primary proportional face (ahead of
    // Ubuntu-Light), so all body text renders with more weight.
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "noto_sans_regular".into());
    // CJK fallback (Simplified-Chinese-shaped Han + kana + hangul) for the
    // zh/ja/ko help-bubble locales. Added to both families below.
    fonts.font_data.insert(
        "noto_sans_cjk".into(),
        Arc::new(egui::FontData::from_static(NOTO_SANS_CJK)),
    );
    fonts.font_data.insert(
        "noto_sans_symbols2".into(),
        // Nudge the geometric glyphs (▶ ◀ ▼ ▲ …) downward so they share the
        // text baseline instead of riding high — Noto's symbol metrics centre
        // them higher than Ubuntu-Light's lowercase text. Tunable.
        Arc::new(
            egui::FontData::from_static(NOTO_SANS_SYMBOLS2_GEOMETRIC).tweak(egui::FontTweak {
                y_offset_factor: 0.12,
                ..Default::default()
            }),
        ),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let fam = fonts.families.entry(family).or_default();
        // After the primary font, before egui's emoji fallback, so geometric
        // shapes render from one consistent symbol font.
        let pos = fam.len().min(1);
        fam.insert(pos, "noto_sans_symbols2".into());
        // CJK fallback — after the symbol fonts so Latin/geometry are unaffected;
        // CJK codepoints (incl. fullwidth punctuation) resolve here.
        fam.insert(pos + 1, "noto_sans_cjk".into());
        // Lowest-priority catch-all for arrows and other symbol blocks.
        fam.push("noto_sans_symbols".into());
    }
    ctx.set_fonts(fonts);
}
