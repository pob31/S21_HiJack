use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::channel::ChannelId;
use super::parameter::{ParameterAddress, ParameterValue};

// ─── Persisted types ───────────────────────────────────────────────

/// A user-defined macro: a named, ordered sequence of console parameter changes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MacroDef {
    pub id: Uuid,
    pub name: String,
    pub steps: Vec<MacroStep>,
    /// When true (default), executing this macro lets the dirty tracker
    /// record which parameters changed — useful for preset-style macros
    /// where the operator wants to see what was modified. When false,
    /// execution suppresses dirty tracking and clears the dirty set
    /// afterward — useful for temporary actions (e.g. soundcheck solos)
    /// that shouldn't pollute the "modified since last recall" view.
    #[serde(default = "default_true")]
    pub mark_dirty: bool,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
}

fn default_true() -> bool {
    true
}

impl MacroDef {
    /// Create a new macro with a generated UUID and current timestamps.
    pub fn new(name: String, steps: Vec<MacroStep>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            steps,
            mark_dirty: true,
            created_at: now,
            modified_at: now,
        }
    }

    /// Touch the modified_at timestamp (call after editing steps).
    pub fn touch(&mut self) {
        self.modified_at = Utc::now();
    }

    /// Remove every other step that targets the same `(channel, parameter)`
    /// as the step at `idx`. Only meaningful for `Parameter`-kind steps;
    /// app-action kinds (Go, Connect, FireMacro, …) have no parameter
    /// address to match against and are returned `None` immediately.
    /// The kept step's index may shift downward as earlier duplicates
    /// are removed; the new kept index is returned along with the
    /// number of removed steps.
    pub fn keep_only_step(&mut self, idx: usize) -> Option<(usize, usize)> {
        let target_addr = self.steps.get(idx)?.parameter_address()?.clone();
        let mut new_idx = idx;
        let mut removed = 0usize;
        let mut i = 0;
        self.steps.retain(|step| {
            let same = step.parameter_address() == Some(&target_addr);
            let kept = !same || i == idx;
            if !kept && i < idx {
                new_idx -= 1;
            }
            if !kept {
                removed += 1;
            }
            i += 1;
            kept
        });
        if removed > 0 {
            self.touch();
        }
        Some((new_idx, removed))
    }
}

/// A single step within a macro. The `kind` field discriminates
/// between an OSC parameter write and the various app-internal
/// commands (cue transport, connect/disconnect, run-another-macro,
/// recall snapshot/palette).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MacroStep {
    pub kind: MacroStepKind,
    /// Delay in milliseconds before this step executes,
    /// measured from the completion of the previous step
    /// (or from macro start for the first step).
    pub delay_ms: u32,
}

/// The action a macro step performs at execution time.
///
/// `Parameter` is what learn-mode recordings produce — a direct OSC
/// write. The other variants are app-internal commands the operator
/// can add through the Add Step UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum MacroStepKind {
    /// Write a parameter value to the console.
    Parameter {
        address: ParameterAddress,
        mode: MacroStepMode,
    },
    /// Advance the cue list to the next cue (same as the top-bar Go).
    GoNextCue,
    /// Step back to the previous cue (same as the top-bar Prev).
    GoPreviousCue,
    /// Trigger a console connection using the current Setup-tab
    /// settings.
    Connect,
    /// Disconnect from the console. **Caveat:** this tears down the
    /// connection that's running the macro — subsequent steps in the
    /// same macro will not execute.
    Disconnect,
    /// Fire another macro. Recursion is guarded at runtime by a
    /// depth counter; a macro that calls itself (directly or
    /// transitively) is rejected once the depth limit is hit.
    FireMacro { id: Uuid },
    /// Recall a specific snapshot directly, bypassing the cue list.
    RecallSnapshot { id: Uuid },
    /// Apply a palette to the given channel.
    RecallPalette { id: Uuid, channel: ChannelId },
    /// Send a `/go` OSC message to QLab — fires QLab's next cue,
    /// independent of this app's internal cue list. Useful in Theatre
    /// setups where QLab is running its own cue stack.
    QLabGo,
    /// Send `/go {cue_number}` to QLab — fires a specific QLab cue by
    /// number. Cue numbers are QLab's own free-form strings (e.g. "1",
    /// "2.5", "Q12"), not numeric IDs.
    QLabGoCue { cue_number: String },
    /// Send `/panic` to QLab — fade out and hard-stop everything.
    QLabPanic,
    /// Send `/stop` to QLab — stop playback but let audio tails decay.
    QLabStop,
    /// Send `/pause` to QLab — pause all running cues.
    QLabPause,
    /// Send `/resume` to QLab — un-pause all paused cues.
    QLabResume,
}

