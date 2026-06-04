//! Live palette tracking — folds in-session operator adjustments into the
//! active palette's working overlay so changes ripple to every linked snapshot
//! without re-capture.
//!
//! Runs as a lightweight background task spawned at connect time. It watches the
//! suppression-aware [`DirtyTracker`] — which already records only genuine
//! operator changes, not recall echoes — and, for each changed EQ/Dyn
//! parameter, writes the live value into whichever palette the *last-recalled*
//! snapshot links for that `(channel, kind)`. The overlay is a diff (see
//! [`ChannelPalette::set_working`](crate::model::palette::ChannelPalette::set_working)),
//! so values that match the stored baseline (e.g. right after `store_changes`)
//! don't linger. Nothing here is persisted — `store_changes` commits it.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::console::cue_manager::CueManager;
use crate::console::palette_manager::PaletteManager;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::parameter::ParameterAddress;
use crate::model::state::ConsoleState;

/// Poll interval. Fast enough to feel live; idle ticks are skipped via the
/// dirty-tracker generation, so this is cheap when nothing is changing.
const ABSORB_INTERVAL: Duration = Duration::from_millis(150);

/// Run the live palette-absorb loop until `cancel` fires.
pub async fn run_absorb_loop(
    state: Arc<RwLock<ConsoleState>>,
    cue_manager: Arc<RwLock<CueManager>>,
    palette_manager: Arc<RwLock<PaletteManager>>,
    dirty_tracker: Arc<RwLock<DirtyTracker>>,
    cancel: CancellationToken,
) {
    let mut ticker = tokio::time::interval(ABSORB_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_gen = u64::MAX; // force a first pass

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("Palette absorb loop shutting down");
                break;
            }
            _ = ticker.tick() => {
                let current_gen = dirty_tracker.read().await.generation();
                if current_gen == last_gen {
                    continue;
                }
                last_gen = current_gen;
                absorb_once(&state, &cue_manager, &palette_manager, &dirty_tracker).await;
            }
        }
    }
}

/// One absorb pass: fold the current operator-changed EQ/Dyn params into the
/// active snapshot's linked palettes' working overlays.
async fn absorb_once(
    state: &Arc<RwLock<ConsoleState>>,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    dirty_tracker: &Arc<RwLock<DirtyTracker>>,
) {
    // Which snapshot is live, and what does it link? Clone the small refs map so
    // we don't hold the cue lock across the state/palette locks.
    let palette_refs = {
        let cm = cue_manager.read().await;
        let Some(snap_id) = cm.last_recalled() else {
            return;
        };
        match cm.get_snapshot(&snap_id) {
            Some(snap) if !snap.palette_refs.is_empty() => snap.palette_refs.clone(),
            _ => return,
        }
    };

    // Snapshot the operator-changed set (suppression-aware; cleared on recall).
    let dirty = dirty_tracker.read().await.dirty_set().clone();
    if dirty.is_empty() {
        return;
    }

    // Lock order is state(read) → palette(write), matching capture/recapture
    // (which release the state guard before taking the palette lock) and recall
    // (palette-read → state-read, both shared), so no cycle can form.
    let st = state.read().await;
    let mut pmgr = palette_manager.write().await;
    for (channel, paths) in &dirty {
        for path in paths {
            // Only EQ/Dyn parameters belong to a palette kind.
            let Some(kind) = path.section().palette_kind() else {
                continue;
            };
            // Only when the active snapshot links a palette for this slot.
            let Some(pid) = palette_refs.get(&(channel.clone(), kind)) else {
                continue;
            };
            let addr = ParameterAddress {
                channel: channel.clone(),
                parameter: path.clone(),
            };
            if let Some(value) = st.get(&addr).cloned()
                && let Some(palette) = pmgr.get_palette_mut(pid)
            {
                palette.set_working(path.clone(), value);
            }
        }
    }
}
