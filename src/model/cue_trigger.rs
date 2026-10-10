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
    /// One of Go.dot's firing verbs, sent to the Go.dot address in Setup
    /// (contract `docs/godot-authoring-protocol-0.1.md` in the Go.dot
    /// repository, §5 and §7.3). Carries no address of its own: it goes
    /// where the show's Go.dot is.
    GoDot { verb: GoDotVerb },
}

/// What a Go.dot trigger asks Go.dot to do (contract §7.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoDotVerb {
    /// GO: the focused list's standby fires and moves on.
    Go,
    /// Fire the cue this snapshot was captured into, standby untouched.
    /// Read when the trigger fires: a snapshot never captured sends nothing.
    FireSnapshotCue,
    /// Park the standby on this snapshot's cue, then GO (one bundle).
    GoAtSnapshotCue,
    /// Fire the cue with this number (exact text), standby untouched.
    FireCueNumber(String),
    /// Park the standby on the cue with this number, then GO (one bundle).
    GoCueNumber(String),
    /// Esc: everything stops gracefully, footers run.
    Esc,
    /// Double Esc: everything is dropped, footers skipped.
    DoubleEsc,
}

impl GoDotVerb {
    /// Every verb, the numbered ones carrying `number`: the choices of the
    /// trigger row's verb menu.
    pub fn all(number: &str) -> [GoDotVerb; 7] {
        [
            GoDotVerb::Go,
            GoDotVerb::FireSnapshotCue,
            GoDotVerb::GoAtSnapshotCue,
            GoDotVerb::FireCueNumber(number.to_string()),
            GoDotVerb::GoCueNumber(number.to_string()),
            GoDotVerb::Esc,
            GoDotVerb::DoubleEsc,
        ]
    }

    /// The verb as the menus name it, with `[N]` for a number.
    pub fn name(&self) -> &'static str {
        match self {
            GoDotVerb::Go => "GO",
            GoDotVerb::FireSnapshotCue => "fire this snapshot's cue",
            GoDotVerb::GoAtSnapshotCue => "GO at this snapshot's cue",
            GoDotVerb::FireCueNumber(_) => "fire cue [N]",
            GoDotVerb::GoCueNumber(_) => "GO cue [N]",
            GoDotVerb::Esc => "Esc",
            GoDotVerb::DoubleEsc => "double Esc",
        }
    }

    /// The cue number a numbered verb carries.
    pub fn number(&self) -> Option<&str> {
        match self {
            GoDotVerb::FireCueNumber(n) | GoDotVerb::GoCueNumber(n) => Some(n),
            _ => None,
        }
    }

    /// The same verb with `number` (a verb without one is unchanged).
    pub fn with_number(&self, number: &str) -> GoDotVerb {
        match self {
            GoDotVerb::FireCueNumber(_) => GoDotVerb::FireCueNumber(number.to_string()),
            GoDotVerb::GoCueNumber(_) => GoDotVerb::GoCueNumber(number.to_string()),
            other => other.clone(),
        }
    }

    /// Whether two verbs are the same verb, whatever their numbers.
    pub fn same_kind(&self, other: &GoDotVerb) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    /// The verb named in full: `GO cue 12` rather than `GO cue [N]`.
    pub fn describe(&self) -> String {
        match self {
            GoDotVerb::FireCueNumber(n) => format!("fire cue {n}"),
            GoDotVerb::GoCueNumber(n) => format!("GO cue {n}"),
            other => other.name().to_string(),
        }
    }
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
            TriggerAction::GoDot { verb } => format!("Go.dot: {}", verb.describe()),
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
        let godot = |verb: GoDotVerb| TriggerAction::GoDot { verb };
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
            // ── Go.dot (contract §7.3; numbers are exact text) ──
            Self::builtin("Go.dot: GO", godot(GoDotVerb::Go)),
            Self::builtin(
                "Go.dot: fire this snapshot's cue",
                godot(GoDotVerb::FireSnapshotCue),
            ),
            Self::builtin(
                "Go.dot: GO at this snapshot's cue",
                godot(GoDotVerb::GoAtSnapshotCue),
            ),
            Self::builtin(
                "Go.dot: fire cue [N]",
                godot(GoDotVerb::FireCueNumber("1".to_string())),
            ),
            Self::builtin(
                "Go.dot: GO cue [N]",
                godot(GoDotVerb::GoCueNumber("1".to_string())),
            ),
            Self::builtin("Go.dot: Esc", godot(GoDotVerb::Esc)),
            Self::builtin("Go.dot: double Esc", godot(GoDotVerb::DoubleEsc)),
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
    fn godot_builtins_are_the_contracts_seven() {
        let names: Vec<String> = TriggerTemplate::builtins()
            .into_iter()
            .filter(|t| matches!(t.action, TriggerAction::GoDot { .. }))
            .map(|t| t.name)
            .collect();
        assert_eq!(
            names,
            vec![
                "Go.dot: GO",
                "Go.dot: fire this snapshot's cue",
                "Go.dot: GO at this snapshot's cue",
                "Go.dot: fire cue [N]",
                "Go.dot: GO cue [N]",
                "Go.dot: Esc",
                "Go.dot: double Esc",
            ]
        );
    }

    #[test]
    fn godot_triggers_round_trip_and_read_their_json() {
        let triggers: Vec<CueTrigger> = GoDotVerb::all("12")
            .into_iter()
            .map(|verb| CueTrigger::new(TriggerAction::GoDot { verb }))
            .collect();
        let json = serde_json::to_string_pretty(&triggers).unwrap();
        let back: Vec<CueTrigger> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, triggers);

        // The shape on disk: internally tagged like the other actions.
        let t: CueTrigger = serde_json::from_str(
            r#"{
                "id": "00000000-0000-0000-0000-000000000001",
                "label": "Go.dot: GO cue 12",
                "action": { "kind": "GoDot", "verb": { "GoCueNumber": "12" } }
            }"#,
        )
        .unwrap();
        assert_eq!(
            t.action,
            TriggerAction::GoDot {
                verb: GoDotVerb::GoCueNumber("12".into())
            }
        );
        let t: CueTrigger = serde_json::from_str(
            r#"{ "id": "00000000-0000-0000-0000-000000000002", "label": "x",
                 "action": { "kind": "GoDot", "verb": "DoubleEsc" } }"#,
        )
        .unwrap();
        assert_eq!(
            t.action,
            TriggerAction::GoDot {
                verb: GoDotVerb::DoubleEsc
            }
        );
    }

    #[test]
    fn godot_labels_and_numbers() {
        let go_12 = TriggerAction::GoDot {
            verb: GoDotVerb::GoCueNumber("12".into()),
        };
        assert_eq!(go_12.default_label(), "Go.dot: GO cue 12");
        assert_eq!(
            TriggerAction::GoDot {
                verb: GoDotVerb::FireSnapshotCue
            }
            .default_label(),
            "Go.dot: fire this snapshot's cue"
        );
        let fire = GoDotVerb::FireCueNumber("1".into());
        assert_eq!(
            fire.with_number("4.5"),
            GoDotVerb::FireCueNumber("4.5".into())
        );
        assert_eq!(GoDotVerb::Esc.with_number("4"), GoDotVerb::Esc);
        assert_eq!(fire.number(), Some("1"));
        assert!(fire.same_kind(&GoDotVerb::FireCueNumber("9".into())));
        assert!(!fire.same_kind(&GoDotVerb::GoCueNumber("1".into())));
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
