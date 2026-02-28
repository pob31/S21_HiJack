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
    /// Discovery response: channel count for a specific type.
    DiscoveryCount {
        channel_type: String,
        count: u8,
    },
    /// Unrecognized message.
    Unknown(String),
}

/// Parse a GP OSC message path and arguments into a typed result.
pub fn parse_gp_osc(path: &str, args: &[OscType]) -> ParsedOscMessage {
    // System commands
    match path {
        "/console/ping" => return ParsedOscMessage::Ping,
        "/console/pong" => return ParsedOscMessage::Pong,
        _ => {}
    }

    // Discovery responses: /console/channel/counts/{type} INT
    if let Some(type_name) = path.strip_prefix("/console/channel/counts/") {
        if let Some(count) = args.first().and_then(extract_u8) {
            return ParsedOscMessage::DiscoveryCount {
                channel_type: type_name.to_string(),
                count,
            };
        }
    }

    // Channel parameter: /channel/{ch}/...
    if let Some(rest) = path.strip_prefix("/channel/") {
        if let Some(parsed) = parse_channel_parameter(rest, args) {
            return ParsedOscMessage::ParameterUpdate(parsed.0, parsed.1);
        }
    }

    ParsedOscMessage::Unknown(path.to_string())
}

fn extract_u8(arg: &OscType) -> Option<u8> {
    match arg {
        OscType::Int(i) => u8::try_from(*i).ok(),
        OscType::Float(f) => u8::try_from(*f as i32).ok(),
        _ => None,
    }
}

/// Parse a channel parameter path like "{ch}/fader" with its args.
fn parse_channel_parameter(
    path: &str,
    args: &[OscType],
) -> Option<(ParameterAddress, ParameterValue)> {
    // Split into channel number and parameter suffix
    let slash = path.find('/')?;
    let ch_str = &path[..slash];
    let suffix = &path[slash + 1..];

    let ch_num: u8 = ch_str.parse().ok()?;
    let channel = ChannelId::from_gp_osc_number(ch_num)?;
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
        | ParameterPath::Polarity
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
        let result = parse_gp_osc("/channel/1/eq/2/frequency", &[OscType::Float(1000.0)]);
        match result {
            ParsedOscMessage::ParameterUpdate(addr, val) => {
                assert_eq!(addr.channel, ChannelId::Input(1));
                assert_eq!(addr.parameter, ParameterPath::EqBandFrequency(2));
                assert_eq!(val, ParameterValue::Float(1000.0));
            }
            _ => panic!("Expected ParameterUpdate"),
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
    fn parse_name() {
        let result = parse_gp_osc(
            "/channel/1/name",
            &[OscType::String("Kick".to_string())],
        );
        match result {
            ParsedOscMessage::ParameterUpdate(addr, val) => {
                assert_eq!(addr.parameter, ParameterPath::Name);
                assert_eq!(val, ParameterValue::String("Kick".to_string()));
            }
            _ => panic!("Expected ParameterUpdate"),
        }
    }
}
