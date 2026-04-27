use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::channel::ChannelId;
use super::parameter::ParameterPath;

/// Block identifiers matching the DiGiCo S21's signal-flow layout for
/// recall scope and recall safe. These are coarser than `ParameterSection`
/// and mirror the toggle blocks shown on the console's recall scope and
/// recall safe screens.
///
/// Note: these settings are manually entered by the operator to match the
/// console configuration — the app cannot read or write them via OSC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RecallBlock {
    // ── Sources ──
    MonoStereoAmm,
    SocketProperties,
    InputRoute,
    BusMode,
    CgMembers,

    // ── Input Processing ──
    ChannelName,
    Filters,
    BalanceWidth,
    Trim,
    Delay,
    DigiTube,

    // ── Inserts ──
    InsertA,
    InsertB,

    // ── Channel Processing ──
    Equaliser,
    Dynamics1,
    Dynamics2,

    // ── Outputs ──
    AuxSends,
    GroupAssigns,
    DirectOuts,
    Mute,
    Fader,
    Pan,

    // ── Global-only (Session Scope top row) ──
    AmmMasters,
    Crossfades,
    FxRack,
    Geqs,
    MatrixGlobal,
}

impl RecallBlock {
    /// Display label matching the console's UI.
    pub fn label(&self) -> &'static str {
        match self {
            Self::MonoStereoAmm => "Mono/Stereo\n& AMM",
            Self::SocketProperties => "Socket\nProperties",
            Self::InputRoute => "Input Route",
            Self::BusMode => "Bus Mode",
            Self::CgMembers => "CG Members",
            Self::ChannelName => "Channel\nName",
            Self::Filters => "Filters",
            Self::BalanceWidth => "Balance &\nWidth",
            Self::Trim => "Trim",
            Self::Delay => "Delay",
            Self::DigiTube => "DiGiTube",
            Self::InsertA => "Send &\nReturn",
            Self::InsertB => "Send &\nReturn",
            Self::Equaliser => "Equaliser",
            Self::Dynamics1 => "Dynamics 1",
            Self::Dynamics2 => "Dynamics 2",
            Self::AuxSends => "Aux Sends",
            Self::GroupAssigns => "Group\nAssigns",
            Self::DirectOuts => "Direct Outs",
            Self::Mute => "Mute",
            Self::Fader => "Fader",
            Self::Pan => "Pan",
            Self::AmmMasters => "AMM\nMasters",
            Self::Crossfades => "Crossfades",
            Self::FxRack => "FX Rack",
            Self::Geqs => "GEQs",
            Self::MatrixGlobal => "Matrix",
        }
    }

    /// Which RecallBlock a ParameterPath belongs to, if any.
    /// Used for the blue conflict colouring in the scope editor.
    pub fn from_parameter_path(path: &ParameterPath) -> Option<RecallBlock> {
        match path {
            ParameterPath::Name => Some(Self::ChannelName),
            ParameterPath::Fader => Some(Self::Fader),
            ParameterPath::Mute => Some(Self::Mute),
            ParameterPath::Pan => Some(Self::Pan),

            ParameterPath::AnalogGain
            | ParameterPath::GainTracking
            | ParameterPath::Polarity
            | ParameterPath::Phantom
            | ParameterPath::MainAltIn
            | ParameterPath::StereoMode => Some(Self::MonoStereoAmm),

            ParameterPath::Trim => Some(Self::Trim),
            ParameterPath::Balance | ParameterPath::Width => Some(Self::BalanceWidth),
            ParameterPath::DelayEnabled | ParameterPath::DelayTime => Some(Self::Delay),
            ParameterPath::DigitubeEnabled
            | ParameterPath::DigitubeDrive
            | ParameterPath::DigitubeBias => Some(Self::DigiTube),

            ParameterPath::HighpassEnabled
            | ParameterPath::HighpassFrequency
            | ParameterPath::LowpassEnabled
            | ParameterPath::LowpassFrequency => Some(Self::Filters),

            ParameterPath::EqEnabled
            | ParameterPath::EqBandFrequency(_)
            | ParameterPath::EqBandGain(_)
            | ParameterPath::EqBandQ(_)
            | ParameterPath::EqBandCurve(_)
            | ParameterPath::EqBandDynEnabled(_)
            | ParameterPath::EqBandDynThreshold(_)
            | ParameterPath::EqBandDynRatio(_)
            | ParameterPath::EqBandDynAttack(_)
            | ParameterPath::EqBandDynRelease(_)
            | ParameterPath::EqBandDynOverUnder(_) => Some(Self::Equaliser),

            ParameterPath::Dyn1Enabled
            | ParameterPath::Dyn1Mode
            | ParameterPath::Dyn1Threshold(_)
            | ParameterPath::Dyn1Knee(_)
            | ParameterPath::Dyn1Ratio(_)
            | ParameterPath::Dyn1Attack(_)
            | ParameterPath::Dyn1Release(_)
            | ParameterPath::Dyn1Gain(_)
            | ParameterPath::Dyn1Listen(_)
            | ParameterPath::Dyn1CrossoverHigh
            | ParameterPath::Dyn1CrossoverLow => Some(Self::Dynamics1),

            ParameterPath::Dyn2Enabled
            | ParameterPath::Dyn2Mode
            | ParameterPath::Dyn2Threshold
            | ParameterPath::Dyn2Knee
            | ParameterPath::Dyn2Ratio
            | ParameterPath::Dyn2Range
            | ParameterPath::Dyn2Attack
            | ParameterPath::Dyn2Hold
            | ParameterPath::Dyn2Release
            | ParameterPath::Dyn2Gain
            | ParameterPath::Dyn2Highpass
            | ParameterPath::Dyn2Lowpass
            | ParameterPath::Dyn2Listen
            | ParameterPath::Dyn2KeySolo => Some(Self::Dynamics2),

            ParameterPath::SendEnabled(_)
            | ParameterPath::SendLevel(_)
            | ParameterPath::SendPan(_) => Some(Self::AuxSends),

            ParameterPath::GroupSendOn(_) => Some(Self::GroupAssigns),

            ParameterPath::CgLevel => Some(Self::CgMembers),

            ParameterPath::GeqEnabled | ParameterPath::GeqBandGain(_) => Some(Self::Geqs),

            ParameterPath::MatrixSendLevel(_) | ParameterPath::MatrixSendOn(_) => {
                Some(Self::MatrixGlobal)
            }

            _ => None,
        }
    }

    /// The signal-flow column blocks for a channel, in display order.
    /// Used by the popup UI to render the block grid.
    pub fn sources_column() -> &'static [RecallBlock] {
        &[
            Self::MonoStereoAmm,
            Self::SocketProperties,
            Self::InputRoute,
            Self::BusMode,
            Self::CgMembers,
        ]
    }

    pub fn input_processing_column() -> &'static [RecallBlock] {
        &[
            Self::ChannelName,
            Self::Filters,
            Self::BalanceWidth,
            Self::Trim,
            Self::Delay,
            Self::DigiTube,
        ]
    }

    pub fn insert_a_column() -> &'static [RecallBlock] {
        &[Self::InsertA]
    }

    pub fn channel_processing_column() -> &'static [RecallBlock] {
        &[Self::Equaliser, Self::Dynamics1, Self::Dynamics2]
    }

    pub fn insert_b_column() -> &'static [RecallBlock] {
        &[Self::InsertB]
    }

    pub fn outputs_column() -> &'static [RecallBlock] {
        &[
            Self::AuxSends,
            Self::GroupAssigns,
            Self::DirectOuts,
            Self::Mute,
            Self::Fader,
            Self::Pan,
        ]
    }

    /// Global-only blocks shown in the session recall scope top row.
    pub fn global_blocks() -> &'static [RecallBlock] {
        &[
            Self::AmmMasters,
            Self::Crossfades,
            Self::FxRack,
            Self::Geqs,
            Self::MatrixGlobal,
        ]
    }

    /// Whether this block is applicable to a given channel type.
    pub fn available_for_channel(block: RecallBlock, channel: &ChannelId) -> bool {
        match channel {
            ChannelId::Input(_) => true, // inputs have all blocks
            ChannelId::Aux(_) | ChannelId::Group(_) => matches!(
                block,
                Self::ChannelName
                    | Self::Filters
                    | Self::BalanceWidth
                    | Self::Equaliser
                    | Self::Dynamics1
                    | Self::Dynamics2
                    | Self::InsertA
                    | Self::InsertB
                    | Self::Mute
                    | Self::Fader
                    | Self::Pan
                    | Self::DirectOuts
            ),
            ChannelId::Matrix(_) => matches!(
                block,
                Self::ChannelName
                    | Self::Equaliser
                    | Self::Dynamics1
                    | Self::Dynamics2
                    | Self::InsertA
                    | Self::InsertB
                    | Self::Mute
                    | Self::Fader
                    | Self::Pan
            ),
            ChannelId::ControlGroup(_) => {
                matches!(block, Self::ChannelName | Self::Mute | Self::Fader)
            }
            _ => false,
        }
    }
}

/// Console's session recall scope (global).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionRecallScope {
    pub active_blocks: HashSet<RecallBlock>,
}

/// Per-channel recall safe.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChannelRecallSafe {
    pub safe_blocks: HashSet<RecallBlock>,
}

/// All recall scope/safe data — stored in the show file.
/// This is a visual reference only — the app cannot read or write these
/// settings on the console via OSC.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConsoleRecallConfig {
    pub session_scope: SessionRecallScope,
    pub channel_safes: HashMap<ChannelId, ChannelRecallSafe>,
}

impl ConsoleRecallConfig {
    /// Check if a parameter on a channel would be recalled by the console
    /// (in scope AND not safe). Used for the blue conflict indicator.
    pub fn is_console_recalled(&self, channel: &ChannelId, path: &ParameterPath) -> bool {
        let Some(block) = RecallBlock::from_parameter_path(path) else {
            return false;
        };
        let in_scope = self.session_scope.active_blocks.contains(&block);
        let is_safe = self
            .channel_safes
            .get(channel)
            .is_some_and(|safe| safe.safe_blocks.contains(&block));
        in_scope && !is_safe
    }
}
