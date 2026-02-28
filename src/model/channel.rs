use serde::{Deserialize, Serialize};
use std::fmt;

/// Logical channel identifier (protocol-agnostic).
/// All channel numbers are 1-based internally.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChannelId {
    Input(u8),        // 1–60
    Aux(u8),          // 1–n (depends on aux/group split)
    Group(u8),        // 1–n
    Matrix(u8),       // 1–8
    ControlGroup(u8), // 1–10
    GraphicEq(u8),    // 1–16
    MatrixInput(u8),  // 1–10
}

impl ChannelId {
    /// Convert to the GP OSC unified channel number.
    /// Returns None for channel types not in the GP OSC number space.
    pub fn to_gp_osc_number(&self) -> Option<u8> {
        match self {
            ChannelId::Input(n) => Some(*n),           // 1–60
            ChannelId::Aux(n) => Some(69 + n),         // Aux 1 → 70
            ChannelId::Group(n) => Some(77 + n),       // Group 1 → 78
            ChannelId::Matrix(n) => Some(119 + n),     // Matrix 1 → 120
            ChannelId::ControlGroup(n) => Some(109 + n), // CG 1 → 110
            // GraphicEq and MatrixInput are not in the GP OSC number space
            ChannelId::GraphicEq(_) | ChannelId::MatrixInput(_) => None,
        }
    }

    /// Parse from a GP OSC unified channel number.
    pub fn from_gp_osc_number(n: u8) -> Option<Self> {
        match n {
            1..=60 => Some(ChannelId::Input(n)),
            70..=77 => Some(ChannelId::Aux(n - 69)),
            78..=93 => Some(ChannelId::Group(n - 77)),
            110..=119 => Some(ChannelId::ControlGroup(n - 109)),
            120..=127 => Some(ChannelId::Matrix(n - 119)),
            _ => None,
        }
    }

    /// Convert to the iPad protocol path prefix.
    pub fn to_ipad_path_prefix(&self) -> String {
        match self {
            ChannelId::Input(n) => format!("/Input_Channels/{n}"),
            ChannelId::Aux(n) => format!("/Aux_Outputs/{n}"),
            ChannelId::Group(n) => format!("/Group_Outputs/{n}"),
            ChannelId::Matrix(n) => format!("/Matrix_Outputs/{n}"),
            ChannelId::ControlGroup(n) => format!("/Control_Groups/{}", n - 1), // iPad is 0-based
            ChannelId::GraphicEq(n) => format!("/Graphic_EQ/{n}"),
            ChannelId::MatrixInput(n) => format!("/Matrix_Inputs/{n}"),
        }
    }

    /// Parse from an iPad protocol path.
    /// Expects a path like "/Input_Channels/1/..." and returns (ChannelId, remaining_path).
    pub fn from_ipad_path(path: &str) -> Option<(Self, &str)> {
        let path = path.strip_prefix('/')?;

        let (type_and_num, rest) = split_ipad_prefix(path)?;
        let (channel_type, num_str) = type_and_num;

        let num: u8 = num_str.parse().ok()?;

        let channel = match channel_type {
            "Input_Channels" => ChannelId::Input(num),
            "Aux_Outputs" => ChannelId::Aux(num),
            "Group_Outputs" => ChannelId::Group(num),
            "Matrix_Outputs" => ChannelId::Matrix(num),
            "Control_Groups" => ChannelId::ControlGroup(num + 1), // iPad 0-based → 1-based
            "Graphic_EQ" => ChannelId::GraphicEq(num),
            "Matrix_Inputs" => ChannelId::MatrixInput(num),
            _ => return None,
        };

        Some((channel, rest))
    }
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
        for n in 1..=60u8 {
            let ch = ChannelId::Input(n);
            let osc = ch.to_gp_osc_number().unwrap();
            assert_eq!(ChannelId::from_gp_osc_number(osc), Some(ch));
        }
        // Aux
        for n in 1..=8u8 {
            let ch = ChannelId::Aux(n);
            let osc = ch.to_gp_osc_number().unwrap();
            assert_eq!(osc, 69 + n);
            assert_eq!(ChannelId::from_gp_osc_number(osc), Some(ch));
        }
        // Group
        for n in 1..=16u8 {
            let ch = ChannelId::Group(n);
            let osc = ch.to_gp_osc_number().unwrap();
            assert_eq!(osc, 77 + n);
            assert_eq!(ChannelId::from_gp_osc_number(osc), Some(ch));
        }
        // Matrix
        for n in 1..=8u8 {
            let ch = ChannelId::Matrix(n);
            let osc = ch.to_gp_osc_number().unwrap();
            assert_eq!(osc, 119 + n);
            assert_eq!(ChannelId::from_gp_osc_number(osc), Some(ch));
        }
        // Control Groups
        for n in 1..=10u8 {
            let ch = ChannelId::ControlGroup(n);
            let osc = ch.to_gp_osc_number().unwrap();
            assert_eq!(osc, 109 + n);
            assert_eq!(ChannelId::from_gp_osc_number(osc), Some(ch));
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
}
