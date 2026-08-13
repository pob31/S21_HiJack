use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

use tokio::sync::RwLock;
use tokio::time;
use tracing::{debug, info, warn};

use super::macro_manager::MacroManager;
use crate::console::console_tx::ConsoleTx;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::macro_def::{MacroDef, MacroStepKind, MacroStepMode};
use crate::model::parameter::{ParameterAddress, ParameterValue};
use crate::model::recall_progress::{RecallKind, RecallProgress};
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::ipad_client::IpadSender;
use crate::ui::UiEvent;

/// Maximum recursion depth for `FireMacro` steps. A macro that calls
/// itself (directly or transitively) is rejected once the chain hits
/// this many nested invocations. Plenty of headroom for legitimate
/// composition (a top-level cue macro firing two nested layers); a
/// hard stop against infinite loops.
const MAX_MACRO_DEPTH: u32 = 5;

/// Result of executing a macro.
#[derive(Debug, Clone)]
pub struct MacroExecutionResult {
    pub macro_name: String,
    pub steps_executed: usize,
    pub steps_skipped: usize,
}

/// Executes macros by resolving step modes against live state
/// and sending the resulting values to the console.
pub struct MacroEngine {
    state: Arc<RwLock<ConsoleState>>,
    /// Console write path: GP OSC with iPad fallback for parameters that
    /// have no GP OSC encoding (analog gain, phantom, GEQ bands, …), so
    /// recorded iPad-only steps actually play back. The iPad half is `None`
    /// until an iPad-protocol session (Mode 2/3) wires it in.
    tx: ConsoleTx,
    dirty_tracker: Option<Arc<RwLock<DirtyTracker>>>,
    /// Used to look up macros by ID for the `FireMacro` step kind.
    macro_manager: Arc<RwLock<MacroManager>>,
    /// Channel to the UI thread for app-internal step kinds (Go,
    /// Prev, Connect, Disconnect, RecallSnapshot, RecallPalette).
    /// The engine doesn't call into UI / cue / palette code directly;
    /// it just sends a `UiEvent` and lets `HiJackApp::drain_events`
    /// dispatch to the appropriate existing handler.
    ui_tx: mpsc::Sender<UiEvent>,
    /// Inter-message pacing (μs) shared with `SnapshotEngine` and the
    /// Advanced Settings UI via `HiJackApp::send_pace_us`. Applied
    /// after each successful Parameter send so a long-step macro
    /// doesn't flood the console any more than a snapshot recall does.
    /// 0 = no pacing.
    pace_us: Arc<AtomicU64>,
    /// Optional shared progress handle. When set, macro execution drives the
    /// thin progress line under the tab bar. `None` for engines built without
    /// a UI (tests, trigger dispatch).
    recall_progress: Option<Arc<RecallProgress>>,
}

impl MacroEngine {
    pub fn new(
        state: Arc<RwLock<ConsoleState>>,
        sender: OscSender,
        macro_manager: Arc<RwLock<MacroManager>>,
        ui_tx: mpsc::Sender<UiEvent>,
        pace_us: Arc<AtomicU64>,
    ) -> Self {
        Self {
            state,
            tx: ConsoleTx::new(sender),
            dirty_tracker: None,
            macro_manager,
            ui_tx,
            pace_us,
            recall_progress: None,
        }
    }

    /// Attach a dirty tracker so macros can optionally suppress it.
    pub fn set_dirty_tracker(&mut self, tracker: Arc<RwLock<DirtyTracker>>) {
        self.dirty_tracker = Some(tracker);
    }

    /// Attach the shared recall-progress handle so macro execution drives the
    /// UI's progress line.
    pub fn set_recall_progress(&mut self, progress: Arc<RecallProgress>) {
        self.recall_progress = Some(progress);
    }

    /// Wire in the iPad-protocol sender so macro steps for iPad-only
    /// parameters fall back to it instead of being skipped.
    pub fn set_ipad_sender(&mut self, sender: Option<IpadSender>) {
        self.tx.set_pad_sender(sender);
    }

