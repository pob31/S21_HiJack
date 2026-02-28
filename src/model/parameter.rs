use serde::{Deserialize, Serialize};
use std::fmt;

use super::channel::ChannelId;

/// A specific parameter on a specific channel.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParameterAddress {
    pub channel: ChannelId,
    pub parameter: ParameterPath,
}

/// Parameter within a channel, organized by section.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParameterPath {
    // Output
    Name,
    Fader,
    Mute,
    Solo,
    Pan,

    // Input section
    Gain,
    GainTracking,
    Trim,
    Balance,
    Width,
    Polarity,
    Phantom,    // iPad protocol only
    MainAltIn,  // iPad protocol only
    StereoMode, // iPad protocol only

    // Delay
    DelayEnabled,
    DelayTime,

    // Digitube
    DigitubeEnabled,
    DigitubeDrive,
    DigitubeBias,

    // EQ
    EqEnabled,
    HighpassEnabled,
    HighpassFrequency,
    LowpassEnabled,
    LowpassFrequency,
    EqBandFrequency(u8), // band 1–4
    EqBandGain(u8),
    EqBandQ(u8),
    EqBandCurve(u8),           // iPad protocol only
    EqBandDynEnabled(u8),
    EqBandDynThreshold(u8),
    EqBandDynRatio(u8),
    EqBandDynAttack(u8),
    EqBandDynRelease(u8),
    EqBandDynOverUnder(u8), // iPad protocol only

    // Dynamics 1 (compressor)
    Dyn1Enabled,
    Dyn1Mode,
    Dyn1MultibandDeesser, // iPad protocol only
    Dyn1Threshold(u8),    // band 1–3
    Dyn1Knee(u8),
    Dyn1Ratio(u8),
    Dyn1Attack(u8),
    Dyn1Release(u8),
    Dyn1Gain(u8),
    Dyn1Listen(u8),
    Dyn1CrossoverHigh,
    Dyn1CrossoverLow,

    // Dynamics 2 (gate)
    Dyn2Enabled,
    Dyn2Mode,
    Dyn2Threshold,
    Dyn2Knee,
    Dyn2Ratio,
    Dyn2Range,
    Dyn2Attack,
    Dyn2Hold,
    Dyn2Release,
    Dyn2Gain,
    Dyn2Highpass,
    Dyn2Lowpass,
    Dyn2Listen,
    Dyn2KeySolo, // iPad protocol only

    // Sends (input channels only)
    SendEnabled(u8), // send/aux number
    SendLevel(u8),
    SendPan(u8),

    // Group routing (iPad protocol only)
    GroupSendOn(u8),
    MasterBusOn,

    // Inserts (iPad protocol only)
    InsertAEnabled,
    InsertBEnabled,

    // CG membership (iPad protocol only)
    CgLevel,
    CgMute,

    // Matrix sends (MatrixInput channels, iPad protocol only)
    MatrixSendLevel(u8),
    MatrixSendOn(u8),

    // Graphic EQ (GraphicEq channels only, iPad protocol only)
    GeqBandGain(u8), // band 1–32
    GeqEnabled,
}

impl ParameterPath {
    /// Convert to GP OSC path suffix (after /channel/{ch}/).
    /// Returns None for iPad-only parameters.
    pub fn to_gp_osc_suffix(&self) -> Option<String> {
        match self {
            ParameterPath::Name => Some("name".into()),
            ParameterPath::Fader => Some("fader".into()),
            ParameterPath::Mute => Some("mute".into()),
            ParameterPath::Solo => Some("solo".into()),
            ParameterPath::Pan => Some("pan".into()),
            ParameterPath::Gain => Some("total/gain".into()),
            ParameterPath::GainTracking => Some("input/gain_tracking".into()),
            ParameterPath::Trim => Some("input/trim".into()),
            ParameterPath::Balance => Some("input/balance".into()),
            ParameterPath::Width => Some("input/width".into()),
            ParameterPath::Polarity => Some("input/polarity".into()),
            ParameterPath::DelayEnabled => Some("input/delay/enabled".into()),
            ParameterPath::DelayTime => Some("input/delay/time".into()),
            ParameterPath::DigitubeEnabled => Some("input/digitube/enabled".into()),
            ParameterPath::DigitubeDrive => Some("input/digitube/drive".into()),
            ParameterPath::DigitubeBias => Some("input/digitube/bias".into()),
            ParameterPath::EqEnabled => Some("eq/enabled".into()),
            ParameterPath::HighpassEnabled => Some("eq/highpass/enabled".into()),
            ParameterPath::HighpassFrequency => Some("eq/highpass/frequency".into()),
            ParameterPath::LowpassEnabled => Some("eq/lowpass/enabled".into()),
            ParameterPath::LowpassFrequency => Some("eq/lowpass/frequency".into()),
            ParameterPath::EqBandFrequency(b) => Some(format!("eq/{b}/frequency")),
            ParameterPath::EqBandGain(b) => Some(format!("eq/{b}/gain")),
            ParameterPath::EqBandQ(b) => Some(format!("eq/{b}/q")),
            ParameterPath::EqBandDynEnabled(b) => Some(format!("eq/{b}/dyn/enabled")),
            ParameterPath::EqBandDynThreshold(b) => Some(format!("eq/{b}/dyn/threshold")),
            ParameterPath::EqBandDynRatio(b) => Some(format!("eq/{b}/dyn/ratio")),
            ParameterPath::EqBandDynAttack(b) => Some(format!("eq/{b}/dyn/attack")),
            ParameterPath::EqBandDynRelease(b) => Some(format!("eq/{b}/dyn/release")),
            ParameterPath::Dyn1Enabled => Some("dyn1/enabled".into()),
            ParameterPath::Dyn1Mode => Some("dyn1/mode".into()),
            ParameterPath::Dyn1Threshold(b) => Some(format!("dyn1/{b}/threshold")),
            ParameterPath::Dyn1Knee(b) => Some(format!("dyn1/{b}/knee")),
            ParameterPath::Dyn1Ratio(b) => Some(format!("dyn1/{b}/ratio")),
            ParameterPath::Dyn1Attack(b) => Some(format!("dyn1/{b}/attack")),
            ParameterPath::Dyn1Release(b) => Some(format!("dyn1/{b}/release")),
            ParameterPath::Dyn1Gain(b) => Some(format!("dyn1/{b}/gain")),
            ParameterPath::Dyn1Listen(b) => Some(format!("dyn1/{b}/listen")),
            ParameterPath::Dyn1CrossoverHigh => Some("dyn1/crossover_high".into()),
            ParameterPath::Dyn1CrossoverLow => Some("dyn1/crossover_low".into()),
            ParameterPath::Dyn2Enabled => Some("dyn2/enabled".into()),
            ParameterPath::Dyn2Mode => Some("dyn2/mode".into()),
            ParameterPath::Dyn2Threshold => Some("dyn2/threshold".into()),
            ParameterPath::Dyn2Knee => Some("dyn2/knee".into()),
            ParameterPath::Dyn2Ratio => Some("dyn2/ratio".into()),
            ParameterPath::Dyn2Range => Some("dyn2/range".into()),
            ParameterPath::Dyn2Attack => Some("dyn2/attack".into()),
            ParameterPath::Dyn2Hold => Some("dyn2/hold".into()),
            ParameterPath::Dyn2Release => Some("dyn2/release".into()),
            ParameterPath::Dyn2Gain => Some("dyn2/gain".into()),
            ParameterPath::Dyn2Highpass => Some("dyn2/highpass".into()),
            ParameterPath::Dyn2Lowpass => Some("dyn2/lowpass".into()),
            ParameterPath::Dyn2Listen => Some("dyn2/listen".into()),
            ParameterPath::SendEnabled(s) => Some(format!("send/{s}/enabled")),
            ParameterPath::SendLevel(s) => Some(format!("send/{s}/level")),
            ParameterPath::SendPan(s) => Some(format!("send/{s}/pan")),
            // iPad-only parameters
            _ => None,
        }
    }

