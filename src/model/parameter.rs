use serde::{Deserialize, Serialize};
use std::fmt;

use super::channel::ChannelId;
use super::family::{ConsoleFamily, ConsoleProfile, PadQuirks, ParamSupport};

/// Fader dB at the very bottom of the track: −150 dB reads as −inf (fully off).
/// The console owns the taper; the proxy only ever passes raw dB.
pub const FADER_INF_DB: f32 = -150.0;

/// Below this level a fader is inaudible and the physical track compresses a
/// huge dB range into a sliver. Gang propagation treats everything below this
/// as a single point (−inf) so a tiny nudge of a parked fader can't slam a
/// large dB delta onto an audible sibling. See `gang_engine`.
pub const FADER_GANG_FLOOR_DB: f32 = -60.0;

/// Floor used when FADING a fader-family level to/from −inf. A naive
/// linear-in-dB fade from `FADER_INF_DB` (−150) spends almost its whole
/// duration below ~−80 dB (inaudible), cramming the audible swell into a sliver
/// so it reads as a jump. Interpolating in "floored" space — endpoints clamped
/// up to this floor, with sub-floor results reported as −inf — spreads the
/// audible portion across the whole fade. Distinct from `FADER_GANG_FLOOR_DB`
/// (−60), which governs gang propagation rather than fades. See `floored_db_lerp`.
pub const FADER_FADE_FLOOR_DB: f32 = -80.0;

/// A specific parameter on a specific channel.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ParameterAddress {
    pub channel: ChannelId,
    pub parameter: ParameterPath,
}

/// Parameter within a channel, organized by section.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ParameterPath {
    // Output
    Name,
    Fader,
    Mute,
    Solo,
    Pan,

    // Input section
    AnalogGain,
    /// Post-fader + CG sum (-20..+60 dB). GP OSC only; no iPad path.
    /// `serde(alias = "Gain")` migrates pre-Phase-6 snapshot files —
    /// the old `Gain` variant always meant GP OSC `total/gain`.
    #[serde(alias = "Gain")]
    TotalGain,
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
    EqBandCurve(u8), // iPad protocol only
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

    // Sends (input channels only). Send/bus numbers are `u16` like channel
    // numbers (sized for the largest console family); EQ/dyn band indices
    // below stay `u8` — they're processing-structure indices, not
    // console-size-dependent.
    SendEnabled(u16), // send/aux number
    SendLevel(u16),
    SendPan(u16),

    // Group routing (iPad protocol only)
    GroupSendOn(u16),
    MasterBusOn,

    // Inserts (iPad protocol only)
    InsertAEnabled,
    InsertBEnabled,

    // CG membership (iPad protocol only)
    CgLevel,
    CgMute,

    // Matrix sends (MatrixInput channels, iPad protocol only)
    MatrixSendLevel(u16),
    MatrixSendOn(u16),

    // Graphic EQ (GraphicEq channels only, iPad protocol only)
    GeqBandGain(u8), // band 1–32
    GeqEnabled,
}

impl ParameterPath {
    /// Level parameters whose value is set by a motorized fader — directly
    /// (the channel/bus `Fader`) or via sends-on-faders (`SendLevel`,
    /// `MatrixSendLevel`, `CgLevel`). Motorized faders don't always settle to a
    /// bit-exact dB after a recall or a console layer change, so the
    /// auto-preselect dirty screening applies a dB deadband to these (see
    /// `connection::is_meaningful_change`). Encoder-driven levels (gain, EQ,
    /// trim, dynamics) are excluded — they return precisely and keep exact
    /// change detection.
    pub fn is_fader_level(&self) -> bool {
        matches!(
            self,
            ParameterPath::Fader
                | ParameterPath::SendLevel(_)
                | ParameterPath::MatrixSendLevel(_)
                | ParameterPath::CgLevel
        )
    }

    /// dB-taper level parameters that should fade in "floored" space (see
    /// [`FADER_FADE_FLOOR_DB`] / [`floored_db_lerp`]): the main fader and the
    /// fader-driven send/matrix/CG levels. Returns the floor for those; `None`
    /// for everything else (EQ gain/freq/Q, dynamics, pan, …), which keep naive
    /// linear interpolation. Same variant set as [`is_fader_level`] but kept a
    /// separate method so fade behavior isn't coupled to the dirty-screen
    /// deadband should either set ever diverge.
    pub fn fade_floor_db(&self) -> Option<f32> {
        matches!(
            self,
            ParameterPath::Fader
                | ParameterPath::SendLevel(_)
                | ParameterPath::MatrixSendLevel(_)
                | ParameterPath::CgLevel
        )
        .then_some(FADER_FADE_FLOOR_DB)
    }

