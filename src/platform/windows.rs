//! Windows display metrics via the per-monitor raw-DPI API. `GetDpiForMonitor`
//! with `MDT_RAW_DPI` returns the physical pixel density Windows derives from
//! the monitor's EDID — independent of the user's scaling setting, which is
//! exactly what's needed to detect a high-PPI panel left at 100%. The monitor's
//! current pixel bounds come from `GetMonitorInfoW`.
//!
//! If `MDT_RAW_DPI` reports an implausible value (some drivers don't populate
//! it), that monitor is skipped and the scaler falls back to the OS scale
//! factor. Reading the raw EDID blob from the registry is the documented next
//! step if raw DPI ever proves unreliable in the field.

use std::mem::size_of;

use windows_sys::Win32::Foundation::{BOOL, LPARAM, RECT, TRUE};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_RAW_DPI};

use super::MonitorMetrics;

pub fn enumerate() -> Vec<MonitorMetrics> {
    let mut monitors: Vec<MonitorMetrics> = Vec::new();
    // SAFETY: standard EnumDisplayMonitors usage — the callback receives our
    // `&mut Vec` back through `dwdata` and only borrows it for the call.
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(enum_proc),
            &mut monitors as *mut Vec<MonitorMetrics> as LPARAM,
        );
    }
    monitors
}

unsafe extern "system" fn enum_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _clip: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let monitors = unsafe { &mut *(lparam as *mut Vec<MonitorMetrics>) };

    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(hmon, &mut info) } == 0 {
        return TRUE; // skip this monitor, keep enumerating
    }
    let r = info.rcMonitor;
    let px_w = (r.right - r.left).max(0) as u32;
    let px_h = (r.bottom - r.top).max(0) as u32;
    if px_w == 0 || px_h == 0 {
        return TRUE;
    }

    let mut dpi_x: u32 = 0;
    let mut dpi_y: u32 = 0;
    // GetDpiForMonitor returns S_OK (0) on success.
    if unsafe { GetDpiForMonitor(hmon, MDT_RAW_DPI, &mut dpi_x, &mut dpi_y) } != 0 {
        return TRUE;
    }
    let ppi = dpi_x as f32;
    // Reject implausible raw DPI (driver didn't populate it). 40–600 spans
    // everything from a wall-mounted TV to a phone-class panel.
    if !(40.0..=600.0).contains(&ppi) {
        return TRUE;
    }
    let mm_w = px_w as f32 / ppi * 25.4;
    let mm_h = px_h as f32 / ppi * 25.4;
    monitors.push(MonitorMetrics {
        px_w,
        px_h,
        mm_w,
        mm_h,
        ppi,
    });
    TRUE
}
