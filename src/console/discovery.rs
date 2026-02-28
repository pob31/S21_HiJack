use rosc::OscType;

use crate::model::config::{ChannelMode, ConsoleConfig};

/// Map a channel type name (from /console/channel/counts/{type}) to a config update.
/// Returns true if the type was recognized and applied.
pub fn apply_channel_count(config: &mut ConsoleConfig, channel_type: &str, count: u8) -> bool {
    match channel_type {
        "input" => config.input_channel_count = count,
        "aux" => config.aux_output_count = count,
        "group" => config.group_output_count = count,
        "matrix" => config.matrix_output_count = count,
        "matrix_input" => config.matrix_input_count = count,
        "control_group" => config.control_group_count = count,
        "graphic_eq" => config.graphic_eq_count = count,
        "talkback" => config.talkback_output_count = count,
        _ => return false,
    }
    true
}

/// Parse mode arrays from the console (e.g., aux output modes: 1 1 1 1 2 2 2 2).
/// Used for /Console/Aux_Outputs/modes, /Console/Input_Channels/modes, etc.
pub fn parse_mode_array(args: &[OscType]) -> Vec<ChannelMode> {
    args.iter()
        .filter_map(|arg| match arg {
            OscType::Int(v) => Some(ChannelMode::from_int(*v)),
            OscType::Float(v) => Some(ChannelMode::from_int(*v as i32)),
            _ => None,
        })
        .collect()
}

/// Parse type arrays from the console (e.g., aux output types: 1 1 1 1 0 0 0 0).
/// 1 = aux, 0 = group/bus.
pub fn parse_type_array(args: &[OscType]) -> Vec<bool> {
    args.iter()
        .filter_map(|arg| match arg {
            OscType::Int(v) => Some(*v == 1),
            OscType::Float(v) => Some(*v as i32 == 1),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_known_types() {
        let mut config = ConsoleConfig::default();
        assert!(apply_channel_count(&mut config, "input", 60));
        assert_eq!(config.input_channel_count, 60);
        assert!(apply_channel_count(&mut config, "aux", 12));
        assert_eq!(config.aux_output_count, 12);
        assert!(apply_channel_count(&mut config, "control_group", 10));
        assert_eq!(config.control_group_count, 10);
    }

    #[test]
    fn reject_unknown_type() {
        let mut config = ConsoleConfig::default();
        assert!(!apply_channel_count(&mut config, "foobar", 5));
    }
}