    /// Convert to iPad protocol path suffix (after /{ChannelType}/{number}/).
    /// Returns None for parameters with no iPad representation (GP OSC-only).
    pub fn to_ipad_suffix(&self) -> Option<String> {
        match self {
            // Output
            ParameterPath::Name => Some("Channel_Input/name".into()),
            ParameterPath::Fader => Some("fader".into()),
            ParameterPath::Mute => Some("mute".into()),
            ParameterPath::Solo => Some("solo".into()),
            ParameterPath::Pan => Some("Panner/pan".into()),

            // Input section
            ParameterPath::Gain => Some("Channel_Input/analog_gain".into()),
            ParameterPath::Trim => Some("Channel_Input/trim".into()),
            ParameterPath::Polarity => Some("Channel_Input/phase".into()),
            ParameterPath::Phantom => Some("Channel_Input/phantom".into()),
            ParameterPath::MainAltIn => Some("Channel_Input/main_alt_in".into()),
            ParameterPath::StereoMode => Some("Channel_Input/stereo_mode".into()),

            // GP OSC-only input params
            ParameterPath::GainTracking
            | ParameterPath::Balance
            | ParameterPath::Width => None,

            // Delay
            ParameterPath::DelayEnabled => Some("Channel_Delay/delay_on".into()),
            ParameterPath::DelayTime => Some("Channel_Delay/delay".into()),

            // Digitube — not in iPad protocol
            ParameterPath::DigitubeEnabled
            | ParameterPath::DigitubeDrive
            | ParameterPath::DigitubeBias => None,

            // EQ
            ParameterPath::EqEnabled => Some("EQ/eq_in".into()),
            ParameterPath::HighpassEnabled => Some("Filters/lo_filter_in".into()),
            ParameterPath::HighpassFrequency => Some("Filters/lo_filter_freq".into()),
            ParameterPath::LowpassEnabled => Some("Filters/hi_filter_in".into()),
            ParameterPath::LowpassFrequency => Some("Filters/hi_filter_freq".into()),
            ParameterPath::EqBandFrequency(b) => Some(format!("EQ/eq_freq_{b}")),
            ParameterPath::EqBandGain(b) => Some(format!("EQ/eq_gain_{b}")),
            ParameterPath::EqBandQ(b) => Some(format!("EQ/eq_Q_{b}")),
            ParameterPath::EqBandCurve(b) => Some(format!("EQ/eq_curve_{b}")),
            ParameterPath::EqBandDynEnabled(b) => Some(format!("EQ/dynamic_eq_on_{b}")),
            ParameterPath::EqBandDynThreshold(b) => Some(format!("EQ/eq_thresh_{b}")),
            ParameterPath::EqBandDynRatio(b) => Some(format!("EQ/eq_ratio_{b}")),
            ParameterPath::EqBandDynAttack(b) => Some(format!("EQ/eq_attack_{b}")),
            ParameterPath::EqBandDynRelease(b) => Some(format!("EQ/eq_release_{b}")),
            ParameterPath::EqBandDynOverUnder(b) => Some(format!("EQ/eq_over-under_{b}")),

            // Dynamics 1 (compressor)
            ParameterPath::Dyn1Enabled => Some("Dynamics/comp_in".into()),
            ParameterPath::Dyn1Mode => None, // GP OSC-only; iPad uses comp_knee per band
            ParameterPath::Dyn1MultibandDeesser => Some("Dynamics/comp-multiband-desser".into()),
            ParameterPath::Dyn1Threshold(1) => Some("Dynamics/comp_thresh".into()),
            ParameterPath::Dyn1Threshold(b) => Some(format!("Dynamics/comp_thresh_{b}")),
            ParameterPath::Dyn1Knee(1) => Some("Dynamics/comp_knee".into()),
            ParameterPath::Dyn1Knee(b) => Some(format!("Dynamics/comp_knee_{b}")),
            ParameterPath::Dyn1Ratio(1) => Some("Dynamics/comp_ratio".into()),
            ParameterPath::Dyn1Ratio(b) => Some(format!("Dynamics/comp_ratio_{b}")),
            ParameterPath::Dyn1Attack(1) => Some("Dynamics/comp_attack".into()),
            ParameterPath::Dyn1Attack(b) => Some(format!("Dynamics/comp_attack_{b}")),
            ParameterPath::Dyn1Release(1) => Some("Dynamics/comp_release".into()),
            ParameterPath::Dyn1Release(b) => Some(format!("Dynamics/comp_release_{b}")),
            ParameterPath::Dyn1Gain(1) => Some("Dynamics/comp_gain".into()),
            ParameterPath::Dyn1Gain(b) => Some(format!("Dynamics/comp_auto-gain_{b}")),
            ParameterPath::Dyn1Listen(b) => Some(format!("Dynamics/comp_listen_{b}")),
            ParameterPath::Dyn1CrossoverHigh => Some("Dynamics/comp_HP_crossover_1".into()),
            ParameterPath::Dyn1CrossoverLow => Some("Dynamics/comp_LP_crossover_1".into()),

            // Dynamics 2 (gate)
            ParameterPath::Dyn2Enabled => Some("Dynamics/gate_in".into()),
            ParameterPath::Dyn2Mode => Some("Dynamics/gate-duck-comp".into()),
            ParameterPath::Dyn2Threshold => Some("Dynamics/gate_thresh".into()),
            ParameterPath::Dyn2Attack => Some("Dynamics/gate_attack".into()),
            ParameterPath::Dyn2Hold => Some("Dynamics/gate_hold".into()),
            ParameterPath::Dyn2Release => Some("Dynamics/gate_release".into()),
            ParameterPath::Dyn2Range => Some("Dynamics/gate_range".into()),
            ParameterPath::Dyn2Highpass => Some("Dynamics/gate_hp".into()),
            ParameterPath::Dyn2Lowpass => Some("Dynamics/gate_lp".into()),
            ParameterPath::Dyn2KeySolo => Some("Dynamics/key_solo".into()),
            // Dyn2 params not in iPad protocol
            ParameterPath::Dyn2Knee
            | ParameterPath::Dyn2Ratio
            | ParameterPath::Dyn2Gain
            | ParameterPath::Dyn2Listen => None,

            // Sends
            ParameterPath::SendLevel(s) => Some(format!("Aux_Send/{s}/send_level")),
            ParameterPath::SendPan(s) => Some(format!("Aux_Send/{s}/send_pan")),
            ParameterPath::SendEnabled(s) => Some(format!("Aux_Send/{s}/send_on")),

            // Group routing (iPad-only)
            ParameterPath::GroupSendOn(g) => Some(format!("Group_Send/{g}/send_on")),
            ParameterPath::MasterBusOn => Some("Group_Send/17/send_on".into()),

            // Inserts (iPad-only)
            ParameterPath::InsertAEnabled => Some("Insert/insert_A_in".into()),
            ParameterPath::InsertBEnabled => Some("Insert/insert_B_in".into()),

            // CG membership (iPad-only)
            ParameterPath::CgLevel => Some("CGs_level".into()),
            ParameterPath::CgMute => Some("CGs_mute".into()),

            // Matrix sends (iPad-only, on MatrixInput channels)
            ParameterPath::MatrixSendLevel(s) => Some(format!("Matrix_Send/{s}/send_level")),
            ParameterPath::MatrixSendOn(s) => Some(format!("Matrix_Send/{s}/send_on")),

            // Graphic EQ (iPad-only, on GraphicEq channels)
            ParameterPath::GeqBandGain(b) => Some(format!("geq_gain_{b}")),
            ParameterPath::GeqEnabled => Some("geq_in".into()),
        }
    }

