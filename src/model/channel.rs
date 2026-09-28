use serde::{Deserialize, Serialize};
use std::fmt;

use crate::model::family::PadQuirks;

/// Logical channel identifier (protocol-agnostic).
/// All channel numbers are 1-based internally.
///
/// Numbers are `u16`: the S-series tops out well below 255, but the SD and
/// Quantum ranges go past it (a Quantum7 has 256 input channels), so the
/// internal model is sized for the largest console, not the first one.
/// The GP OSC *wire* number space stays `u8` (it only spans 1–127).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChannelId {
    Input(u16),        // 1–60 (S series)
    Aux(u16),          // 1–n (depends on aux/group split)
    Group(u16),        // 1–n
    Matrix(u16),       // 1–8
    ControlGroup(u16), // 1–10
    GraphicEq(u16),    // 1–16
    MatrixInput(u16),  // 1–10
}

/// GP OSC numbers 70–93 are one pool of 24 buses, split between auxes and
/// groups by the desk's configuration (`ConsoleConfig::mix_output_types`).
const GP_FIRST_BUS: usize = 70;
const GP_BUS_COUNT: usize = 24;
/// The split assumed when the desk's layout isn't known yet: 8 aux, 16 group.
const GP_DEFAULT_AUX_COUNT: usize = 8;

impl ChannelId {
    /// Convert to the GP OSC unified channel number, assuming the default
    /// 8-aux bus split. See [`Self::to_gp_osc_number_with_config`].
    pub fn to_gp_osc_number(&self) -> Option<u8> {
        self.to_gp_osc_number_with_config(None)
    }

    /// Convert to the GP OSC unified channel number: the exact inverse of
    /// [`Self::from_gp_osc_number_with_config`] for the same bus layout.
    ///
    /// Aux n and Group n are the n-th aux or group bus in `mix_output_types`.
    /// Always assuming 8 auxes (Aux n → 69+n, Group n → 77+n) sent writes to
    /// the wrong bus on any other split (audit H1).
    ///
    /// Returns None for channel types outside the GP OSC number space and for
    /// numbers outside their type's range, so an out-of-range channel can't
    /// land on another type's number ("CG 11" used to encode as Matrix 1).
    pub fn to_gp_osc_number_with_config(&self, mix_output_types: Option<&[bool]>) -> Option<u8> {
        // Every range here tops out at 127, so the narrowing can't truncate.
        let within = |n: u16, max: u16, base: u16| (1..=max).contains(&n).then(|| (base + n) as u8);
        match self {
            ChannelId::Input(n) => within(*n, 60, 0),
            ChannelId::Aux(n) => gp_bus_number(true, *n, mix_output_types),
            ChannelId::Group(n) => gp_bus_number(false, *n, mix_output_types),
            ChannelId::ControlGroup(n) => within(*n, 10, 109), // CG 1 → 110
            ChannelId::Matrix(n) => within(*n, 8, 119),        // Matrix 1 → 120
            // GraphicEq and MatrixInput are not in the GP OSC number space
            ChannelId::GraphicEq(_) | ChannelId::MatrixInput(_) => None,
        }
    }

    /// Parse from a GP OSC unified channel number.
    /// Parse from a GP OSC unified channel number.
    ///
    /// Buses 70-93 are a shared aux/group pool. Without config data, all
    /// buses in this range are classified by their per-type index using the
    /// `mix_output_types` array. If no config is available, falls back to
    /// assuming aux for 70-77 and group for 78-93 (8 aux / 16 group default).
    pub fn from_gp_osc_number(n: u8) -> Option<Self> {
        Self::from_gp_osc_number_with_config(n, None)
    }

    /// Config-aware version that classifies buses correctly.
    pub fn from_gp_osc_number_with_config(
        n: u8,
        mix_output_types: Option<&[bool]>,
    ) -> Option<Self> {
        match n {
            1..=60 => Some(ChannelId::Input(n as u16)),
            70..=93 => {
                let bus_index_0 = (n - 70) as usize; // 0-based bus index
                if let Some(types) = mix_output_types {
                    if let Some(&is_aux) = types.get(bus_index_0) {
                        if is_aux {
                            // Count how many auxes come before this bus
                            let aux_num =
                                types[..=bus_index_0].iter().filter(|&&t| t).count() as u16;
                            Some(ChannelId::Aux(aux_num))
                        } else {
                            let group_num =
                                types[..=bus_index_0].iter().filter(|&&t| !t).count() as u16;
                            Some(ChannelId::Group(group_num))
                        }
                    } else {
                        // Bus index out of range of config
                        None
                    }
                } else {
                    // No config: fallback — first 8 are aux, rest are group
                    if n <= 77 {
                        Some(ChannelId::Aux((n - 69) as u16))
                    } else {
                        Some(ChannelId::Group((n - 77) as u16))
                    }
                }
            }
            110..=119 => Some(ChannelId::ControlGroup((n - 109) as u16)),
            120..=127 => Some(ChannelId::Matrix((n - 119) as u16)),
            _ => None,
        }
    }

