//! Shared `TriggerEvent` dispatcher (audit L1).
//!
//! Both the headless daemon (`main.rs`) and the UI's setup-tab connect path
//! drive an identical `match TriggerEvent { ... }` block — go-next, fire-cue,
//! macro-fire, snapshot-recall, etc. Adding a new variant means editing both,
//! and the two copies have already drifted slightly (silent-fail vs. warn on
//! missing snapshot, "Cue …" vs. "Trigger …" log prefix). This module is the
//! single source of truth.
//!
//! Behavior contract:
//! - Missing-snapshot lookup logs at `warn!`. Silent-fail in the UI copy was
//!   not load-bearing.
//! - `/cue/current` replies via `reply_socket` when `Some`; otherwise just
//!   logs the query (matches the UI copy's pre-consolidation behavior).
//! - Log messages use the `"Trigger {KIND} …"` prefix uniformly.

use std::sync::Arc;

use rosc::OscType;
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::console::cue_manager::CueManager;
use crate::console::macro_engine::MacroEngine;
use crate::console::macro_manager::MacroManager;
use crate::console::palette_manager::PaletteManager;
use crate::console::snapshot_engine::SnapshotEngine;
use crate::osc::trigger_listener::{self, TriggerEvent};

/// Run a single `TriggerEvent` against the shared manager handles.
///
/// `reply_socket` is optional: when `Some`, `/cue/current` writes a reply to
/// the requester; when `None`, the query is just logged (UI default — the
/// setup tab doesn't bind a reply socket).
pub async fn handle_trigger_event(
    event: TriggerEvent,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    macro_manager: &Arc<RwLock<MacroManager>>,
    macro_engine: &Arc<MacroEngine>,
    snapshot_engine: &Arc<SnapshotEngine>,
    reply_socket: Option<&UdpSocket>,
) {
    match event {
        TriggerEvent::GoNext => {
            recall_cue_with_label(cue_manager, palette_manager, snapshot_engine, "GO", |m| {
                m.go_next().cloned()
            })
            .await;
        }
        TriggerEvent::GoPrevious => {
            recall_cue_with_label(cue_manager, palette_manager, snapshot_engine, "PREV", |m| {
                m.go_previous().cloned()
            })
            .await;
        }
        TriggerEvent::FireCue(number) => {
            recall_cue_with_label(cue_manager, palette_manager, snapshot_engine, "FIRE", |m| {
                m.fire_cue_number(number).cloned()
            })
            .await;
        }
        TriggerEvent::QueryCurrent { reply_addr } => {
            let mgr = cue_manager.read().await;
            let current = mgr.current_cue_number().unwrap_or(-1.0);
            drop(mgr);
            if let Some(sock) = reply_socket {
                info!(current, %reply_addr, "Trigger /cue/current → reply");
                let _ = trigger_listener::send_reply(
                    sock,
                    reply_addr,
                    "/cue/current",
                    vec![OscType::Float(current)],
                )
                .await;
            } else {
                info!(current, %reply_addr, "Trigger /cue/current query (no reply socket)");
            }
        }
        TriggerEvent::MacroFire(name) => {
            let mgr = macro_manager.read().await;
            let Some(macro_def) = mgr.find_by_name_or_id(&name).cloned() else {
                warn!(name, "Trigger MacroFire: macro not found");
                return;
            };
            drop(mgr);
            let result = macro_engine.execute(&macro_def).await;
            info!(
                name = %result.macro_name,
                executed = result.steps_executed,
                skipped = result.steps_skipped,
                "Trigger MacroFire complete"
            );
        }
        TriggerEvent::SnapshotRecall {
            identifier,
            ignore_scope,
        } => {
            let mgr = cue_manager.read().await;
            let Some(snapshot) = mgr.resolve_snapshot(&identifier).cloned() else {
                warn!(identifier, "Trigger SnapshotRecall: snapshot not found");
                return;
            };
            drop(mgr);
            let pmgr = palette_manager.read().await;
            let scope = snapshot.scope.clone();
            let result = snapshot_engine
                .recall(&snapshot, &scope, &pmgr.palettes, ignore_scope)
                .await;
            info!(
                identifier,
                ignore_scope,
                sent = result.parameters_sent,
                "Trigger SnapshotRecall complete"
            );
        }
    }
}

/// Common cue-recall path used by `GoNext` / `GoPrevious` / `FireCue`. The
/// `pick` closure runs under the cue-manager **write** lock and returns the
/// cue (cloned) the operator advanced to, or `None` if no cue moved (e.g.
/// `go_next` past the end of the list).
async fn recall_cue_with_label<F>(
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
    snapshot_engine: &Arc<SnapshotEngine>,
    label: &'static str,
    pick: F,
) where
    F: FnOnce(&mut CueManager) -> Option<crate::model::snapshot::Cue>,
{
    let mut mgr = cue_manager.write().await;
    let Some(cue) = pick(&mut mgr) else {
        return;
    };
    let Some(snapshot) = mgr.get_snapshot(&cue.snapshot_id).cloned() else {
        warn!(snapshot_id = %cue.snapshot_id, label, "Trigger: snapshot not found for cue");
        return;
    };
    drop(mgr);
    let pmgr = palette_manager.read().await;
    let result = snapshot_engine
        .recall_cue(&cue, &snapshot, &pmgr.palettes, false)
        .await;
    info!(
        label,
        sent = result.parameters_sent,
        "Trigger {label} recall complete"
    );
}
