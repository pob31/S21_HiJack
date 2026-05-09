use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use rosc::OscType;
use tokio::sync::RwLock;
use tokio::time::Duration;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::console::cue_manager::CueManager;
use crate::console::fade_engine::{FadeController, FadeTarget, MultiFadeController};
use crate::model::channel::ChannelId;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::palette::ChannelPalette;
use crate::model::parameter::{ParameterAddress, ParameterValue, TimingCategory};
use crate::model::snapshot::{Cue, ScopeTemplate, Snapshot};
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::encode;
use crate::osc::ipad_client::IpadSender;
use crate::osc::ipad_encode;

/// Settling delay between firing a console memory and writing the app's
/// parameter overlay on top. Gives the console time to flood its own
/// parameter echoes from the memory load before our writes start to win.
const CONSOLE_MEMORY_SETTLE_MS: u64 = 250;

/// How long a `console_snapshot_fire_suppression` entry is valid. After
/// this window, an inbound `/Snapshots/Current_Snapshot` echo for the
/// same row is no longer assumed to be ours.
pub const CONSOLE_FIRE_SUPPRESSION_MS: u128 = 2_000;

/// Map from "console memory row we just fired" → when we fired it. Used
/// by follow mode to ignore echoes from our own writes. Shared between
/// the snapshot engine and the follow-mode dispatcher.
pub type ConsoleFireSuppression = Arc<RwLock<HashMap<i32, Instant>>>;

/// Result of a snapshot recall operation.
#[derive(Debug)]
pub struct RecallResult {
    /// Number of parameters sent to the console.
    pub parameters_sent: usize,
    /// Number of parameters skipped (no change, iPad-only, etc.).
    pub parameters_skipped: usize,
}

/// Pre-recall live values for one-level undo.
pub struct UndoState {
    /// Live values of parameters that were changed, captured before sending.
    pub previous_values: HashMap<ParameterAddress, ParameterValue>,
    /// Human-readable label (e.g. "Undo: 'Snapshot Name'").
    pub label: String,
}

/// The snapshot recall engine — diffs snapshot data against live state and sends changes.
pub struct SnapshotEngine {
    state: Arc<RwLock<ConsoleState>>,
    sender: OscSender,
    ipad_sender: Option<IpadSender>,
    fade_controller: FadeController,
    multi_fade: MultiFadeController,
    /// Phase C: optional dirty tracker. When present, recall() and
    /// recall_cue() bracket their writes with begin_suppression /
    /// end_suppression so console echoes from the recall don't pollute the
    /// dirty set, and clear() the tracker on a successful recall (mirroring
    /// `ParameterDirtyTracker::endSuppressionAndClear` from WFS-DIY).
    dirty_tracker: Option<Arc<RwLock<DirtyTracker>>>,
    /// Inter-message pacing delay in microseconds. 0 = no pacing.
    /// Prevents flooding the console's ARM chip during large recalls.
    /// Shared with `MacroEngine` and the Advanced Settings UI via the
    /// `HiJackApp::send_pace_us` handle so all three see one value.
    pace_us: Arc<AtomicU64>,
    /// One-level undo: stores the pre-recall live values so the operator
    /// can revert if a recall was triggered by mistake.
    undo: RwLock<Option<UndoState>>,
    /// Optional cue manager handle. When set, the engine can:
    /// - look up the previous app snapshot for auto-update-on-recall
    /// - mark the recalled snapshot as the current one via `set_last_recalled`
    cue_manager: Option<Arc<RwLock<CueManager>>>,
    /// Optional auto-update flag. When set and `true`, recall() merges
    /// the current dirty set into the previously-recalled snapshot
    /// (filtered by that snapshot's scope template) before firing the new
    /// recall.
    auto_update_on_recall: Option<Arc<AtomicBool>>,
    /// Suppression set for console memory fires. Shared with the
    /// follow-mode dispatcher so it can ignore echoes from our own
    /// `/Snapshots/Current_Snapshot` writes.
    console_fire_suppression: ConsoleFireSuppression,
}