    /// Convert to the Pad-protocol path prefix under the given wire quirks.
    /// Use [`Self::to_ipad_path_prefix`] for the S21 quirks.
    pub fn to_pad_path_prefix(&self, q: &PadQuirks) -> String {
        match self {
            ChannelId::Input(n) => format!("/Input_Channels/{n}"),
            ChannelId::Aux(n) => format!("/Aux_Outputs/{n}"),
            ChannelId::Group(n) => format!("/Group_Outputs/{n}"),
            ChannelId::Matrix(n) => format!("/Matrix_Outputs/{n}"),
            ChannelId::ControlGroup(n) => {
                // Control Groups are the one channel type the S21 numbers from
                // zero. `saturating_sub` keeps a malformed CG 0 (which
                // `is_within_bounds` rejects anyway) from underflowing here.
                let wire = if q.control_groups_zero_based {
                    n.saturating_sub(1)
                } else {
                    *n
                };
                format!("/Control_Groups/{wire}")
            }
            ChannelId::GraphicEq(n) => format!("/Graphic_EQ/{n}"),
            ChannelId::MatrixInput(n) => format!("/Matrix_Inputs/{n}"),
        }
    }

    /// [`Self::to_pad_path_prefix`] under the hardware-verified S21 quirks.
    #[inline]
    pub fn to_ipad_path_prefix(&self) -> String {
        self.to_pad_path_prefix(&PadQuirks::S21)
    }

    /// Parse from an iPad protocol path.
    /// Expects a path like "/Input_Channels/1/..." and returns (ChannelId, remaining_path).
    ///
    /// Performs no per-type bounds checking — accepts any `0..=255` channel
    /// number. Use [`Self::from_ipad_path_with_config`] when validating
    /// against a live console config.
    pub fn from_ipad_path(path: &str) -> Option<(Self, &str)> {
        Self::from_ipad_path_with_config(path, None)
    }

    /// Parse from an iPad protocol path, optionally validating the channel
    /// number against the live `ConsoleConfig`. Mirrors the GP OSC parser's
    /// `from_gp_osc_number_with_config`. Pass `None` to skip validation
    /// (used during handshake before config is populated, and in tests).
    pub fn from_ipad_path_with_config<'a>(
        path: &'a str,
        config: Option<&crate::model::config::ConsoleConfig>,
    ) -> Option<(Self, &'a str)> {
        let path = path.strip_prefix('/')?;

        let (type_and_num, rest) = split_ipad_prefix(path)?;
        let (channel_type, num_str) = type_and_num;

        // Wire quirks come from the live console config when we have one;
        // pre-discovery callers (handshake) get the S21 defaults.
        let quirks = config
            .map(|c| c.profile().pad_quirks)
            .unwrap_or(PadQuirks::S21);

        let num: u16 = num_str.parse().ok()?;

        let channel = match channel_type {
            "Input_Channels" => ChannelId::Input(num),
            "Aux_Outputs" => ChannelId::Aux(num),
            "Group_Outputs" => ChannelId::Group(num),
            "Matrix_Outputs" => ChannelId::Matrix(num),
            // The S21 numbers Control Groups from 0 on the wire; we store
            // 1-based. `checked_add` rejects the max value gracefully (would
            // overflow); `is_within_bounds` below catches the rest of the
            // out-of-range cases. Found by audit M6 proptest fuzz.
            "Control_Groups" => {
                if quirks.control_groups_zero_based {
                    ChannelId::ControlGroup(num.checked_add(1)?)
                } else {
                    ChannelId::ControlGroup(num)
                }
            }
            "Graphic_EQ" => ChannelId::GraphicEq(num),
            "Matrix_Inputs" => ChannelId::MatrixInput(num),
            _ => return None,
        };

        if let Some(cfg) = config
            && !channel.is_within_bounds(cfg)
        {
            return None;
        }

        Some((channel, rest))
    }

    /// Whether this channel's number is within the documented range for its
    /// type, given the live console config. Used to drop bogus channel
    /// numbers received from the iPad/console proxy paths.
    pub fn is_within_bounds(&self, config: &crate::model::config::ConsoleConfig) -> bool {
        match self {
            ChannelId::Input(n) => (1..=config.input_channel_count).contains(n),
            ChannelId::Aux(n) => (1..=config.aux_output_count).contains(n),
            ChannelId::Group(n) => (1..=config.group_output_count).contains(n),
            ChannelId::Matrix(n) => (1..=config.matrix_output_count).contains(n),
            ChannelId::ControlGroup(n) => (1..=config.control_group_count).contains(n),
            ChannelId::GraphicEq(n) => (1..=config.graphic_eq_count).contains(n),
            ChannelId::MatrixInput(n) => (1..=config.matrix_input_count).contains(n),
        }
    }
}