    /// Parse from an iPad protocol path suffix (the remaining path after the channel prefix).
    /// Expects input like "/fader" or "/EQ/eq_gain_2" (with leading /).
    pub fn from_ipad_suffix(suffix: &str) -> Option<Self> {
        let suffix = suffix.strip_prefix('/').unwrap_or(suffix);

        // Direct matches
        match suffix {
            "fader" => return Some(ParameterPath::Fader),
            "mute" => return Some(ParameterPath::Mute),
            "solo" => return Some(ParameterPath::Solo),
            "Panner/pan" => return Some(ParameterPath::Pan),
            "Channel_Input/name" => return Some(ParameterPath::Name),
            "Channel_Input/analog_gain" => return Some(ParameterPath::Gain),
            "Channel_Input/trim" => return Some(ParameterPath::Trim),
            "Channel_Input/phase" => return Some(ParameterPath::Polarity),
            "Channel_Input/phantom" => return Some(ParameterPath::Phantom),
            "Channel_Input/main_alt_in" => return Some(ParameterPath::MainAltIn),
            "Channel_Input/stereo_mode" => return Some(ParameterPath::StereoMode),
            "Channel_Delay/delay_on" => return Some(ParameterPath::DelayEnabled),
            "Channel_Delay/delay" => return Some(ParameterPath::DelayTime),
            "EQ/eq_in" => return Some(ParameterPath::EqEnabled),
            "Filters/lo_filter_in" => return Some(ParameterPath::HighpassEnabled),
            "Filters/lo_filter_freq" => return Some(ParameterPath::HighpassFrequency),
            "Filters/hi_filter_in" => return Some(ParameterPath::LowpassEnabled),
            "Filters/hi_filter_freq" => return Some(ParameterPath::LowpassFrequency),
            "Dynamics/comp_in" => return Some(ParameterPath::Dyn1Enabled),
            "Dynamics/comp-multiband-desser" => return Some(ParameterPath::Dyn1MultibandDeesser),
            "Dynamics/comp_thresh" => return Some(ParameterPath::Dyn1Threshold(1)),
            "Dynamics/comp_knee" => return Some(ParameterPath::Dyn1Knee(1)),
            "Dynamics/comp_ratio" => return Some(ParameterPath::Dyn1Ratio(1)),
            "Dynamics/comp_attack" => return Some(ParameterPath::Dyn1Attack(1)),
            "Dynamics/comp_release" => return Some(ParameterPath::Dyn1Release(1)),
            "Dynamics/comp_gain" => return Some(ParameterPath::Dyn1Gain(1)),
            "Dynamics/comp_HP_crossover_1" => return Some(ParameterPath::Dyn1CrossoverHigh),
            "Dynamics/comp_LP_crossover_1" => return Some(ParameterPath::Dyn1CrossoverLow),
            "Dynamics/gate_in" => return Some(ParameterPath::Dyn2Enabled),
            "Dynamics/gate-duck-comp" => return Some(ParameterPath::Dyn2Mode),
            "Dynamics/gate_thresh" => return Some(ParameterPath::Dyn2Threshold),
            "Dynamics/gate_attack" => return Some(ParameterPath::Dyn2Attack),
            "Dynamics/gate_hold" => return Some(ParameterPath::Dyn2Hold),
            "Dynamics/gate_release" => return Some(ParameterPath::Dyn2Release),
            "Dynamics/gate_range" => return Some(ParameterPath::Dyn2Range),
            "Dynamics/gate_hp" => return Some(ParameterPath::Dyn2Highpass),
            "Dynamics/gate_lp" => return Some(ParameterPath::Dyn2Lowpass),
            "Dynamics/key_solo" => return Some(ParameterPath::Dyn2KeySolo),
            "Insert/insert_A_in" => return Some(ParameterPath::InsertAEnabled),
            "Insert/insert_B_in" => return Some(ParameterPath::InsertBEnabled),
            "CGs_level" => return Some(ParameterPath::CgLevel),
            "CGs_mute" => return Some(ParameterPath::CgMute),
            "geq_in" => return Some(ParameterPath::GeqEnabled),
            _ => {}
        }

        // EQ band parameters: EQ/eq_{param}_{band}
        if let Some(rest) = suffix.strip_prefix("EQ/") {
            return parse_ipad_eq_suffix(rest);
        }

        // Dynamics multiband: Dynamics/comp_{param}_{band}
        if let Some(rest) = suffix.strip_prefix("Dynamics/comp_") {
            return parse_ipad_dyn1_suffix(rest);
        }

        // Sends: Aux_Send/{n}/send_{param}
        if let Some(rest) = suffix.strip_prefix("Aux_Send/") {
            return parse_ipad_send_suffix(rest);
        }

        // Group routing: Group_Send/{n}/send_on
        if let Some(rest) = suffix.strip_prefix("Group_Send/") {
            return parse_ipad_group_send_suffix(rest);
        }

        // Matrix sends: Matrix_Send/{n}/send_{param}
        if let Some(rest) = suffix.strip_prefix("Matrix_Send/") {
            return parse_ipad_matrix_send_suffix(rest);
        }

        // GEQ bands: geq_gain_{band}
        if let Some(rest) = suffix.strip_prefix("geq_gain_") {
            let b: u8 = rest.parse().ok()?;
            if (1..=32).contains(&b) {
                return Some(ParameterPath::GeqBandGain(b));
            }
        }

        None
    }

