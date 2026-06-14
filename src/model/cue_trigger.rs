//! External cue triggers (v0.1.2).
//!
//! In addition to recalling a console memory row and/or a snapshot overlay, a
//! cue can fire one or more **external triggers** when it is recalled — OSC to
//! QLab / LiveProfessor / any user-added OSC target, or MIDI (CC / Note On/Off
//! / Program Change) to drive e.g. Ableton Live scenes.
//!
//! Three concepts live here:
//!
//! - [`CueTrigger`] — a per-cue, frozen trigger *instance*. The engine reads
//!   ONLY these at fire time; templates are a UI-authoring convenience and are
//!   never consulted during recall.
//! - [`TriggerTemplate`] — a reusable message *shape* (e.g. "QLab: GO cue [N]").
//!   Built-ins live in code ([`TriggerTemplate::builtins`]); user templates are
//!   persisted per-show in the show file.
//! - [`OscTarget`] — a reusable OSC *destination* (host:port), persisted
//!   per-show. A [`TriggerAction::Osc`] references one by id, or carries an
//!   inline host/port.
//!
//! Serialization follows the established internally-tagged-enum pattern of
//! [`crate::model::macro_def::MacroStepKind`] so JSON is self-describing and
//! forward-tolerant. `rosc::OscType` is not serde-able, so [`OscArg`] is the
//! minimal serde-friendly arg type used on the wire.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn default_true() -> bool {
    true
}

/// One external action fired when a cue is recalled, alongside the cue's
/// console memory row + snapshot overlay. A frozen copy of (optionally) a
/// template — editing or deleting a template never disturbs existing cues.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CueTrigger {
    pub id: Uuid,
    /// Operator-visible label (e.g. "QLab: GO Q12"). Defaults from the
    /// template/action when created; freely editable.
    pub label: String,
    /// Per-instance master enable — lets the operator silence a trigger
    /// without deleting it (mirrors `StreamDeckConfig.enabled`).
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub action: TriggerAction,
}

impl CueTrigger {
    /// Build a fresh trigger instance from an action, deriving a label.
    pub fn new(action: TriggerAction) -> Self {
        Self {
            id: Uuid::new_v4(),
            label: action.default_label(),
            enabled: true,
            action,
        }
    }

    /// Instantiate a frozen trigger from a template (clones the template's
    /// action and uses the template name as the starting label).
    pub fn from_template(tmpl: &TriggerTemplate) -> Self {
        Self {
            id: Uuid::new_v4(),
            label: tmpl.name.clone(),
            enabled: true,
            action: tmpl.action.clone(),
        }
    }
}

/// The concrete action a [`CueTrigger`] performs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TriggerAction {
    /// Fire-and-forget OSC to an OSC target.
    Osc {
        /// Reference to a reusable [`OscTarget`] (per-show). When `None`, the
        /// inline `host`/`port` are used instead.
        #[serde(default)]
        target_id: Option<Uuid>,
        /// Inline host, used when `target_id` is `None`.
        #[serde(default)]
        host: Option<String>,
        /// Inline port, used when `target_id` is `None`.
        #[serde(default)]
        port: Option<u16>,
        /// OSC address path, e.g. `/go` or `/GlobalSnapshots/Recall`.
        path: String,
        /// Typed OSC arguments.
        #[serde(default)]
        args: Vec<OscArg>,
    },
    /// Raw MIDI message out the single active MIDI output (configured in
    /// Setup; machine-bound, so MIDI triggers carry no target reference).
    Midi { message: MidiMessage },
}

impl TriggerAction {
    /// A reasonable default label for a freshly-created trigger.
    pub fn default_label(&self) -> String {
        match self {
            TriggerAction::Osc { path, args, .. } => {
                if args.is_empty() {
                    format!("OSC {path}")
                } else {
                    format!("OSC {path} {}", format_osc_args(args))
                }
            }
            TriggerAction::Midi { message } => message.describe(),
        }
    }
}

/// A serde-friendly OSC argument. Mirrors the subset of `rosc::OscType` that
/// QLab / LiveProfessor / user OSC need; convert with [`OscArg::to_osc`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
pub enum OscArg {
    Int(i32),
    Float(f32),
    Str(String),
    Bool(bool),
}

