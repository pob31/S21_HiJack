//! Snapshot sync direction — which way snapshots flow between the app and the
//! console, switchable per show.
//!
//! Replaces the old binary `console_snapshot_follow` toggle. The link itself is
//! still per-cue (`Cue::console_snapshot`); this just chooses the direction the
//! linkage drives:
//!
//! - [`Off`](SnapshotSyncDirection::Off) — no console/app snapshot linkage.
//! - [`AppToConsole`](SnapshotSyncDirection::AppToConsole) — firing an app cue
//!   with a console row sends `/digico/snapshots/fire <row>` to the desk.
//! - [`ConsoleToApp`](SnapshotSyncDirection::ConsoleToApp) — when the desk
//!   reports a snapshot load, the app recalls the first matching cue's overlay.
//!
//! Unified on GP OSC so it works in every operating mode. The shared handle is
//! lock-free (an `Arc<AtomicU8>`), mirroring the existing `Arc<AtomicBool>` /
//! `Arc<AtomicU64>` flag pattern so neither the UI nor the engines ever block.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

/// Which way snapshot recalls propagate between the app and the desk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SnapshotSyncDirection {
    /// No linkage — neither side drives the other.
    #[default]
    Off,
    /// App drives the console: firing a cue with a console row fires the desk
    /// memory via GP OSC.
    AppToConsole,
    /// Console drives the app: a desk snapshot load recalls the matching cue.
    ConsoleToApp,
}

impl SnapshotSyncDirection {
    pub fn to_u8(self) -> u8 {
        match self {
            SnapshotSyncDirection::Off => 0,
            SnapshotSyncDirection::AppToConsole => 1,
            SnapshotSyncDirection::ConsoleToApp => 2,
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => SnapshotSyncDirection::AppToConsole,
            2 => SnapshotSyncDirection::ConsoleToApp,
            _ => SnapshotSyncDirection::Off,
        }
    }

    /// Short label for the UI selector.
    pub fn label(self) -> &'static str {
        match self {
            SnapshotSyncDirection::Off => "Off",
            SnapshotSyncDirection::AppToConsole => "App → Console",
            SnapshotSyncDirection::ConsoleToApp => "Console → App",
        }
    }

    /// Whether the app should push console-memory fires on recall.
    pub fn pushes_to_console(self) -> bool {
        matches!(self, SnapshotSyncDirection::AppToConsole)
    }

    /// Whether the app should follow the desk's snapshot loads.
    pub fn follows_console(self) -> bool {
        matches!(self, SnapshotSyncDirection::ConsoleToApp)
    }
}

/// Lock-free shared handle for the current sync direction. Construct one and
/// clone it into the UI (writer) and the engines / dispatchers (readers).
#[derive(Clone, Debug)]
pub struct SharedSyncDirection(Arc<AtomicU8>);

impl SharedSyncDirection {
    pub fn new(dir: SnapshotSyncDirection) -> Self {
        Self(Arc::new(AtomicU8::new(dir.to_u8())))
    }

    pub fn get(&self) -> SnapshotSyncDirection {
        SnapshotSyncDirection::from_u8(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, dir: SnapshotSyncDirection) {
        self.0.store(dir.to_u8(), Ordering::Relaxed);
    }
}

impl Default for SharedSyncDirection {
    fn default() -> Self {
        Self::new(SnapshotSyncDirection::Off)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u8_round_trips() {
        for d in [
            SnapshotSyncDirection::Off,
            SnapshotSyncDirection::AppToConsole,
            SnapshotSyncDirection::ConsoleToApp,
        ] {
            assert_eq!(SnapshotSyncDirection::from_u8(d.to_u8()), d);
        }
    }

    #[test]
    fn unknown_u8_is_off() {
        assert_eq!(
            SnapshotSyncDirection::from_u8(99),
            SnapshotSyncDirection::Off
        );
    }

    #[test]
    fn shared_handle_get_set() {
        let h = SharedSyncDirection::default();
        assert_eq!(h.get(), SnapshotSyncDirection::Off);
        h.set(SnapshotSyncDirection::ConsoleToApp);
        assert_eq!(h.get(), SnapshotSyncDirection::ConsoleToApp);
        assert!(h.get().follows_console());
        assert!(!h.get().pushes_to_console());
    }
}