    /// Convert to GP OSC path suffix (after /channel/{ch}/).
    /// Returns None for iPad-only parameters.
    pub fn to_gp_osc_suffix(&self) -> Option<String> {
        match self {
            ParameterPath::Name => Some("name".into()),
            ParameterPath::Fader => Some("fader".into()),
            ParameterPath::Mute => Some("mute".into()),
            ParameterPath::Solo => Some("solo".into()),
            ParameterPath::Pan => Some("pan".into()),
            ParameterPath::TotalGain => Some("total/gain".into()),
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
            // EQ bands: internal model is 1-based, wire format is 0-based.
            ParameterPath::EqBandFrequency(b) => Some(format!("eq/{}/frequency", b - 1)),
            ParameterPath::EqBandGain(b) => Some(format!("eq/{}/gain", b - 1)),
            ParameterPath::EqBandQ(b) => Some(format!("eq/{}/q", b - 1)),
            ParameterPath::EqBandDynEnabled(b) => Some(format!("eq/{}/dyn/enabled", b - 1)),
            ParameterPath::EqBandDynThreshold(b) => Some(format!("eq/{}/dyn/threshold", b - 1)),
            ParameterPath::EqBandDynRatio(b) => Some(format!("eq/{}/dyn/ratio", b - 1)),
            ParameterPath::EqBandDynAttack(b) => Some(format!("eq/{}/dyn/attack", b - 1)),
            ParameterPath::EqBandDynRelease(b) => Some(format!("eq/{}/dyn/release", b - 1)),
            ParameterPath::Dyn1Enabled => Some("dyn1/enabled".into()),
            ParameterPath::Dyn1Mode => Some("dyn1/mode".into()),
            // Dyn1 (multiband compressor) bands: internal 1-based, wire 0-based.
            ParameterPath::Dyn1Threshold(b) => Some(format!("dyn1/{}/threshold", b - 1)),
            ParameterPath::Dyn1Knee(b) => Some(format!("dyn1/{}/knee", b - 1)),
            ParameterPath::Dyn1Ratio(b) => Some(format!("dyn1/{}/ratio", b - 1)),
            ParameterPath::Dyn1Attack(b) => Some(format!("dyn1/{}/attack", b - 1)),
            ParameterPath::Dyn1Release(b) => Some(format!("dyn1/{}/release", b - 1)),
            ParameterPath::Dyn1Gain(b) => Some(format!("dyn1/{}/gain", b - 1)),
            ParameterPath::Dyn1Listen(b) => Some(format!("dyn1/{}/listen", b - 1)),
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

    /// Convert to a Pad-protocol path suffix (after /{ChannelType}/{number}/)
    /// under the given wire quirks.
    ///
    /// Returns None for parameters with no Pad representation (GP OSC-only).
    /// Use [`Self::to_ipad_suffix`] for the S21 quirks.
    pub fn to_pad_suffix(&self, q: &PadQuirks) -> Option<String> {
        match self {
            // Output
            ParameterPath::Name => Some("Channel_Input/name".into()),
            ParameterPath::Fader => Some("fader".into()),
            ParameterPath::Mute => Some("mute".into()),
            ParameterPath::Solo => Some("solo".into()),
            ParameterPath::Pan => Some("Panner/pan".into()),

            // Input section
            ParameterPath::AnalogGain => Some("Channel_Input/analog_gain".into()),
            ParameterPath::Trim => Some("Channel_Input/trim".into()),
            ParameterPath::Polarity => Some("Channel_Input/phase".into()),
            ParameterPath::Phantom => Some("Channel_Input/phantom".into()),
            ParameterPath::MainAltIn => Some("Channel_Input/main_alt_in".into()),
            ParameterPath::StereoMode => Some("Channel_Input/stereo_mode".into()),

            // GP OSC-only input params
            ParameterPath::TotalGain
            | ParameterPath::GainTracking
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
            // S21 firmware numbers the EQ bands in reverse (internal b ↔ wire
            // 5-b) — see `pad_eq_band_map`. Families without the quirk encode
            // the band index unchanged.
            ParameterPath::EqBandFrequency(b) => Some(format!(
                "EQ/eq_freq_{}",
                pad_eq_band_map(*b, q.eq_bands_reversed)?
            )),
            ParameterPath::EqBandGain(b) => Some(format!(
                "EQ/eq_gain_{}",
                pad_eq_band_map(*b, q.eq_bands_reversed)?
            )),
            ParameterPath::EqBandQ(b) => Some(format!(
                "EQ/eq_Q_{}",
                pad_eq_band_map(*b, q.eq_bands_reversed)?
            )),
            ParameterPath::EqBandCurve(b) => Some(format!(
                "EQ/eq_curve_{}",
                pad_eq_band_map(*b, q.eq_bands_reversed)?
            )),
            ParameterPath::EqBandDynEnabled(b) => Some(format!(
                "EQ/dynamic_eq_on_{}",
                pad_eq_band_map(*b, q.eq_bands_reversed)?
            )),
            ParameterPath::EqBandDynThreshold(b) => Some(format!(
                "EQ/eq_thresh_{}",
                pad_eq_band_map(*b, q.eq_bands_reversed)?
            )),
            ParameterPath::EqBandDynRatio(b) => Some(format!(
                "EQ/eq_ratio_{}",
                pad_eq_band_map(*b, q.eq_bands_reversed)?
            )),
            ParameterPath::EqBandDynAttack(b) => Some(format!(
                "EQ/eq_attack_{}",
                pad_eq_band_map(*b, q.eq_bands_reversed)?
            )),
            ParameterPath::EqBandDynRelease(b) => Some(format!(
                "EQ/eq_release_{}",
                pad_eq_band_map(*b, q.eq_bands_reversed)?
            )),
            ParameterPath::EqBandDynOverUnder(b) => Some(format!(
                "EQ/eq_over-under_{}",
                pad_eq_band_map(*b, q.eq_bands_reversed)?
            )),

            // Dynamics 1 (compressor)
            ParameterPath::Dyn1Enabled => Some("Dynamics/comp_in".into()),
            ParameterPath::Dyn1Mode => None, // GP OSC-only; iPad uses comp_knee per band
            ParameterPath::Dyn1MultibandDeesser => Some("Dynamics/comp-multiband-desser".into()),
            // The S21 multiband numbers its bands Low=1, High=2, Mid=3 — i.e.
            // it swaps the Mid/High bands relative to the internal/GP-OSC order
            // (internal 1=Low, 2=Mid, 3=High). Encode the swapped wire band for
            // bands 2/3; band 1 (Low) is unchanged either way. See
            // `pad_dyn1_band_map`.
            //
            // The band-1 *bare path* convention below (`comp_thresh` with no
            // index) is deliberately NOT quirk-parameterized: it's a path-shape
            // question, not an index mapping, and multiband dynamics are marked
            // `Unsupported` on non-S families until a hardware probe settles the
            // real SD/Quantum shape.
            ParameterPath::Dyn1Threshold(1) => Some("Dynamics/comp_thresh".into()),
            ParameterPath::Dyn1Threshold(b) => Some(format!(
                "Dynamics/comp_thresh_{}",
                pad_dyn1_band_map(*b, q.dyn1_mid_high_swapped)?
            )),
            ParameterPath::Dyn1Knee(1) => Some("Dynamics/comp_knee".into()),
            ParameterPath::Dyn1Knee(b) => Some(format!(
                "Dynamics/comp_knee_{}",
                pad_dyn1_band_map(*b, q.dyn1_mid_high_swapped)?
            )),
            ParameterPath::Dyn1Ratio(1) => Some("Dynamics/comp_ratio".into()),
            ParameterPath::Dyn1Ratio(b) => Some(format!(
                "Dynamics/comp_ratio_{}",
                pad_dyn1_band_map(*b, q.dyn1_mid_high_swapped)?
            )),
            ParameterPath::Dyn1Attack(1) => Some("Dynamics/comp_attack".into()),
            ParameterPath::Dyn1Attack(b) => Some(format!(
                "Dynamics/comp_attack_{}",
                pad_dyn1_band_map(*b, q.dyn1_mid_high_swapped)?
            )),
            ParameterPath::Dyn1Release(1) => Some("Dynamics/comp_release".into()),
            ParameterPath::Dyn1Release(b) => Some(format!(
                "Dynamics/comp_release_{}",
                pad_dyn1_band_map(*b, q.dyn1_mid_high_swapped)?
            )),
            ParameterPath::Dyn1Gain(1) => Some("Dynamics/comp_gain".into()),
            ParameterPath::Dyn1Gain(b) => Some(format!(
                "Dynamics/comp_auto-gain_{}",
                pad_dyn1_band_map(*b, q.dyn1_mid_high_swapped)?
            )),
            ParameterPath::Dyn1Listen(b) => Some(format!(
                "Dynamics/comp_listen_{}",
                pad_dyn1_band_map(*b, q.dyn1_mid_high_swapped)?
            )),
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

    /// [`Self::to_pad_suffix`] under the hardware-verified S21 quirks.
    ///
    /// Production code passes the live console's quirks explicitly; this is
    /// the entry point for tests that pin S21 wire strings.
    #[inline]
    pub fn to_ipad_suffix(&self) -> Option<String> {
        self.to_pad_suffix(&PadQuirks::S21)
    }

    /// Parse from a Pad-protocol path suffix (the remaining path after the
    /// channel prefix) under the given wire quirks.
    ///
    /// Expects input like "/fader" or "/EQ/eq_gain_2" (with leading /).
    /// Use [`Self::from_ipad_suffix`] for the S21 quirks.
    pub fn from_pad_suffix(suffix: &str, q: &PadQuirks) -> Option<Self> {
        let suffix = suffix.strip_prefix('/').unwrap_or(suffix);

        // Direct matches
        match suffix {
            "fader" => return Some(ParameterPath::Fader),
            "mute" => return Some(ParameterPath::Mute),
            "solo" => return Some(ParameterPath::Solo),
            "Panner/pan" => return Some(ParameterPath::Pan),
            "Channel_Input/name" => return Some(ParameterPath::Name),
            "Channel_Input/analog_gain" => return Some(ParameterPath::AnalogGain),
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
            return parse_pad_eq_suffix(rest, q.eq_bands_reversed);
        }

        // Dynamics multiband: Dynamics/comp_{param}_{band}
        if let Some(rest) = suffix.strip_prefix("Dynamics/comp_") {
            return parse_pad_dyn1_suffix(rest, q.dyn1_mid_high_swapped);
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

    /// [`Self::from_pad_suffix`] under the hardware-verified S21 quirks.
    #[inline]
    pub fn from_ipad_suffix(suffix: &str) -> Option<Self> {
        Self::from_pad_suffix(suffix, &PadQuirks::S21)
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
            "total/gain" => return Some(ParameterPath::TotalGain),
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
            // EQ band parameters: eq/{wire_band}/{param} — wire is 0-based, internal is 1-based.
            ["eq", band, param] => {
                let wire: u8 = band.parse().ok()?;
                let b = wire.checked_add(1)?;
                if !Self::EQ_BAND_RANGE.contains(&b) {
                    return None;
                }
                match *param {
                    "frequency" => Some(ParameterPath::EqBandFrequency(b)),
                    "gain" => Some(ParameterPath::EqBandGain(b)),
                    "q" => Some(ParameterPath::EqBandQ(b)),
                    _ => None,
                }
            }
            // EQ band dynamic: eq/{wire_band}/dyn/{param} — wire is 0-based.
            ["eq", band, "dyn", param] => {
                let wire: u8 = band.parse().ok()?;
                let b = wire.checked_add(1)?;
                if !Self::EQ_BAND_RANGE.contains(&b) {
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
            // Dyn1 band parameters: dyn1/{wire_band}/{param} — wire is 0-based.
            ["dyn1", band, param] => {
                let wire: u8 = band.parse().ok()?;
                let b = wire.checked_add(1)?;
                if !Self::DYN1_BAND_RANGE.contains(&b) {
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
                let s: u16 = send.parse().ok()?;
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

    // ─── Phase 0: per-path scope granularity ────────────────────────────

    /// EQ band index range for a 4-band strip — every S-series channel, and
    /// input channels on every family. Internal model is 1-based; the GP OSC
    /// wire uses 0..=3 — encode/parse handle the shift. Names: 1=Low, 2=Lo Mid,
    /// 3=Hi Mid, 4=High (see `eq_band_name`).
    ///
    /// Prefer [`Self::eq_band_range`] where the channel and family are known:
    /// SD/Quantum bus outputs carry eight bands, not four.
    pub const EQ_BAND_RANGE: std::ops::RangeInclusive<u8> = 1..=4;
    /// The widest EQ strip any supported console offers (SD/Quantum bus
    /// outputs). The codec validates against this; applicability is decided by
    /// [`Self::eq_band_range`].
    pub const EQ_BAND_RANGE_MAX: std::ops::RangeInclusive<u8> = 1..=8;
    /// Dyn1 band index range (1..=3 for the multiband compressor).
    /// Names: 1=Low, 2=Mid, 3=High (see `dyn1_band_name`).
    pub const DYN1_BAND_RANGE: std::ops::RangeInclusive<u8> = 1..=3;
    /// Graphic EQ band index range (1..=32).
    pub const GEQ_BAND_RANGE: std::ops::RangeInclusive<u8> = 1..=32;

    /// Human-readable display label used as the row label in the scope-editor matrix.
    /// Stable strings — keep them aligned with the DiGiCo console GUI naming.
    ///
    /// Band parameters use human-readable band names (Low/Lo Mid/Hi Mid/High
    /// for EQ; Low/Mid/High for the multiband compressor) — see
    /// [`eq_band_name`] and [`dyn1_band_name`] below.
    pub fn label(&self) -> String {
        match self {
            ParameterPath::Name => "Name".into(),
            ParameterPath::Fader => "Fader".into(),
            ParameterPath::Mute => "Mute".into(),
            ParameterPath::Solo => "Solo".into(),
            ParameterPath::Pan => "Pan".into(),
            ParameterPath::AnalogGain => "Analog Gain".into(),
            ParameterPath::TotalGain => "Total Gain".into(),
            ParameterPath::GainTracking => "Gain Tracking".into(),
            ParameterPath::Trim => "Trim".into(),
            ParameterPath::Balance => "Balance".into(),
            ParameterPath::Width => "Stereo Width".into(),
            ParameterPath::Polarity => "Polarity".into(),
            ParameterPath::Phantom => "+48V".into(),
            ParameterPath::MainAltIn => "Main / Alt In".into(),
            ParameterPath::StereoMode => "Stereo Mode".into(),
            ParameterPath::DelayEnabled => "Delay On/Off".into(),
            ParameterPath::DelayTime => "Delay Time".into(),
            ParameterPath::DigitubeEnabled => "DiGiTube On/Off".into(),
            ParameterPath::DigitubeDrive => "DiGiTube Drive".into(),
            ParameterPath::DigitubeBias => "DiGiTube Bias".into(),
            ParameterPath::EqEnabled => "EQ On/Off".into(),
            ParameterPath::HighpassEnabled => "HPF On/Off".into(),
            ParameterPath::HighpassFrequency => "HPF Frequency".into(),
            ParameterPath::LowpassEnabled => "LPF On/Off".into(),
            ParameterPath::LowpassFrequency => "LPF Frequency".into(),
            ParameterPath::EqBandFrequency(b) => format!("EQ {} Frequency", eq_band_name(*b)),
            ParameterPath::EqBandGain(b) => format!("EQ {} Gain", eq_band_name(*b)),
            ParameterPath::EqBandQ(b) => format!("EQ {} Q", eq_band_name(*b)),
            ParameterPath::EqBandCurve(b) => format!("EQ {} Curve", eq_band_name(*b)),
            ParameterPath::EqBandDynEnabled(b) => format!("Dyn EQ {} On/Off", eq_band_name(*b)),
            ParameterPath::EqBandDynThreshold(b) => {
                format!("Dyn EQ {} Threshold", eq_band_name(*b))
            }
            ParameterPath::EqBandDynRatio(b) => format!("Dyn EQ {} Ratio", eq_band_name(*b)),
            ParameterPath::EqBandDynAttack(b) => format!("Dyn EQ {} Attack", eq_band_name(*b)),
            ParameterPath::EqBandDynRelease(b) => format!("Dyn EQ {} Release", eq_band_name(*b)),
            ParameterPath::EqBandDynOverUnder(b) => {
                format!("Dyn EQ {} Over/Under", eq_band_name(*b))
            }
            ParameterPath::Dyn1Enabled => "Compressor On/Off".into(),
            ParameterPath::Dyn1Mode => "Compressor Mode".into(),
            ParameterPath::Dyn1MultibandDeesser => "Multiband De-esser".into(),
            ParameterPath::Dyn1Threshold(b) => format!("Comp {} Threshold", dyn1_band_name(*b)),
            ParameterPath::Dyn1Knee(b) => format!("Comp {} Knee", dyn1_band_name(*b)),
            ParameterPath::Dyn1Ratio(b) => format!("Comp {} Ratio", dyn1_band_name(*b)),
            ParameterPath::Dyn1Attack(b) => format!("Comp {} Attack", dyn1_band_name(*b)),
            ParameterPath::Dyn1Release(b) => format!("Comp {} Release", dyn1_band_name(*b)),
            ParameterPath::Dyn1Gain(b) => format!("Comp {} Gain", dyn1_band_name(*b)),
            ParameterPath::Dyn1Listen(b) => format!("Comp {} Listen", dyn1_band_name(*b)),
            ParameterPath::Dyn1CrossoverHigh => "Comp Crossover High".into(),
            ParameterPath::Dyn1CrossoverLow => "Comp Crossover Low".into(),
            ParameterPath::Dyn2Enabled => "Gate On/Off".into(),
            ParameterPath::Dyn2Mode => "Gate Mode".into(),
            ParameterPath::Dyn2Threshold => "Gate Threshold".into(),
            ParameterPath::Dyn2Knee => "Gate Knee".into(),
            ParameterPath::Dyn2Ratio => "Gate Ratio".into(),
            ParameterPath::Dyn2Range => "Gate Range".into(),
            ParameterPath::Dyn2Attack => "Gate Attack".into(),
            ParameterPath::Dyn2Hold => "Gate Hold".into(),
            ParameterPath::Dyn2Release => "Gate Release".into(),
            ParameterPath::Dyn2Gain => "Gate Gain".into(),
            ParameterPath::Dyn2Highpass => "Gate Side-Chain HPF".into(),
            ParameterPath::Dyn2Lowpass => "Gate Side-Chain LPF".into(),
            ParameterPath::Dyn2Listen => "Gate Listen".into(),
            ParameterPath::Dyn2KeySolo => "Gate Key Solo".into(),
            ParameterPath::SendEnabled(s) => format!("Aux {s} Send On"),
            ParameterPath::SendLevel(s) => format!("Aux {s} Send Level"),
            ParameterPath::SendPan(s) => format!("Aux {s} Send Pan"),
            ParameterPath::GroupSendOn(g) => format!("Group {g} Send On"),
            ParameterPath::MasterBusOn => "Master Bus Send".into(),
            ParameterPath::InsertAEnabled => "Insert A".into(),
            ParameterPath::InsertBEnabled => "Insert B".into(),
            ParameterPath::CgLevel => "CG Level".into(),
            ParameterPath::CgMute => "CG Mute".into(),
            ParameterPath::MatrixSendLevel(s) => format!("Matrix {s} Level"),
            ParameterPath::MatrixSendOn(s) => format!("Matrix {s} On"),
            ParameterPath::GeqBandGain(b) => format!("GEQ Band {b}"),
            ParameterPath::GeqEnabled => "GEQ On/Off".into(),
        }
    }

    /// Display label that uses the live console config to disambiguate
    /// dynamic parameters. Currently the only disambiguation is bus
    /// sends — `SendEnabled/SendLevel/SendPan` show as "Aux N" or "Group N"
    /// (with "(Stereo)" suffix when applicable) depending on
    /// `ConsoleConfig::mix_output_types`. Every other variant falls through
    /// to `label()` so existing labels stay stable.
    pub fn label_with_config(&self, config: &crate::model::config::ConsoleConfig) -> String {
        match self {
            ParameterPath::SendEnabled(s) => format!("{} Send On", config.bus_label(*s)),
            ParameterPath::SendLevel(s) => format!("{} Send Level", config.bus_label(*s)),
            ParameterPath::SendPan(s) => format!("{} Send Pan", config.bus_label(*s)),
            _ => self.label(),
        }
    }

    /// How well this parameter is known to work on a given console family.
    ///
    /// S-series returns [`ParamSupport::Verified`] for everything — the whole
    /// enum was built from the S21 command set and confirmed against a live
    /// desk. SD/Quantum split into the core Pad tree (reachable in community
    /// implementations of the same protocol → [`ParamSupport::Assumed`], the
    /// backlog for the hardware probe) and everything that is either GP
    /// OSC-only or an S21-dialect oddity ([`ParamSupport::Unsupported`]).
    ///
    /// Keep in sync with [`Self::to_pad_suffix`]: a parameter that is not
    /// `Unsupported` must have a Pad path (enforced by
    /// `support_implies_pad_path`).
    pub fn support(&self, family: ConsoleFamily) -> ParamSupport {
        use ParameterPath as P;

        if family == ConsoleFamily::SSeries {
            return ParamSupport::Verified;
        }

        // SD and Quantum share one classification today; they are one arm so
        // a hardware probe can split them without restructuring the callers.
        match self {
            // ── Core Pad tree: same paths community SD/Quantum tooling drives.
            P::Name
            | P::Fader
            | P::Mute
            | P::Solo
            | P::Pan
            | P::SendEnabled(_)
            | P::SendLevel(_)
            | P::SendPan(_)
            | P::AnalogGain
            | P::Trim
            | P::Polarity
            | P::Phantom
            | P::DelayEnabled
            | P::DelayTime
            | P::EqEnabled
            | P::HighpassEnabled
            | P::HighpassFrequency
            | P::LowpassEnabled
            | P::LowpassFrequency
            | P::EqBandFrequency(_)
            | P::EqBandGain(_)
            | P::EqBandQ(_)
            | P::Dyn1Enabled
            | P::Dyn2Enabled
            | P::Dyn2Threshold
            | P::Dyn2Attack
            | P::Dyn2Hold
            | P::Dyn2Release
            | P::Dyn2Range
            | P::InsertAEnabled
            | P::InsertBEnabled
            | P::GeqEnabled
            | P::GeqBandGain(_)
            | P::MatrixSendLevel(_)
            | P::MatrixSendOn(_) => ParamSupport::Assumed,

            // Single-band compressor: band 1 is the plain comp on every desk.
            // Higher bands are the S21 multiband, handled below.
            P::Dyn1Threshold(1)
            | P::Dyn1Knee(1)
            | P::Dyn1Ratio(1)
            | P::Dyn1Attack(1)
            | P::Dyn1Release(1)
            | P::Dyn1Gain(1) => ParamSupport::Assumed,

            // ── GP OSC-only: no Pad path at all, and Pad-only families have
            // no GP dialect to fall back on.
            P::TotalGain
            | P::GainTracking
            | P::Balance
            | P::Width
            | P::DigitubeEnabled
            | P::DigitubeDrive
            | P::DigitubeBias
            | P::Dyn1Mode
            | P::Dyn2Knee
            | P::Dyn2Ratio
            | P::Dyn2Gain
            | P::Dyn2Listen => ParamSupport::Unsupported,

            // ── S21-dialect oddities: the path shape is an S-series artifact
            // (the `Group_Send/17` master-bus hack, the CG bitmask, the
            // multiband band layout) or the feature is S-series-specific.
            // Unsupported until a hardware probe establishes the real shape.
            P::MasterBusOn
            | P::GroupSendOn(_)
            | P::CgLevel
            | P::CgMute
            | P::MainAltIn
            | P::StereoMode
            | P::Dyn1MultibandDeesser
            | P::Dyn1Threshold(_)
            | P::Dyn1Knee(_)
            | P::Dyn1Ratio(_)
            | P::Dyn1Attack(_)
            | P::Dyn1Release(_)
            | P::Dyn1Gain(_)
            | P::Dyn1Listen(_)
            | P::Dyn1CrossoverHigh
            | P::Dyn1CrossoverLow
            | P::EqBandCurve(_)
            | P::EqBandDynEnabled(_)
            | P::EqBandDynThreshold(_)
            | P::EqBandDynRatio(_)
            | P::EqBandDynAttack(_)
            | P::EqBandDynRelease(_)
            | P::EqBandDynOverUnder(_)
            | P::Dyn2Mode
            | P::Dyn2KeySolo
            | P::Dyn2Highpass
            | P::Dyn2Lowpass => ParamSupport::Unsupported,
        }
    }

    /// [`Self::available_for_channel`] additionally gated by the console
    /// family's support table.
    pub fn available_for_channel_on(&self, channel: &ChannelId, family: ConsoleFamily) -> bool {
        self.support(family).is_usable() && self.available_for_channel(channel)
    }

    /// Whether this path is reachable on the given channel type via either the
    /// GP OSC protocol or the iPad protocol. Source of truth:
    /// - GP OSC: `Documentation/DiGiCo S OSC Commandset_OSCpaths.csv`
    /// - iPad protocol: existing `// iPad protocol only` annotations on this enum
    ///   plus `Documentation/iPad_commands.png`.
    ///
    /// CSV table summary (X = present):
    /// ```text
    /// path family                          | input | aux | grp | mtx | CG
    /// name, mute, solo, fader              |   X   |  X  |  X  |  X  |  X
    /// total/gain, gain_tracking, balance,  |   X   |     |     |     |
    ///   width, pan, send/{n}/*             |       |     |     |     |
    /// trim, polarity, delay/*, digitube/*, |   X   |  X  |  X  |  X  |
    ///   eq/*, dyn1/*, dyn2/*               |       |     |     |     |
    /// ```
    /// CG channels expose only Name/Mute/Solo/Fader on GP OSC.
    pub fn available_for_channel(&self, channel: &ChannelId) -> bool {
        use ChannelId as C;
        use ParameterPath as P;

        // First filter by channel-type restrictions for paths that only exist on
        // specific channel kinds (regardless of GP/iPad).
        match self {
            // GraphicEq channels: only GEQ paths plus name/mute/solo/fader.
            P::GeqBandGain(_) | P::GeqEnabled => return matches!(channel, C::GraphicEq(_)),
            // MatrixInput channels: only matrix-send paths plus name/mute/solo/fader.
            P::MatrixSendLevel(_) | P::MatrixSendOn(_) => {
                return matches!(channel, C::MatrixInput(_));
            }
            _ => {}
        }

        // GraphicEq / MatrixInput channels: outside their specialised paths,
        // they only support the four universal channel verbs.
        match channel {
            C::GraphicEq(_) | C::MatrixInput(_) => {
                return matches!(self, P::Name | P::Mute | P::Solo | P::Fader);
            }
            _ => {}
        }

        // Universal four (every channel kind in the GP OSC table).
        if matches!(self, P::Name | P::Mute | P::Solo | P::Fader) {
            return true;
        }

        // Control Groups: only the universal four (handled above).
        if matches!(channel, C::ControlGroup(_)) {
            return false;
        }

        // input-only paths (GP OSC + iPad-only input fields).
        let input_only = matches!(
            self,
            P::AnalogGain
                | P::TotalGain
                | P::GainTracking
                | P::Balance
                | P::Width
                | P::Pan
                | P::SendEnabled(_)
                | P::SendLevel(_)
                | P::SendPan(_)
                // iPad-only input fields:
                | P::Phantom
                | P::MainAltIn
                | P::StereoMode
                // iPad-only routing originating from inputs:
                | P::GroupSendOn(_)
                | P::MasterBusOn
        );
        if input_only {
            return matches!(channel, C::Input(_));
        }

        // The remaining paths (Trim, Polarity, Delay/*, Digitube/*, all Eq*,
        // all Dyn1*, all Dyn2*, Inserts, CG membership) apply to
        // input/aux/grp/mtx but NOT CG/GraphicEq/MatrixInput. CG was already
        // returned above; GraphicEq/MatrixInput were handled above too.
        matches!(
            channel,
            C::Input(_) | C::Aux(_) | C::Group(_) | C::Matrix(_)
        )
    }

    /// Every applicable `ParameterPath` for a given channel type, in signal-flow
    /// order. Stable across runs so the matrix layout doesn't shuffle between
    /// frames. The aux/group/matrix send-number ranges are passed in via
    /// `aux_count` / `group_count` / `matrix_count` so the result respects the
    /// actual show config.
    pub fn applicable_to(
        channel: &ChannelId,
        aux_count: u16,
        group_count: u16,
        matrix_count: u16,
    ) -> Vec<ParameterPath> {
        Self::applicable_to_on(
            channel,
            aux_count,
            group_count,
            matrix_count,
            ConsoleFamily::SSeries,
        )
    }

    /// The family-aware core of [`Self::applicable_to`]. Family affects only
    /// the EQ strip width today (see [`Self::eq_band_range`]); everything else
    /// is decided by channel type. Support-table filtering is applied by
    /// [`Self::applicable_to_for_family`], not here.
    fn applicable_to_on(
        channel: &ChannelId,
        aux_count: u16,
        group_count: u16,
        matrix_count: u16,
        family: ConsoleFamily,
    ) -> Vec<ParameterPath> {
        let mut out: Vec<ParameterPath> = Vec::new();

        // Helper closure: push if available on this channel.
        let push = |p: ParameterPath, out: &mut Vec<ParameterPath>| {
            if p.available_for_channel(channel) {
                out.push(p);
            }
        };

        // Identity / Output
        push(ParameterPath::Name, &mut out);

        // Fader/Mute/Pan
        push(ParameterPath::Fader, &mut out);
        push(ParameterPath::Mute, &mut out);
        push(ParameterPath::Solo, &mut out);
        push(ParameterPath::Pan, &mut out);

        // Input
        push(ParameterPath::AnalogGain, &mut out);
        // TotalGain (post-fader + CG sum) is a console-derived, read-only
        // monitor value — never selectable for capture/recall.
        push(ParameterPath::GainTracking, &mut out);
        push(ParameterPath::Trim, &mut out);
        push(ParameterPath::Balance, &mut out);
        push(ParameterPath::Width, &mut out);
        push(ParameterPath::Polarity, &mut out);
        push(ParameterPath::Phantom, &mut out);
        push(ParameterPath::MainAltIn, &mut out);
        push(ParameterPath::StereoMode, &mut out);

        // Input Processing
        push(ParameterPath::DelayEnabled, &mut out);
        push(ParameterPath::DelayTime, &mut out);
        push(ParameterPath::DigitubeEnabled, &mut out);
        push(ParameterPath::DigitubeDrive, &mut out);
        push(ParameterPath::DigitubeBias, &mut out);

        // EQ
        push(ParameterPath::EqEnabled, &mut out);
        push(ParameterPath::HighpassEnabled, &mut out);
        push(ParameterPath::HighpassFrequency, &mut out);
        push(ParameterPath::LowpassEnabled, &mut out);
        push(ParameterPath::LowpassFrequency, &mut out);
        for b in Self::eq_band_range(channel, family) {
            push(ParameterPath::EqBandFrequency(b), &mut out);
            push(ParameterPath::EqBandGain(b), &mut out);
            push(ParameterPath::EqBandQ(b), &mut out);
            push(ParameterPath::EqBandCurve(b), &mut out);
            push(ParameterPath::EqBandDynEnabled(b), &mut out);
            push(ParameterPath::EqBandDynThreshold(b), &mut out);
            push(ParameterPath::EqBandDynRatio(b), &mut out);
            push(ParameterPath::EqBandDynAttack(b), &mut out);
            push(ParameterPath::EqBandDynRelease(b), &mut out);
            push(ParameterPath::EqBandDynOverUnder(b), &mut out);
        }

        // Dynamics 1 (compressor / multiband)
        push(ParameterPath::Dyn1Enabled, &mut out);
        push(ParameterPath::Dyn1Mode, &mut out);
        push(ParameterPath::Dyn1MultibandDeesser, &mut out);
        for b in Self::DYN1_BAND_RANGE {
            push(ParameterPath::Dyn1Threshold(b), &mut out);
            push(ParameterPath::Dyn1Knee(b), &mut out);
            push(ParameterPath::Dyn1Ratio(b), &mut out);
            push(ParameterPath::Dyn1Attack(b), &mut out);
            push(ParameterPath::Dyn1Release(b), &mut out);
            push(ParameterPath::Dyn1Gain(b), &mut out);
            push(ParameterPath::Dyn1Listen(b), &mut out);
        }
        push(ParameterPath::Dyn1CrossoverHigh, &mut out);
        push(ParameterPath::Dyn1CrossoverLow, &mut out);

        // Dynamics 2 (gate / duck / comp)
        push(ParameterPath::Dyn2Enabled, &mut out);
        push(ParameterPath::Dyn2Mode, &mut out);
        push(ParameterPath::Dyn2Threshold, &mut out);
        push(ParameterPath::Dyn2Knee, &mut out);
        push(ParameterPath::Dyn2Ratio, &mut out);
        push(ParameterPath::Dyn2Range, &mut out);
        push(ParameterPath::Dyn2Attack, &mut out);
        push(ParameterPath::Dyn2Hold, &mut out);
        push(ParameterPath::Dyn2Release, &mut out);
        push(ParameterPath::Dyn2Gain, &mut out);
        push(ParameterPath::Dyn2Highpass, &mut out);
        push(ParameterPath::Dyn2Lowpass, &mut out);
        push(ParameterPath::Dyn2Listen, &mut out);
        push(ParameterPath::Dyn2KeySolo, &mut out);

        // Bus sends (input only) — the GP OSC `send/{n}/*` path family
        // covers EVERY mix output bus, regardless of whether bus N is
        // currently configured as aux or group. The aux/group split is
        // dynamic; `ConsoleConfig::mix_output_types` tracks which is which.
        // The total bus count is `aux_count + group_count`, so iterate over
        // the union here. The scope editor uses
        // `ParameterPath::label_with_config` to render each bus row with its
        // live "Aux N" / "Group N" label.
        let bus_count = aux_count.saturating_add(group_count);
        for s in 1..=bus_count {
            push(ParameterPath::SendEnabled(s), &mut out);
            push(ParameterPath::SendLevel(s), &mut out);
            push(ParameterPath::SendPan(s), &mut out);
        }

        // Group routing (input only, iPad protocol). This is a SEPARATE
        // concept from the bus sends above — it's the iPad-side input→group
        // routing assignment, not the per-bus send level.
        for g in 1..=group_count {
            push(ParameterPath::GroupSendOn(g), &mut out);
        }
        push(ParameterPath::MasterBusOn, &mut out);

        // Inserts
        push(ParameterPath::InsertAEnabled, &mut out);
        push(ParameterPath::InsertBEnabled, &mut out);

        // CG membership
        push(ParameterPath::CgLevel, &mut out);
        push(ParameterPath::CgMute, &mut out);

        // Matrix sends (only on MatrixInput channels)
        for s in 1..=matrix_count {
            push(ParameterPath::MatrixSendLevel(s), &mut out);
            push(ParameterPath::MatrixSendOn(s), &mut out);
        }

        // Graphic EQ (only on GraphicEq channels)
        push(ParameterPath::GeqEnabled, &mut out);
        for b in Self::GEQ_BAND_RANGE {
            push(ParameterPath::GeqBandGain(b), &mut out);
        }

        out
    }

    /// [`Self::applicable_to`] for a specific family, filtered by that family's
    /// support table. Identical to `applicable_to` on S-series (every path is
    /// `Verified`, and every strip is four bands).
    pub fn applicable_to_for_family(
        channel: &ChannelId,
        aux_count: u16,
        group_count: u16,
        matrix_count: u16,
        family: ConsoleFamily,
    ) -> Vec<ParameterPath> {
        Self::applicable_to_on(channel, aux_count, group_count, matrix_count, family)
            .into_iter()
            .filter(|p| p.support(family).is_usable())
            .collect()
    }

    /// How many parametric EQ bands `channel` has on `family`.
    ///
    /// DiGiCo's SD App guide states "4 band EQ (or 8 band where applicable)",
    /// and the published `/sd/` command list bears it out: Aux, Group and
    /// Matrix **outputs** expose `eq_*_1` through `eq_*_8`, while input
    /// channels stop at 4. S-series is four bands throughout.
    pub fn eq_band_range(
        channel: &ChannelId,
        family: ConsoleFamily,
    ) -> std::ops::RangeInclusive<u8> {
        use ChannelId as C;
        let eight = matches!(family, ConsoleFamily::SdRange | ConsoleFamily::Quantum)
            && matches!(channel, C::Aux(_) | C::Group(_) | C::Matrix(_));
        if eight { 1..=8 } else { Self::EQ_BAND_RANGE }
    }
}

/// Parameter sections for scope control (PRD §4.5).
/// Each section groups related parameters that are captured/recalled together.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ParameterSection {
    Name,
    InputGain,
    /// Channel trim (-40 dB to +40 dB). On the S21 this is exposed for
    /// input, aux, group and matrix channels per the OSC chart, so it
    /// lives in its own section instead of being lumped with head-amp
    /// gain (which is input-only).
    Trim,
    /// Phase / polarity invert. Same broad applicability as `Trim`.
    Polarity,
    /// Stereo balance + width. Input-channel-only on the S21 (GP OSC).
    BalanceWidth,
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
    /// All section variants in display order. Order roughly follows
    /// signal flow (input conditioning → EQ/dyn → routing → output).
    /// `CgMembership` is intentionally absent — there's no OSC writer
    /// for CG membership on the S21, so exposing it as a gangable /
    /// scope-editable section would advertise functionality the
    /// console can't actually receive. The variant still exists for
    /// classifying any incoming `CgLevel` / `CgMute` echoes from the
    /// state mirror.
    pub fn all_variants() -> &'static [ParameterSection] {
        &[
            ParameterSection::FaderMutePan,
            ParameterSection::Name,
            ParameterSection::InputGain,
            ParameterSection::Trim,
            ParameterSection::Polarity,
            ParameterSection::BalanceWidth,
            ParameterSection::Delay,
            ParameterSection::Digitube,
            ParameterSection::Eq,
            ParameterSection::Dyn1,
            ParameterSection::Dyn2,
            ParameterSection::Sends,
            ParameterSection::GroupRouting,
            ParameterSection::Inserts,
            ParameterSection::GraphicEq,
            ParameterSection::MatrixSends,
        ]
    }

    /// Which sections are applicable to a given channel type. Per the
    /// DiGiCo S OSC chart (`Documentation/DiGiCo S OSC Commandset_OSCpaths.csv`):
    /// trim/polarity/delay/digitube apply to input/aux/group/matrix; balance
    /// and width are input-only; CG channels only carry name/fader/mute/solo.
    pub fn applicable_to(channel: &ChannelId) -> Vec<ParameterSection> {
        match channel {
            ChannelId::Input(_) => vec![
                ParameterSection::FaderMutePan,
                ParameterSection::Name,
                ParameterSection::InputGain,
                ParameterSection::Trim,
                ParameterSection::Polarity,
                ParameterSection::BalanceWidth,
                ParameterSection::Delay,
                ParameterSection::Digitube,
                ParameterSection::Eq,
                ParameterSection::Dyn1,
                ParameterSection::Dyn2,
                ParameterSection::Sends,
                ParameterSection::GroupRouting,
                ParameterSection::Inserts,
                // CgMembership is intentionally absent — see the note on
                // `all_variants` above.
            ],
            ChannelId::Aux(_) => vec![
                ParameterSection::FaderMutePan,
                ParameterSection::Name,
                ParameterSection::Trim,
                ParameterSection::Polarity,
                ParameterSection::Delay,
                ParameterSection::Digitube,
                ParameterSection::Eq,
                ParameterSection::Dyn1,
                ParameterSection::Dyn2,
                ParameterSection::Inserts,
            ],
            ChannelId::Group(_) => vec![
                ParameterSection::FaderMutePan,
                ParameterSection::Name,
                ParameterSection::Trim,
                ParameterSection::Polarity,
                ParameterSection::Delay,
                ParameterSection::Digitube,
                ParameterSection::Eq,
                ParameterSection::Dyn1,
                ParameterSection::Dyn2,
                ParameterSection::Inserts,
            ],
            ChannelId::Matrix(_) => vec![
                ParameterSection::FaderMutePan,
                ParameterSection::Name,
                ParameterSection::Trim,
                ParameterSection::Polarity,
                ParameterSection::Delay,
                ParameterSection::Digitube,
                ParameterSection::Eq,
                ParameterSection::Dyn1,
                ParameterSection::Dyn2,
                ParameterSection::Inserts,
            ],
            ChannelId::ControlGroup(_) => {
                vec![ParameterSection::FaderMutePan, ParameterSection::Name]
            }
            ChannelId::GraphicEq(_) => vec![ParameterSection::GraphicEq],
            ChannelId::MatrixInput(_) => vec![ParameterSection::MatrixSends],
        }
    }

    /// All concrete `ParameterPath`s in this section that apply to
    /// `channel`, in display order. Send and EQ-band variants are
    /// expanded using counts from `config`. Returns an empty vec when
    /// the section is not applicable to the channel.
    pub fn paths_for(
        &self,
        channel: &ChannelId,
        config: &crate::model::config::ConsoleConfig,
    ) -> Vec<ParameterPath> {
        ParameterPath::applicable_to(
            channel,
            config.aux_output_count,
            config.group_output_count,
            config.matrix_output_count,
        )
        .into_iter()
        .filter(|p| p.section() == *self)
        .collect()
    }
}

impl fmt::Display for ParameterSection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParameterSection::Name => write!(f, "Name"),
            ParameterSection::InputGain => write!(f, "Input Gain"),
            ParameterSection::Trim => write!(f, "Trim"),
            ParameterSection::Polarity => write!(f, "Polarity"),
            ParameterSection::BalanceWidth => write!(f, "Balance / Width"),
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

            ParameterPath::Fader | ParameterPath::Mute | ParameterPath::Pan => {
                ParameterSection::FaderMutePan
            }

            ParameterPath::Solo => ParameterSection::FaderMutePan,

            ParameterPath::AnalogGain
            | ParameterPath::TotalGain
            | ParameterPath::GainTracking
            | ParameterPath::Phantom
            | ParameterPath::MainAltIn
            | ParameterPath::StereoMode => ParameterSection::InputGain,

            ParameterPath::Trim => ParameterSection::Trim,
            ParameterPath::Polarity => ParameterSection::Polarity,
            ParameterPath::Balance | ParameterPath::Width => ParameterSection::BalanceWidth,

            ParameterPath::DelayEnabled | ParameterPath::DelayTime => ParameterSection::Delay,

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

            ParameterPath::GroupSendOn(_) | ParameterPath::MasterBusOn => {
                ParameterSection::GroupRouting
            }

            ParameterPath::InsertAEnabled | ParameterPath::InsertBEnabled => {
                ParameterSection::Inserts
            }

            ParameterPath::CgLevel | ParameterPath::CgMute => ParameterSection::CgMembership,

            ParameterPath::MatrixSendLevel(_) | ParameterPath::MatrixSendOn(_) => {
                ParameterSection::MatrixSends
            }

            ParameterPath::GeqBandGain(_) | ParameterPath::GeqEnabled => {
                ParameterSection::GraphicEq
            }
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
            ParameterPath::AnalogGain
            | ParameterPath::TotalGain
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

    /// Clamp `value` to the parameter's valid range. Defaults to a
    /// passthrough for parameters whose range we don't yet model —
    /// keep this method conservative; aggressive clamping would mask
    /// legitimate problems elsewhere.
    ///
    /// Currently models the `[-1, +1]` pan-family parameters. Other
    /// parameters (Fader dB, EQ band gain, etc.) have ranges too but
    /// are deliberately left unclamped until we encode their per-
    /// channel-type ranges precisely.
    pub fn clamp_value(&self, value: ParameterValue) -> ParameterValue {
        use ParameterPath as P;
        match self {
            P::Pan | P::SendPan(_) | P::Balance | P::Width => {
                if let ParameterValue::Float(f) = value {
                    return ParameterValue::Float(f.clamp(-1.0, 1.0));
                }
                value
            }
            _ => value,
        }
    }

    /// [`Self::clamp_value`] plus any level range the console profile declares.
    ///
    /// S-series profiles carry no send-level range (`send_level_db_range:
    /// None`), so this is provably identical to `clamp_value` there — the desk's
    /// own range is trusted, as it always has been. Pad-only families declare a
    /// range because their level scaling is a hypothesis until probed, and a
    /// value outside it would be a silent mis-scale rather than an obvious
    /// failure.
    pub fn clamp_value_with_profile(
        &self,
        value: ParameterValue,
        profile: &ConsoleProfile,
    ) -> ParameterValue {
        use ParameterPath as P;
        let value = self.clamp_value(value);
        let Some((lo, hi)) = profile.send_level_db_range else {
            return value;
        };
        match self {
            P::SendLevel(_) | P::MatrixSendLevel(_) | P::CgLevel => {
                if let ParameterValue::Float(f) = value {
                    return ParameterValue::Float(f.clamp(lo, hi));
                }
                value
            }
            _ => value,
        }
    }
}

// ── Timing categories ────────────────────────────────────────────────

/// Coarse groupings of parameters for per-channel recall timing (pre-wait
/// and fade). Each category maps to one or more `ParameterSection` values
/// and defines whether its continuous parameters can be faded.
///
/// Parameters not belonging to any category (Name, Solo, AnalogGain,
/// TotalGain, etc.) are recalled instantly with no pre-wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TimingCategory {
    /// Trim, Balance, Width, Polarity, DelayEnabled, DelayTime.
    Preprocessing,
    /// All EQ/HP/LP params. Discrete switches (enables, modes) are sent
    /// after pre-wait before fades start.
    Eq,
    /// Compressor params. Discrete switches sent before fades.
    Dyn1,
    /// Gate params. Discrete switches sent before fades.
    Dyn2,
    /// Fader + Pan.
    Fader,
    /// Mute only. No fade — pre-wait only. ON before others, OFF after others.
    Mute,
    /// SendLevel, SendPan (fade); SendEnabled follows per-send ordering
    /// (enable AFTER level change, disable BEFORE level change).
    Sends,
}

impl TimingCategory {
    /// All variants in display order.
    pub fn all_variants() -> &'static [TimingCategory] {
        &[
            TimingCategory::Fader,
            TimingCategory::Mute,
            TimingCategory::Preprocessing,
            TimingCategory::Eq,
            TimingCategory::Dyn1,
            TimingCategory::Dyn2,
            TimingCategory::Sends,
        ]
    }

    /// Map a parameter path to its timing category, or None for uncategorized
    /// params that are always recalled instantly.
    pub fn from_parameter_path(path: &ParameterPath) -> Option<TimingCategory> {
        match path {
            // Fader
            ParameterPath::Fader | ParameterPath::Pan => Some(TimingCategory::Fader),

            // Mute
            ParameterPath::Mute => Some(TimingCategory::Mute),

            // Preprocessing (InputGain subset + Delay)
            ParameterPath::Trim
            | ParameterPath::Balance
            | ParameterPath::Width
            | ParameterPath::Polarity
            | ParameterPath::DelayEnabled
            | ParameterPath::DelayTime => Some(TimingCategory::Preprocessing),

            // EQ (all EQ/HP/LP params)
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
            | ParameterPath::EqBandDynOverUnder(_) => Some(TimingCategory::Eq),

            // Dyn1
            ParameterPath::Dyn1Enabled
            | ParameterPath::Dyn1Mode
            | ParameterPath::Dyn1Threshold(_)
            | ParameterPath::Dyn1Knee(_)
            | ParameterPath::Dyn1Ratio(_)
            | ParameterPath::Dyn1Attack(_)
            | ParameterPath::Dyn1Release(_)
            | ParameterPath::Dyn1Gain(_)
            | ParameterPath::Dyn1CrossoverHigh
            | ParameterPath::Dyn1CrossoverLow => Some(TimingCategory::Dyn1),

            // Dyn2
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
            | ParameterPath::Dyn2Lowpass => Some(TimingCategory::Dyn2),

            // Sends
            ParameterPath::SendEnabled(_)
            | ParameterPath::SendLevel(_)
            | ParameterPath::SendPan(_) => Some(TimingCategory::Sends),

            // Uncategorized — instant recall
            _ => None,
        }
    }

    /// Whether continuous params in this category can be faded.
    /// Mute is always discrete (pre-wait only).
    pub fn supports_fade(&self) -> bool {
        !matches!(self, TimingCategory::Mute)
    }

    /// UI display label.
    pub fn label(&self) -> &'static str {
        match self {
            TimingCategory::Preprocessing => "Preprocessing",
            TimingCategory::Eq => "EQ",
            TimingCategory::Dyn1 => "Dyn 1",
            TimingCategory::Dyn2 => "Dyn 2",
            TimingCategory::Fader => "Fader",
            TimingCategory::Mute => "Mute",
            TimingCategory::Sends => "Sends",
        }
    }

    /// Which timing categories are displayed beneath a given section in the
    /// scope editor. Returns an empty slice for sections with no timing.
    pub fn for_section(section: &ParameterSection) -> &'static [TimingCategory] {
        match section {
            ParameterSection::FaderMutePan => &[TimingCategory::Fader, TimingCategory::Mute],
            ParameterSection::InputGain | ParameterSection::Delay => {
                &[TimingCategory::Preprocessing]
            }
            ParameterSection::Eq => &[TimingCategory::Eq],
            ParameterSection::Dyn1 => &[TimingCategory::Dyn1],
            ParameterSection::Dyn2 => &[TimingCategory::Dyn2],
            ParameterSection::Sends => &[TimingCategory::Sends],
            _ => &[],
        }
    }
}

// ── Palette kinds ────────────────────────────────────────────────────

/// Which group of parameters a palette stores.
///
/// Mirrors a subset of `ParameterSection` — the sections that are
/// "templatable" across channels in a way that makes operator sense.
/// Each kind maps to exactly one `ParameterSection`; the mapping lives
/// on `PaletteKind::section()` and `ParameterSection::palette_kind()`.
///
/// Adding a new kind (e.g. for Sends, Inserts, Graphic EQ) is a one-line
/// addition here plus a one-line addition to `palette_kind()` and a label
/// branch on `PaletteKind::label()`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum PaletteKind {
    #[default]
    Eq,
    Dyn1,
    Dyn2,
}

impl PaletteKind {
    /// Which `ParameterSection` this palette stores values for.
    pub fn section(&self) -> ParameterSection {
        match self {
            PaletteKind::Eq => ParameterSection::Eq,
            PaletteKind::Dyn1 => ParameterSection::Dyn1,
            PaletteKind::Dyn2 => ParameterSection::Dyn2,
        }
    }

    /// Operator-facing label for the kind picker. Mirrors the
    /// `ParameterSection` names — Dyn1/Dyn2 are kept generic instead of
    /// "Compressor"/"Gate" because the same processor slot can be
    /// configured as comp / gate / multiband-deesser depending on the
    /// channel.
    pub fn label(&self) -> &'static str {
        match self {
            PaletteKind::Eq => "Eq",
            PaletteKind::Dyn1 => "Dyn1",
            PaletteKind::Dyn2 => "Dyn2",
        }
    }

    /// Every kind, in display order.
    pub fn all() -> &'static [PaletteKind] {
        &[PaletteKind::Eq, PaletteKind::Dyn1, PaletteKind::Dyn2]
    }
}

impl ParameterSection {
    /// Returns Some if this section has a palette kind. Returns None for
    /// sections that don't (yet) participate in the palette system —
    /// FaderMutePan, InputGain, Sends, GroupRouting, Inserts, CgMembership,
    /// GraphicEq, MatrixSends, Name, Delay, Digitube. Adding a new
    /// palette kind requires extending this match arm.
    pub fn palette_kind(&self) -> Option<PaletteKind> {
        match self {
            ParameterSection::Eq => Some(PaletteKind::Eq),
            ParameterSection::Dyn1 => Some(PaletteKind::Dyn1),
            ParameterSection::Dyn2 => Some(PaletteKind::Dyn2),
            _ => None,
        }
    }
}

// ── Protocol-coverage table (for the setup-tab help card) ────────────

/// One row of the protocol-coverage matrix shown in the setup-tab help card.
/// `gp` = reachable via GP OSC, `ipad` = reachable via the iPad protocol.
/// Items where both are false (e.g. dynamic-EQ over/under) are surfaced so
/// the operator knows the parameter exists but is console-surface only.
pub struct ProtocolCoverageRow {
    pub label: &'static str,
    pub gp: bool,
    pub ipad: bool,
}

/// Operator-facing protocol-coverage matrix. Hand-curated to match the
/// `to_gp_osc_suffix` / `to_ipad_suffix` mappings on `ParameterPath`.
/// Keep this in sync when adding/removing parameters from those mappings.
pub const PROTOCOL_COVERAGE: &[ProtocolCoverageRow] = &[
    ProtocolCoverageRow {
        label: "Fader / Mute / Solo / Pan",
        gp: true,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "Channel name",
        gp: true,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "Total gain (post-fader sum)",
        gp: true,
        ipad: false,
    },
    ProtocolCoverageRow {
        label: "Analog preamp gain",
        gp: false,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "+48V phantom power",
        gp: false,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "Polarity, trim, delay",
        gp: true,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "Stereo balance / width",
        gp: true,
        ipad: false,
    },
    ProtocolCoverageRow {
        label: "DiGiTube",
        gp: true,
        ipad: false,
    },
    ProtocolCoverageRow {
        label: "EQ band freq / gain / Q",
        gp: true,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "EQ band curve",
        gp: false,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "Dynamic EQ band threshold/ratio/attack/release",
        gp: true,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "Dynamic EQ over/under mode",
        gp: false,
        ipad: false,
    },
    ProtocolCoverageRow {
        label: "Compressor (single-band + multiband)",
        gp: true,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "Multiband de-esser",
        gp: false,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "Gate / ducker",
        gp: true,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "Gate key solo",
        gp: false,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "Inserts A / B",
        gp: false,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "Aux sends",
        gp: true,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "Group send routing",
        gp: false,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "Matrix send levels / on",
        gp: false,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "Graphic EQ band gains",
        gp: false,
        ipad: true,
    },
    ProtocolCoverageRow {
        label: "Control group level / mute",
        gp: false,
        ipad: true,
    },
];

// ── Band naming helpers ──────────────────────────────────────────────

/// Human-readable name for an EQ band (1-based internal index).
/// 1=Low, 2=Lo Mid, 3=Hi Mid, 4=High. Falls back to the numeric form
/// for any out-of-range index so logging/tests stay informative.
pub fn eq_band_name(b: u8) -> String {
    match b {
        1 => "Low".into(),
        2 => "Lo Mid".into(),
        3 => "Hi Mid".into(),
        4 => "High".into(),
        _ => format!("Band {b}"),
    }
}

/// Human-readable name for a multiband-compressor band (1-based internal index).
/// 1=Low, 2=Mid, 3=High.
pub fn dyn1_band_name(b: u8) -> String {
    match b {
        1 => "Low".into(),
        2 => "Mid".into(),
        3 => "High".into(),
        _ => format!("Band {b}"),
    }
}

// ── Pad suffix parsing helpers ───────────────────────────────────────

/// Map an EQ band index between the internal/GP-OSC numbering and the Pad
/// wire numbering.
///
/// The S21 numbers the four parametric EQ bands in the reverse order of the
/// internal 1-based model (and the GP-OSC wire): internal band `b` is wire
/// band `5 - b` (so 1↔4, 2↔3). The mapping is its own inverse, so the same
/// function converts in either direction. With `reversed = false` the index
/// passes through unchanged. Returns `None` for indices outside the valid
/// 1..=4 band range either way.
///
/// Without the reversal on S-series, iPad-sourced EQ updates land on the
/// mirror-image band and (in Mode 3) collide with the correctly-decoded
/// GP-OSC mirror writes, so a single edit corrupts two bands at once.
///
/// Band width: unreversed indices are accepted across the widest strip any
/// console offers ([`ParameterPath::EQ_BAND_RANGE_MAX`]), because SD/Quantum
/// bus outputs carry eight bands. The reversal, by contrast, is `5 - band` —
/// arithmetic that is only meaningful on a four-band strip — so a reversed
/// index outside 1..=4 is rejected rather than mapped to nonsense. That costs
/// nothing today: reversal is an S21 artifact, and no S-series channel has
/// more than four bands. Should a probe ever find an eight-band console that
/// also reverses, this needs the strip width passed in so it can use
/// `width + 1 - band`.
fn pad_eq_band_map(band: u8, reversed: bool) -> Option<u8> {
    if reversed {
        if !ParameterPath::EQ_BAND_RANGE.contains(&band) {
            return None;
        }
        Some(5 - band)
    } else {
        if !ParameterPath::EQ_BAND_RANGE_MAX.contains(&band) {
            return None;
        }
        Some(band)
    }
}

fn parse_pad_eq_suffix(rest: &str, reversed: bool) -> Option<ParameterPath> {
    // Try patterns: eq_freq_{b}, eq_gain_{b}, eq_Q_{b}, eq_curve_{b},
    // dynamic_eq_on_{b}, eq_thresh_{b}, eq_over-under_{b}, eq_ratio_{b},
    // eq_attack_{b}, eq_release_{b}. On S-series the wire band is reversed
    // relative to the internal model — see `pad_eq_band_map`.
    if let Some(b_str) = rest.strip_prefix("eq_freq_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandFrequency(pad_eq_band_map(
            wire, reversed,
        )?));
    }
    if let Some(b_str) = rest.strip_prefix("eq_gain_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandGain(pad_eq_band_map(wire, reversed)?));
    }
    if let Some(b_str) = rest.strip_prefix("eq_Q_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandQ(pad_eq_band_map(wire, reversed)?));
    }
    if let Some(b_str) = rest.strip_prefix("eq_curve_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandCurve(pad_eq_band_map(wire, reversed)?));
    }
    if let Some(b_str) = rest.strip_prefix("dynamic_eq_on_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandDynEnabled(pad_eq_band_map(
            wire, reversed,
        )?));
    }
    if let Some(b_str) = rest.strip_prefix("eq_thresh_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandDynThreshold(pad_eq_band_map(
            wire, reversed,
        )?));
    }
    if let Some(b_str) = rest.strip_prefix("eq_over-under_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandDynOverUnder(pad_eq_band_map(
            wire, reversed,
        )?));
    }
    if let Some(b_str) = rest.strip_prefix("eq_ratio_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandDynRatio(pad_eq_band_map(
            wire, reversed,
        )?));
    }
    if let Some(b_str) = rest.strip_prefix("eq_attack_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandDynAttack(pad_eq_band_map(
            wire, reversed,
        )?));
    }
    if let Some(b_str) = rest.strip_prefix("eq_release_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::EqBandDynRelease(pad_eq_band_map(
            wire, reversed,
        )?));
    }
    None
}

