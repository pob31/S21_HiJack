use serde::{Deserialize, Serialize};

use crate::model::family::{ConsoleFamily, ConsoleProfile, PadQuirks};

/// Console hardware variant. Encodes the input count (48 vs 60) and the
/// total mix output bus count (16 vs 24). The aux-vs-group split inside
/// those buses is operator-configurable on the console surface and is
/// not controllable over OSC, so it rides along separately in
/// `mix_output_types`.
///
/// Set automatically by the discovery path from the reported input
/// channel count — never user-settable. Persisted in the show file so
/// reloading offline restores the right channel layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlusMode {
    /// 48 input channels, 16 mix output buses.
    #[default]
    S21,
    /// 60 input channels, 24 mix output buses.
    S21Plus,
}

impl PlusMode {
    /// Derive from a discovered input channel count. 60 or more → Plus,
    /// anything else → standard.
    pub fn from_input_count(inputs: u16) -> Self {
        if inputs >= 60 {
            Self::S21Plus
        } else {
            Self::S21
        }
    }

    pub fn input_count(&self) -> u16 {
        match self {
            Self::S21 => 48,
            Self::S21Plus => 60,
        }
    }

    pub fn total_mix_buses(&self) -> u16 {
        match self {
            Self::S21 => 16,
            Self::S21Plus => 24,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::S21 => "S21",
            Self::S21Plus => "S21+",
        }
    }
}

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

    // Counts are `u16`: sized for the largest console family (a Quantum7
    // reports 256 inputs), not the S series. JSON show files carry plain
    // numbers, so widening is serde-compatible in both directions.
    pub input_channel_count: u16,
    pub aux_output_count: u16, // depends on aux/group split
    pub group_output_count: u16,
    pub matrix_output_count: u16, // 8
    pub matrix_input_count: u16,  // 10
    pub control_group_count: u16, // 10
    pub graphic_eq_count: u16,    // 16
    pub talkback_output_count: u16,

    /// Per mix output: true = aux, false = group/bus
    pub mix_output_types: Vec<bool>,
    /// Per mix output: Mono or Stereo
    pub mix_output_modes: Vec<ChannelMode>,
    /// Per input: Mono or Stereo
    pub input_modes: Vec<ChannelMode>,
    /// Per group: Mono or Stereo
    pub group_modes: Vec<ChannelMode>,
    /// Hardware variant (S21 or S21+). Auto-derived from the discovered
    /// input channel count and persisted in the show file. Defaults to
    /// `S21` for legacy show files that pre-date this field.
    #[serde(default)]
    pub plus_mode: PlusMode,
    /// Which console range this show targets. Selects the wire dialect and
    /// lifecycle via [`ConsoleConfig::profile`]. Defaults to
    /// [`ConsoleFamily::SSeries`] so every pre-existing show is unchanged.
    #[serde(default)]
    pub family: ConsoleFamily,
    /// Operator corrections to the family's stock wire quirks, applied on top
    /// of [`ConsoleProfile::for_family`]. Set when a hardware probe disproves
    /// one of the SD/Quantum hypotheses; `None` uses the family defaults.
    #[serde(default)]
    pub pad_quirk_overrides: Option<PadQuirks>,
}

impl ConsoleConfig {
    /// The console profile this show targets: family defaults with any
    /// operator quirk overrides applied. Cheap to build (all `Copy` data), so
    /// callers derive it on demand rather than caching a stale copy.
    pub fn profile(&self) -> ConsoleProfile {
        ConsoleProfile::for_family(self.family).with_overrides(self.pad_quirk_overrides)
    }

    /// Total number of mix output buses (aux + group). Each bus is reachable
    /// via the same `/channel/{ch}/send/{bus}/*` GP OSC path family —
    /// `mix_output_types[bus-1]` decides whether bus N is currently configured
    /// as an aux (`true`) or a group (`false`). The split is dynamic: the
    /// operator can change a bus's type on the console at any time.
    pub fn total_bus_count(&self) -> u16 {
        self.aux_output_count + self.group_output_count
    }

    /// Mode of the Nth aux bus (1-based). Walks `mix_output_types` looking for
    /// the Nth `true` entry and returns the corresponding `mix_output_modes`
    /// value. Returns `None` when the configuration hasn't been discovered yet
    /// (e.g. offline before a show file has loaded) or `aux_1based` is out of
    /// range.
    pub fn aux_mode(&self, aux_1based: u16) -> Option<ChannelMode> {
        if aux_1based == 0 {
            return None;
        }
        let mut count: u16 = 0;
        for (idx, is_aux) in self.mix_output_types.iter().enumerate() {
            if *is_aux {
                count += 1;
                if count == aux_1based {
                    return self.mix_output_modes.get(idx).cloned();
                }
            }
        }
        None
    }