impl OscArg {
    /// Convert to the `rosc` wire type for sending.
    pub fn to_osc(&self) -> rosc::OscType {
        match self {
            OscArg::Int(i) => rosc::OscType::Int(*i),
            OscArg::Float(f) => rosc::OscType::Float(*f),
            OscArg::Str(s) => rosc::OscType::String(s.clone()),
            OscArg::Bool(b) => rosc::OscType::Bool(*b),
        }
    }
}

/// Render a list of OSC args as a compact human string (for labels + the OSC
/// log), e.g. `12 "Q3" 0.5`.
pub fn format_osc_args(args: &[OscArg]) -> String {
    args.iter()
        .map(|a| match a {
            OscArg::Int(i) => i.to_string(),
            OscArg::Float(f) => f.to_string(),
            OscArg::Str(s) => format!("\"{s}\""),
            OscArg::Bool(b) => b.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A raw MIDI channel-voice message. Channel is stored 1-based (1..=16) as the
/// operator sees it; [`MidiMessage::to_bytes`] encodes the 0-based status
/// nibble. Data bytes are 7-bit (0..=127); values are clamped on encode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "msg")]
pub enum MidiMessage {
    /// Control Change: status `0xB0 | (ch-1)`, controller, value.
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
    /// Note On: status `0x90 | (ch-1)`, note, velocity.
    NoteOn { channel: u8, note: u8, velocity: u8 },
    /// Note Off: status `0x80 | (ch-1)`, note, velocity.
    NoteOff { channel: u8, note: u8, velocity: u8 },
    /// Program Change: status `0xC0 | (ch-1)`, program. Two bytes on the wire.
    ProgramChange { channel: u8, program: u8 },
}

/// Clamp a 1-based MIDI channel (1..=16) to its 0-based status nibble (0..=15).
fn channel_nibble(channel: u8) -> u8 {
    channel.clamp(1, 16) - 1
}

/// Clamp a value to the 7-bit MIDI data range.
fn data7(v: u8) -> u8 {
    v.min(127)
}

impl MidiMessage {
    /// Encode to raw MIDI bytes ready for the wire. CC / Note are 3 bytes;
    /// Program Change is 2.
    pub fn to_bytes(self) -> Vec<u8> {
        match self {
            MidiMessage::ControlChange {
                channel,
                controller,
                value,
            } => vec![
                0xB0 | channel_nibble(channel),
                data7(controller),
                data7(value),
            ],
            MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } => vec![0x90 | channel_nibble(channel), data7(note), data7(velocity)],
            MidiMessage::NoteOff {
                channel,
                note,
                velocity,
            } => vec![0x80 | channel_nibble(channel), data7(note), data7(velocity)],
            MidiMessage::ProgramChange { channel, program } => {
                vec![0xC0 | channel_nibble(channel), data7(program)]
            }
        }
    }

    /// Human-readable one-liner for labels + the OSC log.
    pub fn describe(&self) -> String {
        match *self {
            MidiMessage::ControlChange {
                channel,
                controller,
                value,
            } => format!("MIDI CC ch{channel} cc{controller}={value}"),
            MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } => format!("MIDI Note On ch{channel} n{note}@{velocity}"),
            MidiMessage::NoteOff {
                channel,
                note,
                velocity,
            } => format!("MIDI Note Off ch{channel} n{note}@{velocity}"),
            MidiMessage::ProgramChange { channel, program } => {
                format!("MIDI Program Change ch{channel} pc{program}")
            }
        }
    }

    /// The 1-based MIDI channel this message targets.
    pub fn channel(&self) -> u8 {
        match *self {
            MidiMessage::ControlChange { channel, .. }
            | MidiMessage::NoteOn { channel, .. }
            | MidiMessage::NoteOff { channel, .. }
            | MidiMessage::ProgramChange { channel, .. } => channel,
        }
    }
}

/// A reusable OSC destination (host:port). Persisted per-show in the show file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OscTarget {
    pub id: Uuid,
    pub name: String,
    pub host: String,
    pub port: u16,
}

impl OscTarget {
    pub fn new(name: impl Into<String>, host: impl Into<String>, port: u16) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            host: host.into(),
            port,
        }
    }
}

/// A reusable trigger *template* — a message shape the UI offers in the
/// "Add Trigger" picker. Built-ins ([`TriggerTemplate::builtins`]) are always
/// present; user-created templates persist per-show.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TriggerTemplate {
    pub id: Uuid,
    pub name: String,
    /// True for the shipped defaults; the UI shows these read-only (use
    /// "Add Trigger" to instantiate, but the template itself can't be deleted).
    #[serde(default)]
    pub builtin: bool,
    pub action: TriggerAction,
}

