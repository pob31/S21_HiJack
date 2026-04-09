use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{info, warn, debug};
use uuid::Uuid;

use crate::console::fade_engine::{FadeController, FadeTarget};
use crate::model::dirty_tracker::DirtyTracker;
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
    fade_controller: FadeController,
    /// Phase C: optional dirty tracker. When present, recall() and
    /// recall_cue() bracket their writes with begin_suppression /
    /// end_suppression so console echoes from the recall don't pollute the
    /// dirty set, and clear() the tracker on a successful recall (mirroring
    /// `ParameterDirtyTracker::endSuppressionAndClear` from WFS-DIY).
    dirty_tracker: Option<Arc<RwLock<DirtyTracker>>>,
}

impl SnapshotEngine {
    pub fn new(state: Arc<RwLock<ConsoleState>>, sender: OscSender) -> Self {
        Self {
            state,
            sender,
            ipad_sender: None,
            fade_controller: FadeController::new(),
            dirty_tracker: None,
        }
    }

    /// Set (or clear) the iPad sender for iPad-only parameter recall.
    pub fn set_ipad_sender(&mut self, sender: Option<IpadSender>) {
        self.ipad_sender = sender;
    }

    /// Attach a dirty tracker (Phase C). Called once at engine construction.
    pub fn set_dirty_tracker(&mut self, dirty: Arc<RwLock<DirtyTracker>>) {
        self.dirty_tracker = Some(dirty);
    }

