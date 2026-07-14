use std::time::Duration;

use tokio::sync::{Mutex, RwLock};
use tokio::time;
use tokio_util::sync::CancellationToken;
use tracing::info;

use std::collections::HashMap;
use std::sync::Arc;

use crate::console::automation_registry::AutomationOverride;
use crate::model::channel::ChannelId;
use crate::model::parameter::{ParameterAddress, ParameterValue, TimingCategory, floored_db_lerp};
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::encode;
use crate::osc::ipad_client::IpadSender;
use crate::osc::ipad_encode;

/// Update interval for fade interpolation (~20 updates/sec).
const FADE_INTERVAL: Duration = Duration::from_millis(50);

/// A single parameter being faded from start to end value.
pub struct FadeTarget {
    pub address: ParameterAddress,
    pub start_value: ParameterValue,
    pub end_value: ParameterValue,
}

/// Result of a completed (or cancelled) fade.
#[derive(Debug)]
pub struct FadeResult {
    pub total_steps_sent: usize,
    pub cancelled: bool,
}

/// Manages active fades with cancellation support.
///
/// Only one fade runs at a time. Starting a new fade cancels any in-progress one.
///
/// NOTE: not used by the production recall path. Cue recalls run fades via
/// [`run_fade_inline`] with a child of the snapshot engine's per-recall root
/// `CancellationToken` — a new recall cancels that root ("latest cue wins").
/// This controller (and [`MultiFadeController`]) remain as the test harness
/// around [`run_fade`].
pub struct FadeController {
    active_token: Mutex<Option<CancellationToken>>,
}

impl FadeController {
    pub fn new() -> Self {
        Self {
            active_token: Mutex::new(None),
        }
    }

    /// Cancel any in-progress fade.
    pub async fn cancel_active(&self) {
        let mut guard = self.active_token.lock().await;
        if let Some(token) = guard.take() {
            token.cancel();
        }
    }

    /// Start a new fade, cancelling any existing one first.
    pub async fn start_fade(
        &self,
        cue_number: f32,
        fade_time_secs: f32,
        targets: Vec<FadeTarget>,
        sender: OscSender,
        ipad_sender: Option<IpadSender>,
    ) -> tokio::task::JoinHandle<FadeResult> {
        // Cancel existing fade
        self.cancel_active().await;

        // Create new cancellation token
        let token = CancellationToken::new();
        let child = token.child_token();

        {
            let mut guard = self.active_token.lock().await;
            *guard = Some(token);
        }

        tokio::spawn(run_fade(
            cue_number,
            fade_time_secs,
            targets,
            sender,
            ipad_sender,
            child,
            None,
            0,
            None,
        ))
    }
}

// ── Multi-fade controller ──────────────────────────────────────────

/// Key for a fade group — one concurrent fade per (channel, category).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FadeGroupKey {
    pub channel: ChannelId,
    pub category: TimingCategory,
}

/// Manages multiple concurrent fades, keyed by `(ChannelId, TimingCategory)`.
///
/// Each group can have its own fade running independently. Starting a fade
/// for a group that already has an active fade cancels the previous one.
/// `cancel_all` cancels every active group (used when a new cue recall starts).
pub struct MultiFadeController {
    active_groups: Mutex<HashMap<FadeGroupKey, CancellationToken>>,
}

impl MultiFadeController {
    pub fn new() -> Self {
        Self {
            active_groups: Mutex::new(HashMap::new()),
        }
    }

    /// Cancel all active group fades.
    pub async fn cancel_all(&self) {
        let mut guard = self.active_groups.lock().await;
        for (_, token) in guard.drain() {
            token.cancel();
        }
    }

    /// Start a fade for one group, cancelling any existing fade for the same key.
    /// Returns a `JoinHandle` that resolves when the fade completes or is cancelled.
    pub async fn start_group_fade(
        &self,
        key: FadeGroupKey,
        fade_time_secs: f32,
        targets: Vec<FadeTarget>,
        sender: OscSender,
        ipad_sender: Option<IpadSender>,
    ) -> tokio::task::JoinHandle<FadeResult> {
        let mut guard = self.active_groups.lock().await;

        // Cancel existing fade for this group
        if let Some(old_token) = guard.remove(&key) {
            old_token.cancel();
        }

        let token = CancellationToken::new();
        let child = token.child_token();
        guard.insert(key.clone(), token);
        drop(guard);

        tokio::spawn(run_fade(
            0.0, // cue_number not meaningful for group fades
            fade_time_secs,
            targets,
            sender,
            ipad_sender,
            child,
            None,
            0,
            None,
        ))
    }
}

