//! Fader sidecar model.
//!
//! A "sidecar" is an external MIDI control surface — motorized faders
//! and/or endless rotary encoders — whose controls are bound to console
//! parameters (or to arbitrary outbound OSC targets). The binding table
//! travels with the show; which MIDI ports to use is a property of the
//! machine and lives in [`crate::persistence::preferences`] instead.
//!
//! Everything here is pure data + pure math (taper curves), so the
//! decode/feedback engines and the UI share one vocabulary and the
//! interesting logic is unit-testable without hardware.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::cue_trigger::OscArg;
use crate::model::parameter::{FADER_INF_DB, ParameterAddress, ParameterPath};

/// How a physical control announces itself on the MIDI wire.
///
/// Deliberately port-agnostic: the port name is machine-bound
/// (preferences), while the selector travels with the show — so a
/// binding survives WinMM port renumbering and moving the show file to
/// another computer. MIDI channels are 1-based (1..=16), matching how
/// they're printed on hardware.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ControlSelector {
    /// Control Change on (channel, controller number). Covers both the
    /// 7-bit case and the MSB of a 14-bit pair — interpretation lives
    /// in [`ControlMode`].
    Cc { channel: u8, cc: u8 },
    /// Pitch bend on a MIDI channel. This is how Mackie-Control-mode
    /// surfaces (X-Touch et al.) transmit their motorized faders: one
    /// fader per MIDI channel, full 14-bit resolution.
    PitchBend { channel: u8 },
    /// A note number on a channel. Used for MCU fader touch sense
    /// (and, later, buttons).
    Note { channel: u8, note: u8 },
}

impl ControlSelector {
    /// Short human-readable summary for binding rows ("PB ch3",
    /// "CC 16 ch1", "Note 0x68 ch1").
    pub fn summary(&self) -> String {
        match self {
            ControlSelector::Cc { channel, cc } => format!("CC {cc} ch{channel}"),
            ControlSelector::PitchBend { channel } => format!("PB ch{channel}"),
            ControlSelector::Note { channel, note } => format!("Note {note} ch{channel}"),
        }
    }
}

/// How matched MIDI messages become a normalized 0..=1 position (or a
/// relative nudge).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlMode {
    /// Single CC, value 0..=127, absolute position.
    Absolute7,
    /// 14-bit CC pair: the selector's `cc` carries the MSB, `lsb_cc`
    /// (conventionally `cc + 32`) the LSB.
    Absolute14 { lsb_cc: u8 },
    /// Endless encoder sending relative ticks.
    Relative(RelativeMode),
    /// 14-bit pitch bend absolute (the only mode valid for a
    /// [`ControlSelector::PitchBend`] selector).
    PitchBend14,
}

impl ControlMode {
    /// Whether this mode reports an absolute position — the
    /// precondition for motor feedback (a relative encoder has no
    /// position to drive).
    pub fn is_absolute(&self) -> bool {
        !matches!(self, ControlMode::Relative(_))
    }

    /// Short summary for binding rows.
    pub fn summary(&self) -> &'static str {
        match self {
            ControlMode::Absolute7 => "7-bit",
            ControlMode::Absolute14 { .. } => "14-bit",
            ControlMode::Relative(RelativeMode::TwosComplement) => "rel (2's comp)",
            ControlMode::Relative(RelativeMode::BinaryOffset) => "rel (offset)",
            ControlMode::Relative(RelativeMode::SignMagnitude) => "rel (sign-mag)",
            ControlMode::PitchBend14 => "pitch bend",
        }
    }
}

/// Encodings used by endless encoders for signed ticks in a 7-bit CC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelativeMode {
    /// 1..=63 = +n, 65..=127 = −(128−v). X-Touch V-pots, most MCU pots.
    TwosComplement,
    /// 64 = 0, above = +, below = −.
    BinaryOffset,
    /// Bit 6 = sign (set = negative), bits 0..=5 = magnitude.
    SignMagnitude,
}

