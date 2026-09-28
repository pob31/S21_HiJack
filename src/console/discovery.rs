use rosc::OscType;

use crate::model::config::{ChannelMode, ConsoleConfig, PlusMode};

/// Apply a positional `/console/channel/counts` reply to the config in one shot.
///
/// Wire form (operator-confirmed on real S21+, 2026-04-09):
/// `/console/channel/counts <inputs> <aux> <groups> <control_groups> <matrices> <master>`
///
/// Note: the `master` slot is acknowledged but not stored — the console always has
/// exactly one master and the config has no field for it.
pub fn apply_channel_counts(
    config: &mut ConsoleConfig,
    inputs: u16,
    aux: u16,
    groups: u16,
    control_groups: u16,
    matrices: u16,
    _master: u16,
) {
    config.input_channel_count = inputs;
    config.aux_output_count = aux;
    config.group_output_count = groups;
    config.control_group_count = control_groups;
    config.matrix_output_count = matrices;
    config.plus_mode = PlusMode::from_input_count(inputs);

    // Generate the default layout (first `aux` buses are aux, the rest group)
    // unless the existing one agrees with these counts. The iPad handshake
    // reports the real, possibly interleaved, split, so a layout with the
    // right number of auxes is kept. One from an old show file or from before
    // the desk was reconfigured used to be kept too, numbering every bus
    // write wrongly for the whole session (audit H1).
    let known_auxes = config.mix_output_types.iter().filter(|&&t| t).count();
    let known_groups = config.mix_output_types.len() - known_auxes;
    if known_auxes != aux as usize || known_groups < groups as usize {
        config.mix_output_types = std::iter::repeat_n(true, aux as usize)
            .chain(std::iter::repeat_n(false, groups as usize))
            .collect();
    }
}

/// Map a channel type name (from /console/channel/counts/{type}) to a config update.
/// Returns true if the type was recognized and applied.
pub fn apply_channel_count(config: &mut ConsoleConfig, channel_type: &str, count: u16) -> bool {
    match channel_type {
        "input" => {
            config.input_channel_count = count;
            config.plus_mode = PlusMode::from_input_count(count);
        }
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

    /// A layout restored from a show made on an 8-aux desk must not survive
    /// counts saying the desk now has 10 auxes (audit H1).
    #[test]
    fn channel_counts_replace_a_stale_bus_layout() {
        let mut config = ConsoleConfig {
            mix_output_types: (0..24).map(|i| i < 8).collect(),
            ..ConsoleConfig::default()
        };
        apply_channel_counts(&mut config, 48, 10, 14, 10, 8, 1);
        let expected: Vec<bool> = (0..24).map(|i| i < 10).collect();
        assert_eq!(config.mix_output_types, expected);
    }

    /// An interleaved layout from the iPad handshake that agrees with the
    /// counts is the desk's real split and is kept.
    #[test]
    fn channel_counts_keep_a_matching_bus_layout() {
        let interleaved = vec![true, false, true, false, true, false];
        let mut config = ConsoleConfig {
            mix_output_types: interleaved.clone(),
            ..ConsoleConfig::default()
        };
        apply_channel_counts(&mut config, 48, 3, 3, 10, 8, 1);
        assert_eq!(config.mix_output_types, interleaved);
    }
}