impl SnapshotEngine {
    pub fn new(
        state: Arc<RwLock<ConsoleState>>,
        sender: OscSender,
        pace_us: Arc<AtomicU64>,
    ) -> Self {
        Self {
            state,
            sender,
            ipad_sender: None,
            fade_controller: FadeController::new(),
            multi_fade: MultiFadeController::new(),
            dirty_tracker: None,
            pace_us,
            undo: RwLock::new(None),
            cue_manager: None,
            auto_update_on_recall: None,
            console_fire_suppression: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Attach a cue manager handle so the engine can read/write the
    /// "last recalled snapshot" pointer used by auto-update-on-recall.
    pub fn set_cue_manager(&mut self, cue_manager: Arc<RwLock<CueManager>>) {
        self.cue_manager = Some(cue_manager);
    }

    /// Attach the auto-update-on-recall toggle.
    pub fn set_auto_update_flag(&mut self, flag: Arc<AtomicBool>) {
        self.auto_update_on_recall = Some(flag);
    }

    /// Shared handle on the console-memory-fire suppression map. The
    /// follow-mode dispatcher consults this to ignore echoes that came
    /// from our own writes.
    pub fn console_fire_suppression(&self) -> ConsoleFireSuppression {
        self.console_fire_suppression.clone()
    }

    /// Set (or clear) the iPad sender for iPad-only parameter recall.
    pub fn set_ipad_sender(&mut self, sender: Option<IpadSender>) {
        self.ipad_sender = sender;
    }

    /// Attach a dirty tracker (Phase C). Called once at engine construction.
    pub fn set_dirty_tracker(&mut self, dirty: Arc<RwLock<DirtyTracker>>) {
        self.dirty_tracker = Some(dirty);
    }

    /// Set the inter-message pacing delay (microseconds). 0 = no pacing.
    pub fn set_pace_us(&self, us: u64) {
        self.pace_us.store(us, Ordering::Relaxed);
    }

    /// Get the current pacing delay in microseconds.
    pub fn pace_us(&self) -> u64 {
        self.pace_us.load(Ordering::Relaxed)
    }

    /// Auto-save the current dirty parameters into the previously-recalled
    /// snapshot, filtered by that snapshot's scope template. Only runs
    /// when the auto_update flag is on, the cue manager is attached, and
    /// there's a previous snapshot distinct from the new one. MUST be
    /// called BEFORE entering the dirty-suppression bracket — otherwise
    /// the dirty set will already have been cleared.
    async fn auto_save_previous_snapshot(&self, new_snapshot_id: Uuid) -> usize {
        let auto_on = self
            .auto_update_on_recall
            .as_ref()
            .map(|f| f.load(Ordering::Relaxed))
            .unwrap_or(false);
        if !auto_on {
            return 0;
        }
        let Some(cue_arc) = self.cue_manager.as_ref() else {
            return 0;
        };
        let Some(dirty_arc) = self.dirty_tracker.as_ref() else {
            return 0;
        };

        // Snapshot the previous-recalled UUID without holding a write lock.
        let prev_id = {
            let mgr = cue_arc.read().await;
            mgr.last_recalled()
        };
        let Some(prev_id) = prev_id else {
            return 0;
        };
        if prev_id == new_snapshot_id {
            return 0;
        }

        // Snapshot the dirty set into a Vec we can drop the read lock for.
        let dirty_pairs: Vec<(ChannelId, crate::model::parameter::ParameterPath)> = {
            let dirty = dirty_arc.read().await;
            dirty
                .dirty_set()
                .iter()
                .flat_map(|(ch, paths)| paths.iter().map(move |p| (ch.clone(), p.clone())))
                .collect()
        };
        if dirty_pairs.is_empty() {
            return 0;
        }

        let state = self.state.read().await;
        let mut mgr = cue_arc.write().await;
        let Some(prev_snap) = mgr.snapshots.get_mut(&prev_id) else {
            return 0;
        };
        let mut count = 0usize;
        for (channel, parameter) in dirty_pairs {
            let addr = ParameterAddress { channel, parameter };
            if !prev_snap.scope.contains(&addr) {
                continue;
            }
            let Some(live) = state.get(&addr).cloned() else {
                continue;
            };
            prev_snap.data.values.insert(addr, live);
            count += 1;
        }
        if count > 0 {
            prev_snap.modified_at = chrono::Utc::now();
            info!(
                snapshot = %prev_snap.name,
                count,
                "Auto-saved dirty params into previous snapshot"
            );
        }
        count
    }

    /// Fire the console memory referenced by the snapshot, if any. Skips
    /// when the live `current_console_snapshot` already matches (dedup),
    /// and waits for a settling period after firing so the console's
    /// echo flood lands before we start writing the parameter overlay.
    /// Records the fired row in the suppression set so follow mode
    /// ignores the resulting echo.
    async fn fire_console_memory_if_needed(&self, snapshot: &Snapshot) {
        let Some(row) = snapshot.console_snapshot else {
            return;
        };
        // Dedup against live state.
        let already = {
            let s = self.state.read().await;
            s.current_console_snapshot == Some(row)
        };
        if already {
            debug!(row, "Console memory already active — skipping fire");
            return;
        }

        let Some(ipad) = self.ipad_sender.as_ref() else {
            warn!(
                row,
                "Snapshot has console memory ref but no iPad sender — \
                 fire skipped (Mode 1?). Parameter overlay will still recall."
            );
            return;
        };

        // Mark suppression BEFORE sending so the echo dispatcher sees it.
        {
            let mut sup = self.console_fire_suppression.write().await;
            sup.insert(row, Instant::now());
        }
        info!(row, "Firing console memory");
        if let Err(e) = ipad
            .send("/Snapshots/Current_Snapshot", vec![OscType::Int(row)])
            .await
        {
            warn!(row, "Failed to fire console memory: {e}");
            return;
        }

        // Optimistically reflect into our own state mirror so a quick
        // re-recall hits the dedup path immediately even before the
        // console echoes back.
        self.state.write().await.current_console_snapshot = Some(row);

        // Wait for the console to settle after the memory load.
        tokio::time::sleep(Duration::from_millis(CONSOLE_MEMORY_SETTLE_MS)).await;
    }

    /// Update the cue manager's "last recalled" pointer after a successful
    /// recall. No-op if the cue manager isn't attached.
    async fn mark_last_recalled(&self, snapshot_id: Uuid) {
        if let Some(cue_arc) = self.cue_manager.as_ref() {
            cue_arc.write().await.set_last_recalled(snapshot_id);
        }
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
    /// 1. If the parameter's section has a `PaletteKind` AND the snapshot
    ///    has a palette ref for `(channel, kind)`, use the palette's value
    /// 2. Compare against the live state mirror
    /// 3. If different (or not present in live state), send via GP OSC
    /// 4. If GP OSC encoding returns None, fall back to iPad protocol (if sender available)
    ///
    /// After processing snapshot data, also sends palette-only values (params in
    /// palettes but not in snapshot data) for every linked palette within scope.
    /// Generalized in Phase 5/6: works for EQ, Compressor (Dyn1), and Gate (Dyn2)
    /// palettes uniformly via the `(ChannelId, PaletteKind)` ref map.
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
        palettes: &HashMap<Uuid, ChannelPalette>,
        ignore_scope: bool,
    ) -> RecallResult {
        // Auto-update: must run BEFORE dirty suppression bracket clears
        // the tracker. Filtered by the previous snapshot's scope template.
        self.auto_save_previous_snapshot(snapshot.id).await;

        let result = self
            .with_dirty_suppression(async {
                // Fire console memory inside the suppression bracket so
                // its echo flood doesn't pollute the dirty tracker.
                self.fire_console_memory_if_needed(snapshot).await;
                self.recall_inner(snapshot, scope, palettes, ignore_scope)
                    .await
            })
            .await;

        self.mark_last_recalled(snapshot.id).await;
        result
    }

    /// Recall body without dirty-tracker suppression. Used by `recall` (which
    /// wraps it) and by `recall_cue`'s no-fade path (which is itself wrapped
    /// at the cue level so we don't double-suppress).
    async fn recall_inner(
        &self,
        snapshot: &Snapshot,
        scope: &ScopeTemplate,
        palettes: &HashMap<Uuid, ChannelPalette>,
        ignore_scope: bool,
    ) -> RecallResult {
        let state = self.state.read().await;
        let mut sent = 0usize;
        let mut skipped = 0usize;

        // Resolve all candidates (ignore_scope=true) so we can count
        // scope-filtered params as skipped, matching the old behaviour.
        let all = crate::model::snapshot::resolve_recall_values(snapshot, scope, palettes, true);
        let resolved =
            crate::model::snapshot::resolve_recall_values(snapshot, scope, palettes, ignore_scope);
        skipped += all.len() - resolved.len();

        let mut undo_map: HashMap<ParameterAddress, ParameterValue> = HashMap::new();
        let pace = self.pace_us.load(Ordering::Relaxed);
        for (addr, effective_value) in &resolved {
            let did_send = self
                .send_if_changed(
                    &state,
                    addr,
                    effective_value,
                    &mut sent,
                    &mut skipped,
                    Some(&mut undo_map),
                )
                .await;
            if did_send && pace > 0 {
                tokio::time::sleep(Duration::from_micros(pace)).await;
            }
        }

        // Store undo state (overwrites any previous — one level only)
        if !undo_map.is_empty() {
            *self.undo.write().await = Some(UndoState {
                previous_values: undo_map,
                label: format!("Undo '{}'", snapshot.name),
            });
        }

        info!(sent, skipped, "Snapshot recall complete");
        RecallResult {
            parameters_sent: sent,
            parameters_skipped: skipped,
        }
    }

    /// Send a parameter if it differs from the live state.
    /// Returns `true` if the parameter was actually sent.
    /// When `undo_map` is provided, captures the live value before sending
    /// so the recall can be reverted.
    async fn send_if_changed(
        &self,
        state: &ConsoleState,
        addr: &ParameterAddress,
        value: &ParameterValue,
        sent: &mut usize,
        skipped: &mut usize,
        undo_map: Option<&mut HashMap<ParameterAddress, ParameterValue>>,
    ) -> bool {
        // Check if the value differs from live state
        let live_value = state.get(addr);
        if live_value == Some(value) {
            *skipped += 1;
            debug!(%addr, "Recall skip: value unchanged");
            return false;
        }

        // Capture live value for undo before we change it
        if let (Some(undo), Some(live)) = (undo_map, live_value) {
            undo.insert(addr.clone(), live.clone());
        }

        // Encode to GP OSC
        match encode::encode_parameter(addr, value) {
            Some((path, args)) => {
                if let Err(e) = self.sender.send(&path, args).await {
                    warn!(%addr, "Failed to send recall: {e}");
                    *skipped += 1;
                    false
                } else {
                    debug!(%addr, %value, "Recall: sent parameter");
                    *sent += 1;
                    true
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
                                false
                            } else {
                                debug!(%addr, %value, "Recall: sent via iPad protocol");
                                *sent += 1;
                                true
                            }
                        }
                        None => {
                            *skipped += 1;
                            debug!(%addr, "Recall skip: no encoding available");
                            false
                        }
                    }
                } else {
                    *skipped += 1;
                    debug!(%addr, "Recall skip: iPad-only parameter (no iPad sender)");
                    false
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
        palettes: &HashMap<Uuid, ChannelPalette>,
        ignore_scope: bool,
    ) -> RecallResult {
        // Auto-update: must run BEFORE dirty suppression bracket clears
        // the tracker. Filtered by the previous snapshot's scope template.
        self.auto_save_previous_snapshot(snapshot.id).await;

        // Phase C: bracket the whole cue recall in dirty suppression so
        // BOTH the no-fade path (which calls recall_inner) AND the fade
        // path's discrete-target writes are suppressed and the dirty set
        // is cleared on success.
        let result = self
            .with_dirty_suppression(async {
                // Fire console memory inside the suppression bracket so
                // its echo flood doesn't pollute the dirty tracker.
                self.fire_console_memory_if_needed(snapshot).await;
                self.recall_cue_inner(cue, snapshot, palettes, ignore_scope)
                    .await
            })
            .await;

        self.mark_last_recalled(snapshot.id).await;
        result
    }

    async fn recall_cue_inner(
        &self,
        cue: &Cue,
        snapshot: &Snapshot,
        palettes: &HashMap<Uuid, ChannelPalette>,
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

        // Per-category timed recall: if the scope has any category timings,
        // use the new orchestrated path with mute ordering and per-group fades.
        if effective_scope.has_any_category_timing() {
            return self
                .recall_cue_timed(snapshot, effective_scope, palettes, ignore_scope)
                .await;
        }

        // No fade — instant recall (existing behavior). Call the inner
        // body so we don't double-suppress (the outer recall_cue is
        // already inside with_dirty_suppression).
        if cue.fade_time <= 0.0 {
            // Cancel any in-progress fade from a previous cue
            self.fade_controller.cancel_active().await;
            return self
                .recall_inner(snapshot, effective_scope, palettes, ignore_scope)
                .await;
        }

        // Fade recall: split parameters into discrete (immediate) and continuous (fade)
        let state = self.state.read().await;
        let mut sent = 0usize;
        let mut skipped = 0usize;
        let mut fade_targets: Vec<FadeTarget> = Vec::new();
        let mut undo_map: HashMap<ParameterAddress, ParameterValue> = HashMap::new();

        // Track palette params handled via snapshot data
        let mut palette_params_seen: HashMap<(Uuid, ParameterAddress), bool> = HashMap::new();

        for (addr, snap_value) in &snapshot.data.values {
            if !ignore_scope && !effective_scope.contains(addr) {
                skipped += 1;
                continue;
            }

            // Resolve palette override for any palette-eligible section
            // (EQ, Dyn1, Dyn2). Generalized in Phase 5/6.
            let effective_value = if let Some(palette_id) = snapshot.palette_ref_for(addr) {
                if let Some(palette) = palettes.get(&palette_id) {
                    palette_params_seen.insert((palette_id, addr.clone()), true);
                    palette.values.get(&addr.parameter).unwrap_or(snap_value)
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
                // Capture live value for undo before changing it
                undo_map.insert(addr.clone(), live_value.unwrap().clone());
                // Continuous param with known start → fade
                fade_targets.push(FadeTarget {
                    address: addr.clone(),
                    start_value: live_value.unwrap().clone(),
                    end_value: effective_value.clone(),
                });
            } else {
                // Capture live value for undo (if present)
                if let Some(live) = live_value {
                    undo_map.insert(addr.clone(), live.clone());
                }
                // Discrete, or continuous with unknown live value → send immediately
                fade_targets.push(FadeTarget {
                    address: addr.clone(),
                    start_value: effective_value.clone(),
                    end_value: effective_value.clone(),
                });
            }
        }

        // Palette-only params: walk the unified palette_refs map and send
        // any values that weren't already handled via snapshot data above.
        for ((channel, kind), palette_id) in &snapshot.palette_refs {
            let Some(palette) = palettes.get(palette_id) else {
                continue;
            };
            // Defensive: skip if palette kind doesn't match the ref's kind
            // (shouldn't happen, but better to drop than send to wrong section).
            if palette.kind != *kind {
                continue;
            }
            for (param_path, value) in &palette.values {
                let addr = ParameterAddress {
                    channel: channel.clone(),
                    parameter: param_path.clone(),
                };
                if palette_params_seen.contains_key(&(*palette_id, addr.clone())) {
                    continue;
                }
                if !ignore_scope && !effective_scope.contains(&addr) {
                    skipped += 1;
                    continue;
                }
                let live_value = state.get(&addr);
                if live_value == Some(value) {
                    skipped += 1;
                    continue;
                }
                // Capture live value for undo. `Option<&ParameterValue>` is
                // `Copy` (the inner is a reference), so both `if let` arms
                // consume independently without needing to clone or borrow.
                if let Some(live) = live_value {
                    undo_map.insert(addr.clone(), live.clone());
                }
                if let Some(live) = live_value
                    && addr.parameter.is_continuous()
                {
                    fade_targets.push(FadeTarget {
                        address: addr,
                        start_value: live.clone(),
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

        // Send discrete params immediately (with pacing)
        let pace = self.pace_us.load(Ordering::Relaxed);
        for target in &discrete_targets {
            let did_send = self
                .send_now(&target.address, &target.end_value, &mut sent, &mut skipped)
                .await;
            if did_send && pace > 0 {
                tokio::time::sleep(Duration::from_micros(pace)).await;
            }
        }

        // Start continuous fade in background
        if !continuous_targets.is_empty() {
            let fade_count = continuous_targets.len();
            let _handle = self
                .fade_controller
                .start_fade(
                    cue.cue_number,
                    cue.fade_time,
                    continuous_targets,
                    self.sender.clone(),
                    self.ipad_sender.clone(),
                )
                .await;
            info!(fade_count, "Fade started for continuous parameters");
        }

        // Store undo state
        if !undo_map.is_empty() {
            *self.undo.write().await = Some(UndoState {
                previous_values: undo_map,
                label: format!("Undo '{}'", snapshot.name),
            });
        }

        RecallResult {
            parameters_sent: sent,
            parameters_skipped: skipped,
        }
    }

    /// Per-category timed recall with mute ordering and send enable ordering.
    ///
    /// Groups resolved parameters by `(ChannelId, TimingCategory)`, applies
    /// per-group pre-wait and fade, and enforces:
    /// - Mute ON before non-mute params, mute OFF after all others complete
    /// - Send disable before level change, send enable after level change (per-send)
    /// - Discrete params in fade categories sent after pre-wait, before fade starts
    async fn recall_cue_timed(
        &self,
        snapshot: &Snapshot,
        effective_scope: &ScopeTemplate,
        palettes: &HashMap<Uuid, ChannelPalette>,
        ignore_scope: bool,
    ) -> RecallResult {
        use tokio::time::{Duration, sleep};
        use tokio_util::sync::CancellationToken;

        // Cancel all in-progress fades from previous recall
        self.multi_fade.cancel_all().await;
        self.fade_controller.cancel_active().await;

        let state = self.state.read().await;

        // Step 1: Resolve all values with palette overrides
        let resolved = crate::model::snapshot::resolve_recall_values(
            snapshot,
            effective_scope,
            palettes,
            ignore_scope,
        );

        // Step 2: Diff against live state and group by (channel, category)
        //
        // For each changed param, classify into:
        // - (channel, Some(category)): has timing
        // - (channel, None): uncategorized, instant
        struct ParamChange {
            addr: ParameterAddress,
            value: ParameterValue,
            start_value: Option<ParameterValue>,
        }

        // Keyed by (ChannelId, Option<TimingCategory>)
        let mut groups: HashMap<(ChannelId, Option<TimingCategory>), Vec<ParamChange>> =
            HashMap::new();
        let mut total_skipped = 0usize;
        let mut undo_map: HashMap<ParameterAddress, ParameterValue> = HashMap::new();

        for (addr, effective_value) in &resolved {
            let live_value = state.get(addr);
            if live_value == Some(*effective_value) {
                total_skipped += 1;
                continue;
            }
            // Capture live value for undo
            if let Some(live) = live_value {
                undo_map.insert(addr.clone(), live.clone());
            }
            let cat = TimingCategory::from_parameter_path(&addr.parameter);
            groups
                .entry((addr.channel.clone(), cat))
                .or_default()
                .push(ParamChange {
                    addr: addr.clone(),
                    value: (*effective_value).clone(),
                    start_value: live_value.cloned(),
                });
        }

        // Count scope-filtered params as skipped (for RecallResult compatibility)
        let all_resolved = crate::model::snapshot::resolve_recall_values(
            snapshot,
            effective_scope,
            palettes,
            true,
        );
        total_skipped += all_resolved.len() - resolved.len();

        drop(state);

        // Step 3: For each channel, determine mute direction
        let mut mute_directions: HashMap<ChannelId, bool> = HashMap::new(); // true=ON, false=OFF
        for ((channel, cat), changes) in &groups {
            if *cat == Some(TimingCategory::Mute) {
                if let Some(change) = changes.first() {
                    let going_on = match change.value {
                        ParameterValue::Bool(b) => b,
                        ParameterValue::Int(i) => i != 0,
                        ParameterValue::Float(f) => f != 0.0,
                        _ => false,
                    };
                    mute_directions.insert(channel.clone(), going_on);
                }
            }
        }

        // Step 4: Build execution plan — spawn concurrent tasks per channel
        //
        // For each channel:
        //   - If mute going ON:  send mute → then start all non-mute groups
        //   - If mute going OFF: start all non-mute groups → wait → send mute
        //   - Otherwise:         start all groups concurrently

        let sender = self.sender.clone();
        let ipad_sender = self.ipad_sender.clone();
        let pace = self.pace_us.load(Ordering::Relaxed);

        // Collect all spawned handles for tracking
        let mut all_handles: Vec<tokio::task::JoinHandle<usize>> = Vec::new();

        // Group the groups by channel for mute ordering
        let mut by_channel: HashMap<ChannelId, Vec<(Option<TimingCategory>, Vec<ParamChange>)>> =
            HashMap::new();
        for ((channel, cat), changes) in groups {
            by_channel.entry(channel).or_default().push((cat, changes));
        }

        for (channel, channel_groups) in by_channel {
            let mute_dir: Option<bool> = mute_directions.get(&channel).copied();
            let scope_ref = effective_scope;

            // Separate mute group from non-mute groups
            let mut mute_change: Option<ParamChange> = None;
            let mut non_mute_groups: Vec<(Option<TimingCategory>, Vec<ParamChange>)> = Vec::new();

            for (cat, mut changes) in channel_groups {
                if cat == Some(TimingCategory::Mute) {
                    mute_change = changes.pop();
                } else {
                    non_mute_groups.push((cat, changes));
                }
            }

            let mute_timing = scope_ref.timing_for(&channel, TimingCategory::Mute);
            let s = sender.clone();
            let is = ipad_sender.clone();

            // Build the per-group execution closures
            let mut group_tasks: Vec<(
                Option<TimingCategory>,
                Duration, // pre_wait
                Duration, // fade_time
                Vec<ParamChange>,
            )> = Vec::new();

            for (cat, changes) in non_mute_groups {
                let timing = cat
                    .map(|c| scope_ref.timing_for(&channel, c))
                    .unwrap_or_default();
                let pre_wait = Duration::from_secs_f32(timing.pre_wait_secs);
                let fade_time = if cat.map(|c| c.supports_fade()).unwrap_or(false) {
                    Duration::from_secs_f32(timing.fade_time_secs)
                } else {
                    Duration::ZERO
                };
                group_tasks.push((cat, pre_wait, fade_time, changes));
            }

            // Clone sender for spawned tasks
            let sender_for_channel = s.clone();
            let ipad_for_channel = is.clone();

            // Spawn the channel's execution
            let handle = tokio::spawn(async move {
                let mut sent = 0usize;

                // Helper to send one param
                async fn send_one(
                    sender: &OscSender,
                    ipad: &Option<IpadSender>,
                    addr: &ParameterAddress,
                    value: &ParameterValue,
                ) -> bool {
                    match encode::encode_parameter(addr, value) {
                        Some((path, args)) => sender.send(&path, args).await.is_ok(),
                        None => {
                            if let Some(ipad) = ipad {
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

                // Execute a single timing group
                async fn exec_group(
                    sender: &OscSender,
                    ipad: &Option<IpadSender>,
                    pre_wait: Duration,
                    fade_time: Duration,
                    changes: Vec<ParamChange>,
                    pace_us: u64,
                ) -> usize {
                    let mut sent = 0usize;

                    if !pre_wait.is_zero() {
                        sleep(pre_wait).await;
                    }

                    // Separate discrete from continuous
                    let mut discrete = Vec::new();
                    let mut continuous_targets = Vec::new();

                    for change in changes {
                        // Clone start_value so the else branch can still push
                        // `change` whole. The pattern check and `is_continuous`
                        // / fade_time guards are short-circuit-evaluated.
                        if let Some(start_value) = change.start_value.clone()
                            && change.addr.parameter.is_continuous()
                            && !fade_time.is_zero()
                        {
                            continuous_targets.push(FadeTarget {
                                address: change.addr,
                                start_value,
                                end_value: change.value,
                            });
                        } else {
                            discrete.push(change);
                        }
                    }

                    // Send discrete params first (with pacing)
                    for d in &discrete {
                        if send_one(sender, ipad, &d.addr, &d.value).await {
                            sent += 1;
                            if pace_us > 0 {
                                sleep(Duration::from_micros(pace_us)).await;
                            }
                        }
                    }

                    // Fade continuous params
                    if !continuous_targets.is_empty() {
                        let token = CancellationToken::new();
                        let child = token.child_token();
                        let result = crate::console::fade_engine::run_fade_inline(
                            fade_time.as_secs_f32(),
                            continuous_targets,
                            sender.clone(),
                            ipad.clone(),
                            child,
                        )
                        .await;
                        sent += result.total_steps_sent;
                    }

                    sent
                }

                // Execute a Sends group with per-send enable/disable ordering.
                use crate::model::parameter::ParameterPath;
                async fn exec_sends_group(
                    sender: &OscSender,
                    ipad: &Option<IpadSender>,
                    pre_wait: Duration,
                    fade_time: Duration,
                    changes: Vec<ParamChange>,
                    pace_us: u64,
                ) -> usize {
                    let mut sent = 0usize;

                    if !pre_wait.is_zero() {
                        sleep(pre_wait).await;
                    }

                    // Sub-group by send index
                    let mut by_send: HashMap<u8, Vec<ParamChange>> = HashMap::new();
                    let mut non_send_changes: Vec<ParamChange> = Vec::new();

                    for change in changes {
                        match &change.addr.parameter {
                            ParameterPath::SendEnabled(n)
                            | ParameterPath::SendLevel(n)
                            | ParameterPath::SendPan(n) => {
                                by_send.entry(*n).or_default().push(change);
                            }
                            _ => non_send_changes.push(change),
                        }
                    }

                    // Handle non-send params (shouldn't happen but be safe)
                    for c in &non_send_changes {
                        if send_one(sender, ipad, &c.addr, &c.value).await {
                            sent += 1;
                            if pace_us > 0 {
                                sleep(Duration::from_micros(pace_us)).await;
                            }
                        }
                    }

                    // Handle each send with ordering
                    let mut per_send_handles = Vec::new();
                    for (_send_idx, send_changes) in by_send {
                        let s = sender.clone();
                        let i = ipad.clone();
                        let ft = fade_time;
                        let pu = pace_us;
                        per_send_handles.push(tokio::spawn(async move {
                            let mut sent = 0usize;

                            // Separate enable from level/pan
                            let mut enable_change: Option<ParamChange> = None;
                            let mut level_pan: Vec<ParamChange> = Vec::new();

                            for c in send_changes {
                                if matches!(c.addr.parameter, ParameterPath::SendEnabled(_)) {
                                    enable_change = Some(c);
                                } else {
                                    level_pan.push(c);
                                }
                            }

                            // Determine enable direction
                            let enabling = enable_change.as_ref().map(|c| match c.value {
                                ParameterValue::Bool(b) => b,
                                ParameterValue::Int(i) => i != 0,
                                ParameterValue::Float(f) => f != 0.0,
                                _ => false,
                            });

                            match enabling {
                                Some(true) => {
                                    // Enable ON: fade level/pan first, then enable
                                    sent +=
                                        exec_group(&s, &i, Duration::ZERO, ft, level_pan, pu).await;
                                    if let Some(ec) = &enable_change {
                                        if send_one(&s, &i, &ec.addr, &ec.value).await {
                                            sent += 1;
                                        }
                                    }
                                }
                                Some(false) => {
                                    // Enable OFF: disable first, then fade level/pan
                                    if let Some(ec) = &enable_change {
                                        if send_one(&s, &i, &ec.addr, &ec.value).await {
                                            sent += 1;
                                        }
                                    }
                                    sent +=
                                        exec_group(&s, &i, Duration::ZERO, ft, level_pan, pu).await;
                                }
                                None => {
                                    // No enable change — just fade level/pan
                                    sent +=
                                        exec_group(&s, &i, Duration::ZERO, ft, level_pan, pu).await;
                                }
                            }

                            sent
                        }));
                    }

                    for h in per_send_handles {
                        if let Ok(n) = h.await {
                            sent += n;
                        }
                    }

                    sent
                }

                // ── Mute ordering ──
                match mute_dir {
                    Some(true) => {
                        // Mute ON: send mute first, then start everything else
                        if !Duration::from_secs_f32(mute_timing.pre_wait_secs).is_zero() {
                            sleep(Duration::from_secs_f32(mute_timing.pre_wait_secs)).await;
                        }
                        if let Some(mc) = &mute_change {
                            if send_one(&sender_for_channel, &ipad_for_channel, &mc.addr, &mc.value)
                                .await
                            {
                                sent += 1;
                            }
                        }
                        // Now run all non-mute groups concurrently
                        let mut handles = Vec::new();
                        for (cat, pre_wait, fade_time, changes) in group_tasks {
                            let s = sender_for_channel.clone();
                            let i = ipad_for_channel.clone();
                            let pu = pace;
                            if cat == Some(TimingCategory::Sends) {
                                handles.push(tokio::spawn(async move {
                                    exec_sends_group(&s, &i, pre_wait, fade_time, changes, pu).await
                                }));
                            } else {
                                handles.push(tokio::spawn(async move {
                                    exec_group(&s, &i, pre_wait, fade_time, changes, pu).await
                                }));
                            }
                        }
                        for h in handles {
                            if let Ok(n) = h.await {
                                sent += n;
                            }
                        }
                    }
                    Some(false) => {
                        // Mute OFF: run all non-mute groups, wait, then unmute
                        let mut handles = Vec::new();
                        for (cat, pre_wait, fade_time, changes) in group_tasks {
                            let s = sender_for_channel.clone();
                            let i = ipad_for_channel.clone();
                            let pu = pace;
                            if cat == Some(TimingCategory::Sends) {
                                handles.push(tokio::spawn(async move {
                                    exec_sends_group(&s, &i, pre_wait, fade_time, changes, pu).await
                                }));
                            } else {
                                handles.push(tokio::spawn(async move {
                                    exec_group(&s, &i, pre_wait, fade_time, changes, pu).await
                                }));
                            }
                        }
                        for h in handles {
                            if let Ok(n) = h.await {
                                sent += n;
                            }
                        }
                        // All non-mute complete, now unmute
                        if !Duration::from_secs_f32(mute_timing.pre_wait_secs).is_zero() {
                            sleep(Duration::from_secs_f32(mute_timing.pre_wait_secs)).await;
                        }
                        if let Some(mc) = &mute_change {
                            if send_one(&sender_for_channel, &ipad_for_channel, &mc.addr, &mc.value)
                                .await
                            {
                                sent += 1;
                            }
                        }
                    }
                    None => {
                        // No mute change — all groups run concurrently
                        let mut handles = Vec::new();
                        for (cat, pre_wait, fade_time, changes) in group_tasks {
                            let s = sender_for_channel.clone();
                            let i = ipad_for_channel.clone();
                            let pu = pace;
                            if cat == Some(TimingCategory::Sends) {
                                handles.push(tokio::spawn(async move {
                                    exec_sends_group(&s, &i, pre_wait, fade_time, changes, pu).await
                                }));
                            } else {
                                handles.push(tokio::spawn(async move {
                                    exec_group(&s, &i, pre_wait, fade_time, changes, pu).await
                                }));
                            }
                        }
                        for h in handles {
                            if let Ok(n) = h.await {
                                sent += n;
                            }
                        }
                    }
                }

                sent
            });

            all_handles.push(handle);
        }

        // Wait for all channels to complete
        let mut total_sent = 0usize;
        for h in all_handles {
            if let Ok(n) = h.await {
                total_sent += n;
            }
        }

        // Store undo state
        if !undo_map.is_empty() {
            *self.undo.write().await = Some(UndoState {
                previous_values: undo_map,
                label: format!("Undo '{}'", snapshot.name),
            });
        }

        info!(total_sent, total_skipped, "Timed recall complete");
        RecallResult {
            parameters_sent: total_sent,
            parameters_skipped: total_skipped,
        }
    }

    /// Undo the last recall: cancel any active fades and send the pre-recall
    /// values back to the console. Consumes the undo state (can't undo an undo).
    pub async fn undo_recall(&self) -> Option<RecallResult> {
        // Cancel any in-progress fades first
        self.fade_controller.cancel_active().await;
        self.multi_fade.cancel_all().await;

        // Consume undo state
        let undo = self.undo.write().await.take()?;

        let result = self
            .with_dirty_suppression(async {
                let state = self.state.read().await;
                let mut sent = 0usize;
                let mut skipped = 0usize;
                let pace = self.pace_us.load(Ordering::Relaxed);

                for (addr, value) in &undo.previous_values {
                    let did_send = self
                        .send_if_changed(&state, addr, value, &mut sent, &mut skipped, None)
                        .await;
                    if did_send && pace > 0 {
                        tokio::time::sleep(Duration::from_micros(pace)).await;
                    }
                }

                RecallResult {
                    parameters_sent: sent,
                    parameters_skipped: skipped,
                }
            })
            .await;

        info!(sent = result.parameters_sent, "Undo recall complete");
        Some(result)
    }

    /// Whether an undo state is available (for UI button enablement).
    pub fn has_undo(&self) -> bool {
        self.undo.try_read().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Label for the undo button (e.g. "Undo 'MySnapshot'").
    pub fn undo_label(&self) -> Option<String> {
        self.undo.try_read().ok()?.as_ref().map(|u| u.label.clone())
    }

    /// Send a parameter immediately (no state comparison — already checked).
    /// Returns `true` if the parameter was actually sent.
    async fn send_now(
        &self,
        addr: &ParameterAddress,
        value: &ParameterValue,
        sent: &mut usize,
        skipped: &mut usize,
    ) -> bool {
        match encode::encode_parameter(addr, value) {
            Some((path, args)) => {
                if let Err(e) = self.sender.send(&path, args).await {
                    warn!(%addr, "Failed to send recall: {e}");
                    *skipped += 1;
                    false
                } else {
                    debug!(%addr, %value, "Recall: sent discrete param");
                    *sent += 1;
                    true
                }
            }
            None => {
                if let Some(ipad) = &self.ipad_sender {
                    match ipad_encode::encode_ipad_parameter(addr, value) {
                        Some((path, args)) => {
                            if let Err(e) = ipad.send(&path, args).await {
                                warn!(%addr, "Failed to send iPad recall: {e}");
                                *skipped += 1;
                                false
                            } else {
                                debug!(%addr, %value, "Recall: sent discrete via iPad");
                                *sent += 1;
                                true
                            }
                        }
                        None => {
                            *skipped += 1;
                            false
                        }
                    }
                } else {
                    *skipped += 1;
                    false
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::config::ConsoleConfig;
    use crate::model::parameter::{PaletteKind, ParameterPath, ParameterSection};
    use crate::model::snapshot::{
        ChannelScope, CueList, ScopeTemplate, Snapshot, SnapshotData, SnapshotKind,
    };
    use crate::osc::client::OscClient;
    use std::collections::HashSet;
    use std::net::SocketAddr;

    async fn setup_test() -> (SnapshotEngine, Arc<RwLock<ConsoleState>>) {
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        // Port 1 (any non-zero) — Linux's sendto rejects port 0 with
        // EINVAL while Windows accepts it.
        let remote: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let client = OscClient::new(local, remote, None).await.unwrap();
        let (sender, _rx) = client.into_parts();

        let state = Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default())));
        let pace = Arc::new(AtomicU64::new(0));
        let engine = SnapshotEngine::new(state.clone(), sender, pace);
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
        // Port 1 (any non-zero) — Linux's sendto rejects port 0 with
        // EINVAL while Windows accepts it.
        let remote: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let client = OscClient::new(local, remote, None).await.unwrap();
        let (sender, _rx) = client.into_parts();

        let state = Arc::new(RwLock::new(ConsoleState::new(ConsoleConfig::default())));
        let dirty = Arc::new(RwLock::new(DirtyTracker::new()));
        let pace = Arc::new(AtomicU64::new(0));
        let mut engine = SnapshotEngine::new(state.clone(), sender, pace);
        engine.set_dirty_tracker(dirty.clone());
        (engine, state, dirty)
    }

    fn no_palettes() -> HashMap<Uuid, ChannelPalette> {
        HashMap::new()
    }

    #[tokio::test]
    async fn recall_sends_only_changed_params() {
        let (engine, state) = setup_test().await;

        {
            let mut st = state.write().await;
            st.update(
                ParameterAddress {
                    channel: ChannelId::Input(1),
                    parameter: ParameterPath::Fader,
                },
                ParameterValue::Float(-10.0),
            );
            st.update(
                ParameterAddress {
                    channel: ChannelId::Input(1),
                    parameter: ParameterPath::Mute,
                },
                ParameterValue::Bool(false),
            );
        }

        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::FaderMutePan]),
            )],
        );

        let mut values = HashMap::new();
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(0.0),
        );
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Mute,
            },
            ParameterValue::Bool(false),
        );

        let snapshot = Snapshot::new(
            "Test Snap".into(),
            scope.clone(),
            SnapshotData { values },
            SnapshotKind::ApplyOnSave,
        );

        let result = engine
            .recall(&snapshot, &scope, &no_palettes(), false)
            .await;
        assert_eq!(result.parameters_sent, 1);
        assert_eq!(result.parameters_skipped, 1);
    }

    #[tokio::test]
    async fn recall_skips_ipad_only() {
        let (engine, _state) = setup_test().await;

        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::Inserts]),
            )],
        );

        let mut values = HashMap::new();
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::InsertAEnabled,
            },
            ParameterValue::Bool(true),
        );

