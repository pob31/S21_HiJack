use std::collections::HashMap;

use tracing::{info, warn};
use uuid::Uuid;

use crate::model::macro_def::{MacroDef, MacroRecording};
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};

/// Manages the collection of macros and learn mode state.
pub struct MacroManager {
    /// All macros indexed by UUID.
    pub macros: HashMap<Uuid, MacroDef>,
    /// Active recording session. None when not in learn mode.
    recording: Option<MacroRecording>,
}

impl MacroManager {
    pub fn new() -> Self {
        Self {
            macros: HashMap::new(),
            recording: None,
        }
    }

    // ─── CRUD ──────────────────────────────────────────────────────

    pub fn add_macro(&mut self, macro_def: MacroDef) {
        info!(name = %macro_def.name, id = %macro_def.id, "Added macro");
        self.macros.insert(macro_def.id, macro_def);
    }

    pub fn remove_macro(&mut self, id: Uuid) -> bool {
        let removed = self.macros.remove(&id).is_some();
        if removed {
            info!(%id, "Removed macro");
        }
        removed
    }

    pub fn get_macro(&self, id: &Uuid) -> Option<&MacroDef> {
        self.macros.get(id)
    }

    pub fn get_macro_mut(&mut self, id: &Uuid) -> Option<&mut MacroDef> {
        self.macros.get_mut(id)
    }

    /// Find a macro by name (case-insensitive).
    pub fn find_by_name(&self, name: &str) -> Option<&MacroDef> {
        self.macros
            .values()
            .find(|m| m.name.eq_ignore_ascii_case(name))
    }

    /// Find a macro by name or UUID string.
    /// Used by the `/macro/fire` trigger which accepts either form.
    pub fn find_by_name_or_id(&self, identifier: &str) -> Option<&MacroDef> {
        // Try UUID parse first
        if let Ok(uuid) = Uuid::parse_str(identifier) {
            if let Some(m) = self.macros.get(&uuid) {
                return Some(m);
            }
        }
        // Fall back to case-insensitive name search
        self.find_by_name(identifier)
    }

    /// Return all macros sorted by name (for UI display).
    pub fn sorted_macros(&self) -> Vec<&MacroDef> {
        let mut macros: Vec<_> = self.macros.values().collect();
        macros.sort_by(|a, b| a.name.cmp(&b.name));
        macros
    }

    // ─── Recording (Learn Mode) ────────────────────────────────────

    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// Begin a new learn-mode recording session.
    /// If a recording is already in progress, it is discarded.
    pub fn start_recording(&mut self) {
        if self.recording.is_some() {
            warn!("Discarding existing recording to start a new one");
        }
        info!("Macro learn mode started");
        self.recording = Some(MacroRecording::new());
    }

    /// Stop the current recording and return it.
    /// Returns None if no recording was in progress.
    pub fn stop_recording(&mut self) -> Option<MacroRecording> {
        if let Some(rec) = self.recording.take() {
            info!(steps = rec.step_count(), "Macro learn mode stopped");
            Some(rec)
        } else {
            warn!("stop_recording called with no recording in progress");
            None
        }
    }

    /// Feed a parameter change into the active recording.
    /// Called from the state mirror loop whenever a parameter update arrives
    /// from the console. Returns true if the change was recorded.
    ///
    /// `TotalGain` is intentionally filtered out — it's a derived value
    /// (analog gain + trim, exposed by the console for monitoring) and
    /// the operator never sets it directly. Recording it just produces
    /// noise steps that fight whatever the analog gain / trim chain
    /// actually wrote.
    pub fn record_change(&mut self, address: ParameterAddress, value: ParameterValue) -> bool {
        if matches!(address.parameter, ParameterPath::TotalGain) {
            return false;
        }
        if let Some(ref mut rec) = self.recording {
            rec.record(address, value);
            true
        } else {
            false
        }
    }

    /// Number of steps recorded so far (for UI display during recording).
    pub fn recording_step_count(&self) -> usize {
        self.recording.as_ref().map(|r| r.step_count()).unwrap_or(0)
    }

