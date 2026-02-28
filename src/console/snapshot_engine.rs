use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{info, warn, debug};

use crate::model::snapshot::{Cue, ScopeTemplate, Snapshot};
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::encode;

/// Result of a snapshot recall operation.
#[derive(Debug)]
pub struct RecallResult {
    /// Number of parameters sent to the console.
    pub parameters_sent: usize,
    /// Number of parameters skipped (no change, iPad-only, etc.).
    pub parameters_skipped: usize,
}

/// The snapshot recall engine — diffs snapshot data against live state and sends changes.
pub struct SnapshotEngine {
    state: Arc<RwLock<ConsoleState>>,
    sender: OscSender,
}

impl SnapshotEngine {
    pub fn new(state: Arc<RwLock<ConsoleState>>, sender: OscSender) -> Self {
        Self { state, sender }
    }

    /// Recall a snapshot using the given scope.
    ///
    /// For each parameter in the snapshot data that's within the effective scope:
    /// 1. Compare against the live state mirror
    /// 2. If different (or not present in live state), send via GP OSC
    /// 3. Skip iPad-only parameters (encode returns None)
    pub async fn recall(&self, snapshot: &Snapshot, scope: &ScopeTemplate) -> RecallResult {
        let state = self.state.read().await;
        let mut sent = 0usize;
        let mut skipped = 0usize;

        for (addr, snap_value) in &snapshot.data.values {
            // Only recall parameters within the effective scope
            if !scope.contains(addr) {
                skipped += 1;
                continue;
            }

            // Check if the value differs from live state
            let live_value = state.get(addr);
            if live_value == Some(snap_value) {
                skipped += 1;
                debug!(%addr, "Recall skip: value unchanged");
                continue;
            }

            // Encode to GP OSC
            match encode::encode_parameter(addr, snap_value) {
                Some((path, args)) => {
                    if let Err(e) = self.sender.send(&path, args).await {
                        warn!(%addr, "Failed to send recall: {e}");
                        skipped += 1;
                    } else {
                        debug!(%addr, %snap_value, "Recall: sent parameter");
                        sent += 1;
                    }
                }
                None => {
                    // iPad-only parameter — can't send via GP OSC
                    skipped += 1;
                    debug!(%addr, "Recall skip: iPad-only parameter");
                }
            }
        }

        info!(sent, skipped, "Snapshot recall complete");
        RecallResult {
            parameters_sent: sent,
            parameters_skipped: skipped,
        }
    }

    /// Recall a cue — resolves effective scope and delegates to recall().
    pub async fn recall_cue(&self, cue: &Cue, snapshot: &Snapshot) -> RecallResult {
        let effective_scope = cue.scope_override.as_ref().unwrap_or(&snapshot.scope);
        info!(
            cue_number = cue.cue_number,
            cue_name = %cue.name,
            snapshot_name = %snapshot.name,
            "Recalling cue"
        );
        self.recall(snapshot, effective_scope).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use crate::model::channel::ChannelId;
    use crate::model::config::ConsoleConfig;
    use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterSection, ParameterValue};
    use crate::model::snapshot::{ChannelScope, ScopeTemplate, Snapshot, SnapshotData};
    use crate::osc::client::OscClient;
    use std::net::SocketAddr;

    async fn setup_test() -> (SnapshotEngine, Arc<RwLock<ConsoleState>>) {
        // Bind to any available port for the test
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let remote: SocketAddr = "127.0.0.1:0".parse().unwrap();
        // We need a real UDP socket pair for OscSender
        let client = OscClient::new(local, remote).await.unwrap();
        let (sender, _rx) = client.into_parts();

        let state = Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default())));
        let engine = SnapshotEngine::new(state.clone(), sender);
        (engine, state)
    }

    #[tokio::test]
    async fn recall_sends_only_changed_params() {
        let (engine, state) = setup_test().await;

        // Set up live state: Input 1 fader at -10, mute off
        {
            let mut st = state.write().await;
            st.update(
                ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Fader },
                ParameterValue::Float(-10.0),
            );
            st.update(
                ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Mute },
                ParameterValue::Bool(false),
            );
        }

        // Snapshot: fader at 0 (different), mute still off (same)
        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope {
                channel: ChannelId::Input(1),
                sections: HashSet::from([ParameterSection::FaderMutePan]),
            }],
        );

        let mut values = HashMap::new();
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Fader },
            ParameterValue::Float(0.0), // different from live
        );
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Mute },
            ParameterValue::Bool(false), // same as live
        );

        let snapshot = Snapshot::new(
            "Test Snap".into(),
            scope.clone(),
            SnapshotData { values },
        );

        let result = engine.recall(&snapshot, &scope).await;

        // Fader was different → sent. Mute was same → skipped.
        assert_eq!(result.parameters_sent, 1);
        assert_eq!(result.parameters_skipped, 1);
    }

    #[tokio::test]
    async fn recall_skips_ipad_only() {
        let (engine, _state) = setup_test().await;

        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope {
                channel: ChannelId::Input(1),
                sections: HashSet::from([ParameterSection::Inserts]),
            }],
        );

        let mut values = HashMap::new();
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::InsertAEnabled },
            ParameterValue::Bool(true),
        );

        let snapshot = Snapshot::new(
            "Test".into(),
            scope.clone(),
            SnapshotData { values },
        );

        let result = engine.recall(&snapshot, &scope).await;

        // InsertAEnabled is iPad-only → skipped
        assert_eq!(result.parameters_sent, 0);
        assert_eq!(result.parameters_skipped, 1);
    }
}
