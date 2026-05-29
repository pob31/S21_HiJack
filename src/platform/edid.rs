//! Minimal EDID 1.x parser — just enough to recover physical size (mm) and the
//! preferred (native) resolution from a raw 128-byte base block. Used by the
//! Linux backend (sysfs `edid`); kept platform-agnostic so it can be
//! unit-tested anywhere.

/// Parse the physical size (mm) and preferred-timing resolution (px) from a raw
/// EDID base block: `(mm_w, mm_h, px_w, px_h)`. Returns `None` if the block is
/// too short, the header is wrong, or no usable size/resolution is present.
#[allow(dead_code)] // only wired up on Linux; also compiled under cfg(test)
pub(crate) fn parse_edid(edid: &[u8]) -> Option<(f32, f32, u32, u32)> {
    if edid.len() < 128 {
        return None;
    }
    // Fixed EDID header: 00 FF FF FF FF FF FF 00.
    const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
    if edid[0..8] != HEADER {
        return None;
    }

    // The first 18-byte descriptor (offset 54) is the preferred Detailed Timing
    // Descriptor when its pixel clock (bytes 0..2) is non-zero.
    let d = &edid[54..72];
    let pixel_clock = u16::from_le_bytes([d[0], d[1]]);
    if pixel_clock == 0 {
        return None; // not a detailed timing — no native resolution here
    }
    // 12-bit active pixel counts: low byte + high nibble (bits 7..4 of the
    // shared byte hold the upper 4 bits of the count).
    let h_active = (d[2] as u32) | (((d[4] as u32) & 0xF0) << 4);
    let v_active = (d[5] as u32) | (((d[7] as u32) & 0xF0) << 4);
    if h_active == 0 || v_active == 0 {
        return None;
    }
    // Physical image size (mm): prefer the DTD's 12-bit mm fields; fall back to
    // the base block's whole-cm fields (bytes 21/22).
    let h_mm = (d[12] as u32) | (((d[14] as u32) & 0xF0) << 4);
    let v_mm = (d[13] as u32) | (((d[14] as u32) & 0x0F) << 8);
    let (mm_w, mm_h) = if h_mm > 0 && v_mm > 0 {
        (h_mm as f32, v_mm as f32)
    } else {
        let cm_w = edid[21] as f32;
        let cm_h = edid[22] as f32;
        if cm_w > 0.0 && cm_h > 0.0 {
            (cm_w * 10.0, cm_h * 10.0)
        } else {
            return None;
        }
    };
    Some((mm_w, mm_h, h_active, v_active))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic EDID base block for a 1920×1080 panel measuring
    /// 531 mm × 299 mm (≈ 92 PPI), with all the bytes the parser reads.
    fn sample_edid() -> [u8; 128] {
        let mut e = [0u8; 128];
        e[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        // Base-block whole-cm fallback (53 cm × 30 cm).
        e[21] = 53;
        e[22] = 30;
        // Preferred Detailed Timing Descriptor at offset 54.
        let d = 54;
        e[d] = 0x01; // non-zero pixel clock → this is a DTD
        e[d + 1] = 0x00;
        // H active = 1920 = 0x780  → low 0x80, upper nibble 0x7
        e[d + 2] = 0x80;
        e[d + 4] = 0x70;
        // V active = 1080 = 0x438  → low 0x38, upper nibble 0x4
        e[d + 5] = 0x38;
        e[d + 7] = 0x40;
        // Image size: 531 mm (0x213) × 299 mm (0x12B)
        e[d + 12] = 0x13; // H mm low
        e[d + 13] = 0x2B; // V mm low
        e[d + 14] = 0x21; // H mm upper nibble = 2, V mm upper nibble = 1
        e
    }

    #[test]
    fn parses_size_and_resolution() {
        let (mm_w, mm_h, px_w, px_h) = parse_edid(&sample_edid()).expect("valid EDID");
        assert_eq!((px_w, px_h), (1920, 1080));
        assert_eq!(mm_w, 531.0);
        assert_eq!(mm_h, 299.0);
    }

    #[test]
    fn falls_back_to_cm_when_dtd_mm_absent() {
        let mut e = sample_edid();
        e[54 + 12] = 0;
        e[54 + 13] = 0;
        e[54 + 14] = 0;
        let (mm_w, mm_h, _, _) = parse_edid(&e).expect("valid EDID");
        assert_eq!(mm_w, 530.0); // 53 cm → 530 mm
        assert_eq!(mm_h, 300.0); // 30 cm → 300 mm
    }

    #[test]
    fn rejects_bad_header_and_short_input() {
        assert!(parse_edid(&[0u8; 128]).is_none());
        assert!(parse_edid(&[0u8; 10]).is_none());
    }
}
