use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{info, warn, debug};
use uuid::Uuid;

use crate::model::eq_palette::EqPalette;
use crate::model::parameter::{ParameterAddress, ParameterSection, ParameterValue};
use crate::model::snapshot::{Cue, ScopeTemplate, Snapshot};
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::encode;
use crate::osc::ipad_client::IpadSender;
use crate::osc::ipad_encode;

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
    ipad_sender: Option<IpadSender>,
}

impl SnapshotEngine {
    pub fn new(state: Arc<RwLock<ConsoleState>>, sender: OscSender) -> Self {
        Self { state, sender, ipad_sender: None }
    }

    /// Set (or clear) the iPad sender for iPad-only parameter recall.
    pub fn set_ipad_sender(&mut self, sender: Option<IpadSender>) {
        self.ipad_sender = sender;
    }

    /// Recall a snapshot using the given scope.
    ///
    /// For each parameter in the snapshot data that's within the effective scope:
    /// 1. If EQ-section and channel has a palette ref, use palette value instead
    /// 2. Compare against the live state mirror
    /// 3. If different (or not present in live state), send via GP OSC
    /// 4. If GP OSC encoding returns None, fall back to iPad protocol (if sender available)
    ///
    /// After processing snapshot data, also sends palette-only values (params in
    /// palette but not in snapshot) for linked channels within scope.
    pub async fn recall(
        &self,
        snapshot: &Snapshot,
        scope: &ScopeTemplate,
        palettes: &HashMap<Uuid, EqPalette>,
    ) -> RecallResult {
        let state = self.state.read().await;
        let mut sent = 0usize;
        let mut skipped = 0usize;

        // Track which palette params were already handled via snapshot data
        let mut palette_params_seen: HashMap<(uuid::Uuid, _), bool> = HashMap::new();

        for (addr, snap_value) in &snapshot.data.values {
            // Only recall parameters within the effective scope
            if !scope.contains(addr) {
                skipped += 1;
                continue;
            }

            // Determine the effective value: check for palette override on EQ params
            let effective_value = if addr.parameter.section() == ParameterSection::Eq {
                if let Some(palette_id) = snapshot.eq_palette_refs.get(&addr.channel) {
                    if let Some(palette) = palettes.get(palette_id) {
                        // Mark this palette param as seen
                        palette_params_seen.insert((*palette_id, addr.parameter.clone()), true);
                        // Use palette value if present, else fall back to snapshot
                        palette.eq_values.get(&addr.parameter).unwrap_or(snap_value)
                    } else {
                        warn!(%addr, %palette_id, "Palette not found, using snapshot value");
                        snap_value
                    }
                } else {
                    snap_value
                }
            } else {
                snap_value
            };

            self.send_if_changed(&state, addr, effective_value, &mut sent, &mut skipped).await;
        }

        // Send palette-only values: params in palette but not in snapshot data
        for (channel, palette_id) in &snapshot.eq_palette_refs {
            if let Some(palette) = palettes.get(palette_id) {
                for (param_path, value) in &palette.eq_values {
                    if palette_params_seen.contains_key(&(*palette_id, param_path.clone())) {
                        continue; // Already handled above
                    }
                    let addr = ParameterAddress {
                        channel: channel.clone(),
                        parameter: param_path.clone(),
                    };
                    if !scope.contains(&addr) {
                        skipped += 1;
                        continue;
                    }
                    self.send_if_changed(&state, &addr, value, &mut sent, &mut skipped).await;
                }
            }
        }

        info!(sent, skipped, "Snapshot recall complete");
        RecallResult {
            parameters_sent: sent,
            parameters_skipped: skipped,
        }
    }