    /// Attach the shared sent-value log (echo screening on the iPad link).
    pub fn set_sent_log(&mut self, log: crate::console::console_tx::SentLog) {
        self.tx.set_sent_log(log);
    }

    /// Point the write path at the connected console's profile (Pad wire quirks).
    pub fn set_profile(&mut self, profile: Arc<crate::model::family::ConsoleProfile>) {
        self.tx.set_profile(profile);
    }

    /// Send one resolved parameter to the console: GP OSC first, then the
    /// iPad protocol as a fallback for iPad-only parameters (mirrors
    /// `GangEngine::send_to_console`). Returns true on a successful send.
    async fn send_parameter(&self, addr: &ParameterAddress, value: &ParameterValue) -> bool {
        let sent = self.tx.send_parameter_logged("Macro", addr, value).await;
        // Optimistically reflect our own send into the mirror — the desk doesn't
        // echo OSC-set values back (mirrors the gang engine's mirror update).
        if sent {
            self.state.write().await.update(addr.clone(), value.clone());
        }
        sent
    }

    /// Execute all steps of a macro in sequence, respecting per-step delays.
    /// When `macro_def.mark_dirty` is false, dirty tracking is suppressed
    /// for the duration of execution and the dirty set is cleared afterward.
    pub async fn execute(&self, macro_def: &MacroDef) -> MacroExecutionResult {
        let suppress = !macro_def.mark_dirty;
        if suppress {
            if let Some(dirty) = &self.dirty_tracker {
                dirty.write().await.begin_suppression();
            }
        }

        let result = self.execute_inner(macro_def).await;

        if suppress {
            if let Some(dirty) = &self.dirty_tracker {
                let mut t = dirty.write().await;
                t.end_suppression();
                t.clear();
            }
        }

        result
    }

    /// Inner execution logic, called by `execute` with or without dirty suppression.
    async fn execute_inner(&self, macro_def: &MacroDef) -> MacroExecutionResult {
        // Drive the shared progress line for the top-level macro: one op over
        // its top-level steps. Nested FireMacro steps count as a single step of
        // the parent (they don't re-begin), so the bar tracks top-level progress.
        if let Some(p) = &self.recall_progress {
            p.begin(RecallKind::Recall, macro_def.steps.len());
        }
        let result = self.execute_with_depth(macro_def, 0).await;
        if let Some(p) = &self.recall_progress {
            p.finish();
        }
        result
    }

