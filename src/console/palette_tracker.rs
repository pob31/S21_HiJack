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
    console_load: crate::console::snapshot_engine::ConsoleLoadSuppression,
    cancel: CancellationToken,
) {
    let mut ticker = tokio::time::interval(ABSORB_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("Palette absorb loop shutting down");
                break;
            }
            _ = ticker.tick() => {
                // Re-absorb every tick while any operator change is pending —
                // do NOT gate on the dirty-tracker *generation*. A continuous
                // knob sweep of one parameter bumps the generation only on the
                // first sample (marking an already-dirty cell is a no-op for the
                // generation), so generation-gating would freeze the overlay at
                // the sweep's first value. Re-reading the live value each tick
                // lets the overlay track the latest position. `has_any()` keeps
                // idle ticks cheap.
                if dirty_tracker.read().await.has_any() {
                    absorb_once(&state, &cue_manager, &palette_manager, &dirty_tracker, &console_load).await;
                }
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
    console_load: &crate::console::snapshot_engine::ConsoleLoadSuppression,
) {
    // Never absorb while a recall is writing (dirty suppression active) or
    // while a console memory load is flooding echoes — mid-recall mirror
    // values belong to the INCOMING cue and must not be folded into the
    // still-pointed-at snapshot's palettes (the "palette bleed"). Operator
    // tweaks made between cues still mark dirty and absorb on a later tick:
    // the dirty set survives this early-return; only recall completion
    // clears it.
    if dirty_tracker.read().await.is_suppressed() {
        return;
    }
    if crate::console::snapshot_engine::console_load_active(console_load) {
        return;
    }

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

    // Collect (palette_id, path, value) under the state READ lock, then DROP it
    // BEFORE taking the palette WRITE lock. Crucial: a recall holds
    // palette.read() while it takes state.write() per param (see
    // cue_transport.rs / snapshot_engine.rs send_now). If this loop held
    // state.read() across palette.write() (the previous code), the two opposing
    // orders form an ABBA deadlock that wedges the recall task. Releasing the
    // state guard before crossing to palette keeps a single safe order.
    let updates = {
        let st = state.read().await;
        let mut out = Vec::new();
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
                if let Some(value) = st.get(&addr).cloned() {
                    out.push((*pid, path.clone(), value));
                }
            }
        }
        out
    }; // state read guard dropped here

    if updates.is_empty() {
        return;
    }
    let mut pmgr = palette_manager.write().await;
    for (pid, path, value) in updates {
        if let Some(palette) = pmgr.get_palette_mut(&pid) {
            palette.set_working(path, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::console::snapshot_engine::arm_console_load;
    use crate::model::channel::ChannelId;
    use crate::model::config::ConsoleConfig;
    use crate::model::palette::ChannelPalette;
    use crate::model::parameter::{PaletteKind, ParameterPath, ParameterValue};
    use crate::model::snapshot::{CueList, ScopeTemplate, Snapshot, SnapshotData, SnapshotKind};

    /// Harness: one EQ palette linked by one snapshot marked last-recalled,
    /// a live mirror value of +6 dB on EQ band-1 gain, and that cell dirty.
    /// An unguarded absorb pass folds +6 into the palette's working overlay.
    async fn setup() -> (
        Arc<RwLock<ConsoleState>>,
        Arc<RwLock<crate::console::cue_manager::CueManager>>,
        Arc<RwLock<PaletteManager>>,
        Arc<RwLock<DirtyTracker>>,
        crate::console::snapshot_engine::ConsoleLoadSuppression,
        uuid::Uuid,
        ParameterAddress,
    ) {
        let channel = ChannelId::Input(3);
        let mut vals = HashMap::new();
        vals.insert(ParameterPath::EqBandGain(1), ParameterValue::Float(3.0));
        let palette =
            ChannelPalette::new("Vox EQ".into(), channel.clone(), &[PaletteKind::Eq], vals);
        let pid = palette.id;

        let scope = ScopeTemplate::new("s".into(), vec![]);
        let mut snap = Snapshot::new(
            "Snap".into(),
            scope,
            SnapshotData::new(),
            SnapshotKind::ApplyOnRecall,
        );
        snap.palette_refs
            .insert((channel.clone(), PaletteKind::Eq), pid);
        let snap_id = snap.id;

        let mut cue_mgr = crate::console::cue_manager::CueManager::new(CueList::default());
        cue_mgr.add_snapshot(snap);
        cue_mgr.set_last_recalled(snap_id);

        let mut pmgr = PaletteManager::new();
        pmgr.palettes.insert(pid, palette);

        let addr = ParameterAddress {
            channel,
            parameter: ParameterPath::EqBandGain(1),
        };
        let state = Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default())));
        state
            .write()
            .await
            .update(addr.clone(), ParameterValue::Float(6.0));

        let dirty = Arc::new(RwLock::new(DirtyTracker::new()));
        dirty.write().await.mark(&addr);

        let console_load: crate::console::snapshot_engine::ConsoleLoadSuppression =
            Arc::new(std::sync::atomic::AtomicU64::new(0));

        (
            state,
            Arc::new(RwLock::new(cue_mgr)),
            Arc::new(RwLock::new(pmgr)),
            dirty,
            console_load,
            pid,
            addr,
        )
    }

    async fn working_count(pmgr: &Arc<RwLock<PaletteManager>>, pid: &uuid::Uuid) -> usize {
        pmgr.read().await.palettes.get(pid).unwrap().working_count()
    }

    #[tokio::test]
    async fn absorbs_dirty_eq_param_into_linked_palette() {
        let (state, cues, pmgr, dirty, load, pid, _addr) = setup().await;
        absorb_once(&state, &cues, &pmgr, &dirty, &load).await;
        assert_eq!(
            working_count(&pmgr, &pid).await,
            1,
            "an operator EQ tweak absorbs into the live palette's overlay"
        );
    }

    #[tokio::test]
    async fn does_not_absorb_while_recall_suppression_active() {
        let (state, cues, pmgr, dirty, load, pid, _addr) = setup().await;
        dirty.write().await.begin_suppression();
        absorb_once(&state, &cues, &pmgr, &dirty, &load).await;
        assert_eq!(
            working_count(&pmgr, &pid).await,
            0,
            "mid-recall mirror values must not be folded into palettes (bleed)"
        );
        // Once the recall bracket closes, a later tick absorbs normally.
        dirty.write().await.end_suppression();
        absorb_once(&state, &cues, &pmgr, &dirty, &load).await;
        assert_eq!(working_count(&pmgr, &pid).await, 1);
    }

    #[tokio::test]
    async fn does_not_absorb_during_console_load_window() {
        let (state, cues, pmgr, dirty, load, pid, _addr) = setup().await;
        arm_console_load(&load);
        absorb_once(&state, &cues, &pmgr, &dirty, &load).await;
        assert_eq!(
            working_count(&pmgr, &pid).await,
            0,
            "a console memory-load flood must not be absorbed as operator edits"
        );
    }
}