    /// Helper: bracket an async operation with begin/end suppression on the
    /// dirty tracker, if one is attached. Used internally by recall paths.
    /// Clears the dirty set on the way out so the operator's "what's
    /// changed since the last recall" view is correctly anchored to this
    /// recall.
    async fn with_dirty_suppression<F, R>(&self, body: F) -> R
    where
        F: std::future::Future<Output = R>,
    {
        if let Some(dirty) = &self.dirty_tracker {
            dirty.write().await.begin_suppression();
        }
        let result = body.await;
        if let Some(dirty) = &self.dirty_tracker {
            let mut t = dirty.write().await;
            t.end_suppression();
            t.clear();
        }
        result
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
    ///
    /// When `ignore_scope == true`, the scope filter is bypassed entirely and
    /// every parameter in the snapshot data (and every linked palette value)
    /// is recalled. Only meaningful for `SnapshotKind::ApplyOnRecall` snapshots
    /// — `ApplyOnSave` snapshots already filtered at capture time, so the
    /// stored data IS the scope.
    ///
    /// Phase C: brackets the entire body in begin_suppression / end_suppression
    /// on the attached dirty tracker (if any), and clears the dirty set on
    /// the way out so the operator's "what's changed since" view is anchored
    /// to this recall.
    pub async fn recall(
        &self,
        snapshot: &Snapshot,
        scope: &ScopeTemplate,
        palettes: &HashMap<Uuid, EqPalette>,
        ignore_scope: bool,
    ) -> RecallResult {
        self.with_dirty_suppression(self.recall_inner(snapshot, scope, palettes, ignore_scope))
            .await
    }

    /// Recall body without dirty-tracker suppression. Used by `recall` (which
    /// wraps it) and by `recall_cue`'s no-fade path (which is itself wrapped
    /// at the cue level so we don't double-suppress).
    async fn recall_inner(
        &self,
        snapshot: &Snapshot,
        scope: &ScopeTemplate,
        palettes: &HashMap<Uuid, EqPalette>,
        ignore_scope: bool,
    ) -> RecallResult {
        let state = self.state.read().await;
        let mut sent = 0usize;
        let mut skipped = 0usize;

        // Track which palette params were already handled via snapshot data
        let mut palette_params_seen: HashMap<(uuid::Uuid, _), bool> = HashMap::new();

        for (addr, snap_value) in &snapshot.data.values {
            // Only recall parameters within the effective scope (unless
            // ignore_scope is set, in which case every stored param is sent).
            if !ignore_scope && !scope.contains(addr) {
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
                    if !ignore_scope && !scope.contains(&addr) {
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
    ///
    /// When `cue.fade_time > 0`, continuous parameters are interpolated in a
    /// background task while discrete parameters fire immediately.
    ///
    /// When `ignore_scope == true`, the scope filter is bypassed entirely
    /// (see `recall` for details). Only meaningful for `ApplyOnRecall`
    /// snapshots.
    pub async fn recall_cue(
        &self,
        cue: &Cue,
        snapshot: &Snapshot,
        palettes: &HashMap<Uuid, EqPalette>,
        ignore_scope: bool,
    ) -> RecallResult {
        // Phase C: bracket the whole cue recall in dirty suppression so
        // BOTH the no-fade path (which calls recall_inner) AND the fade
        // path's discrete-target writes are suppressed and the dirty set
        // is cleared on success.
        self.with_dirty_suppression(self.recall_cue_inner(cue, snapshot, palettes, ignore_scope))
            .await
    }

    async fn recall_cue_inner(
        &self,
        cue: &Cue,
        snapshot: &Snapshot,
        palettes: &HashMap<Uuid, EqPalette>,
        ignore_scope: bool,
    ) -> RecallResult {
        let effective_scope = cue.scope_override.as_ref().unwrap_or(&snapshot.scope);
        info!(
            cue_number = cue.cue_number,
            cue_name = %cue.name,
            snapshot_name = %snapshot.name,
            fade_time = cue.fade_time,
            ignore_scope,
            "Recalling cue"
        );

        // No fade — instant recall (existing behavior). Call the inner
        // body so we don't double-suppress (the outer recall_cue is
        // already inside with_dirty_suppression).
        if cue.fade_time <= 0.0 {
            // Cancel any in-progress fade from a previous cue
            self.fade_controller.cancel_active().await;
            return self.recall_inner(snapshot, effective_scope, palettes, ignore_scope).await;
        }

        // Fade recall: split parameters into discrete (immediate) and continuous (fade)
        let state = self.state.read().await;
        let mut sent = 0usize;
        let mut skipped = 0usize;
        let mut fade_targets: Vec<FadeTarget> = Vec::new();

        // Track palette params handled via snapshot data
        let mut palette_params_seen: HashMap<(Uuid, _), bool> = HashMap::new();

        for (addr, snap_value) in &snapshot.data.values {
            if !ignore_scope && !effective_scope.contains(addr) {
                skipped += 1;
                continue;
            }

            // Resolve palette override for EQ params
            let effective_value = if addr.parameter.section() == ParameterSection::Eq {
                if let Some(palette_id) = snapshot.eq_palette_refs.get(&addr.channel) {
                    if let Some(palette) = palettes.get(palette_id) {
                        palette_params_seen.insert((*palette_id, addr.parameter.clone()), true);
                        palette.eq_values.get(&addr.parameter).unwrap_or(snap_value)
                    } else {
                        snap_value
                    }
                } else {
                    snap_value
                }
            } else {
                snap_value
            };

            let live_value = state.get(addr);

            if live_value == Some(effective_value) {
                skipped += 1;
            } else if addr.parameter.is_continuous() && live_value.is_some() {
                // Continuous param with known start → fade
                fade_targets.push(FadeTarget {
                    address: addr.clone(),
                    start_value: live_value.unwrap().clone(),
                    end_value: effective_value.clone(),
                });
            } else {
                // Discrete, or continuous with unknown live value → send immediately
                fade_targets.push(FadeTarget {
                    address: addr.clone(),
                    start_value: effective_value.clone(),
                    end_value: effective_value.clone(),
                });
            }
        }

        // Palette-only params
        for (channel, palette_id) in &snapshot.eq_palette_refs {
            if let Some(palette) = palettes.get(palette_id) {
                for (param_path, value) in &palette.eq_values {
                    if palette_params_seen.contains_key(&(*palette_id, param_path.clone())) {
                        continue;
                    }
                    let addr = ParameterAddress {
                        channel: channel.clone(),
                        parameter: param_path.clone(),
                    };
                    if !ignore_scope && !effective_scope.contains(&addr) {
                        skipped += 1;
                        continue;
                    }
                    let live_value = state.get(&addr);
                    if live_value == Some(value) {
                        skipped += 1;
                        continue;
                    }
                    if addr.parameter.is_continuous() && live_value.is_some() {
                        fade_targets.push(FadeTarget {
                            address: addr,
                            start_value: live_value.unwrap().clone(),
                            end_value: value.clone(),
                        });
                    } else {
                        fade_targets.push(FadeTarget {
                            address: addr,
                            start_value: value.clone(),
                            end_value: value.clone(),
                        });
                    }
                }
            }
        }

        drop(state);

        // Separate discrete targets (start == end) from continuous
        let mut discrete_targets = Vec::new();
        let mut continuous_targets = Vec::new();
        for target in fade_targets {
            if target.start_value == target.end_value {
                discrete_targets.push(target);
            } else {
                continuous_targets.push(target);
            }
        }

        // Send discrete params immediately
        for target in &discrete_targets {
            self.send_now(&target.address, &target.end_value, &mut sent, &mut skipped).await;
        }

        // Start continuous fade in background
        if !continuous_targets.is_empty() {
            let fade_count = continuous_targets.len();
            let _handle = self.fade_controller.start_fade(
                cue.cue_number,
                cue.fade_time,
                continuous_targets,
                self.sender.clone(),
                self.ipad_sender.clone(),
            ).await;
            info!(fade_count, "Fade started for continuous parameters");
        }

        RecallResult {
            parameters_sent: sent,
            parameters_skipped: skipped,
        }
    }

    /// Send a parameter immediately (no state comparison — already checked).
    async fn send_now(
        &self,
        addr: &ParameterAddress,
        value: &ParameterValue,
        sent: &mut usize,
        skipped: &mut usize,
    ) {
        match encode::encode_parameter(addr, value) {
            Some((path, args)) => {
                if let Err(e) = self.sender.send(&path, args).await {
                    warn!(%addr, "Failed to send recall: {e}");
                    *skipped += 1;
                } else {
                    debug!(%addr, %value, "Recall: sent discrete param");
                    *sent += 1;
                }
            }
            None => {
                if let Some(ipad) = &self.ipad_sender {
                    match ipad_encode::encode_ipad_parameter(addr, value) {
                        Some((path, args)) => {
                            if let Err(e) = ipad.send(&path, args).await {
                                warn!(%addr, "Failed to send iPad recall: {e}");
                                *skipped += 1;
                            } else {
                                debug!(%addr, %value, "Recall: sent discrete via iPad");
                                *sent += 1;
                            }
                        }
                        None => {
                            *skipped += 1;
                        }
                    }
                } else {
                    *skipped += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashSet};
    use crate::model::channel::ChannelId;
    use crate::model::config::ConsoleConfig;
    use crate::model::parameter::{ParameterPath, ParameterSection};
    use crate::model::snapshot::{ChannelScope, ScopeTemplate, Snapshot, SnapshotData, SnapshotKind};
    use crate::osc::client::OscClient;
    use std::net::SocketAddr;

    async fn setup_test() -> (SnapshotEngine, Arc<RwLock<ConsoleState>>) {
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let remote: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let client = OscClient::new(local, remote, None).await.unwrap();
        let (sender, _rx) = client.into_parts();

        let state = Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default())));
        let engine = SnapshotEngine::new(state.clone(), sender);
        (engine, state)
    }

    /// Phase C test helper: same as `setup_test` but with a dirty tracker
    /// attached to the engine.
    async fn setup_test_with_dirty() -> (
        SnapshotEngine,
        Arc<RwLock<ConsoleState>>,
        Arc<RwLock<DirtyTracker>>,
    ) {
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let remote: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let client = OscClient::new(local, remote, None).await.unwrap();
        let (sender, _rx) = client.into_parts();

        let state = Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default())));
        let dirty = Arc::new(RwLock::new(DirtyTracker::new()));
        let mut engine = SnapshotEngine::new(state.clone(), sender);
        engine.set_dirty_tracker(dirty.clone());
        (engine, state, dirty)
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
                paths: HashSet::new(),
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
            SnapshotKind::ApplyOnSave,
        );

        let result = engine.recall(&snapshot, &scope, &no_palettes(), false).await;
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
                paths: HashSet::new(),
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
            SnapshotKind::ApplyOnSave,
        );

        let result = engine.recall(&snapshot, &scope, &no_palettes(), false).await;
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
                paths: HashSet::new(),
                sections: HashSet::from([ParameterSection::Eq]),
            }],
        );

        // Snapshot has EQ gain = 2.0
        let mut values = HashMap::new();
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::EqBandGain(1) },
            ParameterValue::Float(2.0),
        );
        let mut snapshot = Snapshot::new("Snap".into(), scope.clone(), SnapshotData { values }, SnapshotKind::ApplyOnSave);

        // Palette has EQ gain = 5.0 (should override snapshot's 2.0)
        let mut eq_vals = HashMap::new();
        eq_vals.insert(ParameterPath::EqBandGain(1), ParameterValue::Float(5.0));
        let palette = EqPalette::new("Vocal EQ".into(), ChannelId::Input(1), eq_vals);
        let palette_id = palette.id;

        // Link palette to snapshot
        snapshot.eq_palette_refs.insert(ChannelId::Input(1), palette_id);

        let mut palettes = HashMap::new();
        palettes.insert(palette_id, palette);

        let result = engine.recall(&snapshot, &scope, &palettes, false).await;
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
                paths: HashSet::new(),
                sections: HashSet::from([ParameterSection::Eq]),
            }],
        );

        let mut values = HashMap::new();
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::EqBandGain(1) },
            ParameterValue::Float(3.0),
        );
        let mut snapshot = Snapshot::new("Snap".into(), scope.clone(), SnapshotData { values }, SnapshotKind::ApplyOnSave);

        // Reference a palette that doesn't exist
        snapshot.eq_palette_refs.insert(ChannelId::Input(1), Uuid::new_v4());

        let result = engine.recall(&snapshot, &scope, &no_palettes(), false).await;
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
                paths: HashSet::new(),
                sections: HashSet::from([ParameterSection::FaderMutePan, ParameterSection::Eq]),
            }],
        );

        let mut values = HashMap::new();
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Fader },
            ParameterValue::Float(-5.0),
        );
        let mut snapshot = Snapshot::new("Snap".into(), scope.clone(), SnapshotData { values }, SnapshotKind::ApplyOnSave);

        // Link a palette — should not affect the fader
        let eq_vals = HashMap::new();
        let palette = EqPalette::new("Empty".into(), ChannelId::Input(1), eq_vals);
        snapshot.eq_palette_refs.insert(ChannelId::Input(1), palette.id);

        let mut palettes = HashMap::new();
        palettes.insert(palette.id, palette);

        let result = engine.recall(&snapshot, &scope, &palettes, false).await;
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
            None,
        ).await.unwrap();
        let (ipad_sender, _ipad_rx) = ipad_client.into_parts();
        engine.set_ipad_sender(Some(ipad_sender));

        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope {
                channel: ChannelId::Input(1),
                paths: HashSet::new(),
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
            SnapshotKind::ApplyOnSave,
        );

        let result = engine.recall(&snapshot, &scope, &no_palettes(), false).await;
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
                paths: HashSet::new(),
                sections: HashSet::from([ParameterSection::Eq]),
            }],
        );

        // Snapshot has no EQ data at all
        let snapshot_values = HashMap::new();
        let mut snapshot = Snapshot::new("Snap".into(), scope.clone(), SnapshotData { values: snapshot_values }, SnapshotKind::ApplyOnSave);

        // Palette has EQ values that should be sent even though they're not in snapshot
        let mut eq_vals = HashMap::new();
        eq_vals.insert(ParameterPath::EqBandFrequency(1), ParameterValue::Float(800.0));
        eq_vals.insert(ParameterPath::EqBandGain(1), ParameterValue::Float(4.0));
        let palette = EqPalette::new("Test".into(), ChannelId::Input(1), eq_vals);
        let pid = palette.id;

        snapshot.eq_palette_refs.insert(ChannelId::Input(1), pid);

        let mut palettes = HashMap::new();
        palettes.insert(pid, palette);

        let result = engine.recall(&snapshot, &scope, &palettes, false).await;
        // Both palette-only params sent (live is None for both)
        assert_eq!(result.parameters_sent, 2);
    }

    #[tokio::test]
    async fn recall_cue_zero_fade_same_as_instant() {
        let (engine, _state) = setup_test().await;

        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope {
                channel: ChannelId::Input(1),
                paths: HashSet::new(),
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
            ParameterValue::Bool(true),
        );

        let snapshot = Snapshot::new("Snap".into(), scope.clone(), SnapshotData { values }, SnapshotKind::ApplyOnSave);
        let cue = crate::model::snapshot::Cue::new(1.0, "Test Cue".into(), snapshot.id);

        let result = engine.recall_cue(&cue, &snapshot, &no_palettes(), false).await;
        // Both are new (no live state) → both sent
        assert_eq!(result.parameters_sent, 2);
    }

    #[tokio::test]
    async fn recall_cue_with_fade_sends_discrete_immediately() {
        let (engine, state) = setup_test().await;

        // Set live state so continuous params have a known start value
        {
            let mut st = state.write().await;
            st.update(
                ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Fader },
                ParameterValue::Float(0.0),
            );
        }

        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope {
                channel: ChannelId::Input(1),
                paths: HashSet::new(),
                sections: HashSet::from([ParameterSection::FaderMutePan]),
            }],
        );

        let mut values = HashMap::new();
        // Fader = continuous with known live value → deferred to fade
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Fader },
            ParameterValue::Float(5.0),
        );
        // Mute = discrete → sent immediately
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Mute },
            ParameterValue::Bool(true),
        );

        let snapshot = Snapshot::new("Snap".into(), scope.clone(), SnapshotData { values }, SnapshotKind::ApplyOnSave);
        let mut cue = crate::model::snapshot::Cue::new(1.0, "Fade Cue".into(), snapshot.id);
        cue.fade_time = 2.0;

        let result = engine.recall_cue(&cue, &snapshot, &no_palettes(), false).await;
        // Mute sent immediately; Fader deferred to background fade
        assert_eq!(result.parameters_sent, 1);
        assert_eq!(result.parameters_skipped, 0);
    }

    #[tokio::test]
    async fn recall_cue_scope_override() {
        let (engine, _state) = setup_test().await;

        // Snapshot scope includes FaderMutePan + Eq
        let full_scope = ScopeTemplate::new(
            "Full".into(),
            vec![ChannelScope {
                channel: ChannelId::Input(1),
                paths: HashSet::new(),
                sections: HashSet::from([ParameterSection::FaderMutePan, ParameterSection::Eq]),
            }],
        );

        // Cue scope override: only Eq
        let eq_only_scope = ScopeTemplate::new(
            "EQ Only".into(),
            vec![ChannelScope {
                channel: ChannelId::Input(1),
                paths: HashSet::new(),
                sections: HashSet::from([ParameterSection::Eq]),
            }],
        );

        let mut values = HashMap::new();
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Fader },
            ParameterValue::Float(0.0),
        );
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::EqEnabled },
            ParameterValue::Bool(true),
        );

        let snapshot = Snapshot::new("Snap".into(), full_scope, SnapshotData { values }, SnapshotKind::ApplyOnSave);
        let mut cue = crate::model::snapshot::Cue::new(1.0, "Scoped".into(), snapshot.id);
        cue.scope_override = Some(eq_only_scope);

        let result = engine.recall_cue(&cue, &snapshot, &no_palettes(), false).await;
        // Only EqEnabled sent (within override scope); Fader skipped
        assert_eq!(result.parameters_sent, 1);
        assert_eq!(result.parameters_skipped, 1);
    }

    #[tokio::test]
    async fn recall_ignore_scope_sends_every_stored_param() {
        // Phase B: when ignore_scope=true, the scope filter is bypassed
        // entirely. Even params outside the snapshot's stored scope are sent.
        let (engine, _state) = setup_test().await;

        // Stored data covers Fader (in scope) + EqEnabled (out of scope).
        let mut values = HashMap::new();
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Fader },
            ParameterValue::Float(-10.0),
        );
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::EqEnabled },
            ParameterValue::Bool(true),
        );

        // Narrow scope: only FaderMutePan.
        let scope = ScopeTemplate::new(
            "Narrow".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::FaderMutePan]),
            )],
        );

        let snapshot = Snapshot::new(
            "Test".into(),
            scope.clone(),
            SnapshotData { values },
            SnapshotKind::ApplyOnRecall,
        );

        // Standard recall: only Fader is sent (EqEnabled is filtered out).
        let result = engine.recall(&snapshot, &scope, &no_palettes(), false).await;
        assert_eq!(result.parameters_sent, 1);
        assert_eq!(result.parameters_skipped, 1);

        // Recall with ignore_scope=true: BOTH params are sent.
        let result = engine.recall(&snapshot, &scope, &no_palettes(), true).await;
        assert_eq!(result.parameters_sent, 2);
        assert_eq!(result.parameters_skipped, 0);
    }

    #[tokio::test]
    async fn recall_cue_ignore_scope_bypasses_override() {
        // Phase B: ignore_scope on recall_cue also bypasses cue.scope_override.
        let (engine, _state) = setup_test().await;

        let mut values = HashMap::new();
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::Fader },
            ParameterValue::Float(-10.0),
        );
        values.insert(
            ParameterAddress { channel: ChannelId::Input(1), parameter: ParameterPath::EqEnabled },
            ParameterValue::Bool(true),
        );

        let full_scope = ScopeTemplate::new(
            "Full".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([
                    ParameterSection::FaderMutePan,
                    ParameterSection::Eq,
                ]),
            )],
        );
        let fader_only_scope = ScopeTemplate::new(
            "FaderOnly".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::FaderMutePan]),
            )],
        );

        let snapshot = Snapshot::new(
            "Test".into(),
            full_scope,
            SnapshotData { values },
            SnapshotKind::ApplyOnRecall,
        );
        let mut cue = crate::model::snapshot::Cue::new(1.0, "Cue".into(), snapshot.id);
        cue.scope_override = Some(fader_only_scope);

        // ignore_scope=false honours the override → only Fader sent.
        let result = engine.recall_cue(&cue, &snapshot, &no_palettes(), false).await;
        assert_eq!(result.parameters_sent, 1);
        assert_eq!(result.parameters_skipped, 1);

        // ignore_scope=true bypasses the override → both sent.
        let result = engine.recall_cue(&cue, &snapshot, &no_palettes(), true).await;
        assert_eq!(result.parameters_sent, 2);
    }

    // ─── Phase C: dirty tracker integration ─────────────────────────

    #[tokio::test]
    async fn recall_clears_dirty_tracker_on_success() {
        // Mark a few cells dirty as if the operator had wiggled some
        // parameters since the last recall. Then run a recall and verify
        // the dirty set is cleared on the way out.
        let (engine, _state, dirty) = setup_test_with_dirty().await;
        {
            let mut t = dirty.write().await;
            t.mark(&ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            });
            t.mark(&ParameterAddress {
                channel: ChannelId::Input(2),
                parameter: ParameterPath::Mute,
            });
            assert!(t.has_any());
        }

        // A trivial recall — empty snapshot, empty scope. The point is
        // exercising the with_dirty_suppression wrapper.
        let scope = ScopeTemplate::new("S".into(), vec![]);
        let snapshot = Snapshot::new(
            "Snap".into(),
            scope.clone(),
            SnapshotData::new(),
            SnapshotKind::ApplyOnSave,
        );
        let _ = engine.recall(&snapshot, &scope, &no_palettes(), false).await;

        // After the recall, the tracker should be empty.
        assert!(!dirty.read().await.has_any());
    }

    #[tokio::test]
    async fn recall_cue_clears_dirty_tracker_on_success() {
        let (engine, _state, dirty) = setup_test_with_dirty().await;
        dirty.write().await.mark(&ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        });

        let scope = ScopeTemplate::new("S".into(), vec![]);
        let snapshot = Snapshot::new(
            "Snap".into(),
            scope,
            SnapshotData::new(),
            SnapshotKind::ApplyOnSave,
        );
        let cue = crate::model::snapshot::Cue::new(1.0, "Cue".into(), snapshot.id);
        let _ = engine.recall_cue(&cue, &snapshot, &no_palettes(), false).await;

        assert!(!dirty.read().await.has_any());
    }
}
