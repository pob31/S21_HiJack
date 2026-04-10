use serde::{Deserialize, Serialize};

/// Channel stereo mode.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelMode {
    Mono,   // 1
    Stereo, // 2
}

impl ChannelMode {
    pub fn from_int(v: i32) -> Self {
        match v {
            2 => ChannelMode::Stereo,
            _ => ChannelMode::Mono,
        }
    }

    pub fn to_int(&self) -> i32 {
        match self {
            ChannelMode::Mono => 1,
            ChannelMode::Stereo => 2,
        }
    }
}

/// Console configuration discovered at startup.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConsoleConfig {
    pub console_name: String,
    pub console_serial: String,
    pub session_filename: Option<String>,

    pub input_channel_count: u8,
    pub aux_output_count: u8,     // depends on aux/group split
    pub group_output_count: u8,
    pub matrix_output_count: u8,  // 8
    pub matrix_input_count: u8,   // 10
    pub control_group_count: u8,  // 10
    pub graphic_eq_count: u8,     // 16
    pub talkback_output_count: u8,

    /// Per mix output: true = aux, false = group/bus
    pub mix_output_types: Vec<bool>,
    /// Per mix output: Mono or Stereo
    pub mix_output_modes: Vec<ChannelMode>,
    /// Per input: Mono or Stereo
    pub input_modes: Vec<ChannelMode>,
    /// Per group: Mono or Stereo
    pub group_modes: Vec<ChannelMode>,
}

impl ConsoleConfig {
    /// Total number of mix output buses (aux + group). Each bus is reachable
    /// via the same `/channel/{ch}/send/{bus}/*` GP OSC path family —
    /// `mix_output_types[bus-1]` decides whether bus N is currently configured
    /// as an aux (`true`) or a group (`false`). The split is dynamic: the
    /// operator can change a bus's type on the console at any time.
    pub fn total_bus_count(&self) -> u8 {
        self.aux_output_count + self.group_output_count
    }

    /// Display label for a 1-based bus index, derived from the live console
    /// config. Examples: "Aux 3", "Group 5 (Stereo)", "Bus 14" (fallback when
    /// the bus type isn't yet known). Used by the scope editor to label the
    /// `SendEnabled/SendLevel/SendPan` rows so the operator sees the current
    /// aux-vs-group assignment.
    pub fn bus_label(&self, bus_index_1based: u8) -> String {
        if bus_index_1based == 0 || bus_index_1based > self.total_bus_count() {
            return format!("Bus {bus_index_1based}");
        }
        let idx0 = (bus_index_1based - 1) as usize;
        let kind = self
            .mix_output_types
            .get(idx0)
            .map(|is_aux| if *is_aux { "Aux" } else { "Group" });
        let mode_suffix = match self.mix_output_modes.get(idx0) {
            Some(ChannelMode::Stereo) => " (Stereo)",
            Some(ChannelMode::Mono) => "",
            None => "",
        };
        // Per-type 1-based index: walk the prefix counting buses of the same kind.
        match kind {
            Some(kind_str) => {
                let prefix_count = self
                    .mix_output_types
                    .iter()
                    .take(idx0)
                    .filter(|is_aux| {
                        if kind_str == "Aux" {
                            **is_aux
                        } else {
                            !**is_aux
                        }
                    })
                    .count()
                    + 1;
                format!("{kind_str} {prefix_count}{mode_suffix}")
            }
            // No mix_output_types data yet (pre-discovery): just call it a Bus.
            None => format!("Bus {bus_index_1based}{mode_suffix}"),
        }
    }

}

impl Default for ConsoleConfig {
    fn default() -> Self {
        Self {
            console_name: String::new(),
            console_serial: String::new(),
            session_filename: None,
            input_channel_count: 48,
            aux_output_count: 8,
            group_output_count: 8,
            matrix_output_count: 8,
            matrix_input_count: 10,
            control_group_count: 10,
            graphic_eq_count: 16,
            talkback_output_count: 0,
            mix_output_types: Vec::new(),
            mix_output_modes: Vec::new(),
            input_modes: Vec::new(),
            group_modes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_buses(types: Vec<bool>, modes: Vec<ChannelMode>) -> ConsoleConfig {
        let aux_count = types.iter().filter(|t| **t).count() as u8;
        let group_count = types.iter().filter(|t| !**t).count() as u8;
        ConsoleConfig {
            aux_output_count: aux_count,
            group_output_count: group_count,
            mix_output_types: types,
            mix_output_modes: modes,
            ..ConsoleConfig::default()
        }
    }

    #[test]
    fn total_bus_count_sums_aux_and_group() {
        let mut config = ConsoleConfig::default();
        config.aux_output_count = 8;
        config.group_output_count = 8;
        assert_eq!(config.total_bus_count(), 16);
    }

    #[test]
    fn bus_label_uses_per_type_index() {
        // Buses 1, 2 are auxes; bus 3 is a group; bus 4 is aux; bus 5 is group.
        // Per-type 1-based numbering: Aux 1 = bus 1, Aux 2 = bus 2, Aux 3 = bus 4,
        // Group 1 = bus 3, Group 2 = bus 5.
        let config = config_with_buses(
            vec![true, true, false, true, false],
            vec![],
        );
        assert_eq!(config.bus_label(1), "Aux 1");
        assert_eq!(config.bus_label(2), "Aux 2");
        assert_eq!(config.bus_label(3), "Group 1");
        assert_eq!(config.bus_label(4), "Aux 3");
        assert_eq!(config.bus_label(5), "Group 2");
    }

    #[test]
    fn bus_label_marks_stereo() {
        let config = config_with_buses(
            vec![true, false],
            vec![ChannelMode::Mono, ChannelMode::Stereo],
        );
        assert_eq!(config.bus_label(1), "Aux 1");
        assert_eq!(config.bus_label(2), "Group 1 (Stereo)");
    }

    #[test]
    fn bus_label_falls_back_when_no_type_data() {
        // mix_output_types empty (pre-discovery): should still produce a sensible label.
        let mut config = ConsoleConfig::default();
        config.aux_output_count = 8;
        config.group_output_count = 8;
        config.mix_output_types = Vec::new();
        config.mix_output_modes = Vec::new();
        assert_eq!(config.bus_label(1), "Bus 1");
        assert_eq!(config.bus_label(16), "Bus 16");
    }

    #[test]
    fn bus_label_out_of_range_falls_back() {
        let config = config_with_buses(vec![true, true], vec![]);
        assert_eq!(config.bus_label(0), "Bus 0");
        assert_eq!(config.bus_label(99), "Bus 99");
    }
}