    /// Parse from a GP OSC path suffix (the part after /channel/{ch}/).
    pub fn from_gp_osc_suffix(suffix: &str) -> Option<Self> {
        // Direct matches first
        match suffix {
            "name" => return Some(ParameterPath::Name),
            "fader" => return Some(ParameterPath::Fader),
            "mute" => return Some(ParameterPath::Mute),
            "solo" => return Some(ParameterPath::Solo),
            "pan" => return Some(ParameterPath::Pan),
            "total/gain" => return Some(ParameterPath::Gain),
            "input/gain_tracking" => return Some(ParameterPath::GainTracking),
            "input/trim" => return Some(ParameterPath::Trim),
            "input/balance" => return Some(ParameterPath::Balance),
            "input/width" => return Some(ParameterPath::Width),
            "input/polarity" => return Some(ParameterPath::Polarity),
            "input/delay/enabled" => return Some(ParameterPath::DelayEnabled),
            "input/delay/time" => return Some(ParameterPath::DelayTime),
            "input/digitube/enabled" => return Some(ParameterPath::DigitubeEnabled),
            "input/digitube/drive" => return Some(ParameterPath::DigitubeDrive),
            "input/digitube/bias" => return Some(ParameterPath::DigitubeBias),
            "eq/enabled" => return Some(ParameterPath::EqEnabled),
            "eq/highpass/enabled" => return Some(ParameterPath::HighpassEnabled),
            "eq/highpass/frequency" => return Some(ParameterPath::HighpassFrequency),
            "eq/lowpass/enabled" => return Some(ParameterPath::LowpassEnabled),
            "eq/lowpass/frequency" => return Some(ParameterPath::LowpassFrequency),
            "dyn1/enabled" => return Some(ParameterPath::Dyn1Enabled),
            "dyn1/mode" => return Some(ParameterPath::Dyn1Mode),
            "dyn1/crossover_high" => return Some(ParameterPath::Dyn1CrossoverHigh),
            "dyn1/crossover_low" => return Some(ParameterPath::Dyn1CrossoverLow),
            "dyn2/enabled" => return Some(ParameterPath::Dyn2Enabled),
            "dyn2/mode" => return Some(ParameterPath::Dyn2Mode),
            "dyn2/threshold" => return Some(ParameterPath::Dyn2Threshold),
            "dyn2/knee" => return Some(ParameterPath::Dyn2Knee),
            "dyn2/ratio" => return Some(ParameterPath::Dyn2Ratio),
            "dyn2/range" => return Some(ParameterPath::Dyn2Range),
            "dyn2/attack" => return Some(ParameterPath::Dyn2Attack),
            "dyn2/hold" => return Some(ParameterPath::Dyn2Hold),
            "dyn2/release" => return Some(ParameterPath::Dyn2Release),
            "dyn2/gain" => return Some(ParameterPath::Dyn2Gain),
            "dyn2/highpass" => return Some(ParameterPath::Dyn2Highpass),
            "dyn2/lowpass" => return Some(ParameterPath::Dyn2Lowpass),
            "dyn2/listen" => return Some(ParameterPath::Dyn2Listen),
            _ => {}
        }

        // Parametric matches: eq/{band}/..., dyn1/{band}/..., send/{send}/...
        let parts: Vec<&str> = suffix.splitn(4, '/').collect();

        match parts.as_slice() {
            // EQ band parameters: eq/{band}/{param}
            ["eq", band, param] => {
                let b: u8 = band.parse().ok()?;
                if !(1..=4).contains(&b) {
                    return None;
                }
                match *param {
                    "frequency" => Some(ParameterPath::EqBandFrequency(b)),
                    "gain" => Some(ParameterPath::EqBandGain(b)),
                    "q" => Some(ParameterPath::EqBandQ(b)),
                    _ => None,
                }
            }
            // EQ band dynamic: eq/{band}/dyn/{param}
            ["eq", band, "dyn", param] => {
                let b: u8 = band.parse().ok()?;
                if !(1..=4).contains(&b) {
                    return None;
                }
                match *param {
                    "enabled" => Some(ParameterPath::EqBandDynEnabled(b)),
                    "threshold" => Some(ParameterPath::EqBandDynThreshold(b)),
                    "ratio" => Some(ParameterPath::EqBandDynRatio(b)),
                    "attack" => Some(ParameterPath::EqBandDynAttack(b)),
                    "release" => Some(ParameterPath::EqBandDynRelease(b)),
                    _ => None,
                }
            }
            // Dyn1 band parameters: dyn1/{band}/{param}
            ["dyn1", band, param] => {
                let b: u8 = band.parse().ok()?;
                if !(1..=3).contains(&b) {
                    return None;
                }
                match *param {
                    "threshold" => Some(ParameterPath::Dyn1Threshold(b)),
                    "knee" => Some(ParameterPath::Dyn1Knee(b)),
                    "ratio" => Some(ParameterPath::Dyn1Ratio(b)),
                    "attack" => Some(ParameterPath::Dyn1Attack(b)),
                    "release" => Some(ParameterPath::Dyn1Release(b)),
                    "gain" => Some(ParameterPath::Dyn1Gain(b)),
                    "listen" => Some(ParameterPath::Dyn1Listen(b)),
                    _ => None,
                }
            }
            // Send parameters: send/{send}/{param}
            ["send", send, param] => {
                let s: u8 = send.parse().ok()?;
                match *param {
                    "enabled" => Some(ParameterPath::SendEnabled(s)),
                    "level" => Some(ParameterPath::SendLevel(s)),
                    "pan" => Some(ParameterPath::SendPan(s)),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

/// Parameter sections for scope control (PRD §4.5).
/// Each section groups related parameters that are captured/recalled together.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParameterSection {
    Name,
    InputGain,
    Delay,
    Digitube,
    Eq,
    Dyn1,
    Dyn2,
    Sends,
    GroupRouting,
    Inserts,
    FaderMutePan,
    CgMembership,
    GraphicEq,
    MatrixSends,
}

impl ParameterSection {
    /// All section variants in display order.
    pub fn all_variants() -> &'static [ParameterSection] {
        &[
            ParameterSection::FaderMutePan,
            ParameterSection::Name,
            ParameterSection::InputGain,
            ParameterSection::Delay,
            ParameterSection::Digitube,
            ParameterSection::Eq,
            ParameterSection::Dyn1,
            ParameterSection::Dyn2,
            ParameterSection::Sends,
            ParameterSection::GroupRouting,
            ParameterSection::Inserts,
            ParameterSection::CgMembership,
            ParameterSection::GraphicEq,
            ParameterSection::MatrixSends,
        ]
    }

    /// Which sections are applicable to a given channel type.
    pub fn applicable_to(channel: &ChannelId) -> Vec<ParameterSection> {
        match channel {
            ChannelId::Input(_) => vec![
                ParameterSection::FaderMutePan,
                ParameterSection::Name,
                ParameterSection::InputGain,
                ParameterSection::Delay,
                ParameterSection::Digitube,
                ParameterSection::Eq,
                ParameterSection::Dyn1,
                ParameterSection::Dyn2,
                ParameterSection::Sends,
                ParameterSection::GroupRouting,
                ParameterSection::Inserts,
                ParameterSection::CgMembership,
            ],
            ChannelId::Aux(_) => vec![
                ParameterSection::FaderMutePan,
                ParameterSection::Name,
                ParameterSection::Eq,
                ParameterSection::Dyn1,
                ParameterSection::Dyn2,
                ParameterSection::Inserts,
            ],
            ChannelId::Group(_) => vec![
                ParameterSection::FaderMutePan,
                ParameterSection::Name,
                ParameterSection::Eq,
                ParameterSection::Dyn1,
                ParameterSection::Dyn2,
                ParameterSection::Inserts,
            ],
            ChannelId::Matrix(_) => vec![
                ParameterSection::FaderMutePan,
                ParameterSection::Name,
                ParameterSection::Eq,
                ParameterSection::Dyn1,
                ParameterSection::Dyn2,
            ],
            ChannelId::ControlGroup(_) => vec![
                ParameterSection::FaderMutePan,
                ParameterSection::Name,
            ],
            ChannelId::GraphicEq(_) => vec![
                ParameterSection::GraphicEq,
            ],
            ChannelId::MatrixInput(_) => vec![
                ParameterSection::MatrixSends,
            ],
        }
    }
}

impl fmt::Display for ParameterSection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParameterSection::Name => write!(f, "Name"),
            ParameterSection::InputGain => write!(f, "Input Gain"),
            ParameterSection::Delay => write!(f, "Delay"),
            ParameterSection::Digitube => write!(f, "Digitube"),
            ParameterSection::Eq => write!(f, "EQ"),
            ParameterSection::Dyn1 => write!(f, "Dynamics 1"),
            ParameterSection::Dyn2 => write!(f, "Dynamics 2"),
            ParameterSection::Sends => write!(f, "Sends"),
            ParameterSection::GroupRouting => write!(f, "Group Routing"),
            ParameterSection::Inserts => write!(f, "Inserts"),
            ParameterSection::FaderMutePan => write!(f, "Fader/Mute/Pan"),
            ParameterSection::CgMembership => write!(f, "CG Membership"),
            ParameterSection::GraphicEq => write!(f, "Graphic EQ"),
            ParameterSection::MatrixSends => write!(f, "Matrix Sends"),
        }
    }
}