impl RelativeMode {
    /// Decode one CC value into signed ticks.
    pub fn ticks(&self, value: u8) -> i32 {
        let v = i32::from(value & 0x7f);
        match self {
            RelativeMode::TwosComplement => {
                if v == 64 {
                    0
                } else if v < 64 {
                    v
                } else {
                    v - 128
                }
            }
            RelativeMode::BinaryOffset => v - 64,
            RelativeMode::SignMagnitude => {
                let mag = v & 0x3f;
                if v & 0x40 != 0 { -mag } else { mag }
            }
        }
    }
}

/// Normalized-position → target-value curve.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Taper {
    /// Piecewise-linear dB fader law approximating the console's:
    /// unity gain at ~75% travel, −inf detent at 0. Output spans
    /// [`FADER_INF_DB`, `max_db`].
    ///
    /// The breakpoint table is an approximation of the S21 law (the
    /// real curve isn't discoverable over OSC); calibrate the table if
    /// A/B against the desk feels off — nothing else needs to change.
    FaderDb { max_db: f32 },
    /// Straight line min..=max. Pan family uses −1..=1; raw OSC and
    /// generic continuous parameters default to 0..=1.
    Linear { min: f32, max: f32 },
}

/// The FaderDb law as (normalized position, dB) breakpoints. The final
/// dB entry is replaced by the taper's `max_db`. Monotonic in both
/// columns — both directions interpolate off this one table.
const FADER_DB_TABLE: [(f32, f32); 8] = [
    (0.00, FADER_INF_DB), // −inf detent
    (0.02, -90.0),
    (0.10, -60.0),
    (0.25, -40.0),
    (0.45, -20.0),
    (0.60, -10.0),
    (0.75, 0.0),  // unity at 3/4 travel, console convention
    (1.00, 10.0), // placeholder — replaced by max_db
];

/// Map a normalized 0..=1 position through a taper to a target value.
/// FaderDb: exactly `FADER_INF_DB` at (or below) zero.
pub fn taper_to_value(taper: &Taper, norm: f32) -> f32 {
    let norm = norm.clamp(0.0, 1.0);
    match taper {
        Taper::Linear { min, max } => min + (max - min) * norm,
        Taper::FaderDb { max_db } => {
            if norm <= 0.0 {
                return FADER_INF_DB;
            }
            let db_at = |i: usize| {
                if i == FADER_DB_TABLE.len() - 1 {
                    *max_db
                } else {
                    FADER_DB_TABLE[i].1
                }
            };
            for i in 0..FADER_DB_TABLE.len() - 1 {
                let (n0, n1) = (FADER_DB_TABLE[i].0, FADER_DB_TABLE[i + 1].0);
                if norm <= n1 {
                    let t = (norm - n0) / (n1 - n0);
                    return db_at(i) + (db_at(i + 1) - db_at(i)) * t;
                }
            }
            *max_db
        }
    }
}

/// Inverse of [`taper_to_value`], for motor feedback. FaderDb: any
/// value at or below the −inf detent maps to 0.
pub fn taper_to_norm(taper: &Taper, value: f32) -> f32 {
    match taper {
        Taper::Linear { min, max } => {
            if (max - min).abs() < f32::EPSILON {
                0.0
            } else {
                ((value - min) / (max - min)).clamp(0.0, 1.0)
            }
        }
        Taper::FaderDb { max_db } => {
            if value <= FADER_INF_DB {
                return 0.0;
            }
            let db_at = |i: usize| {
                if i == FADER_DB_TABLE.len() - 1 {
                    *max_db
                } else {
                    FADER_DB_TABLE[i].1
                }
            };
            if value >= *max_db {
                return 1.0;
            }
            for i in 0..FADER_DB_TABLE.len() - 1 {
                let (d0, d1) = (db_at(i), db_at(i + 1));
                if value <= d1 {
                    let (n0, n1) = (FADER_DB_TABLE[i].0, FADER_DB_TABLE[i + 1].0);
                    let t = (value - d0) / (d1 - d0);
                    return (n0 + (n1 - n0) * t).clamp(0.0, 1.0);
                }
            }
            1.0
        }
    }
}