impl TriggerTemplate {
    /// A user template (not a built-in).
    pub fn user(name: impl Into<String>, action: TriggerAction) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            builtin: false,
            action,
        }
    }

    fn builtin(name: &str, action: TriggerAction) -> Self {
        Self {
            // Deterministic ids would require a namespace UUID; built-ins are
            // matched by `builtin` + name in the UI, so a random id is fine —
            // they are regenerated from code each launch and never persisted.
            id: Uuid::new_v4(),
            name: name.to_string(),
            builtin: true,
            action,
        }
    }

    /// The shipped default templates, always available in the picker.
    ///
    /// QLab uses the argument-form `/go <cue>` convention already used by the
    /// macro engine (`MacroStepKind::QLabGoCue`). LiveProfessor uses the real
    /// Audiostrom global-OSC commands; snapshot indices are **zero-based**.
    pub fn builtins() -> Vec<TriggerTemplate> {
        let osc = |path: &str, args: Vec<OscArg>| TriggerAction::Osc {
            target_id: None,
            host: None,
            port: None,
            path: path.to_string(),
            args,
        };
        vec![
            // ── QLab ──
            Self::builtin("QLab: GO", osc("/go", vec![])),
            Self::builtin(
                "QLab: GO cue [N]",
                osc("/go", vec![OscArg::Str("1".to_string())]),
            ),
            Self::builtin("QLab: Stop", osc("/stop", vec![])),
            Self::builtin("QLab: Pause", osc("/pause", vec![])),
            Self::builtin("QLab: Resume", osc("/resume", vec![])),
            Self::builtin("QLab: Panic", osc("/panic", vec![])),
            // ── LiveProfessor (zero-based snapshot index) ──
            Self::builtin(
                "LiveProfessor: recall snapshot [N]",
                osc("/GlobalSnapshots/Recall", vec![OscArg::Int(0)]),
            ),
            Self::builtin(
                "LiveProfessor: recall snapshot by name [name]",
                osc(
                    "/GlobalSnapshots/Recall",
                    vec![OscArg::Str("Snapshot".to_string())],
                ),
            ),
            Self::builtin(
                "LiveProfessor: next snapshot",
                osc("/Command/GlobalSnapshots/RecallNextGlobalSnapshot", vec![]),
            ),
            Self::builtin(
                "LiveProfessor: previous snapshot",
                osc(
                    "/Command/GlobalSnapshots/RecallPreviousGlobalSnapshot",
                    vec![],
                ),
            ),
            // ── MIDI (e.g. Ableton scene launch via a mapped note/CC/PC) ──
            Self::builtin(
                "MIDI: Program Change [N]",
                TriggerAction::Midi {
                    message: MidiMessage::ProgramChange {
                        channel: 1,
                        program: 0,
                    },
                },
            ),
            Self::builtin(
                "MIDI: Control Change [cc]=[v]",
                TriggerAction::Midi {
                    message: MidiMessage::ControlChange {
                        channel: 1,
                        controller: 0,
                        value: 127,
                    },
                },
            ),
            Self::builtin(
                "MIDI: Note On [n]@[vel]",
                TriggerAction::Midi {
                    message: MidiMessage::NoteOn {
                        channel: 1,
                        note: 60,
                        velocity: 100,
                    },
                },
            ),
            Self::builtin(
                "MIDI: Note Off [n]",
                TriggerAction::Midi {
                    message: MidiMessage::NoteOff {
                        channel: 1,
                        note: 60,
                        velocity: 0,
                    },
                },
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_control_change_bytes() {
        let m = MidiMessage::ControlChange {
            channel: 1,
            controller: 7,
            value: 100,
        };
        // ch1 → nibble 0 → status 0xB0.
        assert_eq!(m.to_bytes(), vec![0xB0, 7, 100]);
    }

    #[test]
    fn midi_channel_nibble_encodes_one_based() {
        // ch16 → nibble 15 → status 0xBF.
        let m = MidiMessage::ControlChange {
            channel: 16,
            controller: 1,
            value: 2,
        };
        assert_eq!(m.to_bytes(), vec![0xBF, 1, 2]);
        // ch10 Note On → 0x99.
        let n = MidiMessage::NoteOn {
            channel: 10,
            note: 60,
            velocity: 64,
        };
        assert_eq!(n.to_bytes(), vec![0x99, 60, 64]);
    }

    #[test]
    fn midi_note_off_bytes() {
        let m = MidiMessage::NoteOff {
            channel: 1,
            note: 60,
            velocity: 0,
        };
        assert_eq!(m.to_bytes(), vec![0x80, 60, 0]);
    }

    #[test]
    fn midi_program_change_is_two_bytes() {
        let m = MidiMessage::ProgramChange {
            channel: 1,
            program: 5,
        };
        let bytes = m.to_bytes();
        assert_eq!(bytes.len(), 2);
        assert_eq!(bytes, vec![0xC0, 5]);
    }

    #[test]
    fn midi_clamps_out_of_range() {
        // channel 0 clamps to 1 (nibble 0); value > 127 clamps to 127.
        let m = MidiMessage::ControlChange {
            channel: 0,
            controller: 200,
            value: 200,
        };
        assert_eq!(m.to_bytes(), vec![0xB0, 127, 127]);
    }

    #[test]
    fn osc_arg_maps_to_rosc() {
        assert_eq!(OscArg::Int(3).to_osc(), rosc::OscType::Int(3));
        assert_eq!(OscArg::Float(1.5).to_osc(), rosc::OscType::Float(1.5));
        assert_eq!(
            OscArg::Str("x".into()).to_osc(),
            rosc::OscType::String("x".into())
        );
        assert_eq!(OscArg::Bool(true).to_osc(), rosc::OscType::Bool(true));
    }

    #[test]
    fn cue_trigger_round_trip_mixed() {
        let triggers = vec![
            CueTrigger::new(TriggerAction::Osc {
                target_id: Some(Uuid::new_v4()),
                host: None,
                port: None,
                path: "/go".into(),
                args: vec![
                    OscArg::Str("Q3".into()),
                    OscArg::Int(12),
                    OscArg::Float(0.5),
                ],
            }),
            CueTrigger::new(TriggerAction::Osc {
                target_id: None,
                host: Some("10.0.0.9".into()),
                port: Some(53000),
                path: "/GlobalSnapshots/Recall".into(),
                args: vec![OscArg::Int(4)],
            }),
            CueTrigger::new(TriggerAction::Midi {
                message: MidiMessage::ProgramChange {
                    channel: 1,
                    program: 7,
                },
            }),
            CueTrigger::new(TriggerAction::Midi {
                message: MidiMessage::NoteOn {
                    channel: 2,
                    note: 64,
                    velocity: 100,
                },
            }),
        ];
        let json = serde_json::to_string_pretty(&triggers).unwrap();
        let back: Vec<CueTrigger> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, triggers);
    }

    #[test]
    fn enabled_defaults_true_on_legacy_json() {
        // A trigger serialized without `enabled` (older/hand-authored) loads
        // enabled, matching the serde default.
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "label": "QLab GO",
            "action": { "kind": "Osc", "path": "/go", "args": [] }
        }"#;
        let t: CueTrigger = serde_json::from_str(json).unwrap();
        assert!(t.enabled);
        assert!(matches!(t.action, TriggerAction::Osc { .. }));
    }

    #[test]
    fn builtins_are_present_and_flagged() {
        let b = TriggerTemplate::builtins();
        assert!(b.iter().all(|t| t.builtin));
        assert!(b.iter().any(|t| t.name == "QLab: GO"));
        assert!(b.iter().any(|t| t.name.starts_with("LiveProfessor")));
        assert!(b.iter().any(|t| matches!(
            t.action,
            TriggerAction::Midi {
                message: MidiMessage::ProgramChange { .. }
            }
        )));
        // The LiveProfessor recall built-in uses the verified Audiostrom path.
        let lp = b
            .iter()
            .find(|t| t.name == "LiveProfessor: recall snapshot [N]")
            .unwrap();
        match &lp.action {
            TriggerAction::Osc { path, args, .. } => {
                assert_eq!(path, "/GlobalSnapshots/Recall");
                assert_eq!(args, &vec![OscArg::Int(0)]);
            }
            _ => panic!("expected OSC action"),
        }
    }

    #[test]
    fn from_template_freezes_action_and_label() {
        let tmpl = &TriggerTemplate::builtins()[1]; // QLab: GO cue [N]
        let trig = CueTrigger::from_template(tmpl);
        assert_eq!(trig.label, tmpl.name);
        assert_eq!(trig.action, tmpl.action);
        assert!(trig.enabled);
    }
}