/// The S21 multiband compressor numbers its three bands Low=1, High=2,
/// Mid=3 — it swaps the Mid and High bands relative to the internal/GP-OSC order
/// (internal 1=Low, 2=Mid, 3=High). Map a Pad multiband band index to the
/// internal index; the swap is its own inverse, so the same function converts in
/// both directions. With `swapped = false` the index passes through unchanged.
/// Returns `None` outside the 1..=3 band range either way.
///
/// Verified against the live desk: iPad `comp_thresh_3` and GP `dyn1/1`
/// (internal band 2, Mid) carry the same value, as do iPad `comp_thresh_2` and
/// GP `dyn1/2` (internal band 3, High). Without the swap on S-series,
/// iPad-sourced multiband updates land on the swapped band and collide with the
/// (correct) GP-OSC mirror writes, collapsing bands 2↔3.
fn pad_dyn1_band_map(band: u8, swapped: bool) -> Option<u8> {
    match (band, swapped) {
        (1, _) => Some(1),
        (2, true) => Some(3),
        (3, true) => Some(2),
        (2, false) => Some(2),
        (3, false) => Some(3),
        _ => None,
    }
}

/// Parse Pad Dyn1 multiband suffix (after "Dynamics/comp_").
fn parse_pad_dyn1_suffix(rest: &str, swapped: bool) -> Option<ParameterPath> {
    // Multiband bands: comp_thresh_{b}, comp_knee_{b}, comp_ratio_{b}, etc. On
    // S-series the wire band has Mid/High swapped relative to the internal
    // model — see `pad_dyn1_band_map`.
    if let Some(b_str) = rest.strip_prefix("thresh_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1Threshold(pad_dyn1_band_map(
            wire, swapped,
        )?));
    }
    if let Some(b_str) = rest.strip_prefix("knee_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1Knee(pad_dyn1_band_map(wire, swapped)?));
    }
    if let Some(b_str) = rest.strip_prefix("ratio_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1Ratio(pad_dyn1_band_map(wire, swapped)?));
    }
    if let Some(b_str) = rest.strip_prefix("attack_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1Attack(pad_dyn1_band_map(wire, swapped)?));
    }
    if let Some(b_str) = rest.strip_prefix("release_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1Release(pad_dyn1_band_map(
            wire, swapped,
        )?));
    }
    if let Some(b_str) = rest.strip_prefix("auto-gain_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1Gain(pad_dyn1_band_map(wire, swapped)?));
    }
    if let Some(b_str) = rest.strip_prefix("listen_") {
        let wire: u8 = b_str.parse().ok()?;
        return Some(ParameterPath::Dyn1Listen(pad_dyn1_band_map(wire, swapped)?));
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
    let n: u16 = n_str.parse().ok()?;
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
    let n: u16 = n_str.parse().ok()?;
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
    let n: u16 = n_str.parse().ok()?;
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
    /// Extract the float value, if this is a Float variant.
    pub fn as_float(&self) -> Option<f32> {
        match self {
            ParameterValue::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Extract the bool value, if this is a Bool variant.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ParameterValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

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

/// Floored dB interpolation for fader-family levels (see [`FADER_FADE_FLOOR_DB`]).
/// Both endpoints are clamped UP to `floor`, interpolated linearly in that
/// floored space, and a result at/below the floor is reported as [`FADER_INF_DB`]
/// (fully off). Mirrors the gang engine's floor/snap pattern (`gang_floor` /
/// `apply_fader_gang_delta`) but for fades.
///
/// Exactness: at `t >= 1.0` the RAW `end` is returned unchanged, so a fade
/// landing on true-off ends at exactly −150 and a fade to a real sub-floor level
/// (e.g. −85) ends exactly there rather than being snapped to −inf or to the
/// floor.
pub(crate) fn floored_db_lerp(start: f32, end: f32, t: f32, floor: f32) -> f32 {
    if t >= 1.0 {
        return end; // land exactly on the true target (incl. −150 / sub-floor)
    }
    let s = start.max(floor);
    let e = end.max(floor);
    let v = s + (e - s) * t;
    if v <= floor { FADER_INF_DB } else { v }
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
        assert_eq!(
            ParameterPath::Fader.section(),
            ParameterSection::FaderMutePan
        );
        assert_eq!(
            ParameterPath::Mute.section(),
            ParameterSection::FaderMutePan
        );
        assert_eq!(ParameterPath::Pan.section(), ParameterSection::FaderMutePan);
        assert_eq!(
            ParameterPath::AnalogGain.section(),
            ParameterSection::InputGain
        );
        assert_eq!(ParameterPath::Trim.section(), ParameterSection::Trim);
        assert_eq!(
            ParameterPath::Polarity.section(),
            ParameterSection::Polarity
        );
        assert_eq!(
            ParameterPath::Balance.section(),
            ParameterSection::BalanceWidth
        );
        assert_eq!(
            ParameterPath::Width.section(),
            ParameterSection::BalanceWidth
        );
        assert_eq!(
            ParameterPath::Phantom.section(),
            ParameterSection::InputGain
        );
        assert_eq!(
            ParameterPath::DelayEnabled.section(),
            ParameterSection::Delay
        );
        assert_eq!(
            ParameterPath::DigitubeEnabled.section(),
            ParameterSection::Digitube
        );
        assert_eq!(ParameterPath::EqEnabled.section(), ParameterSection::Eq);
        assert_eq!(
            ParameterPath::EqBandFrequency(1).section(),
            ParameterSection::Eq
        );
        assert_eq!(
            ParameterPath::EqBandDynEnabled(2).section(),
            ParameterSection::Eq
        );
        assert_eq!(
            ParameterPath::HighpassFrequency.section(),
            ParameterSection::Eq
        );
        assert_eq!(ParameterPath::Dyn1Enabled.section(), ParameterSection::Dyn1);
        assert_eq!(
            ParameterPath::Dyn1Threshold(1).section(),
            ParameterSection::Dyn1
        );
        assert_eq!(ParameterPath::Dyn2Enabled.section(), ParameterSection::Dyn2);
        assert_eq!(ParameterPath::Dyn2Range.section(), ParameterSection::Dyn2);
        assert_eq!(
            ParameterPath::SendLevel(1).section(),
            ParameterSection::Sends
        );
        assert_eq!(
            ParameterPath::GroupSendOn(1).section(),
            ParameterSection::GroupRouting
        );
        assert_eq!(
            ParameterPath::MasterBusOn.section(),
            ParameterSection::GroupRouting
        );
        assert_eq!(
            ParameterPath::InsertAEnabled.section(),
            ParameterSection::Inserts
        );
        assert_eq!(
            ParameterPath::CgLevel.section(),
            ParameterSection::CgMembership
        );
        assert_eq!(
            ParameterPath::MatrixSendLevel(1).section(),
            ParameterSection::MatrixSends
        );
        assert_eq!(
            ParameterPath::GeqBandGain(1).section(),
            ParameterSection::GraphicEq
        );
        assert_eq!(
            ParameterPath::GeqEnabled.section(),
            ParameterSection::GraphicEq
        );
    }

    #[test]
    fn parameter_section_applicable_to_aux_includes_trim_polarity_delay_digitube() {
        // Per the DiGiCo S OSC chart, aux channels expose trim, polarity,
        // delay and digitube — these all need to be gangable on aux.
        let sections = ParameterSection::applicable_to(&ChannelId::Aux(1));
        assert!(sections.contains(&ParameterSection::Trim));
        assert!(sections.contains(&ParameterSection::Polarity));
        assert!(sections.contains(&ParameterSection::Delay));
        assert!(sections.contains(&ParameterSection::Digitube));
        // Stereo balance/width is input-only on the S21.
        assert!(!sections.contains(&ParameterSection::BalanceWidth));
    }

    #[test]
    fn parameter_section_applicable_to_matrix_now_includes_inserts() {
        let sections = ParameterSection::applicable_to(&ChannelId::Matrix(1));
        assert!(sections.contains(&ParameterSection::Trim));
        assert!(sections.contains(&ParameterSection::Polarity));
        assert!(sections.contains(&ParameterSection::Delay));
        assert!(sections.contains(&ParameterSection::Digitube));
        assert!(sections.contains(&ParameterSection::Inserts));
    }

    #[test]
    fn parameter_section_applicable_to_input_includes_balance_width() {
        let sections = ParameterSection::applicable_to(&ChannelId::Input(1));
        assert!(sections.contains(&ParameterSection::BalanceWidth));
        assert!(sections.contains(&ParameterSection::Trim));
        assert!(sections.contains(&ParameterSection::Polarity));
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
            ParameterPath::AnalogGain,
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
            let suffix = path
                .to_ipad_suffix()
                .unwrap_or_else(|| panic!("to_ipad_suffix returned None for {path:?}"));
            // from_ipad_suffix expects leading /
            let parsed = ParameterPath::from_ipad_suffix(&format!("/{suffix}"))
                .unwrap_or_else(|| panic!("from_ipad_suffix failed for /{suffix} (from {path:?})"));
            assert_eq!(parsed, path, "iPad round-trip failed for suffix: {suffix}");
        }
    }

    /// Every band-quirk combination must round-trip, not just the S21 one.
    /// The golden tests above pin the S21 wire strings; this pins the
    /// *involution* property across all four combinations, so a family whose
    /// probe disproves either quirk still decodes what it encodes.
    #[test]
    fn pad_suffix_round_trips_under_every_band_quirk_combo() {
        use crate::model::family::PadQuirks;

        let mut paths: Vec<ParameterPath> = Vec::new();
        for b in ParameterPath::EQ_BAND_RANGE {
            paths.extend([
                ParameterPath::EqBandFrequency(b),
                ParameterPath::EqBandGain(b),
                ParameterPath::EqBandQ(b),
                ParameterPath::EqBandCurve(b),
                ParameterPath::EqBandDynEnabled(b),
                ParameterPath::EqBandDynThreshold(b),
                ParameterPath::EqBandDynRatio(b),
                ParameterPath::EqBandDynAttack(b),
                ParameterPath::EqBandDynRelease(b),
                ParameterPath::EqBandDynOverUnder(b),
            ]);
        }
        for b in ParameterPath::DYN1_BAND_RANGE {
            paths.extend([
                ParameterPath::Dyn1Threshold(b),
                ParameterPath::Dyn1Knee(b),
                ParameterPath::Dyn1Ratio(b),
                ParameterPath::Dyn1Attack(b),
                ParameterPath::Dyn1Release(b),
                ParameterPath::Dyn1Gain(b),
                ParameterPath::Dyn1Listen(b),
            ]);
        }
        // A few non-band paths to prove the quirks don't leak sideways.
        paths.extend([
            ParameterPath::Fader,
            ParameterPath::Pan,
            ParameterPath::SendLevel(3),
            ParameterPath::GeqBandGain(16),
        ]);

        for eq_bands_reversed in [true, false] {
            for dyn1_mid_high_swapped in [true, false] {
                let q = PadQuirks {
                    eq_bands_reversed,
                    dyn1_mid_high_swapped,
                    ..PadQuirks::S21
                };
                for path in &paths {
                    let suffix = path.to_pad_suffix(&q).unwrap_or_else(|| {
                        panic!("to_pad_suffix returned None for {path:?} under {q:?}")
                    });
                    let parsed = ParameterPath::from_pad_suffix(&format!("/{suffix}"), &q)
                        .unwrap_or_else(|| {
                            panic!("from_pad_suffix failed for /{suffix} ({path:?}) under {q:?}")
                        });
                    assert_eq!(&parsed, path, "round-trip failed for /{suffix} under {q:?}");
                }
            }
        }
    }

    /// With the S21 band quirks disabled, wire indices must equal internal
    /// ones — the whole point of the parameterization.
    #[test]
    fn pad_suffix_without_band_quirks_uses_identity_indices() {
        use crate::model::family::PadQuirks;
        let q = PadQuirks {
            eq_bands_reversed: false,
            dyn1_mid_high_swapped: false,
            ..PadQuirks::S21
        };
        assert_eq!(
            ParameterPath::EqBandGain(2).to_pad_suffix(&q).unwrap(),
            "EQ/eq_gain_2"
        );
        assert_eq!(
            ParameterPath::EqBandFrequency(1).to_pad_suffix(&q).unwrap(),
            "EQ/eq_freq_1"
        );
        assert_eq!(
            ParameterPath::Dyn1Threshold(3).to_pad_suffix(&q).unwrap(),
            "Dynamics/comp_thresh_3"
        );
        assert_eq!(
            ParameterPath::Dyn1Listen(2).to_pad_suffix(&q).unwrap(),
            "Dynamics/comp_listen_2"
        );
        // And the S21 quirks still reverse/swap (the golden behaviour).
        assert_eq!(
            ParameterPath::EqBandGain(2)
                .to_pad_suffix(&PadQuirks::S21)
                .unwrap(),
            "EQ/eq_gain_3"
        );
        assert_eq!(
            ParameterPath::Dyn1Threshold(3)
                .to_pad_suffix(&PadQuirks::S21)
                .unwrap(),
            "Dynamics/comp_thresh_2"
        );
    }

    /// Out-of-range band indices stay rejected regardless of quirk state.
    ///
    /// The EQ ceiling is quirk-dependent by design: the reversal is `5 - band`,
    /// four-band arithmetic only meaningful on an S21 strip, whereas an
    /// unreversed strip runs to eight bands on SD/Quantum bus outputs. Band
    /// zero and the dyn1 range are absolute either way.
    #[test]
    fn pad_band_maps_reject_out_of_range_under_both_settings() {
        use crate::model::family::PadQuirks;
        for reversed in [true, false] {
            let q = PadQuirks {
                eq_bands_reversed: reversed,
                dyn1_mid_high_swapped: reversed,
                ..PadQuirks::S21
            };
            assert!(ParameterPath::EqBandGain(0).to_pad_suffix(&q).is_none());
            assert!(ParameterPath::Dyn1Threshold(4).to_pad_suffix(&q).is_none());
            assert!(ParameterPath::from_pad_suffix("/Dynamics/comp_thresh_4", &q).is_none());

            if reversed {
                // S21: nothing above band 4 exists to reverse.
                assert!(ParameterPath::EqBandGain(5).to_pad_suffix(&q).is_none());
                assert!(ParameterPath::from_pad_suffix("/EQ/eq_gain_5", &q).is_none());
            } else {
                // SD/Quantum bus outputs: bands 5..=8 are real and round-trip.
                let wire = ParameterPath::EqBandGain(5).to_pad_suffix(&q).unwrap();
                assert_eq!(wire, "EQ/eq_gain_5");
                assert_eq!(
                    ParameterPath::from_pad_suffix("/EQ/eq_gain_5", &q),
                    Some(ParameterPath::EqBandGain(5))
                );
                assert!(ParameterPath::EqBandGain(9).to_pad_suffix(&q).is_none());
                assert!(ParameterPath::from_pad_suffix("/EQ/eq_gain_9", &q).is_none());
            }
        }
    }

    // ── Per-family support table ────────────────────────────────────────

    #[test]
    fn s_series_supports_every_parameter() {
        // The whole enum came from the S21 command set, so nothing is gated
        // away on S-series — this is what guarantees Phase 1 changes no
        // existing behaviour.
        for ch in [
            ChannelId::Input(1),
            ChannelId::Aux(1),
            ChannelId::Group(1),
            ChannelId::Matrix(1),
            ChannelId::ControlGroup(1),
            ChannelId::GraphicEq(1),
            ChannelId::MatrixInput(1),
        ] {
            for p in ParameterPath::applicable_to(&ch, 8, 8, 8) {
                assert_eq!(
                    p.support(ConsoleFamily::SSeries),
                    ParamSupport::Verified,
                    "{p:?} should be Verified on S-series"
                );
            }
            assert_eq!(
                ParameterPath::applicable_to(&ch, 8, 8, 8),
                ParameterPath::applicable_to_for_family(&ch, 8, 8, 8, ConsoleFamily::SSeries),
                "family filtering must be a no-op on S-series for {ch:?}"
            );
        }
    }

    #[test]
    fn pad_only_families_gate_gp_only_and_s21_oddities() {
        for family in [ConsoleFamily::SdRange, ConsoleFamily::Quantum] {
            // Core Pad tree survives.
            for p in [
                ParameterPath::Fader,
                ParameterPath::Mute,
                ParameterPath::Pan,
                ParameterPath::SendLevel(3),
                ParameterPath::AnalogGain,
                ParameterPath::EqBandGain(2),
                ParameterPath::Dyn1Threshold(1),
                ParameterPath::GeqBandGain(4),
            ] {
                assert_eq!(
                    p.support(family),
                    ParamSupport::Assumed,
                    "{p:?} should be Assumed on {family:?}"
                );
            }
            // GP-only and S21-dialect oddities are gated off.
            for p in [
                ParameterPath::TotalGain,
                ParameterPath::DigitubeDrive,
                ParameterPath::Balance,
                ParameterPath::MasterBusOn,
                ParameterPath::CgLevel,
                ParameterPath::Dyn1Threshold(2),
                ParameterPath::EqBandDynEnabled(1),
                ParameterPath::Dyn2KeySolo,
            ] {
                assert_eq!(
                    p.support(family),
                    ParamSupport::Unsupported,
                    "{p:?} should be Unsupported on {family:?}"
                );
            }
        }
    }

    /// The support table and the codec must agree: anything we're willing to
    /// use on a family has to have a Pad path to use it through.
    #[test]
    fn eq_strip_is_four_bands_on_s_series_and_eight_on_sd_outputs() {
        use crate::model::family::ConsoleFamily as F;
        // S-series is four bands everywhere, inputs and outputs alike.
        for ch in [ChannelId::Input(1), ChannelId::Aux(1), ChannelId::Group(1)] {
            assert_eq!(
                ParameterPath::eq_band_range(&ch, F::SSeries),
                1..=4,
                "{ch:?} on S-series"
            );
        }
        for family in [F::SdRange, F::Quantum] {
            // Inputs stay at four.
            assert_eq!(
                ParameterPath::eq_band_range(&ChannelId::Input(1), family),
                1..=4
            );
            // Bus outputs carry eight.
            for ch in [ChannelId::Aux(1), ChannelId::Group(2), ChannelId::Matrix(3)] {
                assert_eq!(
                    ParameterPath::eq_band_range(&ch, family),
                    1..=8,
                    "{ch:?} on {family:?}"
                );
            }
        }
    }

    #[test]
    fn applicable_to_exposes_the_extra_sd_output_bands() {
        use crate::model::family::ConsoleFamily as F;
        let has_band8 = |family: F| {
            ParameterPath::applicable_to_for_family(&ChannelId::Aux(1), 8, 8, 8, family)
                .contains(&ParameterPath::EqBandGain(8))
        };
        assert!(has_band8(F::Quantum), "a Quantum aux must offer EQ band 8");
        assert!(has_band8(F::SdRange), "an SD aux must offer EQ band 8");
        assert!(
            !has_band8(F::SSeries),
            "an S-series aux has only four bands"
        );
        // The plain (family-free) entry point keeps its S-series behaviour.
        assert!(
            !ParameterPath::applicable_to(&ChannelId::Aux(1), 8, 8, 8)
                .contains(&ParameterPath::EqBandGain(8))
        );
    }

    #[test]
    fn pad_eq_band_map_rejects_reversed_indices_beyond_a_four_band_strip() {
        // Unreversed (SD/Quantum): the full eight-band strip round-trips.
        for b in 1..=8u8 {
            assert_eq!(super::pad_eq_band_map(b, false), Some(b));
        }
        assert_eq!(super::pad_eq_band_map(9, false), None);
        // Reversed (S21): defined only on a four-band strip, and involutive.
        for b in 1..=4u8 {
            let wire = super::pad_eq_band_map(b, true).unwrap();
            assert_eq!(super::pad_eq_band_map(wire, true), Some(b));
        }
        for b in 5..=9u8 {
            assert_eq!(
                super::pad_eq_band_map(b, true),
                None,
                "band {b} cannot be reversed on a four-band strip"
            );
        }
    }

    #[test]
    fn support_implies_pad_path() {
        use crate::model::family::PadQuirks;
        let q = PadQuirks::SD_HYPOTHESIS;
        for family in [ConsoleFamily::SdRange, ConsoleFamily::Quantum] {
            for ch in [
                ChannelId::Input(1),
                ChannelId::Aux(1),
                ChannelId::Group(1),
                ChannelId::Matrix(1),
                ChannelId::ControlGroup(1),
                ChannelId::GraphicEq(1),
                ChannelId::MatrixInput(1),
            ] {
                for p in ParameterPath::applicable_to_for_family(&ch, 8, 8, 8, family) {
                    assert!(
                        p.to_pad_suffix(&q).is_some(),
                        "{p:?} is usable on {family:?} but has no Pad path"
                    );
                }
            }
        }
    }

    #[test]
    fn family_filtering_shrinks_the_pad_only_parameter_set() {
        let all = ParameterPath::applicable_to(&ChannelId::Input(1), 8, 8, 8);
        let quantum = ParameterPath::applicable_to_for_family(
            &ChannelId::Input(1),
            8,
            8,
            8,
            ConsoleFamily::Quantum,
        );
        assert!(
            quantum.len() < all.len(),
            "Quantum should expose fewer input params than S-series"
        );
        assert!(
            !quantum.is_empty(),
            "Quantum must still expose the core set"
        );
        assert!(quantum.contains(&ParameterPath::Fader));
        assert!(!quantum.contains(&ParameterPath::DigitubeDrive));
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
        assert_eq!(
            ParameterPath::InsertAEnabled.to_ipad_suffix().unwrap(),
            "Insert/insert_A_in"
        );
        assert_eq!(
            ParameterPath::GeqBandGain(1).to_ipad_suffix().unwrap(),
            "geq_gain_1"
        );
        assert_eq!(
            ParameterPath::SendLevel(3).to_ipad_suffix().unwrap(),
            "Aux_Send/3/send_level"
        );
        assert_eq!(
            ParameterPath::GroupSendOn(4).to_ipad_suffix().unwrap(),
            "Group_Send/4/send_on"
        );
        assert_eq!(
            ParameterPath::MasterBusOn.to_ipad_suffix().unwrap(),
            "Group_Send/17/send_on"
        );
    }

    #[test]
    fn is_continuous_true_for_levels_and_gains() {
        assert!(ParameterPath::Fader.is_continuous());
        assert!(ParameterPath::Pan.is_continuous());
        assert!(ParameterPath::AnalogGain.is_continuous());
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

    // ─── Fader-family fade floor (−inf handling) ────────────────────────

    #[test]
    fn fade_floor_db_set_membership() {
        // Fader-family dB-taper levels are floored.
        assert_eq!(
            ParameterPath::Fader.fade_floor_db(),
            Some(FADER_FADE_FLOOR_DB)
        );
        assert_eq!(
            ParameterPath::SendLevel(1).fade_floor_db(),
            Some(FADER_FADE_FLOOR_DB)
        );
        assert_eq!(
            ParameterPath::CgLevel.fade_floor_db(),
            Some(FADER_FADE_FLOOR_DB)
        );
        assert_eq!(
            ParameterPath::MatrixSendLevel(2).fade_floor_db(),
            Some(FADER_FADE_FLOOR_DB)
        );
        // Other continuous params keep naive interpolation.
        assert_eq!(ParameterPath::Pan.fade_floor_db(), None);
        assert_eq!(ParameterPath::EqBandGain(1).fade_floor_db(), None);
        assert_eq!(ParameterPath::Dyn1Threshold(1).fade_floor_db(), None);
        assert_eq!(ParameterPath::TotalGain.fade_floor_db(), None);
    }

    #[test]
    fn floored_lerp_to_off_lands_on_minus150() {
        // Fade 0 dB → off ends exactly at −150 at t=1.
        assert_eq!(
            floored_db_lerp(0.0, FADER_INF_DB, 1.0, FADER_FADE_FLOOR_DB),
            FADER_INF_DB
        );
    }

    #[test]
    fn floored_lerp_to_off_spreads_audible_and_lands_off() {
        let floor = FADER_FADE_FLOOR_DB;
        // The audible drop is spread linearly across the whole fade (0 → −80),
        // not crammed into a sliver.
        assert!((floored_db_lerp(0.0, FADER_INF_DB, 0.1, floor) - (-8.0)).abs() < 1e-4);
        assert!((floored_db_lerp(0.0, FADER_INF_DB, 0.5, floor) - (-40.0)).abs() < 1e-4);
        // Still audible (just above the floor) right up until the very end.
        let late = floored_db_lerp(0.0, FADER_INF_DB, 0.99, floor);
        assert!(
            late > floor && late < 0.0,
            "late {late} should be just above floor"
        );
        // Lands exactly on true off at t=1.
        assert_eq!(floored_db_lerp(0.0, FADER_INF_DB, 1.0, floor), FADER_INF_DB);
    }

    #[test]
    fn floored_lerp_from_off_spans_floor_to_target() {
        let floor = FADER_FADE_FLOOR_DB;
        // Just after the start we're just above the floor, NOT at −150.
        let early = floored_db_lerp(FADER_INF_DB, 0.0, 0.01, floor);
        assert!(
            early > floor && early < 0.0,
            "early {early} should be just above floor"
        );
        // Midway spans the floored band: −80 → 0 at t=0.5 ≈ −40.
        let mid = floored_db_lerp(FADER_INF_DB, 0.0, 0.5, floor);
        assert!((mid - (-40.0)).abs() < 1e-4, "mid {mid} expected ≈ -40");
        // Lands exactly on the target.
        assert_eq!(floored_db_lerp(FADER_INF_DB, 0.0, 1.0, floor), 0.0);
    }

    #[test]
    fn floored_lerp_to_real_low_value_is_exact() {
        // A real sub-floor target (−85) is preserved exactly at t=1, not snapped.
        assert_eq!(floored_db_lerp(0.0, -85.0, 1.0, FADER_FADE_FLOOR_DB), -85.0);
    }

    #[test]
    fn floored_lerp_both_subfloor_collapses_then_exact() {
        let floor = FADER_FADE_FLOOR_DB;
        // Both endpoints below the floor: −inf throughout, exact end at t=1.
        assert_eq!(floored_db_lerp(-150.0, -120.0, 0.5, floor), FADER_INF_DB);
        assert_eq!(floored_db_lerp(-150.0, -120.0, 1.0, floor), -120.0);
    }

    // ─── Phase 0: per-path scope granularity ────────────────────────────

    /// Sample of `ParameterPath` variants used by tests below. Includes at
    /// least one variant from every section so the matrix-test asserts touch
    /// every category.
    fn sample_paths() -> Vec<ParameterPath> {
        vec![
            ParameterPath::Name,
            ParameterPath::Fader,
            ParameterPath::Mute,
            ParameterPath::Solo,
            ParameterPath::Pan,
            ParameterPath::AnalogGain,
            ParameterPath::TotalGain,
            ParameterPath::GainTracking,
            ParameterPath::Trim,
            ParameterPath::Balance,
            ParameterPath::Width,
            ParameterPath::Polarity,
            ParameterPath::Phantom,
            ParameterPath::MainAltIn,
            ParameterPath::StereoMode,
            ParameterPath::DelayEnabled,
            ParameterPath::DelayTime,
            ParameterPath::DigitubeEnabled,
            ParameterPath::DigitubeDrive,
            ParameterPath::DigitubeBias,
            ParameterPath::EqEnabled,
            ParameterPath::HighpassEnabled,
            ParameterPath::HighpassFrequency,
            ParameterPath::LowpassEnabled,
            ParameterPath::LowpassFrequency,
            ParameterPath::EqBandFrequency(1),
            ParameterPath::EqBandGain(2),
            ParameterPath::EqBandQ(3),
            ParameterPath::EqBandCurve(4),
            ParameterPath::EqBandDynEnabled(1),
            ParameterPath::EqBandDynThreshold(2),
            ParameterPath::EqBandDynRatio(3),
            ParameterPath::EqBandDynAttack(4),
            ParameterPath::EqBandDynRelease(1),
            ParameterPath::EqBandDynOverUnder(2),
            ParameterPath::Dyn1Enabled,
            ParameterPath::Dyn1Mode,
            ParameterPath::Dyn1MultibandDeesser,
            ParameterPath::Dyn1Threshold(1),
            ParameterPath::Dyn1Knee(2),
            ParameterPath::Dyn1Ratio(3),
            ParameterPath::Dyn1Attack(1),
            ParameterPath::Dyn1Release(2),
            ParameterPath::Dyn1Gain(3),
            ParameterPath::Dyn1Listen(1),
            ParameterPath::Dyn1CrossoverHigh,
            ParameterPath::Dyn1CrossoverLow,
            ParameterPath::Dyn2Enabled,
            ParameterPath::Dyn2Mode,
            ParameterPath::Dyn2Threshold,
            ParameterPath::Dyn2Knee,
            ParameterPath::Dyn2Ratio,
            ParameterPath::Dyn2Range,
            ParameterPath::Dyn2Attack,
            ParameterPath::Dyn2Hold,
            ParameterPath::Dyn2Release,
            ParameterPath::Dyn2Gain,
            ParameterPath::Dyn2Highpass,
            ParameterPath::Dyn2Lowpass,
            ParameterPath::Dyn2Listen,
            ParameterPath::Dyn2KeySolo,
            ParameterPath::SendEnabled(1),
            ParameterPath::SendLevel(2),
            ParameterPath::SendPan(3),
            ParameterPath::GroupSendOn(1),
            ParameterPath::MasterBusOn,
            ParameterPath::InsertAEnabled,
            ParameterPath::InsertBEnabled,
            ParameterPath::CgLevel,
            ParameterPath::CgMute,
            ParameterPath::MatrixSendLevel(1),
            ParameterPath::MatrixSendOn(2),
            ParameterPath::GeqEnabled,
            ParameterPath::GeqBandGain(5),
        ]
    }

    #[test]
    fn parameter_path_label_is_unique_per_variant() {
        use std::collections::HashSet;
        let mut labels = HashSet::new();
        for path in sample_paths() {
            let label = path.label();
            assert!(!label.is_empty(), "empty label for {path:?}");
            assert!(
                labels.insert(label.clone()),
                "duplicate label '{label}' on {path:?}",
            );
        }
    }

    #[test]
    fn available_for_channel_pan_is_input_only() {
        assert!(ParameterPath::Pan.available_for_channel(&ChannelId::Input(1)));
        assert!(!ParameterPath::Pan.available_for_channel(&ChannelId::Aux(1)));
        assert!(!ParameterPath::Pan.available_for_channel(&ChannelId::Group(1)));
        assert!(!ParameterPath::Pan.available_for_channel(&ChannelId::Matrix(1)));
        assert!(!ParameterPath::Pan.available_for_channel(&ChannelId::ControlGroup(1)));
    }

    #[test]
    fn available_for_channel_analog_gain_is_input_only() {
        assert!(ParameterPath::AnalogGain.available_for_channel(&ChannelId::Input(1)));
        assert!(!ParameterPath::AnalogGain.available_for_channel(&ChannelId::Aux(1)));
        assert!(!ParameterPath::AnalogGain.available_for_channel(&ChannelId::Group(1)));
        assert!(!ParameterPath::AnalogGain.available_for_channel(&ChannelId::Matrix(1)));
        assert!(!ParameterPath::AnalogGain.available_for_channel(&ChannelId::ControlGroup(1)));
    }

    // ─── Gain split (audit H10): TotalGain (GP OSC) vs AnalogGain (iPad) ────

    #[test]
    fn total_gain_round_trips_through_gp_osc() {
        // The two physical knobs map to disjoint wire paths, no overlap.
        assert_eq!(
            ParameterPath::TotalGain.to_gp_osc_suffix().as_deref(),
            Some("total/gain"),
        );
        assert_eq!(
            ParameterPath::from_gp_osc_suffix("total/gain"),
            Some(ParameterPath::TotalGain),
        );
    }

    #[test]
    fn total_gain_has_no_ipad_path() {
        assert!(ParameterPath::TotalGain.to_ipad_suffix().is_none());
    }

    #[test]
    fn analog_gain_has_no_gp_osc_path() {
        assert!(ParameterPath::AnalogGain.to_gp_osc_suffix().is_none());
    }

    #[test]
    fn legacy_gain_serde_alias_migrates_to_total_gain() {
        // Pre-Phase-6 snapshot files used `"Gain"` for what was always GP OSC
        // total/gain. The serde alias is now on TotalGain so those legacy
        // values land on the correct physical knob on load.
        let path: ParameterPath = serde_json::from_str(r#""Gain""#).unwrap();
        assert_eq!(path, ParameterPath::TotalGain);
    }

    #[test]
    fn analog_gain_round_trips_without_aliasing_legacy_gain() {
        // After moving the alias off AnalogGain, freshly-serialized
        // AnalogGain still round-trips by its real name.
        let json = serde_json::to_string(&ParameterPath::AnalogGain).unwrap();
        assert_eq!(json, r#""AnalogGain""#);
        let parsed: ParameterPath = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, ParameterPath::AnalogGain);
    }

    #[test]
    fn total_gain_section_is_input_gain() {
        assert_eq!(
            ParameterPath::TotalGain.section(),
            ParameterSection::InputGain
        );
    }

    #[test]
    fn total_gain_is_continuous() {
        assert!(ParameterPath::TotalGain.is_continuous());
    }

    #[test]
    fn total_gain_is_input_only() {
        assert!(ParameterPath::TotalGain.available_for_channel(&ChannelId::Input(1)));
        assert!(!ParameterPath::TotalGain.available_for_channel(&ChannelId::Aux(1)));
        assert!(!ParameterPath::TotalGain.available_for_channel(&ChannelId::Group(1)));
        assert!(!ParameterPath::TotalGain.available_for_channel(&ChannelId::Matrix(1)));
        assert!(!ParameterPath::TotalGain.available_for_channel(&ChannelId::ControlGroup(1)));
    }

    #[test]
    fn total_gain_has_no_timing_category() {
        // Same instant-recall behavior as AnalogGain.
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::TotalGain),
            None
        );
    }

    #[test]
    fn available_for_channel_send_is_input_only() {
        let s = ParameterPath::SendLevel(1);
        assert!(s.available_for_channel(&ChannelId::Input(1)));
        assert!(!s.available_for_channel(&ChannelId::Aux(1)));
        assert!(!s.available_for_channel(&ChannelId::Group(1)));
    }

    #[test]
    fn available_for_channel_cg_only_has_name_mute_solo_fader() {
        let cg = ChannelId::ControlGroup(1);
        // The four universal verbs.
        assert!(ParameterPath::Name.available_for_channel(&cg));
        assert!(ParameterPath::Mute.available_for_channel(&cg));
        assert!(ParameterPath::Solo.available_for_channel(&cg));
        assert!(ParameterPath::Fader.available_for_channel(&cg));
        // Everything else from the sample is excluded.
        for path in sample_paths() {
            if matches!(
                path,
                ParameterPath::Name
                    | ParameterPath::Mute
                    | ParameterPath::Solo
                    | ParameterPath::Fader
            ) {
                continue;
            }
            assert!(
                !path.available_for_channel(&cg),
                "{path:?} should NOT be available on Control Group",
            );
        }
    }

    #[test]
    fn available_for_channel_eq_dyn_apply_to_input_aux_grp_mtx_but_not_cg() {
        let eq = ParameterPath::EqBandGain(1);
        assert!(eq.available_for_channel(&ChannelId::Input(1)));
        assert!(eq.available_for_channel(&ChannelId::Aux(1)));
        assert!(eq.available_for_channel(&ChannelId::Group(1)));
        assert!(eq.available_for_channel(&ChannelId::Matrix(1)));
        assert!(!eq.available_for_channel(&ChannelId::ControlGroup(1)));

        let dyn1 = ParameterPath::Dyn1Threshold(1);
        assert!(dyn1.available_for_channel(&ChannelId::Input(1)));
        assert!(dyn1.available_for_channel(&ChannelId::Aux(1)));
        assert!(!dyn1.available_for_channel(&ChannelId::ControlGroup(1)));
    }

    #[test]
    fn available_for_channel_geq_only_on_graphic_eq_channel() {
        let geq = ParameterPath::GeqBandGain(5);
        assert!(geq.available_for_channel(&ChannelId::GraphicEq(1)));
        assert!(!geq.available_for_channel(&ChannelId::Input(1)));
        assert!(!geq.available_for_channel(&ChannelId::Aux(1)));
    }

    #[test]
    fn available_for_channel_matrix_send_only_on_matrix_input_channel() {
        let m = ParameterPath::MatrixSendLevel(1);
        assert!(m.available_for_channel(&ChannelId::MatrixInput(1)));
        assert!(!m.available_for_channel(&ChannelId::Input(1)));
        assert!(!m.available_for_channel(&ChannelId::Matrix(1)));
    }

    #[test]
    fn graphic_eq_channel_supports_only_geq_and_universal_four() {
        let geq_ch = ChannelId::GraphicEq(1);
        // Universal four:
        assert!(ParameterPath::Name.available_for_channel(&geq_ch));
        assert!(ParameterPath::Mute.available_for_channel(&geq_ch));
        assert!(ParameterPath::Solo.available_for_channel(&geq_ch));
        assert!(ParameterPath::Fader.available_for_channel(&geq_ch));
        // GEQ-specific:
        assert!(ParameterPath::GeqEnabled.available_for_channel(&geq_ch));
        assert!(ParameterPath::GeqBandGain(1).available_for_channel(&geq_ch));
        // Channel-processing paths are NOT applicable to a GEQ channel:
        assert!(!ParameterPath::EqBandGain(1).available_for_channel(&geq_ch));
        assert!(!ParameterPath::Dyn1Enabled.available_for_channel(&geq_ch));
        assert!(!ParameterPath::Trim.available_for_channel(&geq_ch));
    }

    #[test]
    fn applicable_to_input_includes_eq_bands_1_through_4() {
        let paths = ParameterPath::applicable_to(&ChannelId::Input(1), 8, 8, 8);
        for b in 1..=4 {
            assert!(
                paths.contains(&ParameterPath::EqBandFrequency(b)),
                "missing EqBandFrequency({b}) for Input",
            );
            assert!(paths.contains(&ParameterPath::EqBandGain(b)));
            assert!(paths.contains(&ParameterPath::EqBandQ(b)));
        }
    }

    #[test]
    fn applicable_to_input_excludes_total_gain() {
        // TotalGain is a console-derived, read-only monitor value — it must not
        // be selectable for capture/recall, while its InputGain siblings remain.
        let paths = ParameterPath::applicable_to(&ChannelId::Input(1), 8, 8, 8);
        assert!(!paths.contains(&ParameterPath::TotalGain));
        assert!(paths.contains(&ParameterPath::AnalogGain));
        assert!(paths.contains(&ParameterPath::GainTracking));
    }

    #[test]
    fn applicable_to_aux_excludes_pan_and_analog_gain() {
        let paths = ParameterPath::applicable_to(&ChannelId::Aux(1), 8, 8, 8);
        assert!(!paths.contains(&ParameterPath::Pan));
        assert!(!paths.contains(&ParameterPath::AnalogGain));
        assert!(!paths.contains(&ParameterPath::SendLevel(1)));
        // EQ + Dyn still apply.
        assert!(paths.contains(&ParameterPath::EqBandFrequency(1)));
        assert!(paths.contains(&ParameterPath::Dyn1Threshold(1)));
        // Universal four.
        assert!(paths.contains(&ParameterPath::Fader));
        assert!(paths.contains(&ParameterPath::Mute));
    }

    #[test]
    fn applicable_to_cg_returns_only_four_paths() {
        let paths = ParameterPath::applicable_to(&ChannelId::ControlGroup(1), 8, 8, 8);
        assert_eq!(paths.len(), 4);
        assert!(paths.contains(&ParameterPath::Name));
        assert!(paths.contains(&ParameterPath::Fader));
        assert!(paths.contains(&ParameterPath::Mute));
        assert!(paths.contains(&ParameterPath::Solo));
    }

    #[test]
    fn applicable_to_input_includes_send_count_rows() {
        // The send-row range covers EVERY mix output bus (aux + group),
        // because GP OSC `send/{n}/*` is one path family for both kinds.
        // 12 aux + 8 group = 20 buses total, so SendLevel(1..=20) should
        // be present and SendLevel(21) should not.
        let paths = ParameterPath::applicable_to(&ChannelId::Input(1), 12, 8, 8);
        assert!(paths.contains(&ParameterPath::SendLevel(1)));
        assert!(paths.contains(&ParameterPath::SendLevel(12)));
        assert!(paths.contains(&ParameterPath::SendLevel(20)));
        assert!(!paths.contains(&ParameterPath::SendLevel(21)));
    }

    #[test]
    fn applicable_to_send_count_uses_aux_plus_group() {
        // 8 + 8 = 16 buses (S21 base config).
        let paths = ParameterPath::applicable_to(&ChannelId::Input(1), 8, 8, 10);
        for s in 1..=16 {
            assert!(
                paths.contains(&ParameterPath::SendLevel(s)),
                "missing SendLevel({s}) — should be present for 8+8=16 buses",
            );
        }
        assert!(!paths.contains(&ParameterPath::SendLevel(17)));
    }

    #[test]
    fn applicable_to_orders_deterministically() {
        let a = ParameterPath::applicable_to(&ChannelId::Input(1), 8, 8, 8);
        let b = ParameterPath::applicable_to(&ChannelId::Input(1), 8, 8, 8);
        assert_eq!(a, b);
    }

    #[test]
    fn label_with_config_uses_bus_type_for_send_paths() {
        use crate::model::config::{ChannelMode, ConsoleConfig};
        let mut config = ConsoleConfig::default();
        // Bus 1 = aux, bus 2 = group (stereo).
        config.aux_output_count = 1;
        config.group_output_count = 1;
        config.mix_output_types = vec![true, false];
        config.mix_output_modes = vec![ChannelMode::Mono, ChannelMode::Stereo];

        assert_eq!(
            ParameterPath::SendLevel(1).label_with_config(&config),
            "Aux 1 Send Level",
        );
        assert_eq!(
            ParameterPath::SendEnabled(2).label_with_config(&config),
            "Group 1 (Stereo) Send On",
        );
        assert_eq!(
            ParameterPath::SendPan(2).label_with_config(&config),
            "Group 1 (Stereo) Send Pan",
        );
    }

    #[test]
    fn label_with_config_falls_through_for_non_send_paths() {
        use crate::model::config::ConsoleConfig;
        let config = ConsoleConfig::default();
        // Non-send paths should produce the same label() output.
        assert_eq!(
            ParameterPath::EqBandFrequency(1).label_with_config(&config),
            ParameterPath::EqBandFrequency(1).label(),
        );
        assert_eq!(
            ParameterPath::Fader.label_with_config(&config),
            ParameterPath::Fader.label(),
        );
    }

    #[test]
    fn paths_for_eq_section_returns_band_variants() {
        use crate::model::config::ConsoleConfig;
        let config = ConsoleConfig::default();
        let paths = ParameterSection::Eq.paths_for(&ChannelId::Input(1), &config);
        // Globals: EqEnabled, HighpassEnabled, HighpassFrequency,
        // LowpassEnabled, LowpassFrequency = 5. Per band (4 bands):
        // 10 fields each = 40. Total 45.
        assert_eq!(paths.len(), 45);
        assert!(paths.contains(&ParameterPath::EqEnabled));
        assert!(paths.contains(&ParameterPath::EqBandFrequency(1)));
        assert!(paths.contains(&ParameterPath::EqBandFrequency(4)));
        assert!(paths.iter().all(|p| p.section() == ParameterSection::Eq));
    }

    #[test]
    fn paths_for_sends_uses_aux_plus_group_count_from_config() {
        use crate::model::config::ConsoleConfig;
        let mut config = ConsoleConfig::default();
        config.aux_output_count = 4;
        config.group_output_count = 2;
        // Sends section enumerates SendEnabled/Level/Pan per bus.
        let paths = ParameterSection::Sends.paths_for(&ChannelId::Input(1), &config);
        let bus_count = (config.aux_output_count + config.group_output_count) as usize;
        let send_enabled = paths
            .iter()
            .filter(|p| matches!(p, ParameterPath::SendEnabled(_)))
            .count();
        let send_level = paths
            .iter()
            .filter(|p| matches!(p, ParameterPath::SendLevel(_)))
            .count();
        let send_pan = paths
            .iter()
            .filter(|p| matches!(p, ParameterPath::SendPan(_)))
            .count();
        assert_eq!(send_enabled, bus_count);
        assert_eq!(send_level, bus_count);
        assert_eq!(send_pan, bus_count);
    }

    #[test]
    fn paths_for_inapplicable_section_returns_empty() {
        use crate::model::config::ConsoleConfig;
        let config = ConsoleConfig::default();
        // CG channels don't have an EQ section.
        let paths = ParameterSection::Eq.paths_for(&ChannelId::ControlGroup(1), &config);
        assert!(paths.is_empty());
    }

    #[test]
    fn parameter_path_orders_via_derived_ord() {
        // Smoke test for the new PartialOrd/Ord derive on ParameterPath.
        let mut paths = [
            ParameterPath::Solo,
            ParameterPath::Fader,
            ParameterPath::Mute,
        ];
        paths.sort();
        // The order is the natural enum-discriminant order, which puts
        // Fader before Mute before Solo (per the variant ordering at the
        // top of this file).
        assert_eq!(paths[0], ParameterPath::Fader);
        assert_eq!(paths[1], ParameterPath::Mute);
        assert_eq!(paths[2], ParameterPath::Solo);
    }

    // ── TimingCategory tests ──────────────────────────────────────

    #[test]
    fn timing_category_fader_and_pan() {
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::Fader),
            Some(TimingCategory::Fader)
        );
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::Pan),
            Some(TimingCategory::Fader)
        );
    }

    #[test]
    fn timing_category_mute() {
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::Mute),
            Some(TimingCategory::Mute)
        );
        assert!(!TimingCategory::Mute.supports_fade());
    }

    #[test]
    fn timing_category_preprocessing() {
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::Trim),
            Some(TimingCategory::Preprocessing)
        );
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::DelayTime),
            Some(TimingCategory::Preprocessing)
        );
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::Balance),
            Some(TimingCategory::Preprocessing)
        );
    }

    #[test]
    fn timing_category_eq() {
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::EqEnabled),
            Some(TimingCategory::Eq)
        );
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::EqBandFrequency(1)),
            Some(TimingCategory::Eq)
        );
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::HighpassFrequency),
            Some(TimingCategory::Eq)
        );
    }

    #[test]
    fn timing_category_dyn1_dyn2() {
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::Dyn1Enabled),
            Some(TimingCategory::Dyn1)
        );
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::Dyn1Threshold(1)),
            Some(TimingCategory::Dyn1)
        );
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::Dyn2Enabled),
            Some(TimingCategory::Dyn2)
        );
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::Dyn2Threshold),
            Some(TimingCategory::Dyn2)
        );
    }

    #[test]
    fn timing_category_sends() {
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::SendLevel(1)),
            Some(TimingCategory::Sends)
        );
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::SendEnabled(1)),
            Some(TimingCategory::Sends)
        );
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::SendPan(1)),
            Some(TimingCategory::Sends)
        );
    }

    #[test]
    fn timing_category_uncategorized_returns_none() {
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::Name),
            None
        );
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::Solo),
            None
        );
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::AnalogGain),
            None
        );
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::Phantom),
            None
        );
        assert_eq!(
            TimingCategory::from_parameter_path(&ParameterPath::DigitubeEnabled),
            None
        );
    }

    #[test]
    fn timing_category_supports_fade() {
        assert!(TimingCategory::Fader.supports_fade());
        assert!(TimingCategory::Eq.supports_fade());
        assert!(TimingCategory::Dyn1.supports_fade());
        assert!(TimingCategory::Dyn2.supports_fade());
        assert!(TimingCategory::Preprocessing.supports_fade());
        assert!(TimingCategory::Sends.supports_fade());
        assert!(!TimingCategory::Mute.supports_fade());
    }

    fn fv(v: f32) -> ParameterValue {
        ParameterValue::Float(v)
    }

    #[test]
    fn clamp_value_pan_family_clamps_above_one() {
        for path in [
            ParameterPath::Pan,
            ParameterPath::SendPan(5),
            ParameterPath::Balance,
            ParameterPath::Width,
        ] {
            assert_eq!(
                path.clamp_value(fv(1.5)),
                fv(1.0),
                "above-one clamp failed for {path:?}",
            );
            assert_eq!(
                path.clamp_value(fv(-1.5)),
                fv(-1.0),
                "below-minus-one clamp failed for {path:?}",
            );
            assert_eq!(
                path.clamp_value(fv(0.42)),
                fv(0.42),
                "in-range value should pass through for {path:?}",
            );
        }
    }

    #[test]
    fn clamp_value_other_parameters_passthrough() {
        // Fader / EqBandGain are unclamped (their ranges are dB and
        // the model doesn't yet encode them precisely).
        assert_eq!(ParameterPath::Fader.clamp_value(fv(15.0)), fv(15.0));
        assert_eq!(
            ParameterPath::EqBandGain(1).clamp_value(fv(-30.0)),
            fv(-30.0)
        );
    }

    #[test]
    fn clamp_value_non_float_passes_through() {
        // Non-float values for pan are nonsensical but shouldn't panic
        // — return them verbatim so the engine can decide.
        assert_eq!(
            ParameterPath::Pan.clamp_value(ParameterValue::Bool(true)),
            ParameterValue::Bool(true),
        );
        assert_eq!(
            ParameterPath::Pan.clamp_value(ParameterValue::Int(7)),
            ParameterValue::Int(7),
        );
    }

    #[test]
    fn clamp_with_s_series_profile_is_identical_to_plain_clamp() {
        use crate::model::family::{ConsoleFamily, ConsoleProfile};
        let profile = ConsoleProfile::for_family(ConsoleFamily::SSeries);
        for (path, value) in [
            (ParameterPath::SendLevel(1), ParameterValue::Float(-200.0)),
            (ParameterPath::SendLevel(1), ParameterValue::Float(40.0)),
            (ParameterPath::CgLevel, ParameterValue::Float(-500.0)),
            (ParameterPath::Fader, ParameterValue::Float(-150.0)),
            (ParameterPath::Pan, ParameterValue::Float(2.0)),
        ] {
            assert_eq!(
                path.clamp_value_with_profile(value.clone(), &profile),
                path.clamp_value(value.clone()),
                "S-series profile must not add clamping for {path:?}"
            );
        }
    }

    #[test]
    fn clamp_with_pad_only_profile_bounds_send_levels() {
        use crate::model::family::{ConsoleFamily, ConsoleProfile};
        let profile = ConsoleProfile::for_family(ConsoleFamily::Quantum);
        let (lo, hi) = profile
            .send_level_db_range
            .expect("Pad profile has a range");

        assert_eq!(
            ParameterPath::SendLevel(1)
                .clamp_value_with_profile(ParameterValue::Float(999.0), &profile),
            ParameterValue::Float(hi),
        );
        assert_eq!(
            ParameterPath::MatrixSendLevel(1)
                .clamp_value_with_profile(ParameterValue::Float(-999.0), &profile),
            ParameterValue::Float(lo),
        );
        // In-range values pass through untouched.
        assert_eq!(
            ParameterPath::CgLevel.clamp_value_with_profile(ParameterValue::Float(-6.0), &profile),
            ParameterValue::Float(-6.0),
        );
        // The channel fader is not a "send level" — left to the desk.
        assert_eq!(
            ParameterPath::Fader.clamp_value_with_profile(ParameterValue::Float(-999.0), &profile),
            ParameterValue::Float(-999.0),
        );
        // Pan still clamps via the base rule.
        assert_eq!(
            ParameterPath::Pan.clamp_value_with_profile(ParameterValue::Float(2.0), &profile),
            ParameterValue::Float(1.0),
        );
    }
}
