//! macOS display metrics via Core Graphics. `CGDisplayScreenSize` gives the
//! physical size in millimetres and the active display mode gives the backing
//! (Retina) pixel resolution, from which we derive true PPI.

use core_graphics::display::CGDisplay;

use super::{MonitorMetrics, ppi_from};

pub fn enumerate() -> Vec<MonitorMetrics> {
    let mut out = Vec::new();
    let Ok(ids) = CGDisplay::active_displays() else {
        return out;
    };
    for id in ids {
        let display = CGDisplay::new(id);
        let Some(mode) = display.display_mode() else {
            continue;
        };
        let px_w = mode.pixel_width() as u32;
        let px_h = mode.pixel_height() as u32;
        let size = display.screen_size(); // physical size in mm
        let mm_w = size.width as f32;
        let mm_h = size.height as f32;
        if px_w == 0 || px_h == 0 || mm_w <= 0.0 {
            continue;
        }
        out.push(MonitorMetrics {
            px_w,
            px_h,
            mm_w,
            mm_h,
            ppi: ppi_from(px_w, mm_w),
        });
    }
    out
}
