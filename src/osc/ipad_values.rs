use crate::model::parameter::FADER_INF_DB;

/// iPad pan 0–1 (0.5 = center) → internal -1 to +1 (0 = center).
pub fn ipad_pan_to_internal(ipad: f32) -> f32 {
    (ipad * 2.0) - 1.0
}

/// Internal pan -1 to +1 (0 = center) → iPad 0–1 (0.5 = center).
pub fn internal_pan_to_ipad(internal: f32) -> f32 {
    (internal + 1.0) / 2.0
}

// ─── Normalized level scaling (the `/sd/` "Other OSC" surface) ──────────
//
// DiGiCo's Other OSC list carries levels as a normalized `0.0 ..= 1.0`
// position rather than dB (`/sd/Input_Channels/1/fader, f, 1` = maximum), so
// levels crossing that surface need converting in both directions.
//
// **HYPOTHESIS — not verified against hardware.** The breakpoints below come
// from the Bitfocus Companion DiGiCo module's fader table, the only published
// mapping we have, and they describe a piecewise-linear law: 0.025 per dB
// above −10 dB, half that from −10 to −30, half again below. Confirm with the
// Phase 2a probe (send a known normalized value, read the desk's dB) before
// trusting recall on this surface. If the real law turns out smooth rather
// than piecewise, replace the table — nothing else needs to change.
const NORMALIZED_LEVEL_TABLE: &[(f32, f32)] = &[
    (FADER_INF_DB, 0.0),
    (-50.0, 0.125),
    (-30.0, 0.25),
    (-10.0, 0.5),
    (0.0, 0.75),
    (10.0, 1.0),
];

/// Internal dB → the normalized `0.0 ..= 1.0` position used by `/sd/`.
pub fn db_to_normalized(db: f32) -> f32 {
    interpolate(db, NORMALIZED_LEVEL_TABLE, |p| p.0, |p| p.1)
}

/// Normalized `0.0 ..= 1.0` position from `/sd/` → internal dB.
pub fn normalized_to_db(norm: f32) -> f32 {
    interpolate(norm, NORMALIZED_LEVEL_TABLE, |p| p.1, |p| p.0)
}

/// Piecewise-linear lookup over a table sorted ascending by `key`, clamping
/// at both ends. Shared by both directions so they cannot drift apart.
fn interpolate(
    x: f32,
    table: &[(f32, f32)],
    key: impl Fn(&(f32, f32)) -> f32,
    val: impl Fn(&(f32, f32)) -> f32,
) -> f32 {
    let first = &table[0];
    let last = &table[table.len() - 1];
    if x <= key(first) {
        return val(first);
    }
    if x >= key(last) {
        return val(last);
    }
    for pair in table.windows(2) {
        let (lo, hi) = (&pair[0], &pair[1]);
        if x <= key(hi) {
            let span = key(hi) - key(lo);
            // Guard a degenerate table entry rather than dividing by zero.
            if span.abs() < f32::EPSILON {
                return val(hi);
            }
            let t = (x - key(lo)) / span;
            return val(lo) + t * (val(hi) - val(lo));
        }
    }
    val(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_conversion_round_trip() {
        for &ipad in &[0.0, 0.25, 0.5, 0.75, 1.0] {
            let internal = ipad_pan_to_internal(ipad);
            let back = internal_pan_to_ipad(internal);
            assert!(
                (back - ipad).abs() < 1e-6,
                "Round-trip failed for iPad pan {ipad}"
            );
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

    #[test]
    fn normalized_level_hits_the_published_breakpoints() {
        for &(db, norm) in NORMALIZED_LEVEL_TABLE {
            assert!(
                (db_to_normalized(db) - norm).abs() < 1e-6,
                "{db} dB should be {norm}"
            );
            assert!(
                (normalized_to_db(norm) - db).abs() < 1e-3,
                "{norm} should be {db} dB"
            );
        }
    }

    #[test]
    fn normalized_level_round_trips_between_breakpoints() {
        // Interpolated points must survive a round trip too, not just the
        // table entries themselves.
        for db in [-120.0, -40.0, -20.0, -5.0, 5.0, 9.9] {
            let back = normalized_to_db(db_to_normalized(db));
            assert!((back - db).abs() < 1e-2, "{db} dB round-tripped to {back}");
        }
    }

    #[test]
    fn normalized_level_clamps_outside_the_table() {
        // Above maximum and below the -inf sentinel both saturate.
        assert!((db_to_normalized(50.0) - 1.0).abs() < 1e-6);
        assert!((db_to_normalized(FADER_INF_DB - 100.0) - 0.0).abs() < 1e-6);
        assert!((normalized_to_db(2.0) - 10.0).abs() < 1e-6);
        assert!((normalized_to_db(-1.0) - FADER_INF_DB).abs() < 1e-6);
    }

    #[test]
    fn normalized_level_is_monotonic() {
        // A fader law that ever went backwards would make recall jump.
        let mut prev = db_to_normalized(FADER_INF_DB);
        let mut db = FADER_INF_DB;
        while db <= 10.0 {
            let n = db_to_normalized(db);
            assert!(n >= prev - 1e-6, "level dipped at {db} dB");
            prev = n;
            db += 0.5;
        }
    }
}