impl ParameterPath {
    /// Classify this parameter into its section.
    pub fn section(&self) -> ParameterSection {
        match self {
            ParameterPath::Name => ParameterSection::Name,

            ParameterPath::Fader
            | ParameterPath::Mute
            | ParameterPath::Pan => ParameterSection::FaderMutePan,

            ParameterPath::Solo => ParameterSection::FaderMutePan,

            ParameterPath::Gain
            | ParameterPath::GainTracking
            | ParameterPath::Trim
            | ParameterPath::Balance
            | ParameterPath::Width
            | ParameterPath::Polarity
            | ParameterPath::Phantom
            | ParameterPath::MainAltIn
            | ParameterPath::StereoMode => ParameterSection::InputGain,

            ParameterPath::DelayEnabled
            | ParameterPath::DelayTime => ParameterSection::Delay,

            ParameterPath::DigitubeEnabled
            | ParameterPath::DigitubeDrive
            | ParameterPath::DigitubeBias => ParameterSection::Digitube,

            ParameterPath::EqEnabled
            | ParameterPath::HighpassEnabled
            | ParameterPath::HighpassFrequency
            | ParameterPath::LowpassEnabled
            | ParameterPath::LowpassFrequency
            | ParameterPath::EqBandFrequency(_)
            | ParameterPath::EqBandGain(_)
            | ParameterPath::EqBandQ(_)
            | ParameterPath::EqBandCurve(_)
            | ParameterPath::EqBandDynEnabled(_)
            | ParameterPath::EqBandDynThreshold(_)
            | ParameterPath::EqBandDynRatio(_)
            | ParameterPath::EqBandDynAttack(_)
            | ParameterPath::EqBandDynRelease(_)
            | ParameterPath::EqBandDynOverUnder(_) => ParameterSection::Eq,

            ParameterPath::Dyn1Enabled
            | ParameterPath::Dyn1Mode
            | ParameterPath::Dyn1MultibandDeesser
            | ParameterPath::Dyn1Threshold(_)
            | ParameterPath::Dyn1Knee(_)
            | ParameterPath::Dyn1Ratio(_)
            | ParameterPath::Dyn1Attack(_)
            | ParameterPath::Dyn1Release(_)
            | ParameterPath::Dyn1Gain(_)
            | ParameterPath::Dyn1Listen(_)
            | ParameterPath::Dyn1CrossoverHigh
            | ParameterPath::Dyn1CrossoverLow => ParameterSection::Dyn1,

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
            | ParameterPath::Dyn2KeySolo => ParameterSection::Dyn2,

            ParameterPath::SendEnabled(_)
            | ParameterPath::SendLevel(_)
            | ParameterPath::SendPan(_) => ParameterSection::Sends,

            ParameterPath::GroupSendOn(_)
            | ParameterPath::MasterBusOn => ParameterSection::GroupRouting,

            ParameterPath::InsertAEnabled
            | ParameterPath::InsertBEnabled => ParameterSection::Inserts,

            ParameterPath::CgLevel
            | ParameterPath::CgMute => ParameterSection::CgMembership,

            ParameterPath::MatrixSendLevel(_)
            | ParameterPath::MatrixSendOn(_) => ParameterSection::MatrixSends,

            ParameterPath::GeqBandGain(_)
            | ParameterPath::GeqEnabled => ParameterSection::GraphicEq,
        }
    }