/// A sensible default taper for a console parameter: the fader law for
/// fader-family levels, −1..=1 for the pan family, ±18 dB for EQ band
/// gain, otherwise a plain 0..=1 the operator can edit.
pub fn default_taper_for(addr: &ParameterAddress) -> Taper {
    let p = &addr.parameter;
    if p.is_fader_level() {
        Taper::FaderDb { max_db: 10.0 }
    } else {
        match p {
            ParameterPath::Pan
            | ParameterPath::SendPan(_)
            | ParameterPath::Balance
            | ParameterPath::Width => Taper::Linear {
                min: -1.0,
                max: 1.0,
            },
            ParameterPath::EqBandGain(_) => Taper::Linear {
                min: -18.0,
                max: 18.0,
            },
            _ => Taper::Linear { min: 0.0, max: 1.0 },
        }
    }
}

/// Where a hardware move goes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BindingTarget {
    /// A typed console parameter — full learn + motor feedback.
    ConsoleParameter { address: ParameterAddress },
    /// Arbitrary outbound OSC. Either references a reusable
    /// [`crate::model::cue_trigger::OscTarget`] by id or carries an
    /// inline host/port. The tapered value is appended as a trailing
    /// Float argument after the fixed `args` prefix. No console
    /// feedback, no motor feedback.
    RawOsc {
        #[serde(default)]
        target_id: Option<Uuid>,
        #[serde(default)]
        host: Option<String>,
        #[serde(default)]
        port: Option<u16>,
        path: String,
        #[serde(default)]
        args: Vec<OscArg>,
    },
}

impl BindingTarget {
    /// The console address, when this target is one.
    pub fn console_address(&self) -> Option<&ParameterAddress> {
        match self {
            BindingTarget::ConsoleParameter { address } => Some(address),
            BindingTarget::RawOsc { .. } => None,
        }
    }
}

/// Is this console parameter a legal sidecar binding target?
///
/// Continuous only (a fader can't drive a name), and never `TotalGain`:
/// the desk emits `total/gain` as a read-only fader+CG sum alongside
/// every fader move — it can't be written back, so binding to it would
/// silently do nothing. The connection layer already drops it on
/// receipt (see `console::connection::process_message`), this is the
/// belt to that suspenders.
pub fn is_valid_console_target(addr: &ParameterAddress) -> bool {
    addr.parameter.is_continuous() && addr.parameter != ParameterPath::TotalGain
}

fn default_true() -> bool {
    true
}

fn default_relative_step() -> f32 {
    // 300 encoder ticks for full travel — fine-grained but not glacial.
    1.0 / 300.0
}

/// One hardware control bound to one target.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SidecarBinding {
    pub id: Uuid,
    /// Operator-facing name; auto-filled from the target on learn.
    #[serde(default)]
    pub label: String,
    pub control: ControlSelector,
    pub mode: ControlMode,
    pub target: BindingTarget,
    pub taper: Taper,
    /// Push console-state changes back to the motor. Only meaningful
    /// for ConsoleParameter targets with an absolute mode; the UI hides
    /// it otherwise.
    #[serde(default = "default_true")]
    pub motor_feedback: bool,
    /// Touch-sense gate (MCU fader touch note). While the note is
    /// held, motor pushes to this control are suppressed so the motor
    /// never fights the operator's hand. Auto-filled for pitch-bend
    /// selectors via [`mcu_default_touch_note`]; editable.
    #[serde(default)]
    pub touch: Option<ControlSelector>,
    /// Relative modes: normalized position change per encoder tick.
    #[serde(default = "default_relative_step")]
    pub relative_step: f32,
    /// Per-binding enable — lets the operator park one binding without
    /// deleting it (the master switch lives on [`SidecarConfig`]).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl SidecarBinding {
    /// Whether this binding can drive a motor: console target, absolute
    /// mode, and the feedback flag on.
    pub fn wants_motor_feedback(&self) -> bool {
        self.motor_feedback
            && self.mode.is_absolute()
            && matches!(self.target, BindingTarget::ConsoleParameter { .. })
    }
}

