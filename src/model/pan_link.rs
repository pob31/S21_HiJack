//! Pan Link bindings: per-input-channel "follow main pan" assignments
//! for stereo aux send pans.
//!
//! When the operator moves an input channel's main pan, every active
//! `(input, aux)` binding causes the matching aux send pan to be set to
//! the same absolute value. Manually moving an aux send pan acts as a
//! temporary override — the next input pan move re-asserts the link.
//!
//! Bindings are NOT applied to console memory recalls (snapshot / cue /
//! macro driven recalls). The pan link engine consults the snapshot
//! engine's dirty-suppression flag and skips propagation while a recall
//! is in flight.
//!
//! Aux numbering is the unified mix-output bus index (1-based), matching
//! `ParameterPath::SendPan(bus_index)`. Only buses where
//! `ConsoleConfig::mix_output_types[bus-1] == true` AND the bus is stereo
//! are linkable.

use std::collections::{BTreeSet, HashSet};

use serde::{Deserialize, Serialize};

/// Active pan-link bindings, keyed by `(input_channel, aux_bus_index)`,
/// both 1-based.
///
/// `mono_overrides` is the operator's manual list of aux buses that
/// should be treated as mono for pan-link purposes regardless of the
/// console-reported mode. GP OSC doesn't expose the bus mode at all
/// (so every aux defaults to "stereo, linkable"), and even with the
/// iPad protocol the operator may want to disable pan-link for a
/// specific stereo aux — so the override is one-direction: marking
/// an aux mono blocks linking. Default: empty (every aux linkable
/// according to its discoverable mode).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanLinkBindings {
    #[serde(default)]
    pub active: HashSet<(u8, u8)>,
    #[serde(default)]
    pub mono_overrides: HashSet<u8>,
}

impl PanLinkBindings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_active(&self, input: u8, aux: u8) -> bool {
        self.active.contains(&(input, aux))
    }

    pub fn set_active(&mut self, input: u8, aux: u8, on: bool) {
        if on {
            self.active.insert((input, aux));
        } else {
            self.active.remove(&(input, aux));
        }
    }

    /// Aux bus indices linked for the given input, sorted ascending.
    pub fn auxes_for(&self, input: u8) -> Vec<u8> {
        let mut v: BTreeSet<u8> = self
            .active
            .iter()
            .filter_map(|(i, a)| (*i == input).then_some(*a))
            .collect();
        std::mem::take(&mut v).into_iter().collect()
    }

    /// Inputs that have at least one active binding, sorted ascending.
    pub fn linked_inputs(&self) -> Vec<u8> {
        let set: BTreeSet<u8> = self.active.iter().map(|(i, _)| *i).collect();
        set.into_iter().collect()
    }

    /// Whether the operator has manually marked this aux as mono. When
    /// true, the engine and UI both refuse to pan-link to this aux
    /// regardless of any console-reported mode.
    pub fn is_aux_mono_override(&self, aux: u8) -> bool {
        self.mono_overrides.contains(&aux)
    }

    /// Toggle the mono override for the given aux. Returns the new
    /// state (true = mono, false = stereo).
    pub fn toggle_mono_override(&mut self, aux: u8) -> bool {
        if !self.mono_overrides.insert(aux) {
            self.mono_overrides.remove(&aux);
            false
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_query() {
        let mut b = PanLinkBindings::new();
        b.set_active(3, 5, true);
        b.set_active(3, 7, true);
        b.set_active(12, 5, true);
        assert!(b.is_active(3, 5));
        assert!(!b.is_active(4, 5));
        assert_eq!(b.auxes_for(3), vec![5, 7]);
        assert_eq!(b.linked_inputs(), vec![3, 12]);
    }

    #[test]
    fn unset() {
        let mut b = PanLinkBindings::new();
        b.set_active(1, 1, true);
        b.set_active(1, 1, false);
        assert!(!b.is_active(1, 1));
    }

    #[test]
    fn serde_round_trip() {
        let mut b = PanLinkBindings::new();
        b.set_active(2, 4, true);
        let json = serde_json::to_string(&b).unwrap();
        let loaded: PanLinkBindings = serde_json::from_str(&json).unwrap();
        assert!(loaded.is_active(2, 4));
    }
}