/// GP OSC number of the `n`-th (1-based) aux (`aux == true`) or group bus.
/// With a layout, that is the `n`-th matching entry of `mix_output_types`;
/// without one, the default split (Aux 1–8 → 70–77, Group 1–16 → 78–93).
/// Mirrors the bus arm of `from_gp_osc_number_with_config`.
fn gp_bus_number(aux: bool, n: u16, mix_output_types: Option<&[bool]>) -> Option<u8> {
    let n = usize::from(n).checked_sub(1)?;
    let bus = match mix_output_types {
        Some(types) => {
            types
                .iter()
                .take(GP_BUS_COUNT)
                .enumerate()
                .filter(|&(_, &is_aux)| is_aux == aux)
                .nth(n)?
                .0
        }
        None if aux => (n < GP_DEFAULT_AUX_COUNT).then_some(n)?,
        None => (n < GP_BUS_COUNT - GP_DEFAULT_AUX_COUNT).then_some(GP_DEFAULT_AUX_COUNT + n)?,
    };
    u8::try_from(GP_FIRST_BUS + bus).ok()
}

/// Split an iPad path (after leading /) into (channel_type, number) and the remaining path.
/// E.g. "Input_Channels/1/fader" → (("Input_Channels", "1"), "/fader")
fn split_ipad_prefix(path: &str) -> Option<((&str, &str), &str)> {
    // Find first slash → channel type
    let slash1 = path.find('/')?;
    let channel_type = &path[..slash1];
    let after_type = &path[slash1 + 1..];

    // Find second slash (or end) → channel number
    let (num_str, rest) = if let Some(slash2) = after_type.find('/') {
        (&after_type[..slash2], &after_type[slash2..])
    } else {
        (after_type, "")
    };

    Some(((channel_type, num_str), rest))
}