/// Per-show sidecar configuration (one per show file).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SidecarConfig {
    /// Master rocker. OFF mutes both directions (hardware → console and
    /// console → motors) without touching the MIDI connection or the
    /// binding table, so ON is instant and re-syncs from console state.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bindings: Vec<SidecarBinding>,
}

impl SidecarConfig {
    /// The binding already claiming this control, if any (one control
    /// drives one binding).
    pub fn binding_for_control(&self, control: &ControlSelector) -> Option<&SidecarBinding> {
        self.bindings.iter().find(|b| b.control == *control)
    }
}

/// MCU convention: fader touch is note `0x68 + fader_index` on MIDI
/// channel 1 while the faders themselves ride pitch bend on channels
/// 1..=9 (9 = main). Returns `None` for channels outside that range.
pub fn mcu_default_touch_note(pitchbend_channel: u8) -> Option<ControlSelector> {
    (1..=9)
        .contains(&pitchbend_channel)
        .then(|| ControlSelector::Note {
            channel: 1,
            note: 0x67 + pitchbend_channel,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;

    fn addr(parameter: ParameterPath) -> ParameterAddress {
        ParameterAddress {
            channel: ChannelId::Input(1),
            parameter,
        }
    }

    const FADER: Taper = Taper::FaderDb { max_db: 10.0 };

    #[test]
    fn fader_taper_endpoints() {
        assert_eq!(taper_to_value(&FADER, 0.0), FADER_INF_DB);
        assert_eq!(taper_to_value(&FADER, -0.5), FADER_INF_DB);
        assert!((taper_to_value(&FADER, 1.0) - 10.0).abs() < 1e-4);
        assert!((taper_to_value(&FADER, 0.75) - 0.0).abs() < 1e-4);
    }

    #[test]
    fn fader_taper_is_monotonic() {
        let mut prev = f32::NEG_INFINITY;
        for i in 0..=1000 {
            let v = taper_to_value(&FADER, i as f32 / 1000.0);
            assert!(v >= prev, "taper not monotonic at step {i}");
            prev = v;
        }
    }

    #[test]
    fn fader_taper_round_trips() {
        for i in 1..=999 {
            let n = i as f32 / 1000.0;
            let back = taper_to_norm(&FADER, taper_to_value(&FADER, n));
            assert!(
                (back - n).abs() < 1e-3,
                "round trip failed at {n}: got {back}"
            );
        }
    }

    #[test]
    fn fader_taper_norm_endpoints() {
        assert_eq!(taper_to_norm(&FADER, FADER_INF_DB), 0.0);
        assert_eq!(taper_to_norm(&FADER, -200.0), 0.0);
        assert_eq!(taper_to_norm(&FADER, 10.0), 1.0);
        assert_eq!(taper_to_norm(&FADER, 25.0), 1.0);
    }

    #[test]
    fn fader_taper_respects_max_db() {
        let t = Taper::FaderDb { max_db: 5.0 };
        assert!((taper_to_value(&t, 1.0) - 5.0).abs() < 1e-4);
        // Unity stays pinned at 3/4 travel regardless of max_db.
        assert!((taper_to_value(&t, 0.75) - 0.0).abs() < 1e-4);
    }

    #[test]
    fn linear_taper_maps_and_inverts() {
        let t = Taper::Linear {
            min: -1.0,
            max: 1.0,
        };
        assert_eq!(taper_to_value(&t, 0.5), 0.0);
        assert_eq!(taper_to_value(&t, 0.0), -1.0);
        assert_eq!(taper_to_value(&t, 1.0), 1.0);
        assert_eq!(taper_to_norm(&t, 0.0), 0.5);
        // Out-of-range values clamp.
        assert_eq!(taper_to_norm(&t, 4.0), 1.0);
        assert_eq!(taper_to_norm(&t, -4.0), 0.0);
    }

    #[test]
    fn degenerate_linear_taper_does_not_divide_by_zero() {
        let t = Taper::Linear { min: 3.0, max: 3.0 };
        assert_eq!(taper_to_norm(&t, 3.0), 0.0);
    }

    #[test]
    fn default_tapers_by_family() {
        assert_eq!(
            default_taper_for(&addr(ParameterPath::Fader)),
            Taper::FaderDb { max_db: 10.0 }
        );
        assert_eq!(
            default_taper_for(&addr(ParameterPath::SendLevel(3))),
            Taper::FaderDb { max_db: 10.0 }
        );
        assert_eq!(
            default_taper_for(&addr(ParameterPath::Pan)),
            Taper::Linear {
                min: -1.0,
                max: 1.0
            }
        );
        assert_eq!(
            default_taper_for(&addr(ParameterPath::EqBandGain(2))),
            Taper::Linear {
                min: -18.0,
                max: 18.0
            }
        );
    }

    #[test]
    fn relative_modes_decode_signed_ticks() {
        use RelativeMode::*;
        // Two's complement: small positives, wrap-around negatives.
        assert_eq!(TwosComplement.ticks(1), 1);
        assert_eq!(TwosComplement.ticks(3), 3);
        assert_eq!(TwosComplement.ticks(127), -1);
        assert_eq!(TwosComplement.ticks(125), -3);
        assert_eq!(TwosComplement.ticks(64), 0);
        // Binary offset around 64.
        assert_eq!(BinaryOffset.ticks(64), 0);
        assert_eq!(BinaryOffset.ticks(65), 1);
        assert_eq!(BinaryOffset.ticks(63), -1);
        // Sign-magnitude: bit 6 = negative.
        assert_eq!(SignMagnitude.ticks(1), 1);
        assert_eq!(SignMagnitude.ticks(0x41), -1);
        assert_eq!(SignMagnitude.ticks(0x45), -5);
        assert_eq!(SignMagnitude.ticks(0), 0);
    }

    #[test]
    fn mcu_touch_notes_cover_nine_faders() {
        assert_eq!(
            mcu_default_touch_note(1),
            Some(ControlSelector::Note {
                channel: 1,
                note: 0x68
            })
        );
        assert_eq!(
            mcu_default_touch_note(9),
            Some(ControlSelector::Note {
                channel: 1,
                note: 0x70
            })
        );
        assert_eq!(mcu_default_touch_note(0), None);
        assert_eq!(mcu_default_touch_note(10), None);
    }

    #[test]
    fn total_gain_and_discrete_params_rejected_as_targets() {
        // Moving a fader makes the desk emit `total/gain` (fader + CG
        // sum, read-only) — it must never become a binding target.
        assert!(!is_valid_console_target(&addr(ParameterPath::TotalGain)));
        assert!(!is_valid_console_target(&addr(ParameterPath::Mute)));
        assert!(is_valid_console_target(&addr(ParameterPath::Fader)));
        assert!(is_valid_console_target(&addr(ParameterPath::Pan)));
    }

    #[test]
    fn wants_motor_feedback_rules() {
        let mut b = SidecarBinding {
            id: Uuid::from_bytes([1; 16]),
            label: String::new(),
            control: ControlSelector::PitchBend { channel: 1 },
            mode: ControlMode::PitchBend14,
            target: BindingTarget::ConsoleParameter {
                address: addr(ParameterPath::Fader),
            },
            taper: FADER,
            motor_feedback: true,
            touch: None,
            relative_step: default_relative_step(),
            enabled: true,
        };
        assert!(b.wants_motor_feedback());
        // Relative encoders have no position to drive.
        b.mode = ControlMode::Relative(RelativeMode::TwosComplement);
        assert!(!b.wants_motor_feedback());
        // Raw OSC targets have no console state to mirror.
        b.mode = ControlMode::Absolute7;
        b.target = BindingTarget::RawOsc {
            target_id: None,
            host: Some("10.0.0.9".into()),
            port: Some(9000),
            path: "/x/fader".into(),
            args: vec![],
        };
        assert!(!b.wants_motor_feedback());
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = SidecarConfig {
            enabled: true,
            bindings: vec![
                SidecarBinding {
                    id: Uuid::from_bytes([2; 16]),
                    label: "CH 12 Fader".into(),
                    control: ControlSelector::PitchBend { channel: 3 },
                    mode: ControlMode::PitchBend14,
                    target: BindingTarget::ConsoleParameter {
                        address: addr(ParameterPath::Fader),
                    },
                    taper: Taper::FaderDb { max_db: 10.0 },
                    motor_feedback: true,
                    touch: mcu_default_touch_note(3),
                    relative_step: default_relative_step(),
                    enabled: true,
                },
                SidecarBinding {
                    id: Uuid::from_bytes([3; 16]),
                    label: "House lights".into(),
                    control: ControlSelector::Cc { channel: 1, cc: 16 },
                    mode: ControlMode::Absolute14 { lsb_cc: 48 },
                    target: BindingTarget::RawOsc {
                        target_id: None,
                        host: Some("192.168.1.50".into()),
                        port: Some(7700),
                        path: "/lights/dim".into(),
                        args: vec![OscArg::Int(4)],
                    },
                    taper: Taper::Linear { min: 0.0, max: 1.0 },
                    motor_feedback: false,
                    touch: None,
                    relative_step: default_relative_step(),
                    enabled: false,
                },
                SidecarBinding {
                    id: Uuid::from_bytes([4; 16]),
                    label: "Pan".into(),
                    control: ControlSelector::Cc { channel: 1, cc: 60 },
                    mode: ControlMode::Relative(RelativeMode::TwosComplement),
                    target: BindingTarget::ConsoleParameter {
                        address: addr(ParameterPath::Pan),
                    },
                    taper: Taper::Linear {
                        min: -1.0,
                        max: 1.0,
                    },
                    motor_feedback: false,
                    touch: None,
                    relative_step: 1.0 / 100.0,
                    enabled: true,
                },
            ],
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: SidecarConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn empty_object_loads_as_default() {
        // Legacy show files (≤ v17) have no `sidecar` field; the type
        // must also accept an empty object.
        let cfg: SidecarConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg, SidecarConfig::default());
        assert!(!cfg.enabled);
        assert!(cfg.bindings.is_empty());
    }

    #[test]
    fn binding_defaults_fill_missing_fields() {
        // A minimal binding JSON (as an older writer might produce)
        // deserialises with feedback on, enabled, default step.
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000009",
            "control": {"PitchBend": {"channel": 2}},
            "mode": "PitchBend14",
            "target": {"ConsoleParameter": {"address": {"channel": {"Input": 5}, "parameter": "Fader"}}},
            "taper": {"FaderDb": {"max_db": 10.0}}
        }"#;
        let b: SidecarBinding = serde_json::from_str(json).unwrap();
        assert!(b.motor_feedback);
        assert!(b.enabled);
        assert!(b.touch.is_none());
        assert!((b.relative_step - 1.0 / 300.0).abs() < 1e-9);
        assert_eq!(b.label, "");
    }

    #[test]
    fn binding_for_control_finds_claimed_control() {
        let control = ControlSelector::Cc { channel: 1, cc: 7 };
        let cfg = SidecarConfig {
            enabled: true,
            bindings: vec![SidecarBinding {
                id: Uuid::from_bytes([5; 16]),
                label: String::new(),
                control,
                mode: ControlMode::Absolute7,
                target: BindingTarget::ConsoleParameter {
                    address: addr(ParameterPath::Fader),
                },
                taper: FADER,
                motor_feedback: true,
                touch: None,
                relative_step: default_relative_step(),
                enabled: true,
            }],
        };
        assert!(cfg.binding_for_control(&control).is_some());
        assert!(
            cfg.binding_for_control(&ControlSelector::Cc { channel: 1, cc: 8 })
                .is_none()
        );
    }
}
