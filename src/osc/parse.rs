use rosc::OscType;

use crate::model::channel::ChannelId;
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};

/// Result of parsing a GP OSC message.
#[derive(Debug)]
pub enum ParsedOscMessage {
    /// A channel parameter update.
    ParameterUpdate(ParameterAddress, ParameterValue),
    /// Console ping (keepalive).
    Ping,
    /// Console pong (keepalive response).
    Pong,
    /// Discovery response: channel count for a specific type (per-type sub-path form).
    DiscoveryCount { channel_type: String, count: u16 },
    /// Discovery response: positional channel counts reply.
    /// `/console/channel/counts <inputs> <aux> <groups> <control_groups> <matrices> <master>`
    /// This is the canonical GP OSC reply form on the real S21+.
    ChannelCounts {
        inputs: u16,
        aux: u16,
        groups: u16,
        control_groups: u16,
        matrices: u16,
        master: u16,
    },
    /// The console's current snapshot row, for Console→App follow. Parsed from
    /// an inbound `/digico/snapshots/fire <n>` — the desk echoing its own
    /// snapshot-fire command (exact inbound path pending real-desk
    /// verification; see `process_message`).
    CurrentSnapshot(i32),
    /// Unrecognized message.
    Unknown(String),
}

/// Parse a GP OSC message path and arguments into a typed result.
pub fn parse_gp_osc(path: &str, args: &[OscType]) -> ParsedOscMessage {
    parse_gp_osc_with_config(path, args, None)
}

/// Config-aware GP OSC parser that correctly classifies aux vs group buses.
pub fn parse_gp_osc_with_config(
    path: &str,
    args: &[OscType],
    mix_output_types: Option<&[bool]>,
) -> ParsedOscMessage {
    // System commands
    match path {
        "/console/ping" => return ParsedOscMessage::Ping,
        "/console/pong" => return ParsedOscMessage::Pong,
        _ => {}
    }

    // Current-snapshot report (Console→App follow). The desk echoes its
    // snapshot-fire command; an inbound `/digico/snapshots/fire <n>` carries
    // the row the desk just loaded.
    if path == "/digico/snapshots/fire"
        && let Some(n) = args.first().and_then(extract_i32)
    {
        return ParsedOscMessage::CurrentSnapshot(n);
    }

    // Positional channel counts (canonical GP OSC reply form):
    // /console/channel/counts <inputs> <aux> <groups> <control_groups> <matrices> <master>
    if path == "/console/channel/counts" && args.len() >= 6 {
        if let (Some(inputs), Some(aux), Some(groups), Some(cgs), Some(matrices), Some(master)) = (
            extract_u16(&args[0]),
            extract_u16(&args[1]),
            extract_u16(&args[2]),
            extract_u16(&args[3]),
            extract_u16(&args[4]),
            extract_u16(&args[5]),
        ) {
            return ParsedOscMessage::ChannelCounts {
                inputs,
                aux,
                groups,
                control_groups: cgs,
                matrices,
                master,
            };
        }
    }

    // Per-type discovery responses (kept for back-compat with mock/iPad sources):
    // /console/channel/counts/{type} INT
    if let Some(type_name) = path.strip_prefix("/console/channel/counts/") {
        if let Some(count) = args.first().and_then(extract_u16) {
            return ParsedOscMessage::DiscoveryCount {
                channel_type: type_name.to_string(),
                count,
            };
        }
    }

    // Channel parameter: /channel/{ch}/...
    if let Some(rest) = path.strip_prefix("/channel/") {
        if let Some(parsed) = parse_channel_parameter(rest, args, mix_output_types) {
            return ParsedOscMessage::ParameterUpdate(parsed.0, parsed.1);
        }
    }

    ParsedOscMessage::Unknown(path.to_string())
}

fn extract_u16(arg: &OscType) -> Option<u16> {
    match arg {
        OscType::Int(i) => u16::try_from(*i).ok(),
        OscType::Float(f) => u16::try_from(*f as i32).ok(),
        _ => None,
    }
}