/// Run a fade interpolation loop inline (not spawned). Used by per-category
/// timed recall where each group already runs in its own spawned task.
///
/// `registry`/`gen_id`: when present, each target is checked against the live
/// operator-override registry every tick — if the operator grabbed that
/// parameter it is dropped from the fade (the rest keep fading); and every
/// sent step is recorded so the console's echo of it is not mistaken for an
/// operator move.
#[allow(clippy::too_many_arguments)]
pub async fn run_fade_inline(
    fade_time_secs: f32,
    targets: Vec<FadeTarget>,
    sender: OscSender,
    ipad_sender: Option<IpadSender>,
    cancel: CancellationToken,
    registry: Option<AutomationOverride>,
    gen_id: u64,
    state: Option<Arc<RwLock<ConsoleState>>>,
) -> FadeResult {
    run_fade(
        0.0,
        fade_time_secs,
        targets,
        sender,
        ipad_sender,
        cancel,
        registry,
        gen_id,
        state,
    )
    .await
}

/// Run a fade interpolation loop.
#[allow(clippy::too_many_arguments)]
async fn run_fade(
    cue_number: f32,
    fade_time_secs: f32,
    targets: Vec<FadeTarget>,
    sender: OscSender,
    ipad_sender: Option<IpadSender>,
    cancel: CancellationToken,
    registry: Option<AutomationOverride>,
    gen_id: u64,
    state: Option<Arc<RwLock<ConsoleState>>>,
) -> FadeResult {
    if targets.is_empty() {
        return FadeResult {
            total_steps_sent: 0,
            cancelled: false,
        };
    }

    let total_duration = Duration::from_secs_f32(fade_time_secs);
    let start = time::Instant::now();
    let mut steps_sent = 0usize;

    info!(
        cue_number,
        targets = targets.len(),
        fade_time_secs,
        "Fade started"
    );

    loop {
        if cancel.is_cancelled() {
            info!(cue_number, steps_sent, "Fade cancelled");
            return FadeResult {
                total_steps_sent: steps_sent,
                cancelled: true,
            };
        }

        let elapsed = start.elapsed();
        let t = if total_duration.is_zero() {
            1.0
        } else {
            (elapsed.as_secs_f32() / fade_time_secs).min(1.0)
        };

        // Interpolate and send each target. The lerp result is
        // clamped to the parameter's valid range — without this, a
        // fade between two in-range pan values is fine, but a fade
        // anchored on an out-of-range start (e.g. legacy show file)
        // would emit out-of-range intermediate values straight to
        // the console.
        for target in &targets {
            // Operator override: if the operator grabbed this exact parameter,
            // it's no longer in the registry — stop fading it (the rest keep
            // going) so the hands-on console value always wins. Generation-
            // aware: a superseding recall re-registering this address under a
            // NEW generation must not revive this (older) fade for it.
            if let Some(reg) = &registry {
                if !reg.is_active_for(&target.address, gen_id) {
                    continue;
                }
            }
            // Fader-family dB levels interpolate in "floored" space so fades
            // to/from −inf are smooth (see `floored_db_lerp`); everything else
            // keeps the naive lerp + range clamp. The floored result is already
            // bounded to [−150, end], so it deliberately skips `clamp_value`
            // (which only models the pan family and is a passthrough for fader dB).
            let interpolated = match (
                &target.start_value,
                &target.end_value,
                target.address.parameter.fade_floor_db(),
            ) {
                (ParameterValue::Float(s), ParameterValue::Float(e), Some(floor)) => {
                    Some(ParameterValue::Float(floored_db_lerp(*s, *e, t, floor)))
                }
                _ => target
                    .start_value
                    .lerp(&target.end_value, t)
                    .map(|v| target.address.parameter.clamp_value(v)),
            };
            if let Some(interpolated) = interpolated {
                let sent =
                    send_parameter(&sender, &ipad_sender, &target.address, &interpolated).await;
                if sent {
                    steps_sent += 1;
                    // Record what we pushed so the resulting console echo isn't
                    // misread as an operator move.
                    if let Some(reg) = &registry {
                        reg.note_sent(&target.address, &interpolated, gen_id);
                    }
                    // Optimistically reflect our own send into the live mirror.
                    // The desk doesn't echo OSC-set values back, so without this
                    // the mirror goes stale and the NEXT fade would start from a
                    // stale value (the "jump after a cue change"). Mirrors the
                    // gang / monitor engines' optimistic mirror updates.
                    if let Some(st) = &state {
                        st.write()
                            .await
                            .update(target.address.clone(), interpolated.clone());
                    }
                }
            }
        }

        if t >= 1.0 {
            break;
        }

        // Cancellable sleep — exit the loop promptly if cancelled mid-tick
        // instead of waiting up to FADE_INTERVAL for the next is_cancelled
        // poll. In-flight `send_parameter().await` calls are not interrupted
        // (avoids leaving parameters in a partially-set state).
        tokio::select! {
            () = cancel.cancelled() => {
                info!(cue_number, steps_sent, "Fade cancelled mid-tick");
                return FadeResult {
                    total_steps_sent: steps_sent,
                    cancelled: true,
                };
            }
            () = time::sleep(FADE_INTERVAL) => {}
        }
    }

    // Normal completion: drop these addresses from the registry so a finished
    // fade no longer shadows the operator.
    if let Some(reg) = &registry {
        for target in &targets {
            reg.deregister(&target.address);
        }
    }

    info!(cue_number, steps_sent, "Fade complete");
    FadeResult {
        total_steps_sent: steps_sent,
        cancelled: false,
    }
}