    /// Send a parameter if it differs from the live state.
    async fn send_if_changed(
        &self,
        state: &ConsoleState,
        addr: &ParameterAddress,
        value: &ParameterValue,
        sent: &mut usize,
        skipped: &mut usize,
    ) {
        // Check if the value differs from live state
        let live_value = state.get(addr);
        if live_value == Some(value) {
            *skipped += 1;
            debug!(%addr, "Recall skip: value unchanged");
            return;
        }

        // Encode to GP OSC
        match encode::encode_parameter(addr, value) {
            Some((path, args)) => {
                if let Err(e) = self.sender.send(&path, args).await {
                    warn!(%addr, "Failed to send recall: {e}");
                    *skipped += 1;
                } else {
                    debug!(%addr, %value, "Recall: sent parameter");
                    *sent += 1;
                }
            }
            None => {
                // Try iPad protocol as fallback
                if let Some(ref ipad) = self.ipad_sender {
                    match ipad_encode::encode_ipad_parameter(addr, value) {
                        Some((path, args)) => {
                            if let Err(e) = ipad.send(&path, args).await {
                                warn!(%addr, "Failed to send iPad recall: {e}");
                                *skipped += 1;
                            } else {
                                debug!(%addr, %value, "Recall: sent via iPad protocol");
                                *sent += 1;
                            }
                        }
                        None => {
                            *skipped += 1;
                            debug!(%addr, "Recall skip: no encoding available");
                        }
                    }
                } else {
                    *skipped += 1;
                    debug!(%addr, "Recall skip: iPad-only parameter (no iPad sender)");
                }
            }
        }
    }

    /// Recall a cue — resolves effective scope and delegates to recall().
    pub async fn recall_cue(
        &self,
        cue: &Cue,
        snapshot: &Snapshot,
        palettes: &HashMap<Uuid, EqPalette>,
    ) -> RecallResult {
        let effective_scope = cue.scope_override.as_ref().unwrap_or(&snapshot.scope);
        info!(
            cue_number = cue.cue_number,
            cue_name = %cue.name,
            snapshot_name = %snapshot.name,
            "Recalling cue"
        );
        self.recall(snapshot, effective_scope, palettes).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashSet};
    use crate::model::channel::ChannelId;
    use crate::model::config::ConsoleConfig;
    use crate::model::parameter::{ParameterPath, ParameterSection};
    use crate::model::snapshot::{ChannelScope, ScopeTemplate, Snapshot, SnapshotData};
    use crate::osc::client::OscClient;
    use std::net::SocketAddr;

