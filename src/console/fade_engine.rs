use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::model::parameter::{ParameterAddress, ParameterValue};
use crate::osc::client::OscSender;
use crate::osc::ipad_client::IpadSender;
use crate::osc::encode;
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
        ))
    }
}

/// Run a fade interpolation loop.
async fn run_fade(
    cue_number: f32,
    fade_time_secs: f32,
    targets: Vec<FadeTarget>,
    sender: OscSender,
    ipad_sender: Option<IpadSender>,
    cancel: CancellationToken,
) -> FadeResult {
    if targets.is_empty() {
        return FadeResult { total_steps_sent: 0, cancelled: false };
    }

    let total_duration = Duration::from_secs_f32(fade_time_secs);
    let start = time::Instant::now();
    let mut steps_sent = 0usize;

    info!(cue_number, targets = targets.len(), fade_time_secs, "Fade started");

    loop {
        if cancel.is_cancelled() {
            info!(cue_number, steps_sent, "Fade cancelled");
            return FadeResult { total_steps_sent: steps_sent, cancelled: true };
        }

        let elapsed = start.elapsed();
        let t = if total_duration.is_zero() {
            1.0
        } else {
            (elapsed.as_secs_f32() / fade_time_secs).min(1.0)
        };

        // Interpolate and send each target
        for target in &targets {
            if let Some(interpolated) = target.start_value.lerp(&target.end_value, t) {
                let sent = send_parameter(&sender, &ipad_sender, &target.address, &interpolated).await;
                if sent {
                    steps_sent += 1;
                }
            }
        }

        if t >= 1.0 {
            break;
        }

        time::sleep(FADE_INTERVAL).await;
    }

    info!(cue_number, steps_sent, "Fade complete");
    FadeResult { total_steps_sent: steps_sent, cancelled: false }
}

/// Send a parameter via GP OSC, falling back to iPad protocol.
async fn send_parameter(
    sender: &OscSender,
    ipad_sender: &Option<IpadSender>,
    addr: &ParameterAddress,
    value: &ParameterValue,
) -> bool {
    match encode::encode_parameter(addr, value) {
        Some((path, args)) => {
            sender.send(&path, args).await.is_ok()
        }
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
    use std::sync::Arc;
    use crate::model::channel::ChannelId;
    use crate::model::parameter::ParameterPath;
    use crate::osc::client::OscClient;
    use std::net::SocketAddr;

    async fn test_sender() -> OscSender {
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let remote: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let client = OscClient::new(local, remote).await.unwrap();
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

        let handle = controller.start_fade(1.0, 0.15, targets, sender, None).await;
        let result = handle.await.unwrap();
        // With 50ms interval over 150ms, should get ~3-4 update rounds
        assert!(result.total_steps_sent >= 2, "Expected at least 2 steps, got {}", result.total_steps_sent);
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
            let remote: SocketAddr = "127.0.0.1:0".parse().unwrap();
            let client = OscClient::new(local, remote).await.unwrap();
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

        let handle1 = controller.start_fade(1.0, 5.0, targets1, sender, None).await;

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
        let handle2 = controller.start_fade(2.0, 0.1, targets2, sender2, None).await;

        // First fade should be cancelled
        let result1 = handle1.await.unwrap();
        assert!(result1.cancelled);

        // Second fade should complete normally
        let result2 = handle2.await.unwrap();
        assert!(!result2.cancelled);
    }

    #[test]
    fn fade_result_debug() {
        let r = FadeResult { total_steps_sent: 42, cancelled: false };
        assert!(format!("{r:?}").contains("42"));
    }
}