    /// Recursive executor — `depth` increments for nested `FireMacro`
    /// invocations, capped at `MAX_MACRO_DEPTH`. Async recursion needs
    /// `Box::pin` at the recursive call site.
    async fn execute_with_depth(&self, macro_def: &MacroDef, depth: u32) -> MacroExecutionResult {
        if depth >= MAX_MACRO_DEPTH {
            warn!(
                name = %macro_def.name,
                depth,
                "Macro recursion depth exceeded; aborting"
            );
            return MacroExecutionResult {
                macro_name: macro_def.name.clone(),
                steps_executed: 0,
                steps_skipped: macro_def.steps.len(),
            };
        }

        info!(
            name = %macro_def.name,
            id = %macro_def.id,
            steps = macro_def.steps.len(),
            mark_dirty = macro_def.mark_dirty,
            depth,
            "Executing macro"
        );

        let mut executed = 0usize;
        let mut skipped = 0usize;

        for (i, step) in macro_def.steps.iter().enumerate() {
            // Wait for the step's delay
            if step.delay_ms > 0 {
                time::sleep(time::Duration::from_millis(step.delay_ms as u64)).await;
            }

            match &step.kind {
                MacroStepKind::Parameter { address, mode } => {
                    let resolved = self.resolve_parameter_value(address, mode).await;
                    match resolved {
                        Some(value) => {
                            // GP OSC first, iPad fallback for iPad-only params.
                            if self.send_parameter(address, &value).await {
                                debug!(
                                    step_index = i,
                                    addr = %address,
                                    value = %value,
                                    "Macro step sent"
                                );
                                executed += 1;
                                // Inter-message pacing: protects the
                                // console's ARM chip during a long-step
                                // macro the same way snapshot recall does.
                                // Independent of `delay_ms`, which is the
                                // user-controlled pre-step gap.
                                let pace = self.pace_us.load(Ordering::Relaxed);
                                if pace > 0 {
                                    time::sleep(time::Duration::from_micros(pace)).await;
                                }
                            } else {
                                skipped += 1;
                            }
                        }
                        None => {
                            warn!(
                                step_index = i,
                                addr = %address,
                                mode = %mode,
                                "Macro step skipped: could not resolve value"
                            );
                            skipped += 1;
                        }
                    }
                }
                MacroStepKind::GoNextCue => {
                    let _ = self.ui_tx.send(UiEvent::MacroFireGo);
                    executed += 1;
                }
                MacroStepKind::GoPreviousCue => {
                    let _ = self.ui_tx.send(UiEvent::MacroFirePrev);
                    executed += 1;
                }
                MacroStepKind::Connect => {
                    let _ = self.ui_tx.send(UiEvent::MacroConnect);
                    executed += 1;
                }
                MacroStepKind::Disconnect => {
                    // Tears down the connection running this macro;
                    // the engine will lose its sender, so any
                    // subsequent Parameter steps quietly fail to
                    // send. This is documented in the model's
                    // `MacroStepKind::Disconnect` doc comment.
                    let _ = self.ui_tx.send(UiEvent::MacroDisconnect);
                    executed += 1;
                }
                MacroStepKind::FireMacro { id } => {
                    let inner = self.macro_manager.read().await.get_macro(id).cloned();
                    match inner {
                        Some(inner_def) => {
                            let result =
                                Box::pin(self.execute_with_depth(&inner_def, depth + 1)).await;
                            executed += result.steps_executed;
                            skipped += result.steps_skipped;
                        }
                        None => {
                            warn!(
                                step_index = i,
                                macro_id = %id,
                                "Macro step skipped: FireMacro target not found"
                            );
                            skipped += 1;
                        }
                    }
                }
                MacroStepKind::RecallSnapshot { id } => {
                    let _ = self
                        .ui_tx
                        .send(UiEvent::MacroRecallSnapshot { snapshot_id: *id });
                    executed += 1;
                }
                MacroStepKind::RecallPalette { id, channel } => {
                    let _ = self.ui_tx.send(UiEvent::MacroRecallPalette {
                        palette_id: *id,
                        channel: channel.clone(),
                    });
                    executed += 1;
                }
                MacroStepKind::QLabGo => {
                    let _ = self.ui_tx.send(UiEvent::MacroQLabSend {
                        addr: "/go".into(),
                        string_arg: None,
                        label: "QLab Go".into(),
                    });
                    executed += 1;
                }
                MacroStepKind::QLabGoCue { cue_number } => {
                    // Argument-form `/go {cue_number}` (per QLab 5 OSC
                    // dictionary) — safer than `/go/{cue_number}` when
                    // cue numbers contain characters that aren't valid
                    // in an OSC address (slashes, spaces, etc.).
                    let _ = self.ui_tx.send(UiEvent::MacroQLabSend {
                        addr: "/go".into(),
                        string_arg: Some(cue_number.clone()),
                        label: format!("QLab Go Cue #{cue_number}"),
                    });
                    executed += 1;
                }
                MacroStepKind::QLabPanic => {
                    let _ = self.ui_tx.send(UiEvent::MacroQLabSend {
                        addr: "/panic".into(),
                        string_arg: None,
                        label: "QLab Panic".into(),
                    });
                    executed += 1;
                }
                MacroStepKind::QLabStop => {
                    let _ = self.ui_tx.send(UiEvent::MacroQLabSend {
                        addr: "/stop".into(),
                        string_arg: None,
                        label: "QLab Stop".into(),
                    });
                    executed += 1;
                }
                MacroStepKind::QLabPause => {
                    let _ = self.ui_tx.send(UiEvent::MacroQLabSend {
                        addr: "/pause".into(),
                        string_arg: None,
                        label: "QLab Pause".into(),
                    });
                    executed += 1;
                }
                MacroStepKind::QLabResume => {
                    let _ = self.ui_tx.send(UiEvent::MacroQLabSend {
                        addr: "/resume".into(),
                        string_arg: None,
                        label: "QLab Resume".into(),
                    });
                    executed += 1;
                }
            }

            // Advance the progress bar once per top-level step (nested
            // FireMacro recursion runs at depth>0 and is already accounted for
            // by its parent step).
            if depth == 0
                && let Some(p) = &self.recall_progress
            {
                p.bump();
            }
        }

        info!(
            name = %macro_def.name,
            executed,
            skipped,
            depth,
            "Macro execution complete"
        );

        MacroExecutionResult {
            macro_name: macro_def.name.clone(),
            steps_executed: executed,
            steps_skipped: skipped,
        }
    }

