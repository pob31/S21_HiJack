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

/// Deepest nesting accepted from the network, counting the packet itself,
/// each enclosing bundle and each array level. DiGiCo consoles, QLab and the
/// monitor clients never nest at all; the headroom is for tools that wrap
/// their messages in a bundle or two.
const MAX_OSC_NESTING: usize = 8;

/// Decode a UDP datagram as OSC, refusing anything nested deeper than
/// [`MAX_OSC_NESTING`]. Every listener that decodes untrusted datagrams goes
/// through here (or through [`decode_udp_tolerant`], which does).
pub fn decode_udp_bounded(data: &[u8]) -> Result<rosc::OscPacket, rosc::OscError> {
    if nesting_exceeds_limit(data) {
        return Err(rosc::OscError::BadPacket("OSC nested too deeply"));
    }
    rosc::decoder::decode_udp(data).map(|(_, packet)| packet)
}

/// Whether rosc would build a packet nested deeper than [`MAX_OSC_NESTING`].
///
/// rosc 0.10 recurses once per bundle level with no limit, and although it
/// reads arrays iteratively, the value it returns nests one level per `[`
/// and is dropped recursively. ~2,800 bundle levels or ~30,000 array levels
/// fit in one datagram, far past a 2 MiB tokio worker stack, and a stack
/// overflow aborts the whole process rather than panicking.
///
/// Walks the datagram the way rosc's decoder does, but with an explicit
/// stack instead of recursion. Where the bytes are malformed it stops early
/// and leaves the verdict to rosc; everything rosc could descend into is
/// visited, so the depth found is never less than rosc's.
fn nesting_exceeds_limit(data: &[u8]) -> bool {
    // (start, end, depth) of each packet still to inspect, as offsets into
    // `data`. A bundle element is bounded by its own size prefix.
    let mut pending = vec![(0, data.len(), 1)];
    while let Some((start, end, depth)) = pending.pop() {
        if depth > MAX_OSC_NESTING {
            return true;
        }
        let Some((addr, after_addr)) = osc_string_at(data, start, end) else {
            continue;
        };
        if addr == b"#bundle" {
            // An 8-byte time tag, then elements each prefixed by their size.
            let mut at = after_addr + 8;
            while let Some(size) = data.get(at..at + 4).filter(|_| at + 4 <= end) {
                let size = u32::from_be_bytes([size[0], size[1], size[2], size[3]]) as usize;
                let element = at + 4;
                let Some(element_end) = element.checked_add(size).filter(|&e| e <= end) else {
                    break;
                };
                pending.push((element, element_end, depth + 1));
                at = element_end;
            }
        } else if addr.first() == Some(&b'/') {
            let Some((type_tags, _)) = osc_string_at(data, after_addr, end) else {
                continue;
            };
            // rosc skips the leading ',' and opens one array level per '['.
            let mut arrays = 0;
            for &tag in type_tags.iter().skip(1) {
                match tag {
                    b'[' => {
                        arrays += 1;
                        if depth + arrays > MAX_OSC_NESTING {
                            return true;
                        }
                    }
                    b']' => arrays = arrays.saturating_sub(1),
                    _ => {}
                }
            }
        }
    }
    false
}

