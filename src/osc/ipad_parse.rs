use rosc::OscType;

use crate::model::channel::ChannelId;
use crate::model::config::ChannelMode;
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
use super::ipad_values;

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
    ConsoleName { name: String, serial: String },
    SessionFilename(Option<String>),
    ChannelCount { channel_type: String, count: u8 },
    OutputModes { channel_type: String, modes: Vec<ChannelMode> },
    OutputTypes { types: Vec<bool> },
}

/// Layout bank data from `/Layout/Layout/Banks`.
#[derive(Debug, Clone)]
pub struct BankData {
    pub side: String,      // "Left" or "Right"
    pub bank_number: u8,
    pub label: String,
    pub channels: Vec<Option<ChannelId>>, // 10 slots, None for empty
}

/// Parse an iPad protocol OSC message.
pub fn parse_ipad_message(path: &str, args: &[OscType]) -> ParsedIpadMessage {
    // Snapshot info
    if path == "/Snapshots/Current_Snapshot" {
        if let Some(n) = args.first().and_then(extract_i32) {
            return ParsedIpadMessage::SnapshotInfo { current: n };
        }
    }

    // Console configuration responses
    if let Some(config) = try_parse_config(path, args) {
        return ParsedIpadMessage::ConfigResponse(config);
    }

    // Layout banks
    if path == "/Layout/Layout/Banks" {
        if let Some(bank) = try_parse_layout_bank(args) {
            return ParsedIpadMessage::LayoutBank(bank);
        }
    }

    // Meter values
    if path == "/Meters/values" {
        return parse_meter_values(args);
    }

    // Channel parameter: /{ChannelType}/{number}/{suffix}
    if let Some((channel, suffix)) = ChannelId::from_ipad_path(path) {
        if let Some(parameter) = ParameterPath::from_ipad_suffix(suffix) {
            if let Some(value) = extract_value(&parameter, args) {
                let addr = ParameterAddress { channel, parameter };
                return ParsedIpadMessage::ParameterUpdate(addr, value);
            }
        }
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
            let count = args.first().and_then(extract_u8)?;
            Some(IpadConfigMessage::ChannelCount {
                channel_type: channel_type.to_string(),
                count,
            })
        }
        p if p.ends_with("/modes") => {
            // Output modes: /Console/{Type}/modes  INT INT INT ...
            let channel_type = p
                .strip_prefix("/Console/")?
                .strip_suffix("/modes")?;
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

fn try_parse_layout_bank(args: &[OscType]) -> Option<BankData> {
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
                if let Some(num) = extract_u8(&args[i + 1]) {
                    let channel = match ch_type.as_str() {
                        "Input_Channels" => Some(ChannelId::Input(num)),
                        "Aux_Outputs" => Some(ChannelId::Aux(num)),
                        "Group_Outputs" => Some(ChannelId::Group(num)),
                        "Matrix_Outputs" => Some(ChannelId::Matrix(num)),
                        "Control_Groups" => Some(ChannelId::ControlGroup(num + 1)), // 0-based
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

/// Extract a typed value from OSC args, applying pan conversion for iPad protocol.
fn extract_value(parameter: &ParameterPath, args: &[OscType]) -> Option<ParameterValue> {
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

    // Apply pan conversion for iPad → internal
    match parameter {
        ParameterPath::Pan | ParameterPath::SendPan(_) => {
            if let ParameterValue::Float(f) = &value {
                Some(ParameterValue::Float(ipad_values::ipad_pan_to_internal(*f)))
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
        let result = parse_ipad_message(
            "/Input_Channels/1/fader",
            &[OscType::Float(-10.0)],
        );
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
        let result = parse_ipad_message(
            "/Input_Channels/1/Panner/pan",
            &[OscType::Float(0.5)],
        );
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
        let result = parse_ipad_message(
            "/Input_Channels/5/Insert/insert_A_in",
            &[OscType::Int(1)],
        );
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
        let result = parse_ipad_message(
            "/Graphic_EQ/3/geq_gain_12",
            &[OscType::Float(6.0)],
        );
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
        let result = parse_ipad_message(
            "/Control_Groups/0/fader",
            &[OscType::Float(-5.0)],
        );
        match result {
            ParsedIpadMessage::ParameterUpdate(addr, _) => {
                assert_eq!(addr.channel, ChannelId::ControlGroup(1)); // 0→1
            }
            _ => panic!("Expected ParameterUpdate"),
        }
    }

    #[test]
    fn parse_config_console_name() {
        let result = parse_ipad_message(
            "/Console/Name",
            &[OscType::String("S21 S21-210385".into())],
        );
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
        let result = parse_ipad_message(
            "/Console/Input_Channels",
            &[OscType::Int(48)],
        );
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
        let result = parse_ipad_message(
            "/Snapshots/Current_Snapshot",
            &[OscType::Int(5)],
        );
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
        let result = parse_ipad_message(
            "/Input_Channels/3/EQ/eq_gain_2",
            &[OscType::Float(3.5)],
        );
        match result {
            ParsedIpadMessage::ParameterUpdate(addr, val) => {
                assert_eq!(addr.channel, ChannelId::Input(3));
                assert_eq!(addr.parameter, ParameterPath::EqBandGain(2));
                assert_eq!(val, ParameterValue::Float(3.5));
            }
            _ => panic!("Expected ParameterUpdate"),
        }
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
}