    /// Display label for a 1-based bus index, derived from the live console
    /// config. Examples: "Aux 3", "Group 5 (Stereo)", "Bus 14" (fallback when
    /// the bus type isn't yet known). Used by the scope editor to label the
    /// `SendEnabled/SendLevel/SendPan` rows so the operator sees the current
    /// aux-vs-group assignment.
    pub fn bus_label(&self, bus_index_1based: u16) -> String {
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
            plus_mode: PlusMode::default(),
            family: ConsoleFamily::default(),
            pad_quirk_overrides: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_buses(types: Vec<bool>, modes: Vec<ChannelMode>) -> ConsoleConfig {
        let aux_count = types.iter().filter(|t| **t).count() as u16;
        let group_count = types.iter().filter(|t| !**t).count() as u16;
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
        let config = config_with_buses(vec![true, true, false, true, false], vec![]);
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
    fn aux_mode_walks_mix_output_types() {
        // Buses 1=aux mono, 2=group stereo, 3=aux stereo, 4=group mono, 5=aux mono.
        let config = config_with_buses(
            vec![true, false, true, false, true],
            vec![
                ChannelMode::Mono,
                ChannelMode::Stereo,
                ChannelMode::Stereo,
                ChannelMode::Mono,
                ChannelMode::Mono,
            ],
        );
        assert_eq!(config.aux_mode(1), Some(ChannelMode::Mono));
        assert_eq!(config.aux_mode(2), Some(ChannelMode::Stereo));
        assert_eq!(config.aux_mode(3), Some(ChannelMode::Mono));
        assert_eq!(config.aux_mode(4), None);
        assert_eq!(config.aux_mode(0), None);
    }

    #[test]
    fn aux_mode_returns_none_pre_discovery() {
        let config = ConsoleConfig::default();
        assert_eq!(config.aux_mode(1), None);
    }

    #[test]
    fn plus_mode_from_input_count() {
        assert_eq!(PlusMode::from_input_count(48), PlusMode::S21);
        assert_eq!(PlusMode::from_input_count(60), PlusMode::S21Plus);
        assert_eq!(PlusMode::from_input_count(72), PlusMode::S21Plus);
        assert_eq!(PlusMode::S21.input_count(), 48);
        assert_eq!(PlusMode::S21Plus.input_count(), 60);
        assert_eq!(PlusMode::S21.total_mix_buses(), 16);
        assert_eq!(PlusMode::S21Plus.total_mix_buses(), 24);
    }

    #[test]
    fn profile_derives_from_family() {
        let mut cfg = ConsoleConfig::default();
        assert_eq!(cfg.family, ConsoleFamily::SSeries);
        assert_eq!(cfg.profile().pad_quirks, PadQuirks::S21);
        assert!(cfg.profile().has_gp_osc);

        cfg.family = ConsoleFamily::Quantum;
        assert_eq!(cfg.profile().pad_quirks, PadQuirks::SD_HYPOTHESIS);
        assert!(!cfg.profile().has_gp_osc);
    }

    #[test]
    fn profile_applies_quirk_overrides() {
        let mut cfg = ConsoleConfig::default();
        cfg.family = ConsoleFamily::SdRange;
        let corrected = PadQuirks {
            eq_bands_reversed: true,
            ..PadQuirks::SD_HYPOTHESIS
        };
        cfg.pad_quirk_overrides = Some(corrected);
        assert_eq!(cfg.profile().pad_quirks, corrected);
        // Non-quirk profile fields still come from the family.
        assert!(!cfg.profile().has_gp_osc);
    }

    #[test]
    fn legacy_config_json_without_family_loads_as_s_series() {
        // A show written before Phase 1 has no `family` / `pad_quirk_overrides`.
        let json = r#"{
            "console_name": "S21",
            "console_serial": "S21-210385",
            "session_filename": null,
            "input_channel_count": 48,
            "aux_output_count": 8,
            "group_output_count": 8,
            "matrix_output_count": 8,
            "matrix_input_count": 10,
            "control_group_count": 10,
            "graphic_eq_count": 16,
            "talkback_output_count": 0,
            "mix_output_types": [],
            "mix_output_modes": [],
            "input_modes": [],
            "group_modes": []
        }"#;
        let cfg: ConsoleConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.family, ConsoleFamily::SSeries);
        assert!(cfg.pad_quirk_overrides.is_none());
        assert_eq!(cfg.profile().pad_quirks, PadQuirks::S21);
    }

    #[test]
    fn bus_label_out_of_range_falls_back() {
        let config = config_with_buses(vec![true, true], vec![]);
        assert_eq!(config.bus_label(0), "Bus 0");
        assert_eq!(config.bus_label(99), "Bus 99");
    }
}
