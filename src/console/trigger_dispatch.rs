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
    let snapshot = cue
        .snapshot_id
        .and_then(|id| mgr.get_snapshot(&id).cloned());
    if cue.snapshot_id.is_some() && snapshot.is_none() {
        warn!(snapshot_id = ?cue.snapshot_id, label, "Trigger: snapshot not found for cue");
    }
    drop(mgr);
    let pmgr = palette_manager.read().await;
    let result = snapshot_engine
        .recall_cue(&cue, snapshot.as_ref(), &pmgr.palettes, false)
        .await;
    info!(
        label,
        sent = result.parameters_sent,
        "Trigger {label} recall complete"
    );
}

#[cfg(test)]
mod tests {
    //! End-to-end integration tests for the trigger dispatch chain (audit M7).
    //!
    //! Each test wires up real `CueManager` / `MacroManager` /
    //! `SnapshotEngine` / `MacroEngine` instances and a sink UDP socket that
    //! plays the role of the console. `SnapshotEngine` sends parameter OSC
    //! messages to the sink; the test verifies that at least one packet
    //! arrives within a short timeout — proving the trigger event reached the
    //! engine and the engine actually ran.

    use super::*;
    use std::collections::HashSet;
    use std::net::SocketAddr;
    use std::time::Duration;

    use crate::console::cue_manager::CueManager;
    use crate::console::macro_engine::MacroEngine;
    use crate::console::macro_manager::MacroManager;
    use crate::console::palette_manager::PaletteManager;
    use crate::console::snapshot_engine::SnapshotEngine;
    use crate::model::channel::ChannelId;
    use crate::model::config::ConsoleConfig;
    use crate::model::parameter::{
        ParameterAddress, ParameterPath, ParameterSection, ParameterValue,
    };
    use crate::model::snapshot::{
        ChannelScope, Cue, CueList, ScopeTemplate, Snapshot, SnapshotData, SnapshotKind,
    };
    use crate::model::state::ConsoleState;
    use crate::osc::client::OscClient;
    use tokio::net::UdpSocket;
    use tokio::sync::RwLock;

