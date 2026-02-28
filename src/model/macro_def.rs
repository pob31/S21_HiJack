use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::parameter::{ParameterAddress, ParameterValue};

// ─── Persisted types ───────────────────────────────────────────────

/// A user-defined macro: a named, ordered sequence of console parameter changes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MacroDef {
    pub id: Uuid,
    pub name: String,
    pub steps: Vec<MacroStep>,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
}

impl MacroDef {
    /// Create a new macro with a generated UUID and current timestamps.
    pub fn new(name: String, steps: Vec<MacroStep>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            steps,
            created_at: now,
            modified_at: now,
        }
    }

    /// Touch the modified_at timestamp (call after editing steps).
    pub fn touch(&mut self) {
        self.modified_at = Utc::now();
    }
}

/// A single step within a macro.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MacroStep {
    /// Which channel and parameter this step targets.
    pub address: ParameterAddress,
    /// How the target value is determined at execution time.
    pub mode: MacroStepMode,
    /// Delay in milliseconds before this step executes,
    /// measured from the completion of the previous step
    /// (or from macro start for the first step).
    pub delay_ms: u32,
}

/// How a macro step resolves its target value at execution time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MacroStepMode {
    /// Send the logical opposite of the current live value.
    /// Bool: negate. Int: 0 <-> 1. Float: 0.0 <-> 1.0.
    Toggle,
    /// Always send this exact value, regardless of live state.
    Fixed(ParameterValue),
    /// Add this offset to the current live value.
    /// Applicable to Float and Int parameters only.
    Relative(f32),
}

impl std::fmt::Display for MacroStepMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MacroStepMode::Toggle => write!(f, "Toggle"),
            MacroStepMode::Fixed(v) => write!(f, "Fixed({v})"),
            MacroStepMode::Relative(o) => {
                if *o >= 0.0 {
                    write!(f, "Relative(+{o})")
                } else {
                    write!(f, "Relative({o})")
                }
            }
        }
    }
}

// ─── Recording types (not persisted, runtime only) ─────────────────

/// An in-progress recording session (learn mode).
/// Not serialized — only exists while recording is active.
#[derive(Clone, Debug)]
pub struct MacroRecording {
    pub steps: Vec<RecordedStep>,
    started_at: std::time::Instant,
    last_step_at: std::time::Instant,
}

/// A single parameter change captured during learn mode.
#[derive(Clone, Debug)]
pub struct RecordedStep {
    pub address: ParameterAddress,
    pub value: ParameterValue,
    /// Milliseconds elapsed since the previous step
    /// (or since recording started, for the first step).
    pub elapsed_ms: u32,
}

impl MacroRecording {
    pub fn new() -> Self {
        let now = std::time::Instant::now();
        Self {
            steps: Vec::new(),
            started_at: now,
            last_step_at: now,
        }
    }