    /// Resolve the concrete `ParameterValue` for a `Parameter`-kind
    /// step based on its mode.
    async fn resolve_parameter_value(
        &self,
        address: &crate::model::parameter::ParameterAddress,
        mode: &MacroStepMode,
    ) -> Option<ParameterValue> {
        match mode {
            MacroStepMode::Fixed(value) => Some(value.clone()),

            MacroStepMode::Toggle => {
                let state = self.state.read().await;
                let current = state.get(address)?;
                Some(toggle_value(current))
            }

            MacroStepMode::Relative(offset) => {
                let state = self.state.read().await;
                let current = state.get(address)?;
                apply_relative_offset(current, *offset)
            }
        }
    }
}

/// Toggle a parameter value.
/// - Bool: negate
/// - Int: 0 becomes 1, anything else becomes 0
/// - Float: 0.0 becomes 1.0, anything else becomes 0.0
/// - String: returned unchanged (cannot toggle)
fn toggle_value(current: &ParameterValue) -> ParameterValue {
    match current {
        ParameterValue::Bool(b) => ParameterValue::Bool(!b),
        ParameterValue::Int(i) => ParameterValue::Int(if *i == 0 { 1 } else { 0 }),
        ParameterValue::Float(f) => ParameterValue::Float(if *f == 0.0 { 1.0 } else { 0.0 }),
        ParameterValue::String(_) => current.clone(),
    }
}

