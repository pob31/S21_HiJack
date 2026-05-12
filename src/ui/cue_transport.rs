//! Cue transport — fire GO / PREV from the top-bar transport strip.
//!
//! Replaces the dedicated Live tab's playback controls. Both helpers
//! advance the [`CueManager`] under its write lock, recall the bound
//! snapshot via [`SnapshotEngine::recall_cue`], and emit a
//! [`UiEvent::CueRecalled`] for any UI that wants to react.

use std::sync::Arc;

use tokio::sync::RwLock;

use super::UiEvent;
use crate::console::cue_manager::CueManager;
use crate::console::palette_manager::PaletteManager;
use crate::console::snapshot_engine::SnapshotEngine;

/// Advance the cue list to the next cue and recall its snapshot. No-op
/// when the snapshot engine isn't ready yet (no console connection) or
/// when the cue list is empty.
pub fn fire_go(
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    snapshot_engine: &Option<Arc<SnapshotEngine>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let Some(engine) = snapshot_engine.clone() else {
        return;
    };
    let cue_mgr = cue_manager.clone();
    let pmgr_arc = palette_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        let mut mgr = cue_mgr.write().await;
        if let Some(cue) = mgr.go_next() {
            let cue = cue.clone();
            let snapshot = cue
                .snapshot_id
                .and_then(|id| mgr.get_snapshot(&id).cloned());
            drop(mgr);
            let pmgr = pmgr_arc.read().await;
            let result = engine
                .recall_cue(&cue, snapshot.as_ref(), &pmgr.palettes, false)
                .await;
            let _ = tx.send(UiEvent::CueRecalled {
                cue_number: cue.cue_number,
                params_sent: result.parameters_sent,
            });
        }
    });
}

/// Step the cue list to the previous cue and recall its snapshot. Same
/// no-op semantics as [`fire_go`].
pub fn fire_prev(
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    snapshot_engine: &Option<Arc<SnapshotEngine>>,
    runtime: &tokio::runtime::Handle,
    ui_tx: &std::sync::mpsc::Sender<UiEvent>,
) {
    let Some(engine) = snapshot_engine.clone() else {
        return;
    };
    let cue_mgr = cue_manager.clone();
    let pmgr_arc = palette_manager.clone();
    let tx = ui_tx.clone();

    runtime.spawn(async move {
        let mut mgr = cue_mgr.write().await;
        if let Some(cue) = mgr.go_previous() {
            let cue = cue.clone();
            let snapshot = cue
                .snapshot_id
                .and_then(|id| mgr.get_snapshot(&id).cloned());
            drop(mgr);
            let pmgr = pmgr_arc.read().await;
            let result = engine
                .recall_cue(&cue, snapshot.as_ref(), &pmgr.palettes, false)
                .await;
            let _ = tx.send(UiEvent::CueRecalled {
                cue_number: cue.cue_number,
                params_sent: result.parameters_sent,
            });
        }
    });
}