        let snapshot = Snapshot::new(
            "Test".into(),
            scope.clone(),
            SnapshotData { values },
            SnapshotKind::ApplyOnSave,
        );

        let result = engine
            .recall(&snapshot, &scope, &no_palettes(), false)
            .await;
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
                ParameterAddress {
                    channel: ChannelId::Input(1),
                    parameter: ParameterPath::EqBandGain(1),
                },
                ParameterValue::Float(0.0),
            );
        }

        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::Eq]),
            )],
        );

        // Snapshot has EQ gain = 2.0
        let mut values = HashMap::new();
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::EqBandGain(1),
            },
            ParameterValue::Float(2.0),
        );
        let mut snapshot = Snapshot::new(
            "Snap".into(),
            scope.clone(),
            SnapshotData { values },
            SnapshotKind::ApplyOnSave,
        );

        // Palette has EQ gain = 5.0 (should override snapshot's 2.0)
        let mut eq_vals = HashMap::new();
        eq_vals.insert(ParameterPath::EqBandGain(1), ParameterValue::Float(5.0));
        let palette = ChannelPalette::new(
            "Vocal EQ".into(),
            PaletteKind::Eq,
            ChannelId::Input(1),
            eq_vals,
        );
        let palette_id = palette.id;

        // Link palette to snapshot
        snapshot
            .palette_refs
            .insert((ChannelId::Input(1), PaletteKind::Eq), palette_id);

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
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::Eq]),
            )],
        );

        let mut values = HashMap::new();
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::EqBandGain(1),
            },
            ParameterValue::Float(3.0),
        );
        let mut snapshot = Snapshot::new(
            "Snap".into(),
            scope.clone(),
            SnapshotData { values },
            SnapshotKind::ApplyOnSave,
        );

        // Reference a palette that doesn't exist
        snapshot
            .palette_refs
            .insert((ChannelId::Input(1), PaletteKind::Eq), Uuid::new_v4());

        let result = engine
            .recall(&snapshot, &scope, &no_palettes(), false)
            .await;
        // Falls back to snapshot value (3.0), live is None → sent
        assert_eq!(result.parameters_sent, 1);
    }

    #[tokio::test]
    async fn non_eq_params_unaffected_by_palette() {
        let (engine, _state) = setup_test().await;

        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::FaderMutePan, ParameterSection::Eq]),
            )],
        );

        let mut values = HashMap::new();
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(-5.0),
        );
        let mut snapshot = Snapshot::new(
            "Snap".into(),
            scope.clone(),
            SnapshotData { values },
            SnapshotKind::ApplyOnSave,
        );

        // Link a palette — should not affect the fader
        let eq_vals = HashMap::new();
        let palette = ChannelPalette::new(
            "Empty".into(),
            PaletteKind::Eq,
            ChannelId::Input(1),
            eq_vals,
        );
        snapshot
            .palette_refs
            .insert((ChannelId::Input(1), PaletteKind::Eq), palette.id);

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

        // Create an iPad sender (pointing at a dummy socket).
        // Local port 0 = bind auto-pick; remote port 1 because Linux's
        // sendto rejects port 0 with EINVAL.
        let ipad_client = crate::osc::ipad_client::IpadClient::new(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:1".parse().unwrap(),
            None,
        )
        .await
        .unwrap();
        let (ipad_sender, _ipad_rx) = ipad_client.into_parts();
        engine.set_ipad_sender(Some(ipad_sender));

        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::Inserts]),
            )],
        );

        let mut values = HashMap::new();
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::InsertAEnabled,
            },
            ParameterValue::Bool(true),
        );

        let snapshot = Snapshot::new(
            "Test".into(),
            scope.clone(),
            SnapshotData { values },
            SnapshotKind::ApplyOnSave,
        );

        let result = engine
            .recall(&snapshot, &scope, &no_palettes(), false)
            .await;
        // With iPad sender: InsertAEnabled is iPad-only but should now be sent
        assert_eq!(result.parameters_sent, 1);
        assert_eq!(result.parameters_skipped, 0);
    }

    #[tokio::test]
    async fn palette_only_params_are_sent() {
        let (engine, _state) = setup_test().await;

        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::Eq]),
            )],
        );

        // Snapshot has no EQ data at all
        let snapshot_values = HashMap::new();
        let mut snapshot = Snapshot::new(
            "Snap".into(),
            scope.clone(),
            SnapshotData {
                values: snapshot_values,
            },
            SnapshotKind::ApplyOnSave,
        );

        // Palette has EQ values that should be sent even though they're not in snapshot
        let mut eq_vals = HashMap::new();
        eq_vals.insert(
            ParameterPath::EqBandFrequency(1),
            ParameterValue::Float(800.0),
        );
        eq_vals.insert(ParameterPath::EqBandGain(1), ParameterValue::Float(4.0));
        let palette =
            ChannelPalette::new("Test".into(), PaletteKind::Eq, ChannelId::Input(1), eq_vals);
        let pid = palette.id;

        snapshot
            .palette_refs
            .insert((ChannelId::Input(1), PaletteKind::Eq), pid);

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
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::FaderMutePan]),
            )],
        );

        let mut values = HashMap::new();
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(0.0),
        );
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Mute,
            },
            ParameterValue::Bool(true),
        );

        let snapshot = Snapshot::new(
            "Snap".into(),
            scope.clone(),
            SnapshotData { values },
            SnapshotKind::ApplyOnSave,
        );
        let cue = crate::model::snapshot::Cue::new(1.0, "Test Cue".into(), snapshot.id);

        let result = engine
            .recall_cue(&cue, &snapshot, &no_palettes(), false)
            .await;
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
                ParameterAddress {
                    channel: ChannelId::Input(1),
                    parameter: ParameterPath::Fader,
                },
                ParameterValue::Float(0.0),
            );
        }

        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::FaderMutePan]),
            )],
        );

        let mut values = HashMap::new();
        // Fader = continuous with known live value → deferred to fade
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(5.0),
        );
        // Mute = discrete → sent immediately
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Mute,
            },
            ParameterValue::Bool(true),
        );

        let snapshot = Snapshot::new(
            "Snap".into(),
            scope.clone(),
            SnapshotData { values },
            SnapshotKind::ApplyOnSave,
        );
        let mut cue = crate::model::snapshot::Cue::new(1.0, "Fade Cue".into(), snapshot.id);
        cue.fade_time = 2.0;

        let result = engine
            .recall_cue(&cue, &snapshot, &no_palettes(), false)
            .await;
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
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::FaderMutePan, ParameterSection::Eq]),
            )],
        );

        // Cue scope override: only Eq
        let eq_only_scope = ScopeTemplate::new(
            "EQ Only".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::Eq]),
            )],
        );

        let mut values = HashMap::new();
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(0.0),
        );
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::EqEnabled,
            },
            ParameterValue::Bool(true),
        );

        let snapshot = Snapshot::new(
            "Snap".into(),
            full_scope,
            SnapshotData { values },
            SnapshotKind::ApplyOnSave,
        );
        let mut cue = crate::model::snapshot::Cue::new(1.0, "Scoped".into(), snapshot.id);
        cue.scope_override = Some(eq_only_scope);

        let result = engine
            .recall_cue(&cue, &snapshot, &no_palettes(), false)
            .await;
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
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(-10.0),
        );
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::EqEnabled,
            },
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
        let result = engine
            .recall(&snapshot, &scope, &no_palettes(), false)
            .await;
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
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(-10.0),
        );
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::EqEnabled,
            },
            ParameterValue::Bool(true),
        );

        let full_scope = ScopeTemplate::new(
            "Full".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::FaderMutePan, ParameterSection::Eq]),
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
        let result = engine
            .recall_cue(&cue, &snapshot, &no_palettes(), false)
            .await;
        assert_eq!(result.parameters_sent, 1);
        assert_eq!(result.parameters_skipped, 1);

        // ignore_scope=true bypasses the override → both sent.
        let result = engine
            .recall_cue(&cue, &snapshot, &no_palettes(), true)
            .await;
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
        let _ = engine
            .recall(&snapshot, &scope, &no_palettes(), false)
            .await;

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
        let _ = engine
            .recall_cue(&cue, &snapshot, &no_palettes(), false)
            .await;

        assert!(!dirty.read().await.has_any());
    }

    // ── Integration tests: trigger → resolve → recall ──────────────

    use crate::console::cue_manager::CueManager;
    use crate::osc::trigger_listener::{TriggerEvent, parse_trigger_message};
    use rosc::OscType;

    #[tokio::test]
    async fn trigger_recall_by_name_end_to_end() {
        let (engine, state) = setup_test().await;

        // Live state: Fader at 0 dB
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        state
            .write()
            .await
            .update(addr.clone(), ParameterValue::Float(0.0));

        // Snapshot with Fader at -10 dB
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

        let mut cue_mgr = CueManager::new(CueList::default());
        cue_mgr.add_snapshot(snapshot);

        // Parse trigger
        let src: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let event = parse_trigger_message(
            crate::osc::SNAPSHOT_RECALL_ADDR,
            &[OscType::String("Verse 1".into())],
            src,
        )
        .unwrap();
        let TriggerEvent::SnapshotRecall {
            identifier,
            ignore_scope,
        } = event
        else {
            panic!("expected SnapshotRecall");
        };
        assert!(!ignore_scope);

        // Resolve and recall
        let resolved = cue_mgr.resolve_snapshot(&identifier).unwrap();
        let result = engine
            .recall(resolved, &resolved.scope, &no_palettes(), ignore_scope)
            .await;
        assert_eq!(result.parameters_sent, 1);
    }

    #[tokio::test]
    async fn trigger_recall_by_uuid_end_to_end() {
        let (engine, state) = setup_test().await;

        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        state
            .write()
            .await
            .update(addr.clone(), ParameterValue::Float(0.0));

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
        let uuid_str = snapshot.id.to_string();

        let mut cue_mgr = CueManager::new(CueList::default());
        cue_mgr.add_snapshot(snapshot);

        // Parse with UUID identifier
        let src: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let event = parse_trigger_message(
            crate::osc::SNAPSHOT_RECALL_ADDR,
            &[OscType::String(uuid_str)],
            src,
        )
        .unwrap();
        let TriggerEvent::SnapshotRecall { identifier, .. } = event else {
            panic!("expected SnapshotRecall");
        };

        let resolved = cue_mgr.resolve_snapshot(&identifier).unwrap();
        let result = engine
            .recall(resolved, &resolved.scope, &no_palettes(), false)
            .await;
        assert_eq!(result.parameters_sent, 1);
    }

    #[tokio::test]
    async fn trigger_recall_unknown_resolves_to_none() {
        let src: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let event = parse_trigger_message(
            crate::osc::SNAPSHOT_RECALL_ADDR,
            &[OscType::String("NonExistent".into())],
            src,
        )
        .unwrap();
        let TriggerEvent::SnapshotRecall { identifier, .. } = event else {
            panic!("expected SnapshotRecall");
        };

        let cue_mgr = CueManager::new(CueList::default());
        assert!(cue_mgr.resolve_snapshot(&identifier).is_none());
    }

    #[tokio::test]
    async fn trigger_recall_full_sends_out_of_scope_params() {
        let (engine, state) = setup_test().await;

        let fader_addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        let eq_addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::EqEnabled,
        };
        {
            let mut st = state.write().await;
            st.update(fader_addr.clone(), ParameterValue::Float(0.0));
            st.update(eq_addr.clone(), ParameterValue::Bool(false));
        }

        // Snapshot stores both Fader and EqEnabled, but scope only covers Fader.
        let mut data = SnapshotData::new();
        data.values.insert(fader_addr, ParameterValue::Float(-10.0));
        data.values.insert(eq_addr, ParameterValue::Bool(true));
        let scope = ScopeTemplate::new(
            "S".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::FaderMutePan]),
            )],
        );
        let snapshot = Snapshot::new("Verse 1".into(), scope, data, SnapshotKind::ApplyOnSave);

        let mut cue_mgr = CueManager::new(CueList::default());
        cue_mgr.add_snapshot(snapshot);

        // Parse /snapshot/recall_full → ignore_scope = true
        let src: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let event = parse_trigger_message(
            crate::osc::SNAPSHOT_RECALL_FULL_ADDR,
            &[OscType::String("Verse 1".into())],
            src,
        )
        .unwrap();
        let TriggerEvent::SnapshotRecall {
            identifier,
            ignore_scope,
        } = event
        else {
            panic!("expected SnapshotRecall");
        };
        assert!(ignore_scope);

        let resolved = cue_mgr.resolve_snapshot(&identifier).unwrap();
        let result = engine
            .recall(resolved, &resolved.scope, &no_palettes(), ignore_scope)
            .await;
        // Both params sent (ignore_scope bypasses the Fader-only scope).
        assert_eq!(result.parameters_sent, 2);
    }

    #[tokio::test]
    async fn trigger_cue_fire_recalls_linked_snapshot() {
        let (engine, state) = setup_test().await;

        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        state
            .write()
            .await
            .update(addr.clone(), ParameterValue::Float(0.0));

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
        let snap_id = snapshot.id;

        let mut cue_mgr = CueManager::new(CueList::default());
        cue_mgr.add_snapshot(snapshot);

        let cue = crate::model::snapshot::Cue::new(1.0, "Cue 1".into(), snap_id);
        cue_mgr.add_cue(cue.clone());

        // Parse /cue/fire 1.0
        let src: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let event = parse_trigger_message("/cue/fire", &[OscType::Float(1.0)], src).unwrap();
        let TriggerEvent::FireCue(number) = event else {
            panic!("expected FireCue");
        };

        // Resolve cue → snapshot (mirrors main.rs dispatch)
        let fired_cue = cue_mgr.fire_cue_number(number).unwrap().clone();
        let snapshot = cue_mgr.snapshots.get(&fired_cue.snapshot_id).unwrap();
        let result = engine
            .recall_cue(&fired_cue, snapshot, &no_palettes(), false)
            .await;
        assert_eq!(result.parameters_sent, 1);
    }
}
