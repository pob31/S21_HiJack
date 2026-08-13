use rosc::OscType;

use super::ipad_values;
use crate::model::channel::ChannelId;
use crate::model::config::ChannelMode;
use crate::model::family::{LevelWire, PadQuirks, PadWire, PanWire};
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};

/// Result of parsing an iPad protocol OSC message.
#[derive(Debug)]
pub enum ParsedIpadMessage {
    /// A channel parameter update.
    ParameterUpdate(ParameterAddress, ParameterValue),
    /// Console configuration response (from handshake).
    ConfigResponse(IpadConfigMessage),
    /// Layout bank data.
    LayoutBank(BankData),
    /// Meter data.
    MeterValues(Vec<(u8, f32)>),
    /// Current snapshot info.
    SnapshotInfo { current: i32 },
    /// Unrecognized message.
    Unknown(String),
}

/// Console configuration message from iPad handshake.
#[derive(Debug)]
pub enum IpadConfigMessage {
    ConsoleName {
        name: String,
        serial: String,
    },
    SessionFilename(Option<String>),
    ChannelCount {
        channel_type: String,
        count: u16,
    },
    OutputModes {
        channel_type: String,
        modes: Vec<ChannelMode>,
    },
    OutputTypes {
        types: Vec<bool>,
    },
}

/// Layout bank data from `/Layout/Layout/Banks`.
#[derive(Debug, Clone)]
pub struct BankData {
    pub side: String, // "Left" or "Right"
    pub bank_number: u8,
    pub label: String,
    pub channels: Vec<Option<ChannelId>>, // 10 slots, None for empty
}

/// Parse an iPad protocol OSC message.
///
/// Performs no per-type channel-number bounds checking — accepts any number
/// the iPad sends. Production paths should use
/// [`parse_ipad_message_with_config`] so out-of-range channels from a buggy
/// or mis-configured peer are dropped against the live `ConsoleConfig`.
pub fn parse_ipad_message(path: &str, args: &[OscType]) -> ParsedIpadMessage {
    parse_ipad_message_with_config(path, args, None)
}

/// Parse an iPad protocol OSC message, optionally validating channel numbers
/// against the live `ConsoleConfig`. Mirrors the GP OSC parser's
/// `parse_gp_osc_with_config` pattern. Pass `None` during handshake (before
/// config is populated) and in tests; pass `Some(&cfg)` on the proxy /
/// state-mirror hot paths.
pub fn parse_ipad_message_with_config(
    path: &str,
    args: &[OscType],
    config: Option<&crate::model::config::ConsoleConfig>,
) -> ParsedIpadMessage {
    // Wire dialect comes from the live console config when we have one;
    // pre-discovery callers (handshake) get the S21 defaults.
    let wire = config
        .map(|c| {
            let p = c.profile();
            PadWire::new(p.primary_surface(), p.pad_quirks)
        })
        .unwrap_or(PadWire::S21);
    parse_pad_message(path, args, config, &wire)
}

