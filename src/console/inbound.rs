//! Shared inbound parameter pipeline.
//!
//! Every path that receives a parameter value FROM the console — the GP OSC
//! state-mirror loop, the Mode 2 iPad mirror loop, and the Mode 3 proxy
//! capture loop — funnels through [`apply_inbound_parameter`], so the full
//! side-effect chain (mirror update, dirty screening, operator-override
//! detection, gang propagation, pan link, macro-learn capture) behaves
//! identically regardless of which dialect carried the message.
//!
//! Historically only the GP loop ran the full chain; the iPad loops updated
//! the mirror and dirty tracker and stopped there, so a desk move seen only
//! via the iPad protocol never gang-propagated. Consolidating here fixes
//! that — and is what lets a future Pad-only console family (SD/Quantum)
//! reuse the whole engine stack unchanged.

use tracing::debug;

use crate::console::connection::DaemonState;
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};

/// Which link produced an inbound parameter. Currently informational (log
/// wording); the side-effect chain itself is source-agnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboundSource {
    /// The GP OSC connection.
    GpOsc,
    /// The iPad/Pad protocol — Mode 2 direct link or Mode 3 proxy capture.
    Pad,
}

// Post-recall / layer-change fader reports jitter by a few tenths of a dB; the
// auto-preselect "dirty" screening ignores that mechanical jitter. Bands widen
// as the level drops — the physical track compresses a large dB range into a
// sliver down low, mirroring the `FADER_GANG_FLOOR_DB` reasoning in `parameter`.
const FADER_DEADBAND_HI_DB: f32 = 0.2; //  value >= -10 dB
const FADER_DEADBAND_MID_DB: f32 = 0.3; // -40 <= value < -10 dB
const FADER_DEADBAND_LO_DB: f32 = 0.4; //  value <  -40 dB

/// Whether an inbound change is a *meaningful* move away from the previous
/// value — i.e. it should mark the cell dirty for the self-actualising scope.
///
/// Fader-controllable level params (see [`ParameterPath::is_fader_level`]) get a
/// dB deadband, banded by the new (settled) value, so post-recall / layer-change
/// fader jitter doesn't auto-preselect. Every other parameter keeps exact
/// equality, matching the original `prev != value` behaviour.
///
/// Note: the comparison is against the immediately-preceding stored value
/// (`ConsoleState::update` overwrites on every message). A very slow continuous
/// fader ride in sub-deadband steps could therefore ride a long way without ever
/// marking dirty — accepted, because this screens recall/layer-change jitter,
/// not live rides (a real ride moves faster than the band per message). True
/// hysteresis would need a separate "last-marked value" map, not a change to the
/// live mirror.
pub(crate) fn is_meaningful_change(
    path: &ParameterPath,
    prev: &ParameterValue,
    new: &ParameterValue,
) -> bool {
    use ParameterValue::Float;
    if path.is_fader_level()
        && let (Float(a), Float(b)) = (prev, new)
    {
        let band = if *b >= -10.0 {
            FADER_DEADBAND_HI_DB
        } else if *b >= -40.0 {
            FADER_DEADBAND_MID_DB
        } else {
            FADER_DEADBAND_LO_DB
        };
        return (a - b).abs() > band;
    }
    prev != new
}

