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
//! - A per-language **CJK** subset (Noto Sans CJK SC/JP/KR) is added as a
//!   fallback so Chinese/Japanese/Korean help-bubble locales render with their
//!   region's glyph shapes; [`install_fonts_for`] swaps it on language change.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use eframe::egui;

const NOTO_SANS_SYMBOLS: &[u8] = include_bytes!("../../assets/fonts/NotoSansSymbols-Regular.ttf");
const NOTO_SANS_SYMBOLS2_GEOMETRIC: &[u8] =
    include_bytes!("../../assets/fonts/NotoSansSymbols2-Geometric-subset.ttf");
/// Bundled regular-weight body font. Heavier than egui's default Ubuntu-Light,
/// so it serves as the primary proportional face in both themes for stronger,
/// more legible text.
const NOTO_SANS_REGULAR: &[u8] = include_bytes!("../../assets/fonts/NotoSans-Regular.ttf");
/// Per-language CJK subsets — Noto Sans CJK in each region's glyph shapes, each
/// subset to only the Han / kana / hangul glyphs that locale uses (plus the
/// 中文 / 日本語 / 한국어 picker labels), so Chinese, Japanese and Korean
/// tooltips render with correct regional forms. Latin text still comes from Noto
/// Sans Regular. CFF outlines — ab_glyph / ttf-parser rasterize them. The active
/// subset is swapped by [`install_fonts_for`].
///
/// Regenerate after editing the zh/ja/ko locales (full source `.otf`s are
/// committed alongside, so this works on any machine; needs `fonttools`):
///   `pip install fonttools && python scripts/subset_cjk.py`
const CJK_SC: &[u8] = include_bytes!("../../assets/fonts/NotoSansCJKsc-subset.otf");
const CJK_JP: &[u8] = include_bytes!("../../assets/fonts/NotoSansCJKjp-subset.otf");
const CJK_KR: &[u8] = include_bytes!("../../assets/fonts/NotoSansCJKkr-subset.otf");

/// Tag of the currently-installed CJK subset (1 = SC, 2 = JP, 3 = KR);
/// 255 = nothing installed yet.
static APPLIED_CJK: AtomicU8 = AtomicU8::new(255);

/// CJK subset to use for a help-language code. Non-CJK languages get the SC
/// subset so the language picker can still render the 中文 / 日本語 / 한국어
/// option labels (every subset includes all three names).
fn cjk_for(help_language: &str) -> (u8, &'static [u8]) {
    match help_language.to_ascii_lowercase().as_str() {
        "ja" => (2, CJK_JP),
        "ko" => (3, CJK_KR),
        _ => (1, CJK_SC),
    }
}

/// Install the UI fonts for the given help-bubble language, swapping the CJK
/// fallback so Chinese / Japanese / Korean tooltips use their region's glyph
/// shapes. Noto Sans Regular stays the primary proportional face; the two Noto
/// symbol fonts and the active CJK subset are fallbacks.
///
/// Safe (and cheap) to call every frame: it only rebuilds the font set when the
/// CJK choice actually changes — the first frame, or when the help language
/// switches between zh / ja / ko / other. Fonts are theme-independent, so no
/// re-install on theme change.
pub fn install_fonts_for(ctx: &egui::Context, help_language: &str) {
    let (tag, cjk) = cjk_for(help_language);
    if APPLIED_CJK.load(Ordering::Relaxed) == tag {
        return;
    }

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
    // Active per-language CJK fallback (region-correct Han + kana + hangul) for
    // the zh/ja/ko help-bubble locales. Added to both families below.
    fonts.font_data.insert(
        "noto_sans_cjk".into(),
        Arc::new(egui::FontData::from_static(cjk)),
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
    APPLIED_CJK.store(tag, Ordering::Relaxed);
}