fn extract_i32(arg: &OscType) -> Option<i32> {
    match arg {
        OscType::Int(i) => Some(*i),
        OscType::Long(l) => Some(*l as i32),
        OscType::Float(f) => Some(*f as i32),
        _ => None,
    }
}

/// Parse a channel parameter path like "{ch}/fader" with its args.
fn parse_channel_parameter(
    path: &str,
    args: &[OscType],
    mix_output_types: Option<&[bool]>,
) -> Option<(ParameterAddress, ParameterValue)> {
    // Split into channel number and parameter suffix
    let slash = path.find('/')?;
    let ch_str = &path[..slash];
    let suffix = &path[slash + 1..];

    let ch_num: u8 = ch_str.parse().ok()?;
    let channel = ChannelId::from_gp_osc_number_with_config(ch_num, mix_output_types)?;
    let parameter = ParameterPath::from_gp_osc_suffix(suffix)?;

    let value = extract_value(&parameter, args)?;

    Some((ParameterAddress { channel, parameter }, value))
}

/// Extract a typed value from OSC args based on the expected parameter type.
fn extract_value(parameter: &ParameterPath, args: &[OscType]) -> Option<ParameterValue> {
    let arg = args.first()?;

    match parameter {
        // String parameters
        ParameterPath::Name => match arg {
            OscType::String(s) => Some(ParameterValue::String(s.clone())),
            _ => None,
        },
        // Boolean parameters
        ParameterPath::Mute
        | ParameterPath::Solo
        | ParameterPath::GainTracking
        | ParameterPath::DelayEnabled
        | ParameterPath::DigitubeEnabled
        | ParameterPath::EqEnabled
        | ParameterPath::HighpassEnabled
        | ParameterPath::LowpassEnabled
        | ParameterPath::EqBandDynEnabled(_)
        | ParameterPath::Dyn1Enabled
        | ParameterPath::Dyn1Listen(_)
        | ParameterPath::Dyn2Enabled
        | ParameterPath::Dyn2Listen
        | ParameterPath::SendEnabled(_) => extract_bool(arg),
        // Integer parameters
        ParameterPath::Polarity
        | ParameterPath::DigitubeBias
        | ParameterPath::Dyn1Mode
        | ParameterPath::Dyn1Knee(_)
        | ParameterPath::Dyn2Mode
        | ParameterPath::Dyn2Knee => extract_int(arg),
        // Everything else is float
        _ => extract_float(arg),
    }
}

fn extract_float(arg: &OscType) -> Option<ParameterValue> {
    match arg {
        OscType::Float(f) => Some(ParameterValue::Float(*f)),
        OscType::Double(d) => Some(ParameterValue::Float(*d as f32)),
        OscType::Int(i) => Some(ParameterValue::Float(*i as f32)),
        OscType::Long(l) => Some(ParameterValue::Float(*l as f32)),
        _ => None,
    }
}

fn extract_int(arg: &OscType) -> Option<ParameterValue> {
    match arg {
        OscType::Int(i) => Some(ParameterValue::Int(*i)),
        OscType::Long(l) => Some(ParameterValue::Int(*l as i32)),
        OscType::Float(f) => Some(ParameterValue::Int(*f as i32)),
        _ => None,
    }
}