    /// Whether this parameter represents a continuous value suitable for
    /// interpolation (fader levels, frequencies, gains, pan, thresholds, etc.).
    /// Discrete parameters (mute, solo, enables, modes, names) return false.
    pub fn is_continuous(&self) -> bool {
        match self {
            // Output
            ParameterPath::Fader | ParameterPath::Pan => true,

            // Input continuous
            ParameterPath::Gain
            | ParameterPath::Trim
            | ParameterPath::Balance
            | ParameterPath::Width => true,

            // Delay
            ParameterPath::DelayTime => true,

            // Digitube
            ParameterPath::DigitubeDrive | ParameterPath::DigitubeBias => true,

            // EQ continuous
            ParameterPath::HighpassFrequency
            | ParameterPath::LowpassFrequency
            | ParameterPath::EqBandFrequency(_)
            | ParameterPath::EqBandGain(_)
            | ParameterPath::EqBandQ(_)
            | ParameterPath::EqBandDynThreshold(_)
            | ParameterPath::EqBandDynRatio(_)
            | ParameterPath::EqBandDynAttack(_)
            | ParameterPath::EqBandDynRelease(_) => true,

            // Dynamics 1 continuous
            ParameterPath::Dyn1Threshold(_)
            | ParameterPath::Dyn1Knee(_)
            | ParameterPath::Dyn1Ratio(_)
            | ParameterPath::Dyn1Attack(_)
            | ParameterPath::Dyn1Release(_)
            | ParameterPath::Dyn1Gain(_)
            | ParameterPath::Dyn1CrossoverHigh
            | ParameterPath::Dyn1CrossoverLow => true,

            // Dynamics 2 continuous
            ParameterPath::Dyn2Threshold
            | ParameterPath::Dyn2Knee
            | ParameterPath::Dyn2Ratio
            | ParameterPath::Dyn2Range
            | ParameterPath::Dyn2Attack
            | ParameterPath::Dyn2Hold
            | ParameterPath::Dyn2Release
            | ParameterPath::Dyn2Gain
            | ParameterPath::Dyn2Highpass
            | ParameterPath::Dyn2Lowpass => true,

            // Sends continuous
            ParameterPath::SendLevel(_) | ParameterPath::SendPan(_) => true,

            // CG level
            ParameterPath::CgLevel => true,

            // Matrix sends continuous
            ParameterPath::MatrixSendLevel(_) => true,

            // Graphic EQ band gains
            ParameterPath::GeqBandGain(_) => true,

            // Everything else is discrete
            _ => false,
        }
    }
}

// ── iPad suffix parsing helpers ──────────────────────────────────────

/// Parse iPad EQ suffix (after "EQ/").
fn parse_ipad_eq_suffix(rest: &str) -> Option<ParameterPath> {
    // Try patterns: eq_freq_{b}, eq_gain_{b}, eq_Q_{b}, eq_curve_{b},
    // dynamic_eq_on_{b}, eq_thresh_{b}, eq_over-under_{b}, eq_ratio_{b},
    // eq_attack_{b}, eq_release_{b}
    if let Some(b_str) = rest.strip_prefix("eq_freq_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandFrequency(b));
    }
    if let Some(b_str) = rest.strip_prefix("eq_gain_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandGain(b));
    }
    if let Some(b_str) = rest.strip_prefix("eq_Q_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandQ(b));
    }
    if let Some(b_str) = rest.strip_prefix("eq_curve_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandCurve(b));
    }
    if let Some(b_str) = rest.strip_prefix("dynamic_eq_on_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandDynEnabled(b));
    }
    if let Some(b_str) = rest.strip_prefix("eq_thresh_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandDynThreshold(b));
    }
    if let Some(b_str) = rest.strip_prefix("eq_over-under_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandDynOverUnder(b));
    }
    if let Some(b_str) = rest.strip_prefix("eq_ratio_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandDynRatio(b));
    }
    if let Some(b_str) = rest.strip_prefix("eq_attack_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandDynAttack(b));
    }
    if let Some(b_str) = rest.strip_prefix("eq_release_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandDynRelease(b));
    }
    None
}

/// Parse iPad Dyn1 multiband suffix (after "Dynamics/comp_").
fn parse_ipad_dyn1_suffix(rest: &str) -> Option<ParameterPath> {
    // Multiband bands: comp_thresh_{b}, comp_knee_{b}, comp_ratio_{b}, etc.
    if let Some(b_str) = rest.strip_prefix("thresh_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1Threshold(b));
    }
    if let Some(b_str) = rest.strip_prefix("knee_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1Knee(b));
    }
    if let Some(b_str) = rest.strip_prefix("ratio_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1Ratio(b));
    }
    if let Some(b_str) = rest.strip_prefix("attack_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1Attack(b));
    }
    if let Some(b_str) = rest.strip_prefix("release_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1Release(b));
    }
    if let Some(b_str) = rest.strip_prefix("auto-gain_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1Gain(b));
    }
    if let Some(b_str) = rest.strip_prefix("listen_") {
        let b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1Listen(b));
    }
    if let Some(b_str) = rest.strip_prefix("HP_crossover_") {
        let _b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1CrossoverHigh);
    }
    if let Some(b_str) = rest.strip_prefix("LP_crossover_") {
        let _b: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1CrossoverLow);
    }
    None
}

/// Parse iPad send suffix (after "Aux_Send/").
fn parse_ipad_send_suffix(rest: &str) -> Option<ParameterPath> {
    // Format: {n}/send_level, {n}/send_pan, {n}/send_on
    let (n_str, param) = rest.split_once('/')?;
    let n: u8 = n_str.parse().ok()?;
    match param {
        "send_level" => Some(ParameterPath::SendLevel(n)),
        "send_pan" => Some(ParameterPath::SendPan(n)),
        "send_on" => Some(ParameterPath::SendEnabled(n)),
        _ => None,
    }
}

/// Parse iPad group send suffix (after "Group_Send/").
fn parse_ipad_group_send_suffix(rest: &str) -> Option<ParameterPath> {
    // Format: {n}/send_on  (17 = master bus)
    let (n_str, param) = rest.split_once('/')?;
    let n: u8 = n_str.parse().ok()?;
    if param != "send_on" {
        return None;
    }
    if n == 17 {
        Some(ParameterPath::MasterBusOn)
    } else {
        Some(ParameterPath::GroupSendOn(n))
    }
}

/// Parse iPad matrix send suffix (after "Matrix_Send/").
fn parse_ipad_matrix_send_suffix(rest: &str) -> Option<ParameterPath> {
    // Format: {n}/send_level, {n}/send_on
    let (n_str, param) = rest.split_once('/')?;
    let n: u8 = n_str.parse().ok()?;
    match param {
        "send_level" => Some(ParameterPath::MatrixSendLevel(n)),
        "send_on" => Some(ParameterPath::MatrixSendOn(n)),
        _ => None,
    }
}

/// Typed parameter value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ParameterValue {
    Float(f32),
    Int(i32),
    Bool(bool),
    String(String),
}

impl ParameterValue {
    /// Linearly interpolate between self and target at position t (0.0..=1.0).
    /// Returns None if types don't match or interpolation is not meaningful.
    pub fn lerp(&self, target: &ParameterValue, t: f32) -> Option<ParameterValue> {
        match (self, target) {
            (ParameterValue::Float(a), ParameterValue::Float(b)) => {
                Some(ParameterValue::Float(a + (b - a) * t))
            }
            (ParameterValue::Int(a), ParameterValue::Int(b)) => {
                let fa = *a as f32;
                let fb = *b as f32;
                Some(ParameterValue::Int((fa + (fb - fa) * t).round() as i32))
            }
            _ => None,
        }
    }
}

impl fmt::Display for ParameterValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParameterValue::Float(v) => write!(f, "{v}"),
            ParameterValue::Int(v) => write!(f, "{v}"),
            ParameterValue::Bool(v) => write!(f, "{v}"),
            ParameterValue::String(v) => write!(f, "\"{v}\""),
        }
    }
}