/// Parse an inbound message for an explicit wire dialect.
///
/// Used where the surface is known independently of the stored config — the
/// Protocol Probe, which exercises one surface at a time regardless of which
/// the profile prefers.
pub fn parse_pad_message(
    path: &str,
    args: &[OscType],
    config: Option<&crate::model::config::ConsoleConfig>,
    wire: &PadWire,
) -> ParsedIpadMessage {
    let quirks = wire.quirks;

    // Strip the surface's prefix before anything else: on the `/sd/` surface
    // every address is prefixed, and the rest of this parser (and the
    // `/Console/…`, `/Snapshots/…`, `/Meters/…` literals below) is written
    // against the bare tree. A prefixed surface that hands us an unprefixed
    // path is not ours to interpret.
    let surface_prefix = wire.surface.path_prefix();
    let path = if surface_prefix.is_empty() {
        path
    } else {
        match path.strip_prefix(surface_prefix) {
            Some(rest) => rest,
            None => return ParsedIpadMessage::Unknown(path.to_string()),
        }
    };

    // Snapshot info
    if path == "/Snapshots/Current_Snapshot"
        && let Some(n) = args.first().and_then(extract_i32)
    {
        return ParsedIpadMessage::SnapshotInfo { current: n };
    }

    // Console configuration responses
    if let Some(config) = try_parse_config(path, args) {
        return ParsedIpadMessage::ConfigResponse(config);
    }

    // Layout banks
    if path == "/Layout/Layout/Banks"
        && let Some(bank) = try_parse_layout_bank(args, &quirks)
    {
        return ParsedIpadMessage::LayoutBank(bank);
    }

    // Meter values
    if path == "/Meters/values" {
        return parse_meter_values(args);
    }

    // Channel parameter: /{ChannelType}/{number}/{suffix}
    if let Some((channel, suffix)) = ChannelId::from_ipad_path_with_config(path, config)
        && let Some(parameter) = ParameterPath::from_pad_suffix(suffix, &quirks)
        && let Some(value) = extract_value(&parameter, args, wire)
    {
        let addr = ParameterAddress { channel, parameter };
        return ParsedIpadMessage::ParameterUpdate(addr, value);
    }

    ParsedIpadMessage::Unknown(path.to_string())
}

fn try_parse_config(path: &str, args: &[OscType]) -> Option<IpadConfigMessage> {
    match path {
        "/Console/Name" => {
            // Args: name string, optional serial string
            let name = args.first().and_then(extract_string).unwrap_or_default();
            let serial = if name.contains(' ') {
                let parts: Vec<&str> = name.splitn(2, ' ').collect();
                return Some(IpadConfigMessage::ConsoleName {
                    name: parts[0].to_string(),
                    serial: parts[1].to_string(),
                });
            } else {
                String::new()
            };
            Some(IpadConfigMessage::ConsoleName { name, serial })
        }
        "/Console/Session/Filename" => {
            let filename = args.first().and_then(extract_string);
            Some(IpadConfigMessage::SessionFilename(filename))
        }
        p if p.starts_with("/Console/") && !p.contains("/modes") && !p.contains("/types") => {
            // Channel count: /Console/{ChannelType}  INT
            let channel_type = p.strip_prefix("/Console/")?;
            let count = args.first().and_then(extract_u16)?;
            Some(IpadConfigMessage::ChannelCount {
                channel_type: channel_type.to_string(),
                count,
            })
        }
        p if p.ends_with("/modes") => {
            // Output modes: /Console/{Type}/modes  INT INT INT ...
            let channel_type = p.strip_prefix("/Console/")?.strip_suffix("/modes")?;
            let modes: Vec<ChannelMode> = args
                .iter()
                .filter_map(extract_i32)
                .map(ChannelMode::from_int)
                .collect();
            if modes.is_empty() {
                return None;
            }
            Some(IpadConfigMessage::OutputModes {
                channel_type: channel_type.to_string(),
                modes,
            })
        }
        p if p.ends_with("/types") => {
            // Output types: /Console/Aux_Outputs/types  INT INT INT ...
            let types: Vec<bool> = args
                .iter()
                .filter_map(extract_i32)
                .map(|v| v == 1)
                .collect();
            if types.is_empty() {
                return None;
            }
            Some(IpadConfigMessage::OutputTypes { types })
        }
        _ => None,
    }
}