impl fmt::Display for ChannelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChannelId::Input(n) => write!(f, "Input {n}"),
            ChannelId::Aux(n) => write!(f, "Aux {n}"),
            ChannelId::Group(n) => write!(f, "Group {n}"),
            ChannelId::Matrix(n) => write!(f, "Matrix {n}"),
            ChannelId::ControlGroup(n) => write!(f, "CG {n}"),
            ChannelId::GraphicEq(n) => write!(f, "GEQ {n}"),
            ChannelId::MatrixInput(n) => write!(f, "MtxIn {n}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gp_osc_round_trip() {
        // Input channels
        for n in 1..=60u16 {
            let ch = ChannelId::Input(n);
            let osc = ch.to_gp_osc_number().unwrap();
            assert_eq!(ChannelId::from_gp_osc_number(osc), Some(ch));
        }
        // Aux
        for n in 1..=8u16 {
            let ch = ChannelId::Aux(n);
            let osc = ch.to_gp_osc_number().unwrap();
            assert_eq!(osc as u16, 69 + n);
            assert_eq!(ChannelId::from_gp_osc_number(osc), Some(ch));
        }
        // Group
        for n in 1..=16u16 {
            let ch = ChannelId::Group(n);
            let osc = ch.to_gp_osc_number().unwrap();
            assert_eq!(osc as u16, 77 + n);
            assert_eq!(ChannelId::from_gp_osc_number(osc), Some(ch));
        }
        // Matrix
        for n in 1..=8u16 {
            let ch = ChannelId::Matrix(n);
            let osc = ch.to_gp_osc_number().unwrap();
            assert_eq!(osc as u16, 119 + n);
            assert_eq!(ChannelId::from_gp_osc_number(osc), Some(ch));
        }
        // Control Groups
        for n in 1..=10u16 {
            let ch = ChannelId::ControlGroup(n);
            let osc = ch.to_gp_osc_number().unwrap();
            assert_eq!(osc as u16, 109 + n);
            assert_eq!(ChannelId::from_gp_osc_number(osc), Some(ch));
        }
    }

    #[test]
    fn gp_osc_number_rejects_out_of_u8_range_channels() {
        // SD/Quantum-sized channel numbers have no GP OSC representation.
        assert_eq!(ChannelId::Input(256).to_gp_osc_number(), None);
        assert_eq!(ChannelId::Aux(200).to_gp_osc_number(), None);
    }

    #[test]
    fn gp_osc_number_does_not_overflow_on_absurd_channel_numbers() {
        // The Macros tab parses a free-text channel number with no upper
        // bound. Adding the bus offset must not panic in debug nor wrap into
        // a plausible-looking channel in release.
        for ch in [
            ChannelId::Aux(u16::MAX),
            ChannelId::Group(u16::MAX),
            ChannelId::Matrix(u16::MAX),
            ChannelId::ControlGroup(u16::MAX),
            ChannelId::Aux(65_500),
            ChannelId::Input(u16::MAX),
        ] {
            assert_eq!(
                ch.to_gp_osc_number(),
                None,
                "{ch:?} must have no GP OSC number"
            );
        }
    }

    /// A 10-aux / 14-group desk. Encoding used to assume 8 auxes, so Aux 9
    /// and Group 1 both went to bus 78 (audit H1).
    #[test]
    fn gp_osc_number_follows_the_bus_layout() {
        let ten_aux: Vec<bool> = (0..24).map(|i| i < 10).collect();
        let layout = Some(ten_aux.as_slice());
        assert_eq!(
            ChannelId::Aux(9).to_gp_osc_number_with_config(layout),
            Some(78)
        );
        assert_eq!(
            ChannelId::Aux(10).to_gp_osc_number_with_config(layout),
            Some(79)
        );
        assert_eq!(
            ChannelId::Group(1).to_gp_osc_number_with_config(layout),
            Some(80)
        );
        assert_eq!(
            ChannelId::Group(14).to_gp_osc_number_with_config(layout),
            Some(93)
        );
        assert_eq!(
            ChannelId::Aux(11).to_gp_osc_number_with_config(layout),
            None
        );
        assert_eq!(
            ChannelId::Group(15).to_gp_osc_number_with_config(layout),
            None
        );

        // Interleaved, as the iPad handshake can report it.
        let mixed = [true, false, true, false];
        let layout = Some(mixed.as_slice());
        assert_eq!(
            ChannelId::Aux(2).to_gp_osc_number_with_config(layout),
            Some(72)
        );
        assert_eq!(
            ChannelId::Group(2).to_gp_osc_number_with_config(layout),
            Some(73)
        );
        assert_eq!(
            ChannelId::Group(3).to_gp_osc_number_with_config(layout),
            None
        );
    }

    #[test]
    fn gp_osc_number_rejects_out_of_range_numbers() {
        // Each of these used to land on another type's number.
        assert_eq!(ChannelId::ControlGroup(11).to_gp_osc_number(), None); // was Matrix 1
        assert_eq!(ChannelId::Aux(9).to_gp_osc_number(), None); // was Group 1
        assert_eq!(ChannelId::Input(61).to_gp_osc_number(), None);
        assert_eq!(ChannelId::Matrix(9).to_gp_osc_number(), None);
        assert_eq!(ChannelId::Group(17).to_gp_osc_number(), None);
        for ch in [
            ChannelId::Input(0),
            ChannelId::Aux(0),
            ChannelId::Group(0),
            ChannelId::ControlGroup(0),
            ChannelId::Matrix(0),
        ] {
            assert_eq!(ch.to_gp_osc_number(), None, "{ch:?}");
        }
    }

    mod gp_layout_props {
        use super::*;
        use proptest::prelude::*;

        fn arb_channel() -> impl Strategy<Value = ChannelId> {
            prop_oneof![
                (0u16..70).prop_map(ChannelId::Input),
                (0u16..30).prop_map(ChannelId::Aux),
                (0u16..30).prop_map(ChannelId::Group),
                (0u16..15).prop_map(ChannelId::ControlGroup),
                (0u16..12).prop_map(ChannelId::Matrix),
            ]
        }

        fn arb_layout() -> impl Strategy<Value = Option<Vec<bool>>> {
            proptest::option::of(proptest::collection::vec(any::<bool>(), 0..=30))
        }

        proptest! {
            /// Encoding and parsing are exact inverses under any layout,
            /// in both directions.
            #[test]
            fn encode_is_the_inverse_of_parse(ch in arb_channel(), layout in arb_layout()) {
                let layout = layout.as_deref();
                if let Some(n) = ch.to_gp_osc_number_with_config(layout) {
                    prop_assert_eq!(ChannelId::from_gp_osc_number_with_config(n, layout), Some(ch));
                }
            }

            #[test]
            fn parse_is_the_inverse_of_encode(n in 0u8..=255, layout in arb_layout()) {
                let layout = layout.as_deref();
                if let Some(ch) = ChannelId::from_gp_osc_number_with_config(n, layout) {
                    prop_assert_eq!(ch.to_gp_osc_number_with_config(layout), Some(n));
                }
            }
        }
    }

    #[test]
    fn graphic_eq_not_in_gp_osc() {
        assert_eq!(ChannelId::GraphicEq(1).to_gp_osc_number(), None);
        assert_eq!(ChannelId::MatrixInput(1).to_gp_osc_number(), None);
    }

    #[test]
    fn ipad_path_prefix() {
        assert_eq!(
            ChannelId::Input(1).to_ipad_path_prefix(),
            "/Input_Channels/1"
        );
        assert_eq!(
            ChannelId::ControlGroup(1).to_ipad_path_prefix(),
            "/Control_Groups/0" // iPad is 0-based
        );
        assert_eq!(
            ChannelId::GraphicEq(5).to_ipad_path_prefix(),
            "/Graphic_EQ/5"
        );
    }

    #[test]
    fn ipad_path_parsing() {
        let (ch, rest) = ChannelId::from_ipad_path("/Input_Channels/1/fader").unwrap();
        assert_eq!(ch, ChannelId::Input(1));
        assert_eq!(rest, "/fader");

        let (ch, rest) = ChannelId::from_ipad_path("/Control_Groups/0/fader").unwrap();
        assert_eq!(ch, ChannelId::ControlGroup(1)); // 0-based → 1-based
        assert_eq!(rest, "/fader");

        let (ch, rest) = ChannelId::from_ipad_path("/Graphic_EQ/3/geq_gain").unwrap();
        assert_eq!(ch, ChannelId::GraphicEq(3));
        assert_eq!(rest, "/geq_gain");
    }

    // ─── iPad path bounds checking (audit H6) ──────────────────────────────

    fn s21_default_config() -> crate::model::config::ConsoleConfig {
        // S21 defaults: 48 inputs, 8 aux, 8 group (default split of 16 buses),
        // 8 matrix outs, 10 matrix inputs, 10 control groups, 16 GEQ.
        crate::model::config::ConsoleConfig {
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
            mix_output_types: vec![],
            mix_output_modes: vec![],
            input_modes: vec![],
            group_modes: vec![],
            plus_mode: crate::model::config::PlusMode::S21,
            family: crate::model::family::ConsoleFamily::SSeries,
            pad_quirk_overrides: None,
        }
    }

    #[test]
    fn ipad_path_with_config_accepts_in_range() {
        let cfg = s21_default_config();
        let parsed = ChannelId::from_ipad_path_with_config("/Input_Channels/48/fader", Some(&cfg));
        assert_eq!(parsed, Some((ChannelId::Input(48), "/fader")));

        let parsed = ChannelId::from_ipad_path_with_config("/Aux_Outputs/8/fader", Some(&cfg));
        assert_eq!(parsed, Some((ChannelId::Aux(8), "/fader")));
    }

    #[test]
    fn ipad_path_with_config_rejects_out_of_range_input() {
        let cfg = s21_default_config();
        // 48-input config — input 49 is out of range.
        assert!(
            ChannelId::from_ipad_path_with_config("/Input_Channels/49/fader", Some(&cfg)).is_none()
        );
        // Pathological: 255 (max u8) — must also reject.
        assert!(
            ChannelId::from_ipad_path_with_config("/Input_Channels/255/fader", Some(&cfg))
                .is_none()
        );
    }

    #[test]
    fn ipad_path_with_config_rejects_out_of_range_aux() {
        let cfg = s21_default_config();
        // 8-aux config — aux 9 is out of range.
        assert!(
            ChannelId::from_ipad_path_with_config("/Aux_Outputs/9/fader", Some(&cfg)).is_none()
        );
    }

    #[test]
    fn ipad_path_with_config_rejects_zero() {
        let cfg = s21_default_config();
        // 1-based numbering — 0 is out of range for everything except
        // Control_Groups, which is iPad 0-based (0 → ControlGroup(1)).
        assert!(
            ChannelId::from_ipad_path_with_config("/Input_Channels/0/fader", Some(&cfg)).is_none()
        );
        assert!(
            ChannelId::from_ipad_path_with_config("/Aux_Outputs/0/fader", Some(&cfg)).is_none()
        );
        // Control_Groups/0 is the first CG → ControlGroup(1), valid.
        assert_eq!(
            ChannelId::from_ipad_path_with_config("/Control_Groups/0/fader", Some(&cfg)),
            Some((ChannelId::ControlGroup(1), "/fader")),
        );
    }

    #[test]
    fn ipad_path_with_no_config_skips_validation() {
        // Backward compat: passing None means accept any number, same as the
        // legacy `from_ipad_path`. Used during handshake before config is
        // populated, and in tests.
        let parsed = ChannelId::from_ipad_path_with_config("/Input_Channels/255/fader", None);
        assert_eq!(parsed, Some((ChannelId::Input(255), "/fader")));
    }

    #[test]
    fn pad_path_prefix_without_cg_zero_base_quirk_is_one_based() {
        let q = PadQuirks {
            control_groups_zero_based: false,
            ..PadQuirks::S21
        };
        assert_eq!(
            ChannelId::ControlGroup(1).to_pad_path_prefix(&q),
            "/Control_Groups/1"
        );
        // Other channel types are unaffected by the CG quirk.
        assert_eq!(
            ChannelId::Input(7).to_pad_path_prefix(&q),
            "/Input_Channels/7"
        );
    }

    #[test]
    fn pad_path_parse_without_cg_zero_base_quirk_is_one_based() {
        let mut cfg = s21_default_config();
        cfg.pad_quirk_overrides = Some(PadQuirks {
            control_groups_zero_based: false,
            ..PadQuirks::S21
        });
        // Wire 1 is now CG 1 (not CG 2).
        assert_eq!(
            ChannelId::from_ipad_path_with_config("/Control_Groups/1/fader", Some(&cfg)),
            Some((ChannelId::ControlGroup(1), "/fader")),
        );
        // Wire 0 is out of range for 1-based numbering.
        assert!(
            ChannelId::from_ipad_path_with_config("/Control_Groups/0/fader", Some(&cfg)).is_none()
        );
    }

    #[test]
    fn control_group_zero_does_not_underflow() {
        // CG 0 is never valid (`is_within_bounds` rejects it), but encoding
        // one must not panic in debug builds.
        assert_eq!(
            ChannelId::ControlGroup(0).to_ipad_path_prefix(),
            "/Control_Groups/0"
        );
    }

    #[test]
    fn is_within_bounds_per_type() {
        let cfg = s21_default_config();
        assert!(ChannelId::Input(1).is_within_bounds(&cfg));
        assert!(ChannelId::Input(48).is_within_bounds(&cfg));
        assert!(!ChannelId::Input(49).is_within_bounds(&cfg));
        assert!(!ChannelId::Input(0).is_within_bounds(&cfg));

        assert!(ChannelId::Aux(8).is_within_bounds(&cfg));
        assert!(!ChannelId::Aux(9).is_within_bounds(&cfg));

        assert!(ChannelId::Matrix(8).is_within_bounds(&cfg));
        assert!(!ChannelId::Matrix(9).is_within_bounds(&cfg));

        assert!(ChannelId::ControlGroup(10).is_within_bounds(&cfg));
        assert!(!ChannelId::ControlGroup(11).is_within_bounds(&cfg));

        assert!(ChannelId::GraphicEq(16).is_within_bounds(&cfg));
        assert!(!ChannelId::GraphicEq(17).is_within_bounds(&cfg));

        assert!(ChannelId::MatrixInput(10).is_within_bounds(&cfg));
        assert!(!ChannelId::MatrixInput(11).is_within_bounds(&cfg));
    }
}