impl fmt::Display for ParameterAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{:?}", self.channel, self.parameter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gp_osc_suffix_round_trip() {
        let paths = vec![
            ParameterPath::Fader,
            ParameterPath::Mute,
            ParameterPath::Solo,
            ParameterPath::Pan,
            ParameterPath::Name,
            ParameterPath::Gain,
            ParameterPath::Trim,
            ParameterPath::EqEnabled,
            ParameterPath::EqBandFrequency(1),
            ParameterPath::EqBandGain(2),
            ParameterPath::EqBandDynEnabled(3),
            ParameterPath::Dyn1Enabled,
            ParameterPath::Dyn1Threshold(2),
            ParameterPath::Dyn2Threshold,
            ParameterPath::SendLevel(3),
            ParameterPath::SendEnabled(1),
        ];

        for path in paths {
            let suffix = path.to_gp_osc_suffix().unwrap();
            let parsed = ParameterPath::from_gp_osc_suffix(&suffix).unwrap();
            assert_eq!(parsed, path, "Round-trip failed for suffix: {suffix}");
        }
    }

    #[test]
    fn section_classification() {
        assert_eq!(ParameterPath::Name.section(), ParameterSection::Name);
        assert_eq!(ParameterPath::Fader.section(), ParameterSection::FaderMutePan);
        assert_eq!(ParameterPath::Mute.section(), ParameterSection::FaderMutePan);
        assert_eq!(ParameterPath::Pan.section(), ParameterSection::FaderMutePan);
        assert_eq!(ParameterPath::Gain.section(), ParameterSection::InputGain);
        assert_eq!(ParameterPath::Trim.section(), ParameterSection::InputGain);
        assert_eq!(ParameterPath::Phantom.section(), ParameterSection::InputGain);
        assert_eq!(ParameterPath::DelayEnabled.section(), ParameterSection::Delay);
        assert_eq!(ParameterPath::DigitubeEnabled.section(), ParameterSection::Digitube);
        assert_eq!(ParameterPath::EqEnabled.section(), ParameterSection::Eq);
        assert_eq!(ParameterPath::EqBandFrequency(1).section(), ParameterSection::Eq);
        assert_eq!(ParameterPath::EqBandDynEnabled(2).section(), ParameterSection::Eq);
        assert_eq!(ParameterPath::HighpassFrequency.section(), ParameterSection::Eq);
        assert_eq!(ParameterPath::Dyn1Enabled.section(), ParameterSection::Dyn1);
        assert_eq!(ParameterPath::Dyn1Threshold(1).section(), ParameterSection::Dyn1);
        assert_eq!(ParameterPath::Dyn2Enabled.section(), ParameterSection::Dyn2);
        assert_eq!(ParameterPath::Dyn2Range.section(), ParameterSection::Dyn2);
        assert_eq!(ParameterPath::SendLevel(1).section(), ParameterSection::Sends);
        assert_eq!(ParameterPath::GroupSendOn(1).section(), ParameterSection::GroupRouting);
        assert_eq!(ParameterPath::MasterBusOn.section(), ParameterSection::GroupRouting);
        assert_eq!(ParameterPath::InsertAEnabled.section(), ParameterSection::Inserts);
        assert_eq!(ParameterPath::CgLevel.section(), ParameterSection::CgMembership);
        assert_eq!(ParameterPath::MatrixSendLevel(1).section(), ParameterSection::MatrixSends);
        assert_eq!(ParameterPath::GeqBandGain(1).section(), ParameterSection::GraphicEq);
        assert_eq!(ParameterPath::GeqEnabled.section(), ParameterSection::GraphicEq);
    }

    #[test]
    fn ipad_only_returns_none() {
        assert!(ParameterPath::Phantom.to_gp_osc_suffix().is_none());
        assert!(ParameterPath::CgLevel.to_gp_osc_suffix().is_none());
        assert!(ParameterPath::InsertAEnabled.to_gp_osc_suffix().is_none());
        assert!(ParameterPath::GeqBandGain(1).to_gp_osc_suffix().is_none());
    }

    #[test]
    fn ipad_suffix_round_trip() {
        let paths = vec![
            // Common params (both protocols)
            ParameterPath::Fader,
            ParameterPath::Mute,
            ParameterPath::Solo,
            ParameterPath::Pan,
            ParameterPath::Name,
            ParameterPath::Gain,
            ParameterPath::Trim,
            ParameterPath::Polarity,
            ParameterPath::DelayEnabled,
            ParameterPath::DelayTime,
            // EQ
            ParameterPath::EqEnabled,
            ParameterPath::HighpassEnabled,
            ParameterPath::HighpassFrequency,
            ParameterPath::LowpassEnabled,
            ParameterPath::LowpassFrequency,
            ParameterPath::EqBandFrequency(1),
            ParameterPath::EqBandGain(2),
            ParameterPath::EqBandQ(3),
            ParameterPath::EqBandDynEnabled(1),
            ParameterPath::EqBandDynThreshold(2),
            ParameterPath::EqBandDynRatio(3),
            ParameterPath::EqBandDynAttack(4),
            ParameterPath::EqBandDynRelease(1),
            // iPad-only EQ
            ParameterPath::EqBandCurve(2),
            ParameterPath::EqBandDynOverUnder(3),
            // Dyn1
            ParameterPath::Dyn1Enabled,
            ParameterPath::Dyn1Threshold(1), // single comp
            ParameterPath::Dyn1Threshold(2), // multiband
            ParameterPath::Dyn1Knee(1),
            ParameterPath::Dyn1Ratio(3),
            ParameterPath::Dyn1Attack(1),
            ParameterPath::Dyn1Release(2),
            ParameterPath::Dyn1Gain(1), // single
            ParameterPath::Dyn1Gain(2), // multiband
            ParameterPath::Dyn1Listen(1),
            ParameterPath::Dyn1CrossoverHigh,
            ParameterPath::Dyn1CrossoverLow,
            // Dyn2
            ParameterPath::Dyn2Enabled,
            ParameterPath::Dyn2Mode,
            ParameterPath::Dyn2Threshold,
            ParameterPath::Dyn2Attack,
            ParameterPath::Dyn2Hold,
            ParameterPath::Dyn2Release,
            ParameterPath::Dyn2Range,
            ParameterPath::Dyn2Highpass,
            ParameterPath::Dyn2Lowpass,
            ParameterPath::Dyn2KeySolo,
            // Sends
            ParameterPath::SendLevel(3),
            ParameterPath::SendPan(1),
            ParameterPath::SendEnabled(5),
            // iPad-only
            ParameterPath::Phantom,
            ParameterPath::MainAltIn,
            ParameterPath::StereoMode,
            ParameterPath::Dyn1MultibandDeesser,
            ParameterPath::GroupSendOn(4),
            ParameterPath::MasterBusOn,
            ParameterPath::InsertAEnabled,
            ParameterPath::InsertBEnabled,
            ParameterPath::CgLevel,
            ParameterPath::CgMute,
            ParameterPath::MatrixSendLevel(2),
            ParameterPath::MatrixSendOn(5),
            ParameterPath::GeqBandGain(16),
            ParameterPath::GeqEnabled,
        ];

        for path in paths {
            let suffix = path.to_ipad_suffix()
                .unwrap_or_else(|| panic!("to_ipad_suffix returned None for {path:?}"));
            // from_ipad_suffix expects leading /
            let parsed = ParameterPath::from_ipad_suffix(&format!("/{suffix}"))
                .unwrap_or_else(|| panic!("from_ipad_suffix failed for /{suffix} (from {path:?})"));
            assert_eq!(parsed, path, "iPad round-trip failed for suffix: {suffix}");
        }
    }

    #[test]
    fn ipad_suffix_gp_only_returns_none() {
        // These params exist only in GP OSC, not iPad
        assert!(ParameterPath::GainTracking.to_ipad_suffix().is_none());
        assert!(ParameterPath::Balance.to_ipad_suffix().is_none());
        assert!(ParameterPath::Width.to_ipad_suffix().is_none());
        assert!(ParameterPath::DigitubeEnabled.to_ipad_suffix().is_none());
        assert!(ParameterPath::DigitubeDrive.to_ipad_suffix().is_none());
        assert!(ParameterPath::DigitubeBias.to_ipad_suffix().is_none());
        assert!(ParameterPath::Dyn2Knee.to_ipad_suffix().is_none());
        assert!(ParameterPath::Dyn2Ratio.to_ipad_suffix().is_none());
    }

    #[test]
    fn ipad_suffix_specific_values() {
        assert_eq!(ParameterPath::Fader.to_ipad_suffix().unwrap(), "fader");
        assert_eq!(ParameterPath::Pan.to_ipad_suffix().unwrap(), "Panner/pan");
        assert_eq!(ParameterPath::InsertAEnabled.to_ipad_suffix().unwrap(), "Insert/insert_A_in");
        assert_eq!(ParameterPath::GeqBandGain(1).to_ipad_suffix().unwrap(), "geq_gain_1");
        assert_eq!(ParameterPath::SendLevel(3).to_ipad_suffix().unwrap(), "Aux_Send/3/send_level");
        assert_eq!(ParameterPath::GroupSendOn(4).to_ipad_suffix().unwrap(), "Group_Send/4/send_on");
        assert_eq!(ParameterPath::MasterBusOn.to_ipad_suffix().unwrap(), "Group_Send/17/send_on");
    }

    #[test]
    fn is_continuous_true_for_levels_and_gains() {
        assert!(ParameterPath::Fader.is_continuous());
        assert!(ParameterPath::Pan.is_continuous());
        assert!(ParameterPath::Gain.is_continuous());
        assert!(ParameterPath::Trim.is_continuous());
        assert!(ParameterPath::SendLevel(1).is_continuous());
        assert!(ParameterPath::SendPan(2).is_continuous());
        assert!(ParameterPath::CgLevel.is_continuous());
        assert!(ParameterPath::MatrixSendLevel(1).is_continuous());
        assert!(ParameterPath::GeqBandGain(5).is_continuous());
    }

    #[test]
    fn is_continuous_true_for_eq_and_dynamics() {
        assert!(ParameterPath::HighpassFrequency.is_continuous());
        assert!(ParameterPath::LowpassFrequency.is_continuous());
        assert!(ParameterPath::EqBandFrequency(1).is_continuous());
        assert!(ParameterPath::EqBandGain(2).is_continuous());
        assert!(ParameterPath::EqBandQ(3).is_continuous());
        assert!(ParameterPath::Dyn1Threshold(1).is_continuous());
        assert!(ParameterPath::Dyn1Ratio(2).is_continuous());
        assert!(ParameterPath::Dyn2Threshold.is_continuous());
        assert!(ParameterPath::Dyn2Attack.is_continuous());
        assert!(ParameterPath::Dyn2Range.is_continuous());
        assert!(ParameterPath::DelayTime.is_continuous());
    }

    #[test]
    fn is_continuous_false_for_discrete() {
        assert!(!ParameterPath::Name.is_continuous());
        assert!(!ParameterPath::Mute.is_continuous());
        assert!(!ParameterPath::Solo.is_continuous());
        assert!(!ParameterPath::Polarity.is_continuous());
        assert!(!ParameterPath::Phantom.is_continuous());
        assert!(!ParameterPath::EqEnabled.is_continuous());
        assert!(!ParameterPath::DelayEnabled.is_continuous());
        assert!(!ParameterPath::Dyn1Enabled.is_continuous());
        assert!(!ParameterPath::Dyn1Mode.is_continuous());
        assert!(!ParameterPath::Dyn2Enabled.is_continuous());
        assert!(!ParameterPath::SendEnabled(1).is_continuous());
        assert!(!ParameterPath::GroupSendOn(1).is_continuous());
        assert!(!ParameterPath::MasterBusOn.is_continuous());
        assert!(!ParameterPath::InsertAEnabled.is_continuous());
        assert!(!ParameterPath::CgMute.is_continuous());
        assert!(!ParameterPath::MatrixSendOn(1).is_continuous());
        assert!(!ParameterPath::GeqEnabled.is_continuous());
        assert!(!ParameterPath::EqBandCurve(1).is_continuous());
        assert!(!ParameterPath::GainTracking.is_continuous());
    }

    #[test]
    fn lerp_float() {
        let a = ParameterValue::Float(0.0);
        let b = ParameterValue::Float(10.0);
        assert_eq!(a.lerp(&b, 0.0), Some(ParameterValue::Float(0.0)));
        assert_eq!(a.lerp(&b, 0.5), Some(ParameterValue::Float(5.0)));
        assert_eq!(a.lerp(&b, 1.0), Some(ParameterValue::Float(10.0)));
    }

    #[test]
    fn lerp_int() {
        let a = ParameterValue::Int(0);
        let b = ParameterValue::Int(100);
        assert_eq!(a.lerp(&b, 0.0), Some(ParameterValue::Int(0)));
        assert_eq!(a.lerp(&b, 0.5), Some(ParameterValue::Int(50)));
        assert_eq!(a.lerp(&b, 1.0), Some(ParameterValue::Int(100)));
    }

    #[test]
    fn lerp_mismatched_types() {
        let f = ParameterValue::Float(1.0);
        let i = ParameterValue::Int(2);
        assert_eq!(f.lerp(&i, 0.5), None);
    }

    #[test]
    fn lerp_bool_returns_none() {
        let a = ParameterValue::Bool(false);
        let b = ParameterValue::Bool(true);
        assert_eq!(a.lerp(&b, 0.5), None);
    }

    #[test]
    fn lerp_string_returns_none() {
        let a = ParameterValue::String("foo".into());
        let b = ParameterValue::String("bar".into());
        assert_eq!(a.lerp(&b, 0.5), None);
    }
}