/// Apply one inbound parameter to the daemon: update the state mirror and run
/// the full post-update side-effect chain.
///
/// The chain, in order (matching the original GP `process_message` body):
/// 1. Drop console-derived read-only values (`TotalGain`).
/// 2. Update the `ConsoleState` mirror (capturing the previous value).
/// 3. Mirror the address for the Macros tab's "track latest OSC" affordance.
/// 4. Dirty-mark on a meaningful change — gated on the console-load window so
///    a memory-load flood can't pollute the dirty set ("palette bleed").
/// 5. Operator live-override: a hands-on move cancels a running timed recall
///    for exactly that parameter.
/// 6. Gang propagation, then pan-link propagation (gangs first so a
///    gang-driven pan change also pushes to linked aux sends). Both are
///    skipped during a console-load window (re-propagating the desk's own
///    coherent snapshot flood fights the desk and can stall outbound OSC).
/// 7. Macro learn-mode capture.
///
/// Echo handling: the iPad/Pad link echoes our own engine writes back at us
/// (GP OSC does not). Steps 5 and 6 are skipped when the value matches the
/// shared [`SentLog`](crate::console::console_tx::SentLog) — otherwise a
/// snapshot recall touching one gang member would be mistaken for an operator
/// move and dragged across the whole gang.
pub async fn apply_inbound_parameter(
    daemon: &DaemonState,
    addr: &ParameterAddress,
    value: &ParameterValue,
    source: InboundSource,
) {
    // TotalGain (GP OSC `total/gain`) is a console-derived, read-only
    // monitor value (post-fader + CG sum). It can't be written back or
    // meaningfully recalled, so drop it on receipt before it pollutes
    // the state mirror, dirty tracker, override registry, gangs,
    // pan-link, or macro recording. Mirrors macro_manager::record_change.
    if matches!(addr.parameter, ParameterPath::TotalGain) {
        return;
    }
    debug!(%addr, %value, ?source, "Parameter update");
    let old_value = daemon
        .state
        .write()
        .await
        .update(addr.clone(), value.clone());

    // Mirror the most recent parameter address for the Macros
    // tab's "track latest OSC" affordance.
    *daemon.last_received.write().await = Some(addr.clone());

    // While a console snapshot is loading, treat the desk's echo flood
    // as the desk applying a coherent snapshot — not operator input.
    // Computed BEFORE the dirty mark below so the flood can't pollute
    // the dirty set either.
    let in_console_load =
        crate::console::snapshot_engine::console_load_active(&daemon.console_load_suppression);

    // Echo of our own engine write? (iPad/Pad link only — GP doesn't echo.)
    // Such a value is the desk confirming OUR send, not an operator move, so
    // it must not cancel automation or propagate through gangs / pan link.
    let is_own_echo = daemon.sent_log.is_echo(addr, value);

    // Mark this cell dirty IF the value actually changed. The dirty
    // tracker is suppression-aware, so echoes from snapshot recall
    // (which set begin_suppression before sending) are ignored. The
    // first sample after a connection comes through old_value=None,
    // which we treat as "this is the baseline" — not a change.
    // Also gated on the console-load window: a memory load floods
    // genuinely-different values for up to CONSOLE_LOAD_SUPPRESSION_MS
    // — well past the recall's settle — and those are the DESK's
    // changes, not the operator's. Without this gate the flood tail
    // lands in the dirty set and the palette absorb loop folds it
    // into whichever palette is live (the "palette bleed").
    if !in_console_load
        && let Some(prev) = &old_value
        && is_meaningful_change(&addr.parameter, prev, value)
    {
        daemon.dirty_tracker.write().await.mark(addr);
    }

    // Operator live-override: if this inbound change is the operator
    // (hands on the desk) grabbing a parameter that a timed recall is
    // currently pre-waiting or fading, the registry drops that one so the
    // running fade/pre-wait stops sending it and the hands-on value wins.
    // Gated on `!in_console_load`: the desk's own snapshot-load flood
    // overlaps the first seconds of a memory-backed recall and would
    // otherwise look like a storm of operator moves, spuriously
    // cancelling the very recall it belongs to. Gated on `!is_own_echo`:
    // the desk echoing our own automation write back (iPad link) is not
    // the operator grabbing the fader.
    if !in_console_load
        && !is_own_echo
        && let Some(reg) = &daemon.automation_override
        && matches!(
            reg.maybe_override(addr, value),
            crate::console::automation_registry::Override::Operator
        )
    {
        debug!(%addr, "Operator override — automation cancelled for this parameter");
    }

    // Gang propagation — before macro recording so the engineer's
    // original change is what gets recorded, not ganged echoes.
    if !in_console_load && !is_own_echo {
        let mut engine = daemon.gang_engine.write().await;
        let manager = daemon.gang_manager.read().await;
        engine
            .process_gang_update(addr, value, old_value.as_ref(), &manager)
            .await;
    }

    // Pan link propagation — runs after gangs so a gang-driven
    // pan change on an input also pushes to its linked aux sends.
    if !in_console_load && !is_own_echo {
        let engine = daemon.pan_link_engine.read().await;
        engine.process_pan_update(addr, value).await;
    }

    // Feed into macro learn mode if recording
    let mut mgr = daemon.macro_manager.write().await;
    if mgr.is_recording() {
        mgr.record_change(addr.clone(), value.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(v: f32) -> ParameterValue {
        ParameterValue::Float(v)
    }

    #[test]
    fn fader_hi_band_ignores_small_jitter_marks_real_change() {
        // value >= -10 dB → 0.2 dB deadband.
        assert!(!is_meaningful_change(
            &ParameterPath::Fader,
            &f(-5.0),
            &f(-4.85)
        )); // 0.15 ≤ 0.2
        assert!(is_meaningful_change(
            &ParameterPath::Fader,
            &f(-5.0),
            &f(-4.75)
        )); // 0.25 > 0.2
    }

    #[test]
    fn fader_mid_band() {
        // -40 <= value < -10 dB → 0.3 dB deadband.
        assert!(!is_meaningful_change(
            &ParameterPath::Fader,
            &f(-25.0),
            &f(-25.25)
        )); // 0.25 ≤ 0.3
        assert!(is_meaningful_change(
            &ParameterPath::Fader,
            &f(-25.0),
            &f(-25.35)
        )); // 0.35 > 0.3
    }

    #[test]
    fn fader_lo_band() {
        // value < -40 dB → 0.4 dB deadband.
        assert!(!is_meaningful_change(
            &ParameterPath::Fader,
            &f(-60.0),
            &f(-60.35)
        )); // 0.35 ≤ 0.4
        assert!(is_meaningful_change(
            &ParameterPath::Fader,
            &f(-60.0),
            &f(-60.45)
        )); // 0.45 > 0.4
    }

    #[test]
    fn band_boundaries_use_new_value() {
        // Exactly -10.0 → HI band (>= -10). 0.25 dB is a change at HI(0.2)…
        assert!(is_meaningful_change(
            &ParameterPath::Fader,
            &f(-10.25),
            &f(-10.0)
        ));
        // …but the same 0.25 dB landing at -40.0 → MID band (>= -40) is ignored.
        assert!(!is_meaningful_change(
            &ParameterPath::Fader,
            &f(-40.25),
            &f(-40.0)
        ));
    }

    #[test]
    fn send_levels_share_the_fader_deadband() {
        assert!(!is_meaningful_change(
            &ParameterPath::SendLevel(3),
            &f(-5.0),
            &f(-4.85)
        ));
        assert!(is_meaningful_change(
            &ParameterPath::SendLevel(3),
            &f(-5.0),
            &f(-4.5)
        ));
        assert!(!is_meaningful_change(
            &ParameterPath::CgLevel,
            &f(-5.0),
            &f(-4.85)
        ));
        assert!(!is_meaningful_change(
            &ParameterPath::MatrixSendLevel(1),
            &f(-5.0),
            &f(-4.85)
        ));
    }

    #[test]
    fn encoder_params_keep_exact_equality() {
        // A 0.5 dB EQ-gain move is a real change — no deadband for encoder params.
        assert!(is_meaningful_change(
            &ParameterPath::EqBandGain(1),
            &f(0.0),
            &f(0.5)
        ));
        assert!(is_meaningful_change(
            &ParameterPath::TotalGain,
            &f(0.0),
            &f(0.1)
        ));
        // Identical values are never a change, for any param.
        assert!(!is_meaningful_change(
            &ParameterPath::EqBandGain(1),
            &f(0.0),
            &f(0.0)
        ));
        assert!(!is_meaningful_change(
            &ParameterPath::Fader,
            &f(-5.0),
            &f(-5.0)
        ));
    }

    #[test]
    fn non_float_params_use_exact_equality() {
        assert!(is_meaningful_change(
            &ParameterPath::Mute,
            &ParameterValue::Bool(true),
            &ParameterValue::Bool(false),
        ));
        assert!(!is_meaningful_change(
            &ParameterPath::Mute,
            &ParameterValue::Bool(true),
            &ParameterValue::Bool(true),
        ));
    }

    #[test]
    fn is_fader_level_classification() {
        assert!(ParameterPath::Fader.is_fader_level());
        assert!(ParameterPath::SendLevel(0).is_fader_level());
        assert!(ParameterPath::MatrixSendLevel(0).is_fader_level());
        assert!(ParameterPath::CgLevel.is_fader_level());
        assert!(!ParameterPath::EqBandGain(1).is_fader_level());
        assert!(!ParameterPath::TotalGain.is_fader_level());
        assert!(!ParameterPath::Pan.is_fader_level());
    }

    // ── Echo screening through the full chain ────────────────────────────

    use std::collections::HashSet;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use crate::console::console_tx::SentLog;
    use crate::console::gang_engine::GangEngine;
    use crate::console::gang_manager::GangManager;
    use crate::console::macro_manager::MacroManager;
    use crate::console::pan_link_engine::PanLinkEngine;
    use crate::model::channel::ChannelId;
    use crate::model::config::ConsoleConfig;
    use crate::model::dirty_tracker::DirtyTracker;
    use crate::model::gang::GangGroup;
    use crate::model::parameter::ParameterSection;
    use crate::model::state::ConsoleState;

    /// Daemon with a two-member absolute-copy gang over Fader/Mute/Pan:
    /// Input 1 + Input 2. The OSC sender points at a loopback port nobody
    /// listens on — UDP sends still "succeed", which is all dispatch needs.
    async fn gang_test_daemon() -> (DaemonState, SentLog) {
        let state = Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default())));
        let client = crate::osc::client::OscClient::new(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:1".parse().unwrap(),
            None,
        )
        .await
        .unwrap();
        let (sender, _rx) = client.into_parts();

        let sent_log = SentLog::new();
        let mut gang_engine = GangEngine::new(state.clone(), sender.clone());
        gang_engine.set_sent_log(sent_log.clone());
        let gang_engine = Arc::new(RwLock::new(gang_engine));

        let mut manager = GangManager::new();
        let mut sections = HashSet::new();
        sections.insert(ParameterSection::FaderMutePan);
        let gang = GangGroup::new(
            "test".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            sections,
        );
        manager.add_group(gang);
        let gang_manager = Arc::new(RwLock::new(manager));

        let dirty_tracker = Arc::new(RwLock::new(DirtyTracker::new()));
        let pan_link_engine = Arc::new(RwLock::new(PanLinkEngine::new(
            state.clone(),
            sender,
            Arc::new(RwLock::new(
                crate::model::pan_link::PanLinkBindings::default(),
            )),
            dirty_tracker.clone(),
            gang_manager.clone(),
        )));

        let daemon = DaemonState {
            state,
            macro_manager: Arc::new(RwLock::new(MacroManager::new())),
            gang_engine,
            gang_manager,
            pan_link_engine,
            dirty_tracker,
            offline_mode: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            recall_progress: Arc::new(crate::model::recall_progress::RecallProgress::new()),
            last_received: Arc::new(RwLock::new(None)),
            console_snapshot_tx: None,
            console_load_suppression: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            automation_override: None,
            sent_log: sent_log.clone(),
        };
        (daemon, sent_log)
    }

    fn mute(n: u16) -> ParameterAddress {
        ParameterAddress {
            channel: ChannelId::Input(n),
            parameter: ParameterPath::Mute,
        }
    }

    #[tokio::test]
    async fn operator_move_gang_propagates_to_siblings() {
        let (daemon, _sent_log) = gang_test_daemon().await;
        // A desk/iPad-originated mute on gang member 1…
        apply_inbound_parameter(
            &daemon,
            &mute(1),
            &ParameterValue::Bool(true),
            InboundSource::Pad,
        )
        .await;
        // …is copied to member 2 (dispatch updates the mirror on success).
        let state = daemon.state.read().await;
        assert_eq!(state.get(&mute(2)), Some(&ParameterValue::Bool(true)));
    }

    #[tokio::test]
    async fn engine_write_echo_does_not_gang_propagate() {
        let (daemon, sent_log) = gang_test_daemon().await;
        // An engine (snapshot recall, macro, sidecar…) writes member 1's
        // mute; the ConsoleTx records it in the sent log…
        sent_log.note_sent(&mute(1), &ParameterValue::Bool(true));
        // …then the iPad link echoes it back at us.
        apply_inbound_parameter(
            &daemon,
            &mute(1),
            &ParameterValue::Bool(true),
            InboundSource::Pad,
        )
        .await;
        // The echo must not be mistaken for an operator move: member 2 is
        // untouched (the mirror itself still records member 1's value).
        let state = daemon.state.read().await;
        assert_eq!(state.get(&mute(1)), Some(&ParameterValue::Bool(true)));
        assert_eq!(state.get(&mute(2)), None);
    }
}