    async fn setup_test() -> (SnapshotEngine, Arc<RwLock<ConsoleState>>) {
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let remote: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let client = OscClient::new(local, remote).await.unwrap();
        let (sender, _rx) = client.into_parts();

        let state = Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default())));
        let engine = SnapshotEngine::new(state.clone(), sender);
        (engine, state)
    }

    fn no_palettes() -> HashMap<Uuid, EqPalette> {
        HashMap::new()
    }

    #[tokio::test]
    async fn recall_sends_only_changed_params() {
        let (engine, state) = setup_test().await;

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
            ParameterValue::Float(0.0),
        );
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Mute },
            ParameterValue::Bool(false),
        );

        let snapshot = Snapshot::new(
            "Test Snap".into(),
            scope.clone(),
            SnapshotData { values },
        );

        let result = engine.recall(&snapshot, &scope, &no_palettes()).await;
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

        let result = engine.recall(&snapshot, &scope, &no_palettes()).await;
        assert_eq!(result.parameters_sent, 0);
        assert_eq!(result.parameters_skipped, 1);
    }

    #[tokio::test]
    async fn recall_with_palette_uses_palette_eq_values() {
        let (engine, state) = setup_test().await;

        // Live state has old EQ values
        {
            let mut st = state.write().await;
            st.update(
                ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::EqBandGain(1) },
                ParameterValue::Float(0.0),
            );
        }

        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope {
                channel: ChannelId::Input(1),
                sections: HashSet::from([ParameterSection::Eq]),
            }],
        );

        // Snapshot has EQ gain = 2.0
        let mut values = HashMap::new();
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::EqBandGain(1) },
            ParameterValue::Float(2.0),
        );
        let mut snapshot = Snapshot::new("Snap".into(), scope.clone(), SnapshotData { values });

        // Palette has EQ gain = 5.0 (should override snapshot's 2.0)
        let mut eq_vals = HashMap::new();
        eq_vals.insert(ParameterPath::EqBandGain(1), ParameterValue::Float(5.0));
        let palette = EqPalette::new("Vocal EQ".into(), ChannelId::Input(1), eq_vals);
        let palette_id = palette.id;

        // Link palette to snapshot
        snapshot.eq_palette_refs.insert(ChannelId::Input(1), palette_id);

        let mut palettes = HashMap::new();
        palettes.insert(palette_id, palette);

        let result = engine.recall(&snapshot, &scope, &palettes).await;
        // Palette value (5.0) differs from live (0.0) → sent
        assert_eq!(result.parameters_sent, 1);
    }

    #[tokio::test]
    async fn recall_with_missing_palette_falls_back() {
        let (engine, _state) = setup_test().await;

        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope {
                channel: ChannelId::Input(1),
                sections: HashSet::from([ParameterSection::Eq]),
            }],
        );

        let mut values = HashMap::new();
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::EqBandGain(1) },
            ParameterValue::Float(3.0),
        );
        let mut snapshot = Snapshot::new("Snap".into(), scope.clone(), SnapshotData { values });

        // Reference a palette that doesn't exist
        snapshot.eq_palette_refs.insert(ChannelId::Input(1), Uuid::new_v4());

        let result = engine.recall(&snapshot, &scope, &no_palettes()).await;
        // Falls back to snapshot value (3.0), live is None → sent
        assert_eq!(result.parameters_sent, 1);
    }

    #[tokio::test]
    async fn non_eq_params_unaffected_by_palette() {
        let (engine, _state) = setup_test().await;

        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope {
                channel: ChannelId::Input(1),
                sections: HashSet::from([ParameterSection::FaderMutePan, ParameterSection::Eq]),
            }],
        );

        let mut values = HashMap::new();
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Fader },
            ParameterValue::Float(-5.0),
        );
        let mut snapshot = Snapshot::new("Snap".into(), scope.clone(), SnapshotData { values });

        // Link a palette — should not affect the fader
        let eq_vals = HashMap::new();
        let palette = EqPalette::new("Empty".into(), ChannelId::Input(1), eq_vals);
        snapshot.eq_palette_refs.insert(ChannelId::Input(1), palette.id);

        let mut palettes = HashMap::new();
        palettes.insert(palette.id, palette);

        let result = engine.recall(&snapshot, &scope, &palettes).await;
        // Fader is non-EQ → uses snapshot value (-5.0), live None → sent
        assert_eq!(result.parameters_sent, 1);
    }

    #[tokio::test]
    async fn recall_ipad_only_with_ipad_sender() {
        // When an iPad sender is available, iPad-only params should be sent
        let (mut engine, _state) = setup_test().await;

        // Create an iPad sender (pointing at a dummy socket)
        let ipad_client = crate::osc::ipad_client::IpadClient::new(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        ).await.unwrap();
        let (ipad_sender, _ipad_rx) = ipad_client.into_parts();
        engine.set_ipad_sender(Some(ipad_sender));

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

        let result = engine.recall(&snapshot, &scope, &no_palettes()).await;
        // With iPad sender: InsertAEnabled is iPad-only but should now be sent
        assert_eq!(result.parameters_sent, 1);
        assert_eq!(result.parameters_skipped, 0);
    }

    #[tokio::test]
    async fn palette_only_params_are_sent() {
        let (engine, _state) = setup_test().await;

        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope {
                channel: ChannelId::Input(1),
                sections: HashSet::from([ParameterSection::Eq]),
            }],
        );

        // Snapshot has no EQ data at all
        let snapshot_values = HashMap::new();
        let mut snapshot = Snapshot::new("Snap".into(), scope.clone(), SnapshotData { values: snapshot_values });

        // Palette has EQ values that should be sent even though they're not in snapshot
        let mut eq_vals = HashMap::new();
        eq_vals.insert(ParameterPath::EqBandFrequency(1), ParameterValue::Float(800.0));
        eq_vals.insert(ParameterPath::EqBandGain(1), ParameterValue::Float(4.0));
        let palette = EqPalette::new("Test".into(), ChannelId::Input(1), eq_vals);
        let pid = palette.id;

        snapshot.eq_palette_refs.insert(ChannelId::Input(1), pid);

        let mut palettes = HashMap::new();
        palettes.insert(pid, palette);

        let result = engine.recall(&snapshot, &scope, &palettes).await;
        // Both palette-only params sent (live is None for both)
        assert_eq!(result.parameters_sent, 2);
    }
}
