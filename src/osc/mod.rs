pub mod client;
pub mod encode;
pub mod ipad_client;
pub mod ipad_encode;
pub mod ipad_parse;
pub mod ipad_values;
pub mod monitor_server;
pub mod parse;
pub mod qlab_client;
pub mod qlab_cue_builder;
pub mod qlab_reply;
pub mod trigger_listener;

// ─── Shared OSC address constants ───────────────────────────────────

/// OSC address that triggers a snapshot recall on the daemon. The Phase D
/// QLab "Create Trigger Cue" feature embeds this in a network cue's
/// customString; the Phase E trigger listener parses incoming OSC for it.
/// Defining the constant once here keeps the two sides in lockstep.
pub const SNAPSHOT_RECALL_ADDR: &str = "/snapshot/recall";

/// Variant that bypasses the snapshot's exclusion scope (only meaningful for
/// `SnapshotKind::ApplyOnRecall` snapshots — see `SnapshotEngine::recall`).
pub const SNAPSHOT_RECALL_FULL_ADDR: &str = "/snapshot/recall_full";

// ─── Tolerant OSC decoding ──────────────────────────────────────────

/// Decode a UDP datagram as OSC, tolerating DiGiCo's non-4-byte-aligned
/// packets.
///
/// SD/Quantum consoles (and the S21's iPad link in places) emit OSC whose
/// total length is not a multiple of 4 — strict decoders reject those
/// outright. When the standard decode fails on an unaligned datagram, retry
/// once with zero-padding up to the next 4-byte boundary (padding bytes are
/// exactly what a conformant encoder would have appended). Bare-path DiGiCo
/// query packets (path + NUL, no type tag) are NOT handled here — callers
/// keep their existing `parse_digico_packet` / `parse_bare_path` fallbacks.
pub fn decode_udp_tolerant(data: &[u8]) -> Option<rosc::OscPacket> {
    match rosc::decoder::decode_udp(data) {
        Ok((_, packet)) => Some(packet),
        Err(_) if !data.is_empty() && !data.len().is_multiple_of(4) && starts_a_type_tag(data) => {
            let mut padded = data.to_vec();
            padded.resize(data.len().next_multiple_of(4), 0);
            rosc::decoder::decode_udp(&padded).ok().map(|(_, p)| p)
        }
        Err(_) => None,
    }
}

/// Whether `data` looks like a real OSC message rather than a DiGiCo
/// bare-path packet: a conformant message follows the padded address with a
/// type-tag string, which always begins with `,`.
///
/// Without this check the zero-padded retry can *succeed* on a bare path
/// carrying a one-character inline value (`/…/mute\0\0\0"1"`): the decoder
/// reads the stray character as a type tag, yields a message with no
/// arguments, and the caller's `parse_digico_packet` fallback — which would
/// have recovered the value — never runs, silently dropping the update.
fn starts_a_type_tag(data: &[u8]) -> bool {
    let Some(null) = data.iter().position(|&b| b == 0) else {
        return false;
    };
    // The address is null-terminated then padded to a 4-byte boundary; the
    // type-tag string starts immediately after. Same rounding as
    // `ipad_client::parse_digico_packet`.
    let type_tag_start = (null + 4) & !3;
    matches!(data.get(type_tag_start), Some(b','))
}

#[cfg(test)]
mod tolerant_decode_tests {
    use super::*;
    use rosc::{OscMessage, OscPacket, OscType};

    fn encode(path: &str, args: Vec<OscType>) -> Vec<u8> {
        rosc::encoder::encode(&OscPacket::Message(OscMessage {
            addr: path.to_string(),
            args,
        }))
        .unwrap()
    }

    #[test]
    fn decodes_wellformed_packets() {
        let bytes = encode("/Input_Channels/1/fader", vec![OscType::Float(-10.0)]);
        let packet = decode_udp_tolerant(&bytes).expect("well-formed packet decodes");
        match packet {
            OscPacket::Message(m) => assert_eq!(m.addr, "/Input_Channels/1/fader"),
            other => panic!("expected message, got {other:?}"),
        }
    }

    #[test]
    fn decodes_truncated_padding() {
        // Simulate a DiGiCo console dropping the trailing alignment zeros.
        // A no-arg message ends in the type-tag block "," + 3 NUL pad bytes,
        // so stripping up to 3 trailing zeros leaves an unaligned datagram
        // that a strict decoder rejects.
        let bytes = encode("/Console/Name/?", vec![]);
        let mut truncated = bytes.clone();
        for _ in 0..3 {
            assert_eq!(truncated.pop(), Some(0), "test premise: trailing pad is 0");
        }
        assert_ne!(truncated.len() % 4, 0, "test premise: unaligned length");
        let packet = decode_udp_tolerant(&truncated).expect("unaligned packet decodes");
        match packet {
            OscPacket::Message(m) => {
                assert_eq!(m.addr, "/Console/Name/?");
                assert!(m.args.is_empty());
            }
            other => panic!("expected message, got {other:?}"),
        }
    }

    #[test]
    fn garbage_stays_none() {
        assert!(decode_udp_tolerant(&[]).is_none());
        assert!(decode_udp_tolerant(&[0xFF, 0x13, 0x37]).is_none());
    }

    #[test]
    fn bare_path_with_inline_value_is_left_to_the_digico_fallback() {
        // DiGiCo also emits non-standard packets: a null-terminated path,
        // padded, then an inline value with no type tag. Zero-padding one of
        // those can accidentally decode (the stray value byte reads as a type
        // tag) and produce an argument-less message, which would silently
        // discard the value instead of letting `parse_digico_packet` recover
        // it. We must decline these.
        let mut bare = b"/Input_Channels/1/mute".to_vec();
        bare.push(0);
        while !bare.len().is_multiple_of(4) {
            bare.push(0);
        }
        bare.extend_from_slice(b"1"); // one-character inline value
        assert!(!bare.len().is_multiple_of(4), "test premise: unaligned");
        assert!(
            decode_udp_tolerant(&bare).is_none(),
            "bare-path packet must fall through to the DiGiCo parser"
        );
    }

    #[test]
    fn type_tag_detection() {
        // Well-formed message: address, pad, then ",f".
        let msg = encode("/a/b", vec![OscType::Float(1.0)]);
        assert!(starts_a_type_tag(&msg));
        // Bare path with no type tag at all.
        let mut bare = b"/a/b".to_vec();
        bare.push(0);
        while !bare.len().is_multiple_of(4) {
            bare.push(0);
        }
        assert!(!starts_a_type_tag(&bare));
        // No null terminator at all.
        assert!(!starts_a_type_tag(b"/a/b"));
    }
}
