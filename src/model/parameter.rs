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
}

/// Typed parameter value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ParameterValue {
    Float(f32),
    Int(i32),
    Bool(bool),
    String(String),
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
}