    /// Record a parameter change. Computes delay from the previous step automatically.
    pub fn record(&mut self, address: ParameterAddress, value: ParameterValue) {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_step_at).as_millis() as u32;
        self.last_step_at = now;
        self.steps.push(RecordedStep {
            address,
            value,
            elapsed_ms: elapsed,
        });
    }

    /// Convert this recording into a MacroDef.
    /// All steps become Fixed mode with the recorded values.
    pub fn to_macro_def(&self, name: String) -> MacroDef {
        let steps = self
            .steps
            .iter()
            .map(|rs| MacroStep {
                address: rs.address.clone(),
                mode: MacroStepMode::Fixed(rs.value.clone()),
                delay_ms: rs.elapsed_ms,
            })
            .collect();
        MacroDef::new(name, steps)
    }

    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// Total elapsed time since recording started, in milliseconds.
    pub fn elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::parameter::ParameterPath;

    fn make_addr(ch: u8, param: ParameterPath) -> ParameterAddress {
        ParameterAddress {
            channel: ChannelId::Input(ch),
            parameter: param,
        }
    }

    #[test]
    fn macro_def_creation() {
        let steps = vec![
            MacroStep {
                address: make_addr(1, ParameterPath::Mute),
                mode: MacroStepMode::Toggle,
                delay_ms: 0,
            },
            MacroStep {
                address: make_addr(2, ParameterPath::Fader),
                mode: MacroStepMode::Fixed(ParameterValue::Float(-10.0)),
                delay_ms: 100,
            },
        ];
        let m = MacroDef::new("Test Macro".into(), steps);

        assert_eq!(m.name, "Test Macro");
        assert_eq!(m.steps.len(), 2);
        assert!(m.created_at <= Utc::now());
        assert_eq!(m.created_at, m.modified_at);
    }

    #[test]
    fn macro_def_touch() {
        let mut m = MacroDef::new("Test".into(), vec![]);
        let created = m.created_at;
        std::thread::sleep(std::time::Duration::from_millis(2));
        m.touch();
        assert_eq!(m.created_at, created);
        assert!(m.modified_at > created);
    }

    #[test]
    fn recording_captures_steps_with_delays() {
        let mut rec = MacroRecording::new();

        rec.record(
            make_addr(1, ParameterPath::Mute),
            ParameterValue::Bool(true),
        );
        assert_eq!(rec.step_count(), 1);

        std::thread::sleep(std::time::Duration::from_millis(50));

        rec.record(
            make_addr(1, ParameterPath::Fader),
            ParameterValue::Float(-5.0),
        );
        assert_eq!(rec.step_count(), 2);

        // The second step should have a delay of roughly 50ms
        let delay = rec.steps[1].elapsed_ms;
        assert!(delay >= 30, "Expected delay >= 30ms, got {delay}ms");
        assert!(delay <= 200, "Expected delay <= 200ms, got {delay}ms");
    }

    #[test]
    fn recording_to_macro_def() {
        let mut rec = MacroRecording::new();
        rec.record(
            make_addr(1, ParameterPath::Mute),
            ParameterValue::Bool(true),
        );
        rec.record(
            make_addr(2, ParameterPath::Fader),
            ParameterValue::Float(0.0),
        );

        let m = rec.to_macro_def("Recorded".into());
        assert_eq!(m.name, "Recorded");
        assert_eq!(m.steps.len(), 2);

        // All steps should be Fixed mode
        assert_eq!(
            m.steps[0].mode,
            MacroStepMode::Fixed(ParameterValue::Bool(true))
        );
        assert_eq!(
            m.steps[1].mode,
            MacroStepMode::Fixed(ParameterValue::Float(0.0))
        );
    }

    #[test]
    fn serialization_round_trip() {
        let steps = vec![
            MacroStep {
                address: make_addr(1, ParameterPath::Mute),
                mode: MacroStepMode::Toggle,
                delay_ms: 0,
            },
            MacroStep {
                address: make_addr(2, ParameterPath::Fader),
                mode: MacroStepMode::Fixed(ParameterValue::Float(-10.0)),
                delay_ms: 100,
            },
            MacroStep {
                address: make_addr(3, ParameterPath::Gain),
                mode: MacroStepMode::Relative(3.5),
                delay_ms: 200,
            },
        ];
        let original = MacroDef::new("Serialize Test".into(), steps);

        let json = serde_json::to_string_pretty(&original).unwrap();
        let loaded: MacroDef = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.name, original.name);
        assert_eq!(loaded.id, original.id);
        assert_eq!(loaded.steps.len(), 3);
        assert_eq!(loaded.steps[0].mode, MacroStepMode::Toggle);
        assert_eq!(loaded.steps[1].delay_ms, 100);
        assert_eq!(loaded.steps[2].mode, MacroStepMode::Relative(3.5));
    }

    #[test]
    fn step_mode_display() {
        assert_eq!(format!("{}", MacroStepMode::Toggle), "Toggle");
        assert_eq!(
            format!("{}", MacroStepMode::Fixed(ParameterValue::Bool(true))),
            "Fixed(true)"
        );
        assert_eq!(format!("{}", MacroStepMode::Relative(3.0)), "Relative(+3)");
        assert_eq!(
            format!("{}", MacroStepMode::Relative(-2.5)),
            "Relative(-2.5)"
        );
    }
}
