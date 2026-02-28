use eframe::egui;

// Touch-friendly sizing constants (Apple HIG minimum: 44px)
pub const GO_BUTTON_SIZE: egui::Vec2 = egui::Vec2::new(200.0, 100.0);
pub const PREV_BUTTON_SIZE: egui::Vec2 = egui::Vec2::new(120.0, 80.0);

pub const FONT_SIZE_HEADING: f32 = 28.0;
pub const FONT_SIZE_BODY: f32 = 16.0;
pub const FONT_SIZE_CUE_CURRENT: f32 = 48.0;
pub const FONT_SIZE_CUE_NEXT: f32 = 24.0;
pub const FONT_SIZE_GO_BUTTON: f32 = 36.0;

// Colors
pub const COLOR_CONNECTED: egui::Color32 = egui::Color32::from_rgb(0, 180, 0);
pub const COLOR_CONNECTING: egui::Color32 = egui::Color32::from_rgb(220, 180, 0);
pub const COLOR_DISCONNECTED: egui::Color32 = egui::Color32::from_rgb(180, 0, 0);
pub const COLOR_GO_BUTTON: egui::Color32 = egui::Color32::from_rgb(0, 150, 0);
pub const COLOR_PREV_BUTTON: egui::Color32 = egui::Color32::from_rgb(200, 150, 0);
pub const COLOR_MACRO_BUTTON: egui::Color32 = egui::Color32::from_rgb(100, 60, 180);
pub const MACRO_BUTTON_SIZE: egui::Vec2 = egui::Vec2::new(120.0, 60.0);
pub const COLOR_RECORDING: egui::Color32 = egui::Color32::from_rgb(220, 0, 0);
pub const COLOR_RECORDING_BG: egui::Color32 = egui::Color32::from_rgb(60, 0, 0);

/// Configure egui style for touch-friendly use.
pub fn configure_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.button_padding = egui::Vec2::new(12.0, 8.0);
    style.spacing.item_spacing = egui::Vec2::new(10.0, 8.0);

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

    ctx.set_style(style);
}