/// Apply a relative offset to a numeric parameter value.
/// - Float: add offset directly
/// - Int: add offset rounded to nearest integer
/// - Bool/String: returns None (not applicable)
fn apply_relative_offset(current: &ParameterValue, offset: f32) -> Option<ParameterValue> {
    match current {
        ParameterValue::Float(f) => Some(ParameterValue::Float(f + offset)),
        ParameterValue::Int(i) => Some(ParameterValue::Int(i + offset.round() as i32)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::config::ConsoleConfig;
    use crate::model::macro_def::MacroStep;
    use crate::model::parameter::{ParameterAddress, ParameterPath};
    use crate::osc::client::OscClient;
    use std::net::SocketAddr;

    // ─── Pure function tests ───────────────────────────────────────

    #[test]
    fn toggle_bool_true() {
        assert_eq!(
            toggle_value(&ParameterValue::Bool(true)),
            ParameterValue::Bool(false)
        );
    }

    #[test]
    fn toggle_bool_false() {
        assert_eq!(
            toggle_value(&ParameterValue::Bool(false)),
            ParameterValue::Bool(true)
        );
    }

    #[test]
    fn toggle_int_zero() {
        assert_eq!(
            toggle_value(&ParameterValue::Int(0)),
            ParameterValue::Int(1)
        );
    }

    #[test]
    fn toggle_int_nonzero() {
        assert_eq!(
            toggle_value(&ParameterValue::Int(1)),
            ParameterValue::Int(0)
        );
        assert_eq!(
            toggle_value(&ParameterValue::Int(42)),
            ParameterValue::Int(0)
        );
    }

    #[test]
    fn toggle_float_zero() {
        assert_eq!(
            toggle_value(&ParameterValue::Float(0.0)),
            ParameterValue::Float(1.0)
        );
    }

    #[test]
    fn toggle_float_nonzero() {
        assert_eq!(
            toggle_value(&ParameterValue::Float(0.75)),
            ParameterValue::Float(0.0)
        );
    }

    #[test]
    fn toggle_string_unchanged() {
        let val = ParameterValue::String("Kick".into());
        assert_eq!(toggle_value(&val), val);
    }

    #[test]
    fn relative_float() {
        let result = apply_relative_offset(&ParameterValue::Float(0.5), 0.3);
        match result {
            Some(ParameterValue::Float(f)) => {
                assert!((f - 0.8).abs() < 0.001);
            }
            _ => panic!("Expected Some(Float)"),
        }
    }

    #[test]
    fn relative_float_negative() {
        let result = apply_relative_offset(&ParameterValue::Float(1.0), -0.4);
        match result {
            Some(ParameterValue::Float(f)) => {
                assert!((f - 0.6).abs() < 0.001);
            }
            _ => panic!("Expected Some(Float)"),
        }
    }

    #[test]
    fn relative_int() {
        assert_eq!(
            apply_relative_offset(&ParameterValue::Int(5), 2.7),
            Some(ParameterValue::Int(8)) // 5 + round(2.7) = 5 + 3
        );
    }

    #[test]
    fn relative_int_negative() {
        assert_eq!(
            apply_relative_offset(&ParameterValue::Int(10), -3.0),
            Some(ParameterValue::Int(7))
        );
    }

    #[test]
    fn relative_bool_returns_none() {
        assert_eq!(
            apply_relative_offset(&ParameterValue::Bool(true), 1.0),
            None
        );
    }

    #[test]
    fn relative_string_returns_none() {
        assert_eq!(
            apply_relative_offset(&ParameterValue::String("x".into()), 1.0),
            None
        );
    }

    // ─── Integration test with real OscSender ──────────────────────

    async fn setup_test() -> (
        MacroEngine,
        Arc<RwLock<ConsoleState>>,
        Arc<RwLock<MacroManager>>,
        std::sync::mpsc::Receiver<UiEvent>,
    ) {
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        // Port 1 (any non-zero) — Linux's sendto rejects port 0 with
        // EINVAL while Windows accepts it.
        let remote: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let client = OscClient::new(local, remote, None).await.unwrap();
        let (sender, _rx) = client.into_parts();

        let state = Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default())));
        let macro_manager = Arc::new(RwLock::new(MacroManager::new()));
        let (ui_tx, ui_rx) = std::sync::mpsc::channel();
        let pace = Arc::new(AtomicU64::new(0));
        let engine = MacroEngine::new(state.clone(), sender, macro_manager.clone(), ui_tx, pace);
        (engine, state, macro_manager, ui_rx)
    }

    #[tokio::test]
    async fn pacing_delays_consecutive_parameter_sends() {
        // Two Parameter steps with `delay_ms = 0` and pacing = 50 ms
        // should still take at least ~50 ms total — the pacing fires
        // *after* the first send, before the second. We measure
        // generously (>= 40 ms) to absorb scheduler jitter.
        let (engine, _state, _mgr, _rx) = setup_test().await;
        engine.pace_us.store(50_000, Ordering::Relaxed);

        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        let macro_def = MacroDef::new(
            "Two-step paced".into(),
            vec![
                MacroStep::parameter(
                    addr.clone(),
                    MacroStepMode::Fixed(ParameterValue::Float(-10.0)),
                    0,
                ),
                MacroStep::parameter(addr, MacroStepMode::Fixed(ParameterValue::Float(-12.0)), 0),
            ],
        );

        let start = std::time::Instant::now();
        let result = engine.execute(&macro_def).await;
        let elapsed = start.elapsed();
        assert_eq!(result.steps_executed, 2);
        assert!(
            elapsed >= std::time::Duration::from_millis(40),
            "expected pacing to delay second send by ~50 ms, got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn execute_fixed_step() {
        let (engine, _state, _mgr, _rx) = setup_test().await;

        let macro_def = MacroDef::new(
            "Test Fixed".into(),
            vec![MacroStep::parameter(
                ParameterAddress {
                    channel: ChannelId::Input(1),
                    parameter: ParameterPath::Fader,
                },
                MacroStepMode::Fixed(ParameterValue::Float(-10.0)),
                0,
            )],
        );

        let result = engine.execute(&macro_def).await;
        assert_eq!(result.steps_executed, 1);
        assert_eq!(result.steps_skipped, 0);
    }

    #[tokio::test]
    async fn execute_toggle_with_live_state() {
        let (engine, state, _mgr, _rx) = setup_test().await;

        // Set up live state: mute is currently off
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Mute,
        };
        state
            .write()
            .await
            .update(addr.clone(), ParameterValue::Bool(false));

        let macro_def = MacroDef::new(
            "Toggle Mute".into(),
            vec![MacroStep::parameter(addr, MacroStepMode::Toggle, 0)],
        );

        let result = engine.execute(&macro_def).await;
        assert_eq!(result.steps_executed, 1);
        assert_eq!(result.steps_skipped, 0);
    }

    #[tokio::test]
    async fn execute_toggle_without_live_state_skips() {
        let (engine, _state, _mgr, _rx) = setup_test().await;

        // No live state for this parameter — toggle cannot resolve
        let macro_def = MacroDef::new(
            "Toggle Unknown".into(),
            vec![MacroStep::parameter(
                ParameterAddress {
                    channel: ChannelId::Input(99),
                    parameter: ParameterPath::Mute,
                },
                MacroStepMode::Toggle,
                0,
            )],
        );

        let result = engine.execute(&macro_def).await;
        assert_eq!(result.steps_executed, 0);
        assert_eq!(result.steps_skipped, 1);
    }

    #[tokio::test]
    async fn execute_ipad_only_skips() {
        let (engine, _state, _mgr, _rx) = setup_test().await;

        let macro_def = MacroDef::new(
            "iPad Only".into(),
            vec![MacroStep::parameter(
                ParameterAddress {
                    channel: ChannelId::GraphicEq(1),
                    parameter: ParameterPath::GeqBandGain(1),
                },
                MacroStepMode::Fixed(ParameterValue::Float(3.0)),
                0,
            )],
        );

        let result = engine.execute(&macro_def).await;
        assert_eq!(result.steps_executed, 0);
        assert_eq!(result.steps_skipped, 1);
    }

    #[tokio::test]
    async fn execute_ipad_only_sends_when_ipad_sender_present() {
        // With an iPad sender wired in, an iPad-only parameter (no GP OSC
        // encoding) falls back to the iPad protocol and is executed, not
        // skipped. A UDP send to a dead local port still returns Ok, so the
        // step counts as sent.
        let (mut engine, _state, _mgr, _rx) = setup_test().await;

        let std_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        std_socket.set_nonblocking(true).unwrap();
        let socket = std::sync::Arc::new(tokio::net::UdpSocket::from_std(std_socket).unwrap());
        let ipad = IpadSender::from_socket(socket, "127.0.0.1:1".parse().unwrap());
        engine.set_ipad_sender(Some(ipad));

        let macro_def = MacroDef::new(
            "iPad Only".into(),
            vec![MacroStep::parameter(
                ParameterAddress {
                    channel: ChannelId::GraphicEq(1),
                    parameter: ParameterPath::GeqBandGain(1),
                },
                MacroStepMode::Fixed(ParameterValue::Float(3.0)),
                0,
            )],
        );

        let result = engine.execute(&macro_def).await;
        assert_eq!(result.steps_executed, 1);
        assert_eq!(result.steps_skipped, 0);
    }

    #[tokio::test]
    async fn fire_macro_emits_inner_steps() {
        // A macro that calls another macro should run the inner
        // macro's steps inline. Verify by counting executed+skipped
        // — inner has 1 step, outer wraps it via FireMacro.
        let (engine, _state, mgr, _rx) = setup_test().await;
        let inner = MacroDef::new(
            "Inner".into(),
            vec![MacroStep::parameter(
                ParameterAddress {
                    channel: ChannelId::Input(1),
                    parameter: ParameterPath::Fader,
                },
                MacroStepMode::Fixed(ParameterValue::Float(0.0)),
                0,
            )],
        );
        let inner_id = inner.id;
        mgr.write().await.add_macro(inner);

        let outer = MacroDef::new(
            "Outer".into(),
            vec![MacroStep {
                kind: MacroStepKind::FireMacro { id: inner_id },
                delay_ms: 0,
            }],
        );

        let result = engine.execute(&outer).await;
        // Inner had 1 parameter step; total executed reflects it.
        assert_eq!(result.steps_executed, 1);
        assert_eq!(result.steps_skipped, 0);
    }

    #[tokio::test]
    async fn fire_macro_recursion_depth_guarded() {
        // A macro that calls itself should hit the depth limit and
        // bail out — no infinite recursion. The outer Fire step is
        // counted as "executed" each iteration (it dispatches), but
        // at MAX_MACRO_DEPTH the inner call returns all-skipped, so
        // the chain terminates cleanly.
        let (engine, _state, mgr, _rx) = setup_test().await;
        let id = uuid::Uuid::new_v4();
        let mut self_ref = MacroDef::new("Loop".into(), vec![]);
        self_ref.id = id;
        self_ref.steps.push(MacroStep {
            kind: MacroStepKind::FireMacro { id },
            delay_ms: 0,
        });
        mgr.write().await.add_macro(self_ref.clone());

        // Should return without panicking or hanging.
        let result = engine.execute(&self_ref).await;
        // Depth limit hits at MAX_MACRO_DEPTH=5; before then each
        // call counts its single step as executed (via inner). Each
        // depth's inner call adds 1. Final depth returns 1 skipped.
        assert!(
            result.steps_skipped >= 1,
            "depth-limit hit produced at least 1 skipped step"
        );
    }

    #[tokio::test]
    async fn fire_macro_unknown_id_is_skipped() {
        let (engine, _state, _mgr, _rx) = setup_test().await;
        let outer = MacroDef::new(
            "Bad Ref".into(),
            vec![MacroStep {
                kind: MacroStepKind::FireMacro {
                    id: uuid::Uuid::new_v4(),
                },
                delay_ms: 0,
            }],
        );
        let result = engine.execute(&outer).await;
        assert_eq!(result.steps_executed, 0);
        assert_eq!(result.steps_skipped, 1);
    }

    #[tokio::test]
    async fn app_action_steps_emit_ui_events() {
        let (engine, _state, _mgr, rx) = setup_test().await;
        let m = MacroDef::new(
            "Transport".into(),
            vec![
                MacroStep {
                    kind: MacroStepKind::GoNextCue,
                    delay_ms: 0,
                },
                MacroStep {
                    kind: MacroStepKind::GoPreviousCue,
                    delay_ms: 0,
                },
                MacroStep {
                    kind: MacroStepKind::Disconnect,
                    delay_ms: 0,
                },
            ],
        );
        let result = engine.execute(&m).await;
        assert_eq!(result.steps_executed, 3);
        // Drain the events: order matches steps.
        let mut events: Vec<&'static str> = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            match ev {
                UiEvent::MacroFireGo => events.push("Go"),
                UiEvent::MacroFirePrev => events.push("Prev"),
                UiEvent::MacroDisconnect => events.push("Disconnect"),
                _ => {}
            }
        }
        assert_eq!(events, vec!["Go", "Prev", "Disconnect"]);
    }
}