    /// Fixture: bind a sink socket on a random port and build a
    /// `SnapshotEngine` whose sender targets that sink. Returns the engine,
    /// the state mirror, and the sink (so the test can recv_from it).
    async fn setup_engine_with_sink() -> (Arc<SnapshotEngine>, Arc<RwLock<ConsoleState>>, UdpSocket)
    {
        // Sink socket — plays the role of the "console". Bind to ephemeral
        // port so tests don't conflict.
        let sink = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sink_addr = sink.local_addr().unwrap();

        // Engine sends to the sink.
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let client = OscClient::new(local, sink_addr, None).await.unwrap();
        let (sender, _rx) = client.into_parts();
        let state = Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default())));
        let pace = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let engine = SnapshotEngine::new(state.clone(), sender, pace);
        (Arc::new(engine), state, sink)
    }

    /// Drain any pending packets at the sink and report how many arrived.
    /// 100 ms timeout is plenty for loopback sends with no real network.
    async fn count_packets_on_sink(sink: &UdpSocket) -> usize {
        let mut buf = vec![0u8; 65536];
        let mut count = 0;
        while let Ok(Ok(_)) =
            tokio::time::timeout(Duration::from_millis(100), sink.recv_from(&mut buf)).await
        {
            count += 1;
        }
        count
    }

    fn empty_palette_manager() -> Arc<RwLock<PaletteManager>> {
        Arc::new(RwLock::new(PaletteManager::new()))
    }

    fn empty_macro_manager() -> Arc<RwLock<MacroManager>> {
        Arc::new(RwLock::new(MacroManager::new()))
    }

    async fn empty_macro_engine() -> Arc<MacroEngine> {
        // Macro engine isn't exercised in the snapshot tests below — passing
        // a real one is fine because handle_trigger_event won't reach it.
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let remote: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let client = OscClient::new(local, remote, None).await.unwrap();
        let (sender, _rx) = client.into_parts();
        let state = Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default())));
        let macro_mgr = Arc::new(RwLock::new(
            crate::console::macro_manager::MacroManager::new(),
        ));
        let (ui_tx, _ui_rx) = std::sync::mpsc::channel::<crate::ui::UiEvent>();
        let pace = Arc::new(std::sync::atomic::AtomicU64::new(0));
        Arc::new(MacroEngine::new(state, sender, macro_mgr, ui_tx, pace))
    }

    #[tokio::test]
    async fn snapshot_recall_via_dispatch_fires_engine() {
        let (engine, state, sink) = setup_engine_with_sink().await;

        // Live state: Input(1) Fader at 0 dB.
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        state
            .write()
            .await
            .update(addr.clone(), ParameterValue::Float(0.0));

        // Snapshot moves Fader to -10 dB on Input(1).
        let mut data = SnapshotData::new();
        data.values.insert(addr, ParameterValue::Float(-10.0));
        let scope = ScopeTemplate::new(
            "S".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::FaderMutePan]),
            )],
        );
        let snapshot = Snapshot::new("Verse 1".into(), scope, data, SnapshotKind::ApplyOnSave);

        let mut mgr_inner = CueManager::new(CueList::default());
        mgr_inner.add_snapshot(snapshot);
        let cue_mgr = Arc::new(RwLock::new(mgr_inner));

        // Dispatch a SnapshotRecall trigger by name.
        handle_trigger_event(
            TriggerEvent::SnapshotRecall {
                identifier: "Verse 1".into(),
                ignore_scope: false,
            },
            &cue_mgr,
            &empty_palette_manager(),
            &empty_macro_manager(),
            &empty_macro_engine().await,
            &engine,
            None, // no reply socket needed
        )
        .await;

        // Engine should have sent at least the one Fader packet to the sink.
        let received = count_packets_on_sink(&sink).await;
        assert!(
            received >= 1,
            "expected at least 1 OSC packet at sink, got {received}"
        );
    }

    #[tokio::test]
    async fn fire_cue_via_dispatch_fires_engine() {
        let (engine, state, sink) = setup_engine_with_sink().await;

        let addr = ParameterAddress {
            channel: ChannelId::Input(2),
            parameter: ParameterPath::Fader,
        };
        state
            .write()
            .await
            .update(addr.clone(), ParameterValue::Float(0.0));

        let mut data = SnapshotData::new();
        data.values.insert(addr, ParameterValue::Float(-15.0));
        let scope = ScopeTemplate::new(
            "S".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(2),
                HashSet::from([ParameterSection::FaderMutePan]),
            )],
        );
        let snapshot = Snapshot::new("Chorus".into(), scope, data, SnapshotKind::ApplyOnSave);
        let snapshot_id = snapshot.id;

        let mut mgr_inner = CueManager::new(CueList::default());
        mgr_inner.add_snapshot(snapshot);
        // Add a cue that points at the snapshot, so FireCue(1.0) works.
        mgr_inner
            .cue_list
            .cues
            .push(Cue::new(1.0, "Cue 1".into()).with_snapshot_id(snapshot_id));
        let cue_mgr = Arc::new(RwLock::new(mgr_inner));

        handle_trigger_event(
            TriggerEvent::FireCue(1.0),
            &cue_mgr,
            &empty_palette_manager(),
            &empty_macro_manager(),
            &empty_macro_engine().await,
            &engine,
            None,
        )
        .await;

        let received = count_packets_on_sink(&sink).await;
        assert!(
            received >= 1,
            "expected at least 1 OSC packet at sink for FireCue, got {received}"
        );
    }

    #[tokio::test]
    async fn snapshot_recall_unknown_identifier_is_silent() {
        // No snapshot named "Bogus" — dispatch should warn and return.
        // The test passes if it doesn't panic; sink should receive nothing.
        let (engine, _state, sink) = setup_engine_with_sink().await;

        let cue_mgr = Arc::new(RwLock::new(CueManager::new(CueList::default())));

        handle_trigger_event(
            TriggerEvent::SnapshotRecall {
                identifier: "Bogus".into(),
                ignore_scope: false,
            },
            &cue_mgr,
            &empty_palette_manager(),
            &empty_macro_manager(),
            &empty_macro_engine().await,
            &engine,
            None,
        )
        .await;

        let received = count_packets_on_sink(&sink).await;
        assert_eq!(received, 0, "no packets expected for unknown snapshot");
    }
}