    /// Elapsed recording time in milliseconds (for UI display).
    pub fn recording_elapsed_ms(&self) -> u64 {
        self.recording.as_ref().map(|r| r.elapsed_ms()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::macro_def::{MacroStep, MacroStepMode};
    use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};

    fn make_macro(name: &str) -> MacroDef {
        MacroDef::new(
            name.to_string(),
            vec![MacroStep::parameter(
                ParameterAddress {
                    channel: ChannelId::Input(1),
                    parameter: ParameterPath::Mute,
                },
                MacroStepMode::Toggle,
                0,
            )],
        )
    }

    #[test]
    fn add_and_get() {
        let mut mgr = MacroManager::new();
        let m = make_macro("Mute Toggle");
        let id = m.id;
        mgr.add_macro(m);

        assert!(mgr.get_macro(&id).is_some());
        assert_eq!(mgr.get_macro(&id).unwrap().name, "Mute Toggle");
    }

    #[test]
    fn remove() {
        let mut mgr = MacroManager::new();
        let m = make_macro("Test");
        let id = m.id;
        mgr.add_macro(m);

        assert!(mgr.remove_macro(id));
        assert!(mgr.get_macro(&id).is_none());
    }

    #[test]
    fn find_by_name_case_insensitive() {
        let mut mgr = MacroManager::new();
        mgr.add_macro(make_macro("Mute All"));

        assert!(mgr.find_by_name("mute all").is_some());
        assert!(mgr.find_by_name("MUTE ALL").is_some());
        assert!(mgr.find_by_name("nonexistent").is_none());
    }

    #[test]
    fn find_by_name_or_id() {
        let mut mgr = MacroManager::new();
        let m = make_macro("Test Macro");
        let id = m.id;
        mgr.add_macro(m);

        // By name
        assert!(mgr.find_by_name_or_id("test macro").is_some());
        // By UUID string
        assert!(mgr.find_by_name_or_id(&id.to_string()).is_some());
        // Not found
        assert!(mgr.find_by_name_or_id("nonexistent").is_none());
    }

    #[test]
    fn recording_lifecycle() {
        let mut mgr = MacroManager::new();

        assert!(!mgr.is_recording());
        assert_eq!(mgr.recording_step_count(), 0);

        mgr.start_recording();
        assert!(mgr.is_recording());

        let recorded = mgr.record_change(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(-10.0),
        );
        assert!(recorded);
        assert_eq!(mgr.recording_step_count(), 1);

        let rec = mgr.stop_recording().unwrap();
        assert_eq!(rec.step_count(), 1);
        assert!(!mgr.is_recording());

        // No recording in progress
        assert!(mgr.stop_recording().is_none());
    }

    #[test]
    fn record_skips_total_gain() {
        // TotalGain is derived (analog gain + trim) and the operator
        // never sets it directly — recording it just produces noise
        // steps. Verify it's filtered out even while recording.
        let mut mgr = MacroManager::new();
        mgr.start_recording();

        // A real operator change (Fader) is recorded.
        assert!(mgr.record_change(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(-3.0),
        ));
        // TotalGain echo is filtered.
        assert!(!mgr.record_change(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::TotalGain,
            },
            ParameterValue::Float(20.0),
        ));
        assert_eq!(mgr.recording_step_count(), 1);
    }

    #[test]
    fn record_change_without_recording() {
        let mut mgr = MacroManager::new();
        let recorded = mgr.record_change(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(0.0),
        );
        assert!(!recorded);
    }

    #[test]
    fn sorted_macros() {
        let mut mgr = MacroManager::new();
        mgr.add_macro(make_macro("Zebra"));
        mgr.add_macro(make_macro("Alpha"));
        mgr.add_macro(make_macro("Middle"));

        let sorted = mgr.sorted_macros();
        assert_eq!(sorted[0].name, "Alpha");
        assert_eq!(sorted[1].name, "Middle");
        assert_eq!(sorted[2].name, "Zebra");
    }
}
