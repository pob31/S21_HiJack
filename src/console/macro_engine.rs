use std::sync::Arc;

use tokio::sync::RwLock;
use tokio::time;
use tracing::{info, warn, debug};

use crate::model::macro_def::{MacroDef, MacroStep, MacroStepMode};
use crate::model::parameter::ParameterValue;
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::encode;

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
    sender: OscSender,
}

impl MacroEngine {
    pub fn new(state: Arc<RwLock<ConsoleState>>, sender: OscSender) -> Self {
        Self { state, sender }
    }

    /// Execute all steps of a macro in sequence, respecting per-step delays.
    pub async fn execute(&self, macro_def: &MacroDef) -> MacroExecutionResult {
        info!(
            name = %macro_def.name,
            id = %macro_def.id,
            steps = macro_def.steps.len(),
            "Executing macro"
        );

        let mut executed = 0usize;
        let mut skipped = 0usize;

        for (i, step) in macro_def.steps.iter().enumerate() {
            // Wait for the step's delay
            if step.delay_ms > 0 {
                time::sleep(time::Duration::from_millis(step.delay_ms as u64)).await;
            }

            // Resolve the concrete value based on mode
            let resolved = self.resolve_step_value(step).await;

            match resolved {
                Some(value) => {
                    // Encode to GP OSC
                    match encode::encode_parameter(&step.address, &value) {
                        Some((path, args)) => {
                            if let Err(e) = self.sender.send(&path, args).await {
                                warn!(
                                    step_index = i,
                                    addr = %step.address,
                                    "Macro step send failed: {e}"
                                );
                                skipped += 1;
                            } else {
                                debug!(
                                    step_index = i,
                                    addr = %step.address,
                                    value = %value,
                                    "Macro step sent"
                                );
                                executed += 1;
                            }
                        }
                        None => {
                            debug!(
                                step_index = i,
                                addr = %step.address,
                                "Macro step skipped: iPad-only parameter"
                            );
                            skipped += 1;
                        }
                    }
                }
                None => {
                    warn!(
                        step_index = i,
                        addr = %step.address,
                        mode = %step.mode,
                        "Macro step skipped: could not resolve value"
                    );
                    skipped += 1;
                }
            }
        }

        info!(
            name = %macro_def.name,
            executed,
            skipped,
            "Macro execution complete"
        );

        MacroExecutionResult {
            macro_name: macro_def.name.clone(),
            steps_executed: executed,
            steps_skipped: skipped,
        }
    }

    /// Resolve the concrete ParameterValue for a step based on its mode.
    async fn resolve_step_value(&self, step: &MacroStep) -> Option<ParameterValue> {
        match &step.mode {
            MacroStepMode::Fixed(value) => Some(value.clone()),

            MacroStepMode::Toggle => {
                let state = self.state.read().await;
                let current = state.get(&step.address)?;
                Some(toggle_value(current))
            }

            MacroStepMode::Relative(offset) => {
                let state = self.state.read().await;
                let current = state.get(&step.address)?;
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

    async fn setup_test() -> (MacroEngine, Arc<RwLock<ConsoleState>>) {
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let remote: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let client = OscClient::new(local, remote).await.unwrap();
        let (sender, _rx) = client.into_parts();

        let state = Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default())));
        let engine = MacroEngine::new(state.clone(), sender);
        (engine, state)
    }

    #[tokio::test]
    async fn execute_fixed_step() {
        let (engine, _state) = setup_test().await;

        let macro_def = MacroDef::new(
            "Test Fixed".into(),
            vec![MacroStep {
                address: ParameterAddress {
                    channel: ChannelId::Input(1),
                    parameter: ParameterPath::Fader,
                },
                mode: MacroStepMode::Fixed(ParameterValue::Float(-10.0)),
                delay_ms: 0,
            }],
        );

        let result = engine.execute(&macro_def).await;
        assert_eq!(result.steps_executed, 1);
        assert_eq!(result.steps_skipped, 0);
    }

    #[tokio::test]
    async fn execute_toggle_with_live_state() {
        let (engine, state) = setup_test().await;

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
            vec![MacroStep {
                address: addr,
                mode: MacroStepMode::Toggle,
                delay_ms: 0,
            }],
        );

        let result = engine.execute(&macro_def).await;
        assert_eq!(result.steps_executed, 1);
        assert_eq!(result.steps_skipped, 0);
    }

    #[tokio::test]
    async fn execute_toggle_without_live_state_skips() {
        let (engine, _state) = setup_test().await;

        // No live state for this parameter — toggle cannot resolve
        let macro_def = MacroDef::new(
            "Toggle Unknown".into(),
            vec![MacroStep {
                address: ParameterAddress {
                    channel: ChannelId::Input(99),
                    parameter: ParameterPath::Mute,
                },
                mode: MacroStepMode::Toggle,
                delay_ms: 0,
            }],
        );

        let result = engine.execute(&macro_def).await;
        assert_eq!(result.steps_executed, 0);
        assert_eq!(result.steps_skipped, 1);
    }

    #[tokio::test]
    async fn execute_ipad_only_skips() {
        let (engine, _state) = setup_test().await;

        let macro_def = MacroDef::new(
            "iPad Only".into(),
            vec![MacroStep {
                address: ParameterAddress {
                    channel: ChannelId::GraphicEq(1),
                    parameter: ParameterPath::GeqBandGain(1),
                },
                mode: MacroStepMode::Fixed(ParameterValue::Float(3.0)),
                delay_ms: 0,
            }],
        );

        let result = engine.execute(&macro_def).await;
        assert_eq!(result.steps_executed, 0);
        assert_eq!(result.steps_skipped, 1);
    }
}
