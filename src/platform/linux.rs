//! Linux/DRM display metrics via sysfs EDID. Reads `/sys/class/drm/*/edid`
//! for connected, enabled connectors — no external dependency, so it works on
//! Raspberry Pi (DRM/KMS) and Wayland without pulling in X11. The
//! preferred-timing resolution from the EDID is used as the panel's pixel size,
//! which is correct when the connector runs at its native mode (the common case
//! on embedded/Pi installs). At a non-native mode the resolution won't match the
//! one egui reports, the monitor is simply skipped, and the scaler falls back to
//! the OS scale factor.

use super::{MonitorMetrics, edid, ppi_from};

pub fn enumerate() -> Vec<MonitorMetrics> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Only connector directories expose `status`/`enabled`/`edid`.
        if std::fs::read_to_string(path.join("status"))
            .unwrap_or_default()
            .trim()
            != "connected"
        {
            continue;
        }
        if std::fs::read_to_string(path.join("enabled"))
            .unwrap_or_default()
            .trim()
            != "enabled"
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(path.join("edid")) else {
            continue;
        };
        if let Some((mm_w, mm_h, px_w, px_h)) = edid::parse_edid(&bytes) {
            if px_w == 0 || mm_w <= 0.0 {
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
    }
    out
}
