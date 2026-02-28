use rosc::OscType;

use crate::model::channel::ChannelId;
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
use super::ipad_values;

/// Encode a parameter address and value into an iPad protocol OSC path and args.
/// Returns None for parameters with no iPad representation (GP OSC-only).
///
/// Applies pan value conversion: internal -1..+1 → iPad 0..1.
pub fn encode_ipad_parameter(
    addr: &ParameterAddress,
    value: &ParameterValue,
) -> Option<(String, Vec<OscType>)> {
    let prefix = addr.channel.to_ipad_path_prefix();
    let suffix = addr.parameter.to_ipad_suffix()?;
    let path = format!("{prefix}/{suffix}");

    // Apply pan conversion for Pan and SendPan
    let effective_value = convert_value_for_ipad(&addr.parameter, value);
    let args = vec![value_to_osc_type(&effective_value)];
    Some((path, args))
}

/// Encode an iPad protocol query message (append /? to parameter path).
pub fn encode_ipad_query(
    channel: &ChannelId,
    parameter: &ParameterPath,
) -> Option<String> {
    let prefix = channel.to_ipad_path_prefix();
    let suffix = parameter.to_ipad_suffix()?;
    Some(format!("{prefix}/{suffix}/?"))
}

/// Convert internal value to iPad protocol value (e.g., pan conversion).
fn convert_value_for_ipad(parameter: &ParameterPath, value: &ParameterValue) -> ParameterValue {
    match parameter {
        ParameterPath::Pan | ParameterPath::SendPan(_) => {
            if let ParameterValue::Float(f) = value {
                ParameterValue::Float(ipad_values::internal_pan_to_ipad(*f))
            } else {
                value.clone()
            }
        }
        _ => value.clone(),
    }
}

fn value_to_osc_type(value: &ParameterValue) -> OscType {
    match value {
        ParameterValue::Float(f) => OscType::Float(*f),
        ParameterValue::Int(i) => OscType::Int(*i),
        ParameterValue::Bool(b) => OscType::Int(if *b { 1 } else { 0 }),
        ParameterValue::String(s) => OscType::String(s.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_fader() {
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        let (path, args) = encode_ipad_parameter(&addr, &ParameterValue::Float(-10.0)).unwrap();
        assert_eq!(path, "/Input_Channels/1/fader");
        assert_eq!(args, vec![OscType::Float(-10.0)]);
    }

    #[test]
    fn encode_ipad_only_param() {
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::InsertAEnabled,
        };
        let (path, args) = encode_ipad_parameter(&addr, &ParameterValue::Bool(true)).unwrap();
        assert_eq!(path, "/Input_Channels/1/Insert/insert_A_in");
        assert_eq!(args, vec![OscType::Int(1)]);
    }

    #[test]
    fn encode_graphic_eq() {
        let addr = ParameterAddress {
            channel: ChannelId::GraphicEq(3),
            parameter: ParameterPath::GeqBandGain(12),
        };
        let (path, args) = encode_ipad_parameter(&addr, &ParameterValue::Float(6.0)).unwrap();
        assert_eq!(path, "/Graphic_EQ/3/geq_gain_12");
        assert_eq!(args, vec![OscType::Float(6.0)]);
    }

    #[test]
    fn encode_pan_with_conversion() {
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Pan,
        };
        // Internal 0.0 (center) → iPad 0.5
        let (path, args) = encode_ipad_parameter(&addr, &ParameterValue::Float(0.0)).unwrap();
        assert_eq!(path, "/Input_Channels/1/Panner/pan");
        match &args[0] {
            OscType::Float(f) => assert!((*f - 0.5).abs() < 1e-6),
            _ => panic!("Expected float"),
        }
    }

    #[test]
    fn encode_send_pan_with_conversion() {
        let addr = ParameterAddress {
            channel: ChannelId::Input(5),
            parameter: ParameterPath::SendPan(3),
        };
        // Internal -1.0 (hard left) → iPad 0.0
        let (path, args) = encode_ipad_parameter(&addr, &ParameterValue::Float(-1.0)).unwrap();
        assert_eq!(path, "/Input_Channels/5/Aux_Send/3/send_pan");
        match &args[0] {
            OscType::Float(f) => assert!((*f - 0.0).abs() < 1e-6),
            _ => panic!("Expected float"),
        }
    }

    #[test]
    fn encode_gp_only_returns_none() {
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::DigitubeEnabled,
        };
        assert!(encode_ipad_parameter(&addr, &ParameterValue::Bool(true)).is_none());
    }

    #[test]
    fn encode_query() {
        let path = encode_ipad_query(&ChannelId::Input(1), &ParameterPath::Fader).unwrap();
        assert_eq!(path, "/Input_Channels/1/fader/?");

        let path = encode_ipad_query(&ChannelId::ControlGroup(1), &ParameterPath::Fader).unwrap();
        assert_eq!(path, "/Control_Groups/0/fader/?"); // CG 0-based

        assert!(encode_ipad_query(&ChannelId::Input(1), &ParameterPath::DigitubeEnabled).is_none());
    }

    #[test]
    fn encode_control_group_0_based() {
        let addr = ParameterAddress {
            channel: ChannelId::ControlGroup(1),
            parameter: ParameterPath::Fader,
        };
        let (path, _) = encode_ipad_parameter(&addr, &ParameterValue::Float(0.0)).unwrap();
        assert_eq!(path, "/Control_Groups/0/fader");
    }
}