fn extract_bool(arg: &OscType) -> Option<ParameterValue> {
    match arg {
        OscType::Bool(b) => Some(ParameterValue::Bool(*b)),
        OscType::Int(i) => Some(ParameterValue::Bool(*i != 0)),
        OscType::Float(f) => Some(ParameterValue::Bool(*f != 0.0)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parameter::ParameterAddress;

    #[test]
    fn parse_fader() {
        let result = parse_gp_osc("/channel/1/fader", &[OscType::Float(-10.0)]);
        match result {
            ParsedOscMessage::ParameterUpdate(addr, val) => {
                assert_eq!(addr.channel, ChannelId::Input(1));
                assert_eq!(addr.parameter, ParameterPath::Fader);
                assert_eq!(val, ParameterValue::Float(-10.0));
            }
            _ => panic!("Expected ParameterUpdate"),
        }
    }

    #[test]
    fn parse_mute() {
        let result = parse_gp_osc("/channel/70/mute", &[OscType::Int(1)]);
        match result {
            ParsedOscMessage::ParameterUpdate(addr, val) => {
                assert_eq!(addr.channel, ChannelId::Aux(1));
                assert_eq!(addr.parameter, ParameterPath::Mute);
                assert_eq!(val, ParameterValue::Bool(true));
            }
            _ => panic!("Expected ParameterUpdate"),
        }
    }

    #[test]
    fn parse_eq_band() {
        // GP OSC wire band index is 0-based: wire `2` = the third band (internal band 3).
        let result = parse_gp_osc("/channel/1/eq/2/frequency", &[OscType::Float(1000.0)]);
        match result {
            ParsedOscMessage::ParameterUpdate(addr, val) => {
                assert_eq!(addr.channel, ChannelId::Input(1));
                assert_eq!(addr.parameter, ParameterPath::EqBandFrequency(3));
                assert_eq!(val, ParameterValue::Float(1000.0));
            }
            _ => panic!("Expected ParameterUpdate"),
        }
    }

    #[test]
    fn gp_osc_eq_band_round_trip() {
        // Round-trip every band through encode → parse to lock the 0-based wire convention.
        use crate::osc::encode::encode_parameter;
        for b in 1u8..=4 {
            let addr = ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::EqBandGain(b),
            };
            let (path, args) =
                encode_parameter(&addr, &ParameterValue::Float(0.0)).expect("encode EQ band");
            // Wire path uses (b-1)
            assert_eq!(path, format!("/channel/1/eq/{}/gain", b - 1));
            match parse_gp_osc(&path, &args) {
                ParsedOscMessage::ParameterUpdate(parsed_addr, _) => {
                    assert_eq!(parsed_addr, addr);
                }
                _ => panic!("round-trip failed for band {b}"),
            }
        }
    }

    #[test]
    fn gp_osc_dyn1_band_round_trip() {
        use crate::osc::encode::encode_parameter;
        for b in 1u8..=3 {
            let addr = ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Dyn1Threshold(b),
            };
            let (path, args) =
                encode_parameter(&addr, &ParameterValue::Float(-20.0)).expect("encode Dyn1 band");
            assert_eq!(path, format!("/channel/1/dyn1/{}/threshold", b - 1));
            match parse_gp_osc(&path, &args) {
                ParsedOscMessage::ParameterUpdate(parsed_addr, _) => {
                    assert_eq!(parsed_addr, addr);
                }
                _ => panic!("round-trip failed for dyn1 band {b}"),
            }
        }
    }

    #[test]
    fn parse_send_level() {
        let result = parse_gp_osc("/channel/1/send/3/level", &[OscType::Float(-5.0)]);
        match result {
            ParsedOscMessage::ParameterUpdate(addr, val) => {
                assert_eq!(addr.channel, ChannelId::Input(1));
                assert_eq!(addr.parameter, ParameterPath::SendLevel(3));
                assert_eq!(val, ParameterValue::Float(-5.0));
            }
            _ => panic!("Expected ParameterUpdate"),
        }
    }

    #[test]
    fn parse_channel_counts_positional() {
        let result = parse_gp_osc(
            "/console/channel/counts",
            &[
                OscType::Int(60),
                OscType::Int(10),
                OscType::Int(14),
                OscType::Int(10),
                OscType::Int(8),
                OscType::Int(1),
            ],
        );
        match result {
            ParsedOscMessage::ChannelCounts {
                inputs,
                aux,
                groups,
                control_groups,
                matrices,
                master,
            } => {
                assert_eq!(inputs, 60);
                assert_eq!(aux, 10);
                assert_eq!(groups, 14);
                assert_eq!(control_groups, 10);
                assert_eq!(matrices, 8);
                assert_eq!(master, 1);
            }
            other => panic!("Expected ChannelCounts, got {other:?}"),
        }
    }

    #[test]
    fn parse_ping_pong() {
        assert!(matches!(
            parse_gp_osc("/console/ping", &[]),
            ParsedOscMessage::Ping
        ));
        assert!(matches!(
            parse_gp_osc("/console/pong", &[]),
            ParsedOscMessage::Pong
        ));
    }

    #[test]
    fn parse_unknown() {
        assert!(matches!(
            parse_gp_osc("/some/unknown/path", &[]),
            ParsedOscMessage::Unknown(_)
        ));
    }

    #[test]
    fn parse_current_snapshot() {
        // An inbound `/digico/snapshots/fire <n>` is the desk's current-snapshot
        // report (Console→App follow). Accept Int and Float-encoded rows.
        assert!(matches!(
            parse_gp_osc("/digico/snapshots/fire", &[OscType::Int(7)]),
            ParsedOscMessage::CurrentSnapshot(7)
        ));
        assert!(matches!(
            parse_gp_osc("/digico/snapshots/fire", &[OscType::Float(7.0)]),
            ParsedOscMessage::CurrentSnapshot(7)
        ));
        // No argument → not a current-snapshot report.
        assert!(matches!(
            parse_gp_osc("/digico/snapshots/fire", &[]),
            ParsedOscMessage::Unknown(_)
        ));
    }

    #[test]
    fn parse_name() {
        let result = parse_gp_osc("/channel/1/name", &[OscType::String("Kick".to_string())]);
        match result {
            ParsedOscMessage::ParameterUpdate(addr, val) => {
                assert_eq!(addr.parameter, ParameterPath::Name);
                assert_eq!(val, ParameterValue::String("Kick".to_string()));
            }
            _ => panic!("Expected ParameterUpdate"),
        }
    }

    // ─── Audit M6: property-based fuzz coverage ────────────────────────
    use proptest::prelude::*;

    fn arb_osc_type() -> impl Strategy<Value = OscType> {
        prop_oneof![
            any::<f32>().prop_map(OscType::Float),
            any::<f64>().prop_map(OscType::Double),
            any::<i32>().prop_map(OscType::Int),
            any::<i64>().prop_map(OscType::Long),
            any::<bool>().prop_map(OscType::Bool),
            ".*".prop_map(OscType::String),
        ]
    }

    proptest! {
        /// Fully arbitrary path + arg list. Most paths fall through to
        /// `Unknown`; the rest exercise channel-counts parsing,
        /// channel-parameter parsing, and value extraction.
        #[test]
        fn parse_gp_osc_no_panic_on_arbitrary(
            path in ".*",
            args in proptest::collection::vec(arb_osc_type(), 0..8)
        ) {
            let _ = parse_gp_osc(&path, &args);
        }

        /// Bias toward `/channel/{n}/...` paths so we exercise the channel
        /// number → ChannelId mapping with the per-type bounds checks
        /// (audit H6 territory).
        #[test]
        fn parse_gp_osc_no_panic_on_channel_paths(
            num in any::<u8>(),
            suffix in ".*",
            args in proptest::collection::vec(arb_osc_type(), 0..4)
        ) {
            let path = format!("/channel/{num}/{suffix}");
            let _ = parse_gp_osc(&path, &args);
        }

        /// `/console/channel/counts` with positional args of arbitrary
        /// type — exercises the `extract_u8` cast path on weird inputs
        /// (huge ints, NaN floats, signed negatives).
        #[test]
        fn parse_gp_osc_no_panic_on_channel_counts(
            args in proptest::collection::vec(arb_osc_type(), 0..10)
        ) {
            let _ = parse_gp_osc("/console/channel/counts", &args);
        }
    }
}