impl MacroStep {
    /// Convenience constructor for the (overwhelmingly common)
    /// parameter-write case — keeps existing call sites brief.
    pub fn parameter(address: ParameterAddress, mode: MacroStepMode, delay_ms: u32) -> Self {
        Self {
            kind: MacroStepKind::Parameter { address, mode },
            delay_ms,
        }
    }

    /// `Some(&address)` when this step is a `Parameter` write,
    /// `None` for the app-internal kinds. Used by `keep_only_step`
    /// and any UI that wants to compare addresses across steps.
    pub fn parameter_address(&self) -> Option<&ParameterAddress> {
        match &self.kind {
            MacroStepKind::Parameter { address, .. } => Some(address),
            _ => None,
        }
    }

    /// `Some(&mode)` when this step is a `Parameter` write,
    /// `None` for the app-internal kinds.
    pub fn parameter_mode(&self) -> Option<&MacroStepMode> {
        match &self.kind {
            MacroStepKind::Parameter { mode, .. } => Some(mode),
            _ => None,
        }
    }
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

/// Float tolerance for treating an echoed value as identical to the last
/// recorded value for the same address (console echoes can round-trip with a
/// tiny difference, e.g. across the GP-OSC vs iPad encodings).
const RECORD_VALUE_TOLERANCE: f32 = 0.001;

/// An in-progress recording session (learn mode).
/// Not serialized — only exists while recording is active.
#[derive(Clone, Debug)]
pub struct MacroRecording {
    pub steps: Vec<RecordedStep>,
    started_at: std::time::Instant,
    last_step_at: std::time::Instant,
    /// Last value recorded per address, used to drop echoes — a value
    /// re-reported for an address that already holds it (see
    /// [`MacroRecording::record`]).
    last_value_per_address: HashMap<ParameterAddress, ParameterValue>,
}

/// Equality used for echo-dedup: floats within [`RECORD_VALUE_TOLERANCE`],
/// everything else exact.
fn values_equivalent(a: &ParameterValue, b: &ParameterValue) -> bool {
    match (a, b) {
        (ParameterValue::Float(x), ParameterValue::Float(y)) => {
            (x - y).abs() < RECORD_VALUE_TOLERANCE
        }
        _ => a == b,
    }
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
            last_value_per_address: HashMap::new(),
        }
    }

    /// Record a parameter change. Computes delay from the previous step automatically.
    ///
    /// In Operating Modes 2/3 the daemon mirrors the console over both the
    /// GP-OSC and iPad-protocol links at once, so a single console action can
    /// echo on both — sometimes much later, and with other parameters recorded
    /// in between — which would record the identical step twice. Drop a change
    /// whose value equals the **last value already recorded for that address**
    /// (within [`RECORD_VALUE_TOLERANCE`]): re-writing a value the channel is
    /// already at is a no-op on playback, so collapsing consecutive identical
    /// values per address is safe and — unlike a fixed time window — catches
    /// late echoes too. A genuine A→B→A sequence still records every step,
    /// because the intervening B changes the address's last recorded value.
    pub fn record(&mut self, address: ParameterAddress, value: ParameterValue) {
        if let Some(last) = self.last_value_per_address.get(&address) {
            if values_equivalent(last, &value) {
                return;
            }
        }
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_step_at).as_millis() as u32;
        self.last_step_at = now;
        self.last_value_per_address
            .insert(address.clone(), value.clone());
        self.steps.push(RecordedStep {
            address,
            value,
            elapsed_ms: elapsed,
        });
    }

    /// Convert this recording into a MacroDef.
    /// All steps become Fixed-mode `Parameter` writes with the
    /// recorded values — recordings can only ever produce parameter
    /// writes (the app-internal kinds are added through the UI).
    pub fn to_macro_def(&self, name: String) -> MacroDef {
        let steps = self
            .steps
            .iter()
            .map(|rs| {
                MacroStep::parameter(
                    rs.address.clone(),
                    MacroStepMode::Fixed(rs.value.clone()),
                    rs.elapsed_ms,
                )
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
            MacroStep::parameter(make_addr(1, ParameterPath::Mute), MacroStepMode::Toggle, 0),
            MacroStep::parameter(
                make_addr(2, ParameterPath::Fader),
                MacroStepMode::Fixed(ParameterValue::Float(-10.0)),
                100,
            ),
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
    fn recording_dedupes_rapid_duplicate_echo() {
        // Same (address, value) arriving back-to-back (the GP-OSC + iPad
        // double-echo in Modes 2/3) records only once.
        let mut rec = MacroRecording::new();
        rec.record(
            make_addr(1, ParameterPath::Fader),
            ParameterValue::Float(-3.0),
        );
        rec.record(
            make_addr(1, ParameterPath::Fader),
            ParameterValue::Float(-3.0),
        );
        assert_eq!(rec.step_count(), 1, "duplicate echo should be coalesced");

        // A different value on the same channel is a real change — recorded.
        rec.record(
            make_addr(1, ParameterPath::Fader),
            ParameterValue::Float(-2.0),
        );
        assert_eq!(rec.step_count(), 2);

        // Same value on a different parameter is also distinct.
        rec.record(
            make_addr(1, ParameterPath::Mute),
            ParameterValue::Float(-2.0),
        );
        assert_eq!(rec.step_count(), 3);
    }

    #[test]
    fn recording_drops_late_echo_after_other_steps() {
        // A late echo of an earlier value — arriving after other parameters
        // were recorded, so it is NOT the immediately-preceding step — is
        // still dropped (the value already matches the address's last record).
        let mut rec = MacroRecording::new();
        rec.record(
            make_addr(1, ParameterPath::Fader),
            ParameterValue::Float(-3.0),
        );
        rec.record(
            make_addr(2, ParameterPath::Fader),
            ParameterValue::Float(-5.0),
        );
        assert_eq!(rec.step_count(), 2);

        // Late echo of channel 1's -3.0 — must NOT add a third step.
        rec.record(
            make_addr(1, ParameterPath::Fader),
            ParameterValue::Float(-3.0),
        );
        assert_eq!(rec.step_count(), 2, "late echo should be dropped");

        // But a genuine A→B→A on channel 1 records every step.
        rec.record(
            make_addr(1, ParameterPath::Fader),
            ParameterValue::Float(0.0),
        );
        rec.record(
            make_addr(1, ParameterPath::Fader),
            ParameterValue::Float(-3.0),
        );
        assert_eq!(rec.step_count(), 4);
    }

    #[test]
    fn recording_drops_echo_within_float_tolerance() {
        // An echo that round-trips with a sub-tolerance float difference is
        // still treated as the same value.
        let mut rec = MacroRecording::new();
        rec.record(
            make_addr(1, ParameterPath::Pan),
            ParameterValue::Float(0.25),
        );
        rec.record(
            make_addr(1, ParameterPath::Pan),
            ParameterValue::Float(0.2505),
        );
        assert_eq!(rec.step_count(), 1, "near-equal echo should be dropped");
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

        // All steps should be Fixed-mode parameter writes.
        assert_eq!(
            m.steps[0].parameter_mode().cloned(),
            Some(MacroStepMode::Fixed(ParameterValue::Bool(true)))
        );
        assert_eq!(
            m.steps[1].parameter_mode().cloned(),
            Some(MacroStepMode::Fixed(ParameterValue::Float(0.0)))
        );
    }

    #[test]
    fn serialization_round_trip() {
        let steps = vec![
            MacroStep::parameter(make_addr(1, ParameterPath::Mute), MacroStepMode::Toggle, 0),
            MacroStep::parameter(
                make_addr(2, ParameterPath::Fader),
                MacroStepMode::Fixed(ParameterValue::Float(-10.0)),
                100,
            ),
            MacroStep::parameter(
                make_addr(3, ParameterPath::AnalogGain),
                MacroStepMode::Relative(3.5),
                200,
            ),
            MacroStep {
                kind: MacroStepKind::GoNextCue,
                delay_ms: 50,
            },
            MacroStep {
                kind: MacroStepKind::FireMacro { id: Uuid::new_v4() },
                delay_ms: 0,
            },
        ];
        let original = MacroDef::new("Serialize Test".into(), steps);

        let json = serde_json::to_string_pretty(&original).unwrap();
        let loaded: MacroDef = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.name, original.name);
        assert_eq!(loaded.id, original.id);
        assert_eq!(loaded.steps.len(), 5);
        assert_eq!(
            loaded.steps[0].parameter_mode().cloned(),
            Some(MacroStepMode::Toggle)
        );
        assert_eq!(loaded.steps[1].delay_ms, 100);
        assert_eq!(
            loaded.steps[2].parameter_mode().cloned(),
            Some(MacroStepMode::Relative(3.5))
        );
        assert_eq!(loaded.steps[3].kind, MacroStepKind::GoNextCue);
        // FireMacro round-trips with the same UUID.
        match (&loaded.steps[4].kind, &original.steps[4].kind) {
            (MacroStepKind::FireMacro { id: a }, MacroStepKind::FireMacro { id: b }) => {
                assert_eq!(a, b)
            }
            _ => panic!("expected FireMacro both sides"),
        }
    }

    #[test]
    fn keep_only_step_removes_duplicates_of_same_address() {
        let addr_fader_1 = make_addr(1, ParameterPath::Fader);
        let addr_mute_1 = make_addr(1, ParameterPath::Mute);
        let mk = |addr: ParameterAddress, val: f32| {
            MacroStep::parameter(addr, MacroStepMode::Fixed(ParameterValue::Float(val)), 0)
        };
        let mut m = MacroDef::new(
            "Test".into(),
            vec![
                mk(addr_fader_1.clone(), -10.0), // 0
                mk(addr_mute_1.clone(), 1.0),    // 1 — different param, keep
                mk(addr_fader_1.clone(), -5.0),  // 2 ← keep
                mk(addr_fader_1.clone(), 0.0),   // 3
                mk(addr_mute_1.clone(), 0.0),    // 4 — different param, keep
            ],
        );

        let (new_idx, removed) = m.keep_only_step(2).unwrap();
        assert_eq!(removed, 2, "two duplicate fader-1 steps removed");
        assert_eq!(
            new_idx, 1,
            "kept index shifts from 2 to 1 after the earlier duplicate is removed"
        );
        assert_eq!(m.steps.len(), 3);
        assert_eq!(m.steps[0].parameter_address(), Some(&addr_mute_1));
        assert_eq!(m.steps[1].parameter_address(), Some(&addr_fader_1));
        assert_eq!(m.steps[2].parameter_address(), Some(&addr_mute_1));

        // Verify the kept step is the one we asked for (-5.0)
        assert_eq!(
            m.steps[1].parameter_mode().cloned(),
            Some(MacroStepMode::Fixed(ParameterValue::Float(-5.0)))
        );
    }

    #[test]
    fn keep_only_step_no_duplicates_is_noop() {
        let mut m = MacroDef::new(
            "Test".into(),
            vec![
                MacroStep::parameter(make_addr(1, ParameterPath::Fader), MacroStepMode::Toggle, 0),
                MacroStep::parameter(make_addr(2, ParameterPath::Fader), MacroStepMode::Toggle, 0),
            ],
        );
        let modified_before = m.modified_at;
        std::thread::sleep(std::time::Duration::from_millis(2));

        let (new_idx, removed) = m.keep_only_step(0).unwrap();
        assert_eq!(removed, 0);
        assert_eq!(new_idx, 0);
        assert_eq!(m.steps.len(), 2);
        assert_eq!(m.modified_at, modified_before, "no-op should not touch");
    }

    #[test]
    fn keep_only_step_skips_app_action_steps() {
        // App-action steps don't have a parameter address, so
        // `keep_only_step` returns None when called on them. Other
        // steps in the macro are unaffected.
        let mut m = MacroDef::new(
            "Test".into(),
            vec![
                MacroStep {
                    kind: MacroStepKind::GoNextCue,
                    delay_ms: 0,
                },
                MacroStep::parameter(make_addr(1, ParameterPath::Fader), MacroStepMode::Toggle, 0),
            ],
        );
        assert!(m.keep_only_step(0).is_none(), "GoNextCue has no address");
        assert_eq!(m.steps.len(), 2, "macro untouched after the no-op call");
    }

    #[test]
    fn keep_only_step_invalid_index_returns_none() {
        let mut m = MacroDef::new("Test".into(), vec![]);
        assert!(m.keep_only_step(0).is_none());
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
