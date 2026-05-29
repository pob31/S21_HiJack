//! Physical display metrics (real PPI) for DPI-aware UI scaling.
//!
//! egui only knows the OS scale factor (`native_pixels_per_point`), which is
//! correct on a properly-configured display but *lies* when a high-PPI panel
//! is run at 100%. To scale the UI to a consistent physical size we need each
//! monitor's true pixel density, which means querying physical dimensions
//! per-platform:
//!
//! - **macOS** — Core Graphics (`CGDisplayScreenSize` + the active display
//!   mode's backing pixels).
//! - **Windows** — the raw-DPI monitor API (`GetDpiForMonitor`/`MDT_RAW_DPI`),
//!   which returns the physical DPI Windows derives from the monitor's EDID,
//!   independent of the user's scaling setting.
//! - **Linux** — DRM sysfs EDID (`/sys/class/drm/*/edid`), no extra dependency
//!   (covers Raspberry Pi and Wayland without X11 deps).
//!
//! Every backend is best-effort: on any failure it omits that monitor (or
//! returns an empty list), and the caller falls back to respecting the OS
//! scale factor. Nothing here can break the UI.

#[cfg(any(target_os = "linux", test))]
mod edid;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Physical metrics for one monitor. `px_*` is the current framebuffer
/// resolution (used to match the monitor egui reports), `mm_*` its physical
/// size, and `ppi` the derived horizontal pixel density.
#[derive(Clone, Copy, Debug)]
pub struct MonitorMetrics {
    pub px_w: u32,
    pub px_h: u32,
    pub mm_w: f32,
    pub mm_h: f32,
    pub ppi: f32,
}

/// Enumerate all monitors with their physical metrics. Best-effort: returns an
/// empty list on an unsupported platform or total failure, and silently skips
/// any individual monitor whose physical size can't be determined.
pub fn enumerate() -> Vec<MonitorMetrics> {
    #[cfg(target_os = "windows")]
    {
        windows::enumerate()
    }
    #[cfg(target_os = "macos")]
    {
        macos::enumerate()
    }
    #[cfg(target_os = "linux")]
    {
        linux::enumerate()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Vec::new()
    }
}

/// Horizontal pixel density from a pixel width and physical width in mm.
/// Returns 0.0 if the physical size is unusable (caller treats 0 as unknown).
#[allow(dead_code)] // unused on Windows, whose backend reports ppi directly
pub(crate) fn ppi_from(px_w: u32, mm_w: f32) -> f32 {
    if mm_w <= 0.0 {
        0.0
    } else {
        px_w as f32 / (mm_w / 25.4)
    }
}