/// The null-terminated OSC string starting at `start`, and the offset where
/// the next field begins once the terminator and padding are skipped. As in
/// rosc, padding is counted from the start of the datagram, and a string
/// whose padding runs past `end` is malformed.
fn osc_string_at(data: &[u8], start: usize, end: usize) -> Option<(&[u8], usize)> {
    let field = data.get(start..end)?;
    let len = field.iter().position(|&b| b == 0)?;
    let next = (start + len + 4) & !3;
    (next <= end).then(|| (&field[..len], next))
}

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
    match decode_udp_bounded(data) {
        Ok(packet) => Some(packet),
        Err(_) if !data.is_empty() && !data.len().is_multiple_of(4) && starts_a_type_tag(data) => {
            let mut padded = data.to_vec();
            padded.resize(data.len().next_multiple_of(4), 0);
            decode_udp_bounded(&padded).ok()
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

    /// `levels` bundles, each wrapping the next, around one empty message.
    /// Every level costs 20 bytes: "#bundle\0", an 8-byte time tag and the
    /// 4-byte element size.
    fn nested_bundles(levels: usize) -> Vec<u8> {
        let mut packet = encode("/a", vec![]);
        for _ in 0..levels {
            let mut outer = b"#bundle\0".to_vec();
            outer.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
            outer.extend_from_slice(&(packet.len() as u32).to_be_bytes());
            outer.extend_from_slice(&packet);
            packet = outer;
        }
        packet
    }

    /// A message whose one argument is `levels` arrays, each inside the next:
    /// type tags `,[[[…]]]`, one byte per bracket.
    fn nested_arrays(levels: usize) -> Vec<u8> {
        let mut packet = b"/a\0\0,".to_vec();
        packet.extend(std::iter::repeat_n(b'[', levels));
        packet.extend(std::iter::repeat_n(b']', levels));
        packet.push(0);
        while !packet.len().is_multiple_of(4) {
            packet.push(0);
        }
        packet
    }

    /// Decode on a thread with tokio's default 2 MiB worker stack, the stack
    /// the receive loops really run on.
    fn decode_on_worker_stack(data: Vec<u8>) -> Option<OscPacket> {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || decode_udp_tolerant(&data))
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn deeply_nested_bundle_is_refused_not_a_stack_overflow() {
        // ~2,800 levels fit in one 56 KB datagram; rosc recurses once per
        // level with no limit, so without the guard this aborts the process.
        let data = nested_bundles(2_800);
        assert!(data.len() < 65_536, "test premise: fits one datagram");
        assert!(decode_on_worker_stack(data).is_none());
    }

    #[test]
    fn deeply_nested_array_is_refused_not_a_stack_overflow() {
        // rosc parses arrays iteratively, but the result is a value nested
        // one level per bracket, and dropping it recurses just as deep.
        let data = nested_arrays(30_000);
        assert!(data.len() < 65_536, "test premise: fits one datagram");
        assert!(decode_on_worker_stack(data).is_none());
    }

    #[test]
    fn shallow_nesting_still_decodes() {
        match decode_udp_tolerant(&nested_bundles(MAX_OSC_NESTING - 1)) {
            Some(OscPacket::Bundle(_)) => {}
            other => panic!("expected a bundle, got {other:?}"),
        }
        assert!(decode_udp_tolerant(&nested_bundles(MAX_OSC_NESTING)).is_none());

        match decode_udp_tolerant(&nested_arrays(MAX_OSC_NESTING - 1)) {
            Some(OscPacket::Message(m)) => assert_eq!(m.args.len(), 1),
            other => panic!("expected a message, got {other:?}"),
        }
        assert!(decode_udp_tolerant(&nested_arrays(MAX_OSC_NESTING)).is_none());
    }

    #[test]
    fn flat_bundle_of_many_messages_decodes() {
        // Width is not depth: a single bundle carrying lots of messages is
        // ordinary traffic and must not trip the guard.
        let messages: Vec<OscPacket> = (0..50)
            .map(|i| {
                OscPacket::Message(OscMessage {
                    addr: format!("/Input_Channels/{i}/fader"),
                    args: vec![OscType::Float(-10.0)],
                })
            })
            .collect();
        let bytes = rosc::encoder::encode(&OscPacket::Bundle(rosc::OscBundle {
            timetag: rosc::OscTime {
                seconds: 0,
                fractional: 1,
            },
            content: messages,
        }))
        .unwrap();
        match decode_udp_tolerant(&bytes) {
            Some(OscPacket::Bundle(b)) => assert_eq!(b.content.len(), 50),
            other => panic!("expected a bundle, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod nesting_fuzz_tests {
    use super::*;
    use proptest::prelude::*;
    use rosc::{OscArray, OscBundle, OscMessage, OscPacket, OscTime, OscType};

    const TIMETAG: OscTime = OscTime {
        seconds: 0,
        fractional: 1,
    };

    fn arb_arg() -> impl Strategy<Value = OscType> {
        let leaf = prop_oneof![
            any::<i32>().prop_map(OscType::Int),
            "[a-z]{0,6}".prop_map(OscType::String),
        ];
        leaf.prop_recursive(3, 16, 3, |inner| {
            proptest::collection::vec(inner, 0..3)
                .prop_map(|content| OscType::Array(OscArray { content }))
        })
    }

    /// A small random packet, a few levels deep at most.
    fn arb_shallow_packet() -> impl Strategy<Value = OscPacket> {
        let message = ("/[a-z]{1,6}", proptest::collection::vec(arb_arg(), 0..3))
            .prop_map(|(addr, args)| OscPacket::Message(OscMessage { addr, args }));
        message.prop_recursive(3, 16, 3, |inner| {
            proptest::collection::vec(inner, 0..3).prop_map(|content| {
                OscPacket::Bundle(OscBundle {
                    timetag: TIMETAG,
                    content,
                })
            })
        })
    }

    /// A spine of up to 13 bundles around a message whose argument is up to
    /// 13 arrays deep, with a random sibling beside it at every bundle level.
    /// Random recursion alone rarely goes deep, so the spine is what puts
    /// cases on both sides of the limit.
    fn arb_packet() -> impl Strategy<Value = OscPacket> {
        (
            0usize..14,
            0usize..14,
            proptest::collection::vec((arb_shallow_packet(), any::<bool>()), 14),
        )
            .prop_map(|(bundles, arrays, siblings)| {
                let mut arg = OscType::Int(0);
                for _ in 0..arrays {
                    arg = OscType::Array(OscArray { content: vec![arg] });
                }
                let mut packet = OscPacket::Message(OscMessage {
                    addr: "/spine".to_string(),
                    args: vec![arg],
                });
                for (sibling, before) in siblings.into_iter().take(bundles) {
                    let content = if before {
                        vec![sibling, packet]
                    } else {
                        vec![packet, sibling]
                    };
                    packet = OscPacket::Bundle(OscBundle {
                        timetag: TIMETAG,
                        content,
                    });
                }
                packet
            })
    }

    fn arg_depth(arg: &OscType) -> usize {
        match arg {
            OscType::Array(a) => 1 + a.content.iter().map(arg_depth).max().unwrap_or(0),
            _ => 0,
        }
    }

    /// Nesting depth as [`nesting_exceeds_limit`] counts it: 1 for the
    /// packet, plus one per enclosing bundle or array.
    fn packet_depth(packet: &OscPacket) -> usize {
        match packet {
            OscPacket::Message(m) => 1 + m.args.iter().map(arg_depth).max().unwrap_or(0),
            OscPacket::Bundle(b) => 1 + b.content.iter().map(packet_depth).max().unwrap_or(0),
        }
    }

    proptest! {
        /// Whatever the bytes, the decoder never hands back a packet nested
        /// deeper than the limit. Truncation and a stray byte stand in for
        /// hostile datagrams whose sizes and tags don't add up; this checks
        /// the guard against rosc itself, not against a copy of its logic.
        #[test]
        fn decoded_nesting_never_exceeds_limit(
            packet in arb_packet(),
            cut in proptest::option::of(any::<prop::sample::Index>()),
            poke in proptest::option::of((any::<prop::sample::Index>(), any::<u8>())),
        ) {
            let mut bytes = rosc::encoder::encode(&packet).unwrap();
            if let Some((at, value)) = poke {
                let at = at.index(bytes.len());
                bytes[at] = value;
            }
            if let Some(cut) = cut {
                bytes.truncate(cut.index(bytes.len()));
            }
            if let Some(decoded) = decode_udp_tolerant(&bytes) {
                prop_assert!(packet_depth(&decoded) <= MAX_OSC_NESTING);
            }
        }

        /// The guard is exact on well-formed packets: everything within the
        /// limit decodes unchanged, and everything past it is refused.
        #[test]
        fn limit_is_exact_on_wellformed_packets(packet in arb_packet()) {
            let bytes = rosc::encoder::encode(&packet).unwrap();
            let decoded = decode_udp_tolerant(&bytes);
            if packet_depth(&packet) <= MAX_OSC_NESTING {
                prop_assert_eq!(decoded, Some(packet));
            } else {
                prop_assert!(decoded.is_none());
            }
        }
    }
}