fn try_parse_layout_bank(args: &[OscType], q: &PadQuirks) -> Option<BankData> {
    // Args: Side BankNumber Label Unknown1 Unknown2 [ChannelType Number] * 10
    if args.len() < 5 {
        return None;
    }
    let side = extract_string(&args[0])?;
    let bank_number = extract_u8(&args[1])?;
    let label = extract_string(&args[2])?;

    let mut channels = Vec::new();
    let mut i = 5; // Skip Side, BankNumber, Label, Unknown1, Unknown2
    while i < args.len() {
        if let Some(ch_type) = extract_string(&args[i]) {
            if ch_type == "0" {
                channels.push(None);
                i += 1;
            } else if i + 1 < args.len() {
                if let Some(num) = extract_u16(&args[i + 1]) {
                    let channel = match ch_type.as_str() {
                        "Input_Channels" => Some(ChannelId::Input(num)),
                        "Aux_Outputs" => Some(ChannelId::Aux(num)),
                        "Group_Outputs" => Some(ChannelId::Group(num)),
                        "Matrix_Outputs" => Some(ChannelId::Matrix(num)),
                        // Same CG numbering quirk as the parameter path (see
                        // `ChannelId::from_ipad_path_with_config`); `checked_add`
                        // keeps a malformed max-value index from overflowing.
                        "Control_Groups" => {
                            if q.control_groups_zero_based {
                                num.checked_add(1).map(ChannelId::ControlGroup)
                            } else {
                                Some(ChannelId::ControlGroup(num))
                            }
                        }
                        "Graphic_EQ" => Some(ChannelId::GraphicEq(num)),
                        "Solo_Outputs" => None, // Not tracked
                        _ => None,
                    };
                    channels.push(channel);
                    i += 2;
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
        } else if let Some(n) = extract_i32(&args[i]) {
            if n == 0 {
                channels.push(None);
            }
            i += 1;
        } else {
            i += 1;
        }
    }

    Some(BankData {
        side,
        bank_number,
        label,
        channels,
    })
}

fn parse_meter_values(args: &[OscType]) -> ParsedIpadMessage {
    // Pairs of (meter_index, value)
    let mut meters = Vec::new();
    let mut i = 0;
    while i + 1 < args.len() {
        if let (Some(idx), Some(val)) = (extract_u8(&args[i]), extract_f32(&args[i + 1])) {
            meters.push((idx, val));
            i += 2;
        } else {
            i += 1;
        }
    }
    ParsedIpadMessage::MeterValues(meters)
}

/// Extract a typed value from OSC args, undoing the surface's pan and level
/// wire conventions so the mirror always holds internal units.
fn extract_value(
    parameter: &ParameterPath,
    args: &[OscType],
    wire: &PadWire,
) -> Option<ParameterValue> {
    let q = &wire.quirks;
    let arg = args.first()?;

    let value = match parameter {
        // String parameters
        ParameterPath::Name => match arg {
            OscType::String(s) => Some(ParameterValue::String(s.clone())),
            _ => None,
        },
        // Boolean parameters
        ParameterPath::Mute
        | ParameterPath::Solo
        | ParameterPath::DelayEnabled
        | ParameterPath::EqEnabled
        | ParameterPath::HighpassEnabled
        | ParameterPath::LowpassEnabled
        | ParameterPath::EqBandDynEnabled(_)
        | ParameterPath::Dyn1Enabled
        | ParameterPath::Dyn1Listen(_)
        | ParameterPath::Dyn2Enabled
        | ParameterPath::Dyn2KeySolo
        | ParameterPath::SendEnabled(_)
        | ParameterPath::GroupSendOn(_)
        | ParameterPath::MasterBusOn
        | ParameterPath::InsertAEnabled
        | ParameterPath::InsertBEnabled
        | ParameterPath::MatrixSendOn(_)
        | ParameterPath::GeqEnabled
        | ParameterPath::Phantom => extract_bool(arg),
        // Integer parameters
        ParameterPath::Polarity
        | ParameterPath::EqBandCurve(_)
        | ParameterPath::EqBandDynOverUnder(_)
        | ParameterPath::Dyn1MultibandDeesser
        | ParameterPath::Dyn1Knee(_)
        | ParameterPath::Dyn2Mode
        | ParameterPath::StereoMode
        | ParameterPath::MainAltIn => extract_int(arg),
        // CG membership — bitmask/int
        ParameterPath::CgLevel | ParameterPath::CgMute => extract_int(arg),
        // Everything else is float
        _ => extract_float(arg),
    }?;

    // Undo the wire conventions, wire → internal. `SignedUnit` pan and `Db`
    // levels already match the internal representation, so only the converted
    // forms do work here. Mirrors `ipad_encode::convert_value_for_pad`.
    match parameter {
        ParameterPath::Pan | ParameterPath::SendPan(_) if q.pan_wire == PanWire::ZeroToOne => {
            if let ParameterValue::Float(f) = &value {
                Some(ParameterValue::Float(ipad_values::ipad_pan_to_internal(*f)))
            } else {
                Some(value)
            }
        }
        p if p.is_fader_level() && wire.surface.level_wire() == LevelWire::Normalized01 => {
            if let ParameterValue::Float(n) = &value {
                Some(ParameterValue::Float(ipad_values::normalized_to_db(*n)))
            } else {
                Some(value)
            }
        }
        _ => Some(value),
    }
}

fn extract_string(arg: &OscType) -> Option<String> {
    match arg {
        OscType::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn extract_u8(arg: &OscType) -> Option<u8> {
    match arg {
        OscType::Int(i) => u8::try_from(*i).ok(),
        OscType::Float(f) => u8::try_from(*f as i32).ok(),
        _ => None,
    }
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
        OscType::Float(f) => Some(*f as i32),
        OscType::Long(l) => Some(*l as i32),
        _ => None,
    }
}

fn extract_f32(arg: &OscType) -> Option<f32> {
    match arg {
        OscType::Float(f) => Some(*f),
        OscType::Int(i) => Some(*i as f32),
        _ => None,
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

    #[test]
    fn parse_fader_response() {
        let result = parse_ipad_message("/Input_Channels/1/fader", &[OscType::Float(-10.0)]);
        match result {
            ParsedIpadMessage::ParameterUpdate(addr, val) => {
                assert_eq!(addr.channel, ChannelId::Input(1));
                assert_eq!(addr.parameter, ParameterPath::Fader);
                assert_eq!(val, ParameterValue::Float(-10.0));
            }
            _ => panic!("Expected ParameterUpdate, got {result:?}"),
        }
    }

    #[test]
    fn parse_pan_with_conversion() {
        let result = parse_ipad_message("/Input_Channels/1/Panner/pan", &[OscType::Float(0.5)]);
        match result {
            ParsedIpadMessage::ParameterUpdate(addr, val) => {
                assert_eq!(addr.parameter, ParameterPath::Pan);
                // iPad 0.5 → internal 0.0
                match val {
                    ParameterValue::Float(f) => assert!((f - 0.0).abs() < 1e-6),
                    _ => panic!("Expected float"),
                }
            }
            _ => panic!("Expected ParameterUpdate"),
        }
    }

    #[test]
    fn parse_ipad_only_insert() {
        let result = parse_ipad_message("/Input_Channels/5/Insert/insert_A_in", &[OscType::Int(1)]);
        match result {
            ParsedIpadMessage::ParameterUpdate(addr, val) => {
                assert_eq!(addr.channel, ChannelId::Input(5));
                assert_eq!(addr.parameter, ParameterPath::InsertAEnabled);
                assert_eq!(val, ParameterValue::Bool(true));
            }
            _ => panic!("Expected ParameterUpdate"),
        }
    }

    #[test]
    fn parse_graphic_eq() {
        let result = parse_ipad_message("/Graphic_EQ/3/geq_gain_12", &[OscType::Float(6.0)]);
        match result {
            ParsedIpadMessage::ParameterUpdate(addr, val) => {
                assert_eq!(addr.channel, ChannelId::GraphicEq(3));
                assert_eq!(addr.parameter, ParameterPath::GeqBandGain(12));
                assert_eq!(val, ParameterValue::Float(6.0));
            }
            _ => panic!("Expected ParameterUpdate"),
        }
    }

    #[test]
    fn parse_control_group_0_based() {
        let result = parse_ipad_message("/Control_Groups/0/fader", &[OscType::Float(-5.0)]);
        match result {
            ParsedIpadMessage::ParameterUpdate(addr, _) => {
                assert_eq!(addr.channel, ChannelId::ControlGroup(1)); // 0→1
            }
            _ => panic!("Expected ParameterUpdate"),
        }
    }

    #[test]
    fn parse_config_console_name() {
        let result =
            parse_ipad_message("/Console/Name", &[OscType::String("S21 S21-210385".into())]);
        match result {
            ParsedIpadMessage::ConfigResponse(IpadConfigMessage::ConsoleName { name, serial }) => {
                assert_eq!(name, "S21");
                assert_eq!(serial, "S21-210385");
            }
            _ => panic!("Expected ConsoleName, got {result:?}"),
        }
    }

    #[test]
    fn parse_config_channel_count() {
        let result = parse_ipad_message("/Console/Input_Channels", &[OscType::Int(48)]);
        match result {
            ParsedIpadMessage::ConfigResponse(IpadConfigMessage::ChannelCount {
                channel_type,
                count,
            }) => {
                assert_eq!(channel_type, "Input_Channels");
                assert_eq!(count, 48);
            }
            _ => panic!("Expected ChannelCount, got {result:?}"),
        }
    }

    #[test]
    fn parse_config_output_modes() {
        let result = parse_ipad_message(
            "/Console/Aux_Outputs/modes",
            &[OscType::Int(1), OscType::Int(2), OscType::Int(1)],
        );
        match result {
            ParsedIpadMessage::ConfigResponse(IpadConfigMessage::OutputModes {
                channel_type,
                modes,
            }) => {
                assert_eq!(channel_type, "Aux_Outputs");
                assert_eq!(modes.len(), 3);
                assert_eq!(modes[0], ChannelMode::Mono);
                assert_eq!(modes[1], ChannelMode::Stereo);
            }
            _ => panic!("Expected OutputModes, got {result:?}"),
        }
    }

    #[test]
    fn parse_snapshot_info() {
        let result = parse_ipad_message("/Snapshots/Current_Snapshot", &[OscType::Int(5)]);
        match result {
            ParsedIpadMessage::SnapshotInfo { current } => {
                assert_eq!(current, 5);
            }
            _ => panic!("Expected SnapshotInfo"),
        }
    }

    #[test]
    fn parse_unknown() {
        let result = parse_ipad_message("/some/unknown/path", &[]);
        assert!(matches!(result, ParsedIpadMessage::Unknown(_)));
    }

    #[test]
    fn parse_eq_band_param() {
        // The iPad numbers EQ bands in reverse of the internal model:
        // iPad wire band 2 (Hi Mid) maps to internal band 5-2 = 3.
        let result = parse_ipad_message("/Input_Channels/3/EQ/eq_gain_2", &[OscType::Float(3.5)]);
        match result {
            ParsedIpadMessage::ParameterUpdate(addr, val) => {
                assert_eq!(addr.channel, ChannelId::Input(3));
                assert_eq!(addr.parameter, ParameterPath::EqBandGain(3));
                assert_eq!(val, ParameterValue::Float(3.5));
            }
            _ => panic!("Expected ParameterUpdate"),
        }
    }

    /// The iPad reverses EQ band numbering (internal b ↔ iPad wire 5-b). Verify
    /// every EQ band parameter parses to the reversed internal band, and that
    /// an out-of-range wire band is rejected.
    #[test]
    fn parse_eq_bands_are_reversed() {
        // iPad wire 1 → internal 4, wire 4 → internal 1, wire 2 → 3, wire 3 → 2.
        for (wire, internal) in [(1u8, 4u8), (2, 3), (3, 2), (4, 1)] {
            let path = format!("/Input_Channels/7/EQ/eq_gain_{wire}");
            match parse_ipad_message(&path, &[OscType::Float(0.0)]) {
                ParsedIpadMessage::ParameterUpdate(addr, _) => {
                    assert_eq!(
                        addr.parameter,
                        ParameterPath::EqBandGain(internal),
                        "iPad wire band {wire} should map to internal band {internal}"
                    );
                }
                _ => panic!("Expected ParameterUpdate for {path}"),
            }
        }
        // Out-of-range wire band (5) has no internal equivalent → Unknown.
        assert!(matches!(
            parse_ipad_message("/Input_Channels/7/EQ/eq_gain_5", &[OscType::Float(0.0)]),
            ParsedIpadMessage::Unknown(_)
        ));
    }

    /// The iPad multiband compressor swaps Mid/High: iPad band 2 → internal 3,
    /// band 3 → internal 2, band 1 → internal 1 (verified against the live desk).
    #[test]
    fn parse_multiband_bands_are_swapped() {
        for (wire, internal) in [(1u8, 1u8), (2, 3), (3, 2)] {
            let path = format!("/Input_Channels/21/Dynamics/comp_thresh_{wire}");
            match parse_ipad_message(&path, &[OscType::Float(-30.0)]) {
                ParsedIpadMessage::ParameterUpdate(addr, _) => assert_eq!(
                    addr.parameter,
                    ParameterPath::Dyn1Threshold(internal),
                    "iPad comp band {wire} should map to internal band {internal}"
                ),
                _ => panic!("expected ParameterUpdate for {path}"),
            }
        }
        // Band 1 (Low) also arrives via the bare path.
        match parse_ipad_message(
            "/Input_Channels/21/Dynamics/comp_thresh",
            &[OscType::Float(-30.0)],
        ) {
            ParsedIpadMessage::ParameterUpdate(addr, _) => {
                assert_eq!(addr.parameter, ParameterPath::Dyn1Threshold(1))
            }
            _ => panic!("expected ParameterUpdate for bare comp_thresh"),
        }
        // Out-of-range band has no internal equivalent → Unknown.
        assert!(matches!(
            parse_ipad_message(
                "/Input_Channels/21/Dynamics/comp_thresh_4",
                &[OscType::Float(0.0)]
            ),
            ParsedIpadMessage::Unknown(_)
        ));
    }

    #[test]
    fn parse_aux_send() {
        let result = parse_ipad_message(
            "/Input_Channels/1/Aux_Send/3/send_level",
            &[OscType::Float(-12.0)],
        );
        match result {
            ParsedIpadMessage::ParameterUpdate(addr, val) => {
                assert_eq!(addr.channel, ChannelId::Input(1));
                assert_eq!(addr.parameter, ParameterPath::SendLevel(3));
                assert_eq!(val, ParameterValue::Float(-12.0));
            }
            _ => panic!("Expected ParameterUpdate"),
        }
    }

    // ─── Audit M6: property-based fuzz coverage ────────────────────────
    use proptest::prelude::*;

    /// Arbitrary `OscType` strategy covering the variants the parser
    /// actually inspects. Skips `Blob` / `Time` / `MidiMessage` etc. — those
    /// are unreachable from the iPad protocol and add noise without
    /// coverage.
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
        /// Fully random path + small random arg list. Most paths fall
        /// through to `Unknown(_)`; the rest exercise the channel/path
        /// dispatch and the value-extraction switch.
        #[test]
        fn parse_ipad_message_no_panic_on_arbitrary(
            path in ".*",
            args in proptest::collection::vec(arb_osc_type(), 0..8)
        ) {
            let _ = parse_ipad_message(&path, &args);
        }

        /// Bias toward channel-shaped paths (`/{ChannelType}/{n}/...`) so
        /// the deeper code path gets exercised more often than fully-random
        /// strings.
        #[test]
        fn parse_ipad_message_no_panic_on_channel_paths(
            channel_type in "(Input_Channels|Aux_Outputs|Group_Outputs|Matrix_Outputs|Control_Groups|Graphic_EQ|Matrix_Inputs)",
            num in any::<u8>(),
            suffix in ".*",
            args in proptest::collection::vec(arb_osc_type(), 0..4)
        ) {
            let path = format!("/{channel_type}/{num}/{suffix}");
            let _ = parse_ipad_message(&path, &args);
        }
    }
}

/// End-to-end wire round-trip across **every** quirk combination, on **every**
/// surface that uses the shared parameter tree.
///
/// The golden tests elsewhere pin the S21 byte strings; this proves the
/// encoder and parser stay mutual inverses for any family's quirks and either
/// surface — which is what protects a console whose hardware probe disproves
/// one of the SD/Quantum hypotheses, and what lets the `/sd/` surface (prefix
/// plus normalized levels) share one codec with the Pad surface.
#[cfg(test)]
mod quirk_round_trip_tests {
    use super::*;
    use crate::model::family::{BoolWire, ConsoleSurface, PadQuirks};
    use crate::osc::ipad_encode::encode_pad_parameter;

    /// The surfaces that address parameters through the shared TitleCase tree.
    fn tree_surfaces() -> [ConsoleSurface; 2] {
        [ConsoleSurface::Pad, ConsoleSurface::SdOther]
    }

    /// All 48 combinations of the five quirk flags.
    fn all_quirk_combos() -> Vec<PadQuirks> {
        let mut out = Vec::new();
        for eq_bands_reversed in [true, false] {
            for dyn1_mid_high_swapped in [true, false] {
                for bool_wire in [BoolWire::Float01, BoolWire::OscBool, BoolWire::Int01] {
                    for pan_wire in [PanWire::ZeroToOne, PanWire::SignedUnit] {
                        for control_groups_zero_based in [true, false] {
                            out.push(PadQuirks {
                                eq_bands_reversed,
                                dyn1_mid_high_swapped,
                                bool_wire,
                                pan_wire,
                                control_groups_zero_based,
                            });
                        }
                    }
                }
            }
        }
        out
    }

    fn config_with(q: PadQuirks) -> crate::model::config::ConsoleConfig {
        crate::model::config::ConsoleConfig {
            // Generous counts so no fixture address trips the bounds check.
            input_channel_count: 64,
            aux_output_count: 16,
            group_output_count: 16,
            matrix_output_count: 16,
            matrix_input_count: 16,
            control_group_count: 16,
            graphic_eq_count: 16,
            pad_quirk_overrides: Some(q),
            ..Default::default()
        }
    }

    /// Representative addresses spanning every quirk-sensitive dimension:
    /// EQ bands, multiband dynamics, Control Groups, pan, and booleans.
    fn fixtures() -> Vec<(ParameterAddress, ParameterValue)> {
        let mut v: Vec<(ParameterAddress, ParameterValue)> = Vec::new();
        let input = |p: ParameterPath| ParameterAddress {
            channel: ChannelId::Input(3),
            parameter: p,
        };
        for b in 1..=4u8 {
            v.push((
                input(ParameterPath::EqBandGain(b)),
                ParameterValue::Float(-4.5),
            ));
            v.push((
                input(ParameterPath::EqBandFrequency(b)),
                ParameterValue::Float(1000.0),
            ));
        }
        for b in 1..=3u8 {
            v.push((
                input(ParameterPath::Dyn1Threshold(b)),
                ParameterValue::Float(-18.0),
            ));
            v.push((
                input(ParameterPath::Dyn1Listen(b)),
                ParameterValue::Bool(true),
            ));
        }
        v.push((input(ParameterPath::Fader), ParameterValue::Float(-10.0)));
        v.push((input(ParameterPath::Mute), ParameterValue::Bool(true)));
        v.push((input(ParameterPath::Mute), ParameterValue::Bool(false)));
        v.push((input(ParameterPath::Pan), ParameterValue::Float(-1.0)));
        v.push((input(ParameterPath::Pan), ParameterValue::Float(0.0)));
        v.push((input(ParameterPath::Pan), ParameterValue::Float(0.75)));
        v.push((
            input(ParameterPath::SendPan(2)),
            ParameterValue::Float(-0.5),
        ));
        v.push((
            input(ParameterPath::SendLevel(2)),
            ParameterValue::Float(-6.0),
        ));
        v.push((
            input(ParameterPath::SendEnabled(2)),
            ParameterValue::Bool(true),
        ));
        // Control Groups exercise the CG numbering quirk.
        v.push((
            ParameterAddress {
                channel: ChannelId::ControlGroup(1),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(-3.0),
        ));
        v.push((
            ParameterAddress {
                channel: ChannelId::ControlGroup(4),
                parameter: ParameterPath::Mute,
            },
            ParameterValue::Bool(true),
        ));
        v
    }

    #[test]
    fn encode_then_parse_round_trips_under_every_quirk_combo_and_surface() {
        for surface in tree_surfaces() {
            for q in all_quirk_combos() {
                let cfg = config_with(q);
                let wire = PadWire::new(surface, q);
                for (addr, value) in fixtures() {
                    let (path, args) =
                        encode_pad_parameter(&addr, &value, &wire).unwrap_or_else(|| {
                            panic!("encode returned None for {addr} on {surface:?} under {q:?}")
                        });
                    assert!(
                        path.starts_with(surface.path_prefix()),
                        "{path} is missing the {surface:?} prefix"
                    );
                    // Parse with the same explicit surface: `config_with`
                    // stores quirks, but the surface is a property of the
                    // connection, not the show file.
                    let parsed = parse_pad_message(&path, &args, Some(&cfg), &wire);
                    match parsed {
                        ParsedIpadMessage::ParameterUpdate(got_addr, got_value) => {
                            assert_eq!(
                                got_addr, addr,
                                "address round-trip failed for {path} on {surface:?} under {q:?}"
                            );
                            match (&value, &got_value) {
                                (ParameterValue::Float(a), ParameterValue::Float(b)) => {
                                    // Normalized levels quantize through a
                                    // piecewise table, so allow a wider band
                                    // for those than for a straight dB copy.
                                    let tol = if addr.parameter.is_fader_level()
                                        && surface.level_wire()
                                            == crate::model::family::LevelWire::Normalized01
                                    {
                                        0.05
                                    } else {
                                        1e-5
                                    };
                                    assert!(
                                        (a - b).abs() < tol,
                                        "value round-trip failed for {path} on {surface:?} \
                                         under {q:?}: {a} != {b}"
                                    );
                                }
                                (a, b) => assert_eq!(
                                    a, b,
                                    "value round-trip failed for {path} on {surface:?} under {q:?}"
                                ),
                            }
                        }
                        other => panic!(
                            "expected ParameterUpdate for {path} on {surface:?} \
                             under {q:?}, got {other:?}"
                        ),
                    }
                }
            }
        }
    }

    #[test]
    fn a_prefixed_surface_rejects_an_unprefixed_address() {
        // Guards the strip step: if a `/sd/` connection ever receives a bare
        // Pad-tree address it is not ours to interpret, and silently accepting
        // it would let two surfaces' traffic blur together in the mirror.
        let q = PadQuirks::SD_HYPOTHESIS;
        let cfg = config_with(q);
        let wire = PadWire::new(ConsoleSurface::SdOther, q);
        let parsed = parse_pad_message(
            "/Input_Channels/1/fader",
            &[OscType::Float(0.5)],
            Some(&cfg),
            &wire,
        );
        assert!(
            matches!(parsed, ParsedIpadMessage::Unknown(_)),
            "an unprefixed address must not parse on the /sd/ surface"
        );
    }
}