/// Send a parameter via GP OSC, falling back to iPad protocol.
async fn send_parameter(
    sender: &OscSender,
    ipad_sender: &Option<IpadSender>,
    addr: &ParameterAddress,
    value: &ParameterValue,
) -> bool {
    match encode::encode_parameter(addr, value) {
        Some((path, args)) => sender.send(&path, args).await.is_ok(),
        None => {
            if let Some(ipad) = ipad_sender {
                match ipad_encode::encode_ipad_parameter(addr, value) {
                    Some((path, args)) => ipad.send(&path, args).await.is_ok(),
                    None => false,
                }
            } else {
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::parameter::ParameterPath;
    use crate::osc::client::OscClient;
    use std::net::SocketAddr;
    use std::sync::Arc;

    async fn test_sender() -> OscSender {
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        // Port 1 (any non-zero) — Linux's sendto rejects port 0 with
        // EINVAL while Windows accepts it, so the test sender needs a
        // valid destination even though no one listens.
        let remote: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let client = OscClient::new(local, remote, None).await.unwrap();
        let (sender, _rx) = client.into_parts();
        sender
    }

    #[tokio::test]
    async fn fade_empty_targets_completes_immediately() {
        let sender = test_sender().await;
        let controller = FadeController::new();
        let handle = controller.start_fade(1.0, 1.0, vec![], sender, None).await;
        let result = handle.await.unwrap();
        assert_eq!(result.total_steps_sent, 0);
        assert!(!result.cancelled);
    }

    #[tokio::test]
    async fn fade_sends_updates() {
        let sender = test_sender().await;
        let controller = FadeController::new();

        let targets = vec![FadeTarget {
            address: ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            start_value: ParameterValue::Float(0.0),
            end_value: ParameterValue::Float(1.0),
        }];

        let handle = controller
            .start_fade(1.0, 0.15, targets, sender, None)
            .await;
        let result = handle.await.unwrap();
        // With 50ms interval over 150ms, should get ~3-4 update rounds
        assert!(
            result.total_steps_sent >= 2,
            "Expected at least 2 steps, got {}",
            result.total_steps_sent
        );
        assert!(!result.cancelled);
    }

    #[tokio::test]
    async fn fade_cancellation() {
        let sender = test_sender().await;
        let controller = Arc::new(FadeController::new());

        let targets = vec![FadeTarget {
            address: ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            start_value: ParameterValue::Float(0.0),
            end_value: ParameterValue::Float(1.0),
        }];

        let handle = controller.start_fade(1.0, 5.0, targets, sender, None).await;

        // Let it run briefly then cancel
        time::sleep(Duration::from_millis(80)).await;
        controller.cancel_active().await;

        let result = handle.await.unwrap();
        assert!(result.cancelled);
    }

    #[tokio::test]
    async fn start_fade_replaces_active() {
        let sender = test_sender().await;
        let sender2 = {
            let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
            // Port 1 (any non-zero) — Linux's sendto rejects port 0 with
            // EINVAL while Windows accepts it, so the test sender needs a
            // valid destination even though no one listens.
            let remote: SocketAddr = "127.0.0.1:1".parse().unwrap();
            let client = OscClient::new(local, remote, None).await.unwrap();
            let (s, _rx) = client.into_parts();
            s
        };
        let controller = FadeController::new();

        let targets1 = vec![FadeTarget {
            address: ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            start_value: ParameterValue::Float(0.0),
            end_value: ParameterValue::Float(1.0),
        }];

        let handle1 = controller
            .start_fade(1.0, 5.0, targets1, sender, None)
            .await;

        // Start a second fade — should cancel the first
        time::sleep(Duration::from_millis(80)).await;
        let targets2 = vec![FadeTarget {
            address: ParameterAddress {
                channel: ChannelId::Input(2),
                parameter: ParameterPath::Fader,
            },
            start_value: ParameterValue::Float(1.0),
            end_value: ParameterValue::Float(0.0),
        }];
        let handle2 = controller
            .start_fade(2.0, 0.1, targets2, sender2, None)
            .await;

        // First fade should be cancelled
        let result1 = handle1.await.unwrap();
        assert!(result1.cancelled);

        // Second fade should complete normally
        let result2 = handle2.await.unwrap();
        assert!(!result2.cancelled);
    }

    #[test]
    fn fade_result_debug() {
        let r = FadeResult {
            total_steps_sent: 42,
            cancelled: false,
        };
        assert!(format!("{r:?}").contains("42"));
    }
}
