//! Stream Deck integration model.
//!
//! Maps each physical Stream Deck button to a *sequence* of macros.
//! Each press fires the next-to-fire macro, then advances the cursor
//! (`current_step`) through the button's step list, wrapping back to
//! 0 after the last entry. The LCD on the button always displays the
//! name of the next-to-fire macro so the operator can read what the
//! next press will do.
//!
//! Persisted in the show file alongside `Vec<MacroDef>`, so a button
//! map travels with the show. `current_step` is also persisted — an
//! ephemeral playback cursor, but persisting it survives quit /
//! reopen without surprising the operator with a reset.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Top-level Stream Deck configuration. One per show file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StreamDeckConfig {
    /// Master enable. When `false`, the engine skips device connection
    /// even if `device_serial` is set — gives the operator a one-click
    /// off without losing the device selection or any per-button maps.
    #[serde(default)]
    pub enabled: bool,
    /// HID serial of the most-recently-selected device. Used to
    /// auto-reconnect across launches when the same device is plugged
    /// in. `None` means "no device chosen yet".
    #[serde(default)]
    pub device_serial: Option<String>,
    /// One slot per physical button, indexed top-left → bottom-right
    /// in row-major order. Resized to match the device's button count
    /// on connect (overlap is preserved; new slots default-init).
    #[serde(default)]
    pub buttons: Vec<StreamDeckButton>,
}

/// One physical button's macro sequence + playback cursor.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StreamDeckButton {
    /// Macros to fire in order, one per press.
    #[serde(default)]
    pub steps: Vec<StreamDeckStep>,
    /// Index of the *next-to-fire* step (0..steps.len()). On press,
    /// fire `steps[current_step]`, then advance via `(current_step + 1) %
    /// steps.len()`.
    #[serde(default)]
    pub current_step: u32,
}

impl StreamDeckButton {
    /// Advance the cursor after firing. Wraps to 0 at the end.
    /// No-op when there are no steps.
    pub fn advance(&mut self) {
        if self.steps.is_empty() {
            self.current_step = 0;
        } else {
            self.current_step = (self.current_step + 1) % self.steps.len() as u32;
        }
    }

    /// The step that will fire on the next press, if any.
    pub fn next_step(&self) -> Option<&StreamDeckStep> {
        if self.steps.is_empty() {
            return None;
        }
        let idx = (self.current_step as usize).min(self.steps.len().saturating_sub(1));
        self.steps.get(idx)
    }
}

/// A single step: which macro to fire on press.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StreamDeckStep {
    pub macro_id: Uuid,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_uuid(n: u8) -> Uuid {
        Uuid::from_bytes([n; 16])
    }

    #[test]
    fn advance_wraps_after_last_step() {
        let mut b = StreamDeckButton {
            steps: vec![
                StreamDeckStep {
                    macro_id: make_uuid(1),
                },
                StreamDeckStep {
                    macro_id: make_uuid(2),
                },
                StreamDeckStep {
                    macro_id: make_uuid(3),
                },
            ],
            current_step: 2,
        };
        b.advance();
        assert_eq!(b.current_step, 0);
    }

    #[test]
    fn advance_with_no_steps_stays_at_zero() {
        let mut b = StreamDeckButton::default();
        b.advance();
        assert_eq!(b.current_step, 0);
        assert!(b.next_step().is_none());
    }

    #[test]
    fn next_step_returns_current_index() {
        let b = StreamDeckButton {
            steps: vec![
                StreamDeckStep {
                    macro_id: make_uuid(7),
                },
                StreamDeckStep {
                    macro_id: make_uuid(8),
                },
            ],
            current_step: 1,
        };
        assert_eq!(b.next_step().unwrap().macro_id, make_uuid(8));
    }

    #[test]
    fn next_step_clamps_when_cursor_out_of_range() {
        // current_step beyond steps.len() can happen if the operator
        // deletes a trailing step; we should still hand back the last
        // step rather than panic or return None.
        let b = StreamDeckButton {
            steps: vec![StreamDeckStep {
                macro_id: make_uuid(5),
            }],
            current_step: 9,
        };
        assert_eq!(b.next_step().unwrap().macro_id, make_uuid(5));
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = StreamDeckConfig {
            enabled: true,
            device_serial: Some("AL12K1A12345".into()),
            buttons: vec![
                StreamDeckButton::default(),
                StreamDeckButton {
                    steps: vec![
                        StreamDeckStep {
                            macro_id: make_uuid(1),
                        },
                        StreamDeckStep {
                            macro_id: make_uuid(2),
                        },
                    ],
                    current_step: 1,
                },
            ],
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: StreamDeckConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn empty_object_loads_as_default() {
        // Old show files (v14) have no `stream_deck` field — when added
        // to a v15 ShowFile via #[serde(default)] the default takes over.
        // Verify the type itself accepts an empty JSON object too, in
        // case a future deserialiser hands us one.
        let cfg: StreamDeckConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg, StreamDeckConfig::default());
        assert!(!cfg.enabled);
        assert!(cfg.device_serial.is_none());
        assert!(cfg.buttons.is_empty());
    }
}
