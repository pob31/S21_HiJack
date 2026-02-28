/// iPad pan 0–1 (0.5 = center) → internal -1 to +1 (0 = center).
pub fn ipad_pan_to_internal(ipad: f32) -> f32 {
    (ipad * 2.0) - 1.0
}

/// Internal pan -1 to +1 (0 = center) → iPad 0–1 (0.5 = center).
pub fn internal_pan_to_ipad(internal: f32) -> f32 {
    (internal + 1.0) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_conversion_round_trip() {
        for &ipad in &[0.0, 0.25, 0.5, 0.75, 1.0] {
            let internal = ipad_pan_to_internal(ipad);
            let back = internal_pan_to_ipad(internal);
            assert!((back - ipad).abs() < 1e-6, "Round-trip failed for iPad pan {ipad}");
        }
    }

    #[test]
    fn pan_conversion_values() {
        // iPad 0.0 (hard left) → internal -1.0
        assert!((ipad_pan_to_internal(0.0) - (-1.0)).abs() < 1e-6);
        // iPad 0.5 (center) → internal 0.0
        assert!((ipad_pan_to_internal(0.5) - 0.0).abs() < 1e-6);
        // iPad 1.0 (hard right) → internal 1.0
        assert!((ipad_pan_to_internal(1.0) - 1.0).abs() < 1e-6);

        // Internal -1.0 → iPad 0.0
        assert!((internal_pan_to_ipad(-1.0) - 0.0).abs() < 1e-6);
        // Internal 0.0 → iPad 0.5
        assert!((internal_pan_to_ipad(0.0) - 0.5).abs() < 1e-6);
        // Internal 1.0 → iPad 1.0
        assert!((internal_pan_to_ipad(1.0) - 1.0).abs() < 1e-6);
    }
}
