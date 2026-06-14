use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use tokio::sync::RwLock;
use tokio::time::{Duration, sleep};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::console::automation_registry::AutomationOverride;
use crate::console::cue_manager::CueManager;
use crate::console::fade_engine::{FadeController, FadeTarget, MultiFadeController};
use crate::console::recall_cache::RecallCache;
use crate::model::channel::ChannelId;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::palette::ChannelPalette;
use crate::model::parameter::{
    FADER_INF_DB, ParameterAddress, ParameterPath, ParameterValue, TimingCategory,
};
use crate::model::recall_progress::{RecallKind, RecallProgress};
use crate::model::snapshot::{Cue, ScopeTemplate, Snapshot};
use crate::model::state::ConsoleState;
use crate::model::sync_direction::SharedSyncDirection;
use crate::osc::client::OscSender;
use crate::osc::encode;
use crate::osc::encode::SystemCommand;
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

/// Offset between the app's STORED 1-based console-snapshot row
/// (`Cue::console_snapshot`) and the value the desk uses ON THE WIRE for
/// `/digico/snapshots/fire`: `wire = stored + CONSOLE_SNAPSHOT_WIRE_OFFSET`.
///
/// Applied symmetrically — outbound at the fire encode, inbound when a desk
/// report is normalized back to stored space (see `connection.rs`) — so the
/// dispatcher, suppression map, and `current_console_snapshot` mirror all stay
/// in one canonical (stored) base and the two directions can never drift.
///
/// **Defaults to 0 (no offset) — the desk's wire base is UNVERIFIED.** To
/// confirm on a real S21: enable Console→App, recall snapshot **1** from the
/// desk surface, and read the logged `wire=` value at
/// `connection.rs` ("Console current-snapshot (GP OSC)"). If `wire=0` for
/// snapshot 1, the desk is 0-based → set this to `-1`. If `wire=1`, it is
/// 1-based → leave at `0` (and any remaining one-off is an authoring issue or
/// the multiple-cues-per-row first-match rule).
pub const CONSOLE_SNAPSHOT_WIRE_OFFSET: i32 = 0;

/// Lock-free shared "a console snapshot is currently loading" window, stored as
/// a deadline in millis since process start (0 = inactive). While active, the
/// GP OSC dispatcher SKIPS gang + pan-link propagation: a console memory load
/// is the desk applying a coherent snapshot, so re-propagating its echo flood
/// through the app's gangs both fights the desk and can saturate the outbound
/// path into a self-sustaining storm (the App→Console "stall"). An `AtomicU64`
/// (not a lock) so the per-message check on the inbound hot path can never
/// itself become a contended/starved lock.
pub type ConsoleLoadSuppression = Arc<AtomicU64>;

/// How long gang/pan propagation stays suppressed after a console memory load
/// — generous enough to cover the desk's echo flood.
pub const CONSOLE_LOAD_SUPPRESSION_MS: u64 = 2_500;

/// Monotonic base for the lock-free console-load deadline.
static CONSOLE_LOAD_BASE: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Arm the console-load suppression window — call right after firing a memory
/// or when the desk reports a snapshot load.
pub fn arm_console_load(handle: &AtomicU64) {
    let deadline = CONSOLE_LOAD_BASE.elapsed().as_millis() as u64 + CONSOLE_LOAD_SUPPRESSION_MS;
    handle.store(deadline, Ordering::Relaxed);
}

/// Whether a console snapshot load is currently within its suppression window.
pub fn console_load_active(handle: &AtomicU64) -> bool {
    let deadline = handle.load(Ordering::Relaxed);
    deadline != 0 && (CONSOLE_LOAD_BASE.elapsed().as_millis() as u64) < deadline
}

/// Poll interval for [`cancellable_pre_wait`] — small enough (below the 50 ms
/// fade tick) that an operator grab during a pre-wait is honoured promptly.
const PRE_WAIT_POLL: Duration = Duration::from_millis(25);

/// Sleep up to `pre_wait`, but return EARLY the moment none of `addrs` remain
/// under automation (the operator grabbed them all during the pre-wait). A
/// single-parameter group's pre-wait ends instantly when that parameter is
/// grabbed; a multi-parameter group waits for its still-active members, which
/// then fade normally. With no registry (legacy path) this is a plain sleep.
async fn cancellable_pre_wait(
    pre_wait: Duration,
    addrs: &[ParameterAddress],
    reg: &Option<AutomationOverride>,
) {
    if pre_wait.is_zero() {
        return;
    }
    let Some(r) = reg else {
        // Legacy path / no registry: can't observe overrides — plain sleep.
        sleep(pre_wait).await;
        return;
    };
    let deadline = Instant::now() + pre_wait;
    loop {
        // Early-out: every address has been overridden (removed from registry).
        if !addrs.iter().any(|a| r.is_active(a)) {
            return;
        }
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        // `.min` keeps the final partial slice exact so we never overshoot.
        sleep((deadline - now).min(PRE_WAIT_POLL)).await;
    }
}

/// Effective fade start value for a param given its (possibly missing) live
/// mirror value. Present mirror values pass through. For FADER-FAMILY params a
/// mirror miss (`None`) defaults to [`FADER_INF_DB`] (off) so the param fades IN
/// from silence (and, via the −80 floor, smoothly) instead of jumping straight
/// to target as a discrete send. Non-fader params with no start stay `None`
/// (the caller keeps their discrete behavior).
fn effective_fade_start(
    start: &Option<ParameterValue>,
    path: &ParameterPath,
) -> Option<ParameterValue> {
    match start {
        Some(sv) => Some(sv.clone()),
        None if path.fade_floor_db().is_some() => Some(ParameterValue::Float(FADER_INF_DB)),
        None => None,
    }
}

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
    /// Optional shared progress handle. When set, recalls drive the thin
    /// progress line under the tab bar (begin / bump / finish). `None` for
    /// engines built in contexts with no UI (tests, trigger dispatch).
    recall_progress: Option<Arc<RecallProgress>>,
    /// Per-show snapshot sync direction. When attached, a cue's console-memory
    /// row is fired over GP OSC only while the direction is App→Console. When
    /// absent (tests, headless, trigger dispatch — no UI master switch) the
    /// engine defaults to firing any cue that carries a row, preserving the
    /// long-standing "a row means drive the desk" behavior.
    sync_direction: Option<SharedSyncDirection>,
    /// Shared console-load suppression window. Armed right after firing a
    /// console memory so the inbound dispatcher skips gang/pan propagation of
    /// the desk's resulting snapshot-load flood. Shared with the connection
    /// loop via `DaemonState.console_load_suppression`.
    console_load_suppression: Option<ConsoleLoadSuppression>,
    /// Shared live-override registry for timed recalls. Lets the operator take
    /// back any parameter mid pre-wait/fade. Shared with the connection loop via
    /// `DaemonState.automation_override`.
    automation_override: Option<AutomationOverride>,
    /// Optional look-ahead recall cache. When attached, recall paths first
    /// try the pre-resolved `(snapshot, scope)` entry — skipping the double
    /// `resolve_recall_values` pass — and fall back to synchronous
    /// resolution on any staleness mismatch. `None` for engines built in
    /// contexts without the precompute loop (tests, trigger dispatch).
    recall_cache: Option<Arc<RecallCache>>,
    /// Optional external-trigger dispatcher. When set, a cue's `triggers`
    /// (OSC to QLab/LiveProfessor/custom, MIDI) fire at the end of `recall_cue`.
    /// `None` in tests/contexts without external trigger support → no-op.
    trigger_dispatcher: Option<Arc<crate::console::trigger_dispatcher::TriggerDispatcher>>,
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
            recall_progress: None,
            sync_direction: None,
            console_load_suppression: None,
            automation_override: None,
            recall_cache: None,
            trigger_dispatcher: None,
        }
    }

    /// Attach the look-ahead recall cache so recalls can start from
    /// pre-resolved data instead of resolving synchronously.
    pub fn set_recall_cache(&mut self, cache: Arc<RecallCache>) {
        self.recall_cache = Some(cache);
    }

    /// Attach the external-trigger dispatcher so a cue's `triggers` fire after
    /// its console row + overlay are applied.
    pub fn set_trigger_dispatcher(
        &mut self,
        dispatcher: Arc<crate::console::trigger_dispatcher::TriggerDispatcher>,
    ) {
        self.trigger_dispatcher = Some(dispatcher);
    }

    /// Attach the shared snapshot sync-direction handle. When set to
    /// App→Console, recalls fire the cue's console-memory row over GP OSC.
    pub fn set_sync_direction(&mut self, dir: SharedSyncDirection) {
        self.sync_direction = Some(dir);
    }

    /// Attach the shared console-load suppression window so firing a console
    /// memory tells the inbound dispatcher to skip gang/pan propagation of the
    /// desk's load flood.
    pub fn set_console_load_suppression(&mut self, handle: ConsoleLoadSuppression) {
        self.console_load_suppression = Some(handle);
    }

    /// Attach the shared live-override registry so the operator can take back
    /// any parameter during a timed recall's pre-wait or fade.
    pub fn set_automation_override(&mut self, handle: AutomationOverride) {
        self.automation_override = Some(handle);
    }

    /// Attach the shared recall-progress handle so recalls drive the UI's
    /// progress line. Called once at engine construction in the UI/daemon path.
    pub fn set_recall_progress(&mut self, progress: Arc<RecallProgress>) {
        self.recall_progress = Some(progress);
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

        // Canonical lock order when more than one of these is held at once:
        // state → cue_manager → palette_manager → dirty_tracker. We take
        // state(read) before cue(write) here; the palette-absorb loop releases
        // its cue read before taking state/palette, so no cycle can form.
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

    /// Fire the given console memory row, if any, over **GP OSC**
    /// (`/digico/snapshots/fire <row>`) so it works in every operating mode —
    /// not just the iPad-protocol modes. Honors the per-show sync direction:
    /// the fire only happens while the direction is App→Console (or when no
    /// direction handle is attached, e.g. headless / trigger dispatch, where a
    /// cue carrying a row is taken to mean "drive the desk").
    ///
    /// Always fires when a row is supplied and pushing is enabled — repeating a
    /// cue must reload the desk snapshot so any drift between recalls is reset.
    /// Waits for a settling period after firing so the console's echo flood
    /// lands before we start writing the parameter overlay. Records the fired
    /// row in the suppression set so Console→App follow ignores the echo.
    async fn fire_console_memory_if_needed(&self, row: Option<i32>) {
        let Some(row) = row else {
            return;
        };

        // Only push to the desk when the operator selected App→Console. With no
        // direction handle attached (tests / headless / trigger dispatch) default
        // to firing so a cue's console row still drives the desk.
        let should_push = self
            .sync_direction
            .as_ref()
            .map(|d| d.get().pushes_to_console())
            .unwrap_or(true);
        if !should_push {
            debug!(
                row,
                "Console fire skipped (sync direction is not App→Console)"
            );
            return;
        }

        // Mark suppression BEFORE sending so the follow dispatcher ignores the
        // resulting echo as our own write. Keyed in STORED space (the desk's
        // echo is normalized back to stored space before the dispatcher sees
        // it), so the key matches regardless of the wire offset.
        {
            let mut sup = self.console_fire_suppression.write().await;
            sup.insert(row, Instant::now());
        }
        // Convert the stored 1-based row to the desk's wire base only at the
        // wire boundary (see CONSOLE_SNAPSHOT_WIRE_OFFSET).
        let wire = row + CONSOLE_SNAPSHOT_WIRE_OFFSET;
        info!(stored = row, wire, "Firing console memory over GP OSC");
        let cmd = SystemCommand::SnapshotFire(wire);
        if let Err(e) = self.sender.send(cmd.path(), cmd.args()).await {
            warn!(row, "Failed to fire console memory: {e}");
            return;
        }

        // Arm the console-load suppression window: the desk is about to flood
        // its snapshot-load echoes, and the app must NOT gang/pan-propagate
        // them back (that fights the desk and can stall outbound OSC).
        if let Some(handle) = &self.console_load_suppression {
            arm_console_load(handle);
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
        // Re-target the look-ahead precompute: the playhead likely moved,
        // so the "next cues" set changed. Existing entries stay valid.
        if let Some(cache) = &self.recall_cache {
            cache.poke();
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
                self.fire_console_memory_if_needed(None).await;
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
        let mut sent = 0usize;
        let mut skipped = 0usize;

        // Look-ahead fast path: when the precompute task already resolved
        // this exact (snapshot, scope) at the current model generation, skip
        // both resolve passes. Falls back to synchronous resolution on any
        // staleness mismatch — identical output either way.
        let cached = self
            .recall_cache
            .as_ref()
            .and_then(|c| c.get_valid(snapshot, scope.id));
        debug!(cache_hit = cached.is_some(), snapshot = %snapshot.name, "Recall: resolution source");

        // Resolve all candidates (ignore_scope=true) so we can count
        // scope-filtered params as skipped, matching the old behaviour.
        // `fallback` keeps the synchronous resolution alive for the borrows
        // in `resolved` when there's no cache hit.
        let fallback: Vec<(ParameterAddress, &ParameterValue)>;
        let (all_len, resolved): (usize, Vec<(&ParameterAddress, &ParameterValue)>) = match &cached
        {
            Some(c) => {
                let src = if ignore_scope { &c.all } else { &c.scoped };
                (c.all.len(), src.iter().map(|(a, v)| (a, v)).collect())
            }
            None => {
                let all_len =
                    crate::model::snapshot::resolve_recall_values(snapshot, scope, palettes, true)
                        .len();
                fallback = crate::model::snapshot::resolve_recall_values(
                    snapshot,
                    scope,
                    palettes,
                    ignore_scope,
                );
                (all_len, fallback.iter().map(|(a, v)| (a, *v)).collect())
            }
        };
        skipped += all_len - resolved.len();

        // Capture pre-recall live values for undo — best-effort, via TRY_read.
        // This recall runs right after firing a console memory, while the desk
        // floods echoes; a blocking `state.read().await` would, on tokio's
        // write-preferring RwLock, STARVE behind that continuous flood and hang
        // the whole recall here (the App→Console stall). `try_read` never
        // blocks: if the lock is busy we simply skip undo capture this time
        // (undo is a convenience, and the recall force-sends every param
        // regardless, so it needs no live read to be correct).
        let mut undo_map: HashMap<ParameterAddress, ParameterValue> = HashMap::new();
        if let Ok(state) = self.state.try_read() {
            for &(addr, _) in &resolved {
                if let Some(live) = state.get(addr) {
                    undo_map.insert(addr.clone(), live.clone());
                }
            }
        }

        // Drive the shared progress line: one op spanning every resolved
        // parameter, bumped per iteration so the bar reaches 100% (sent +
        // skipped) and finishes when the loop ends.
        if let Some(p) = &self.recall_progress {
            p.begin(RecallKind::Recall, resolved.len());
        }

        let pace = self.pace_us.load(Ordering::Relaxed);
        for &(addr, effective_value) in &resolved {
            let did_send = self
                .send_now(addr, effective_value, &mut sent, &mut skipped)
                .await;
            if let Some(p) = &self.recall_progress {
                p.bump();
            }
            if did_send && pace > 0 {
                tokio::time::sleep(Duration::from_micros(pace)).await;
            }
        }

        if let Some(p) = &self.recall_progress {
            p.finish();
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

    /// Send a parameter to the console.
    /// Returns `true` if the parameter was actually sent.
    /// When `undo_map` is provided, captures the live value before sending
    /// so the recall can be reverted.
    ///
    /// When `force` is false this skips parameters whose value already equals
    /// the live state mirror (a traffic optimisation). When `force` is true it
    /// always sends, regardless of the mirror — the mirror is only kept in
    /// sync by lossy UDP echoes, so trusting it for recall correctness can
    /// silently drop channels that genuinely need to move. Both recall and
    /// undo pass `force = true` for this reason.
    async fn send_if_changed(
        &self,
        state: &ConsoleState,
        addr: &ParameterAddress,
        value: &ParameterValue,
        sent: &mut usize,
        skipped: &mut usize,
        undo_map: Option<&mut HashMap<ParameterAddress, ParameterValue>>,
        force: bool,
    ) -> bool {
        // Check if the value differs from live state. Without `force`, an
        // unchanged value is skipped; with `force` we send anyway.
        let live_value = state.get(addr);
        if !force && live_value == Some(value) {
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

    /// Reload a palette's last-captured (stored) values onto its channel — the
    /// "Revert changes" action. Force-sends every stored parameter (ignoring the
    /// in-session working overlay, which the caller clears separately) so the
    /// desk returns to the captured state. Returns the number of params sent.
    pub async fn reload_palette(&self, palette: &ChannelPalette) -> usize {
        let mut sent = 0usize;
        let mut skipped = 0usize;
        for (path, value) in &palette.values {
            let addr = ParameterAddress {
                channel: palette.channel.clone(),
                parameter: path.clone(),
            };
            // Force-send with NO state lock held across the await — a revert
            // never trusts the lossy live mirror, and holding state.read()
            // across the send loop would reproduce the same reader-starvation
            // that froze the recall path during the desk's echo flood.
            self.send_now(&addr, value, &mut sent, &mut skipped).await;
        }
        info!(palette = %palette.name, sent, "Palette revert: reloaded stored values");
        sent
    }

    /// Recall a cue — resolves effective scope and delegates to recall().
    ///
    /// When the effective scope carries per-category timing, continuous
    /// parameters are faded per category (see `recall_cue_timed`); otherwise
    /// the recall is instant.
    ///
    /// When `ignore_scope == true`, the scope filter is bypassed entirely
    /// (see `recall` for details). Only meaningful for `ApplyOnRecall`
    /// snapshots.
    pub async fn recall_cue(
        &self,
        cue: &Cue,
        snapshot: Option<&Snapshot>,
        palettes: &HashMap<Uuid, ChannelPalette>,
        ignore_scope: bool,
    ) -> RecallResult {
        // Auto-update only applies when there's a snapshot overlay — there's
        // nothing to compare a "previous snapshot's scope" against otherwise.
        if let Some(snap) = snapshot {
            self.auto_save_previous_snapshot(snap.id).await;
        }

        // Phase C: bracket the whole cue recall in dirty suppression so
        // BOTH the no-fade path (which calls recall_inner) AND the fade
        // path's discrete-target writes are suppressed and the dirty set
        // is cleared on success.
        let result = self
            .with_dirty_suppression(async {
                // Fire the cue's console memory row (if any) inside the
                // suppression bracket so its echo flood doesn't pollute
                // the dirty tracker.
                self.fire_console_memory_if_needed(cue.console_snapshot)
                    .await;
                match snapshot {
                    Some(snap) => {
                        self.recall_cue_inner(cue, snap, palettes, ignore_scope)
                            .await
                    }
                    None => RecallResult {
                        parameters_sent: 0,
                        parameters_skipped: 0,
                    },
                }
            })
            .await;

        if let Some(snap) = snapshot {
            self.mark_last_recalled(snap.id).await;
        }

        // Fire external triggers (QLab / LiveProfessor / MIDI) LAST — after the
        // console row + overlay are applied and outside the dirty-suppression
        // window. Best-effort: a failed send logs and never affects `result`.
        if !cue.triggers.is_empty() {
            if let Some(dispatcher) = &self.trigger_dispatcher {
                dispatcher.fire(&cue.triggers).await;
            }
        }

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
            ignore_scope,
            "Recalling cue"
        );

        // Per-category timed recall: if the scope has any category timings,
        // use the orchestrated path with mute ordering and per-group fades.
        if effective_scope.has_any_category_timing() {
            return self
                .recall_cue_timed(snapshot, effective_scope, palettes, ignore_scope)
                .await;
        }

        // Otherwise an instant recall. Fades live in the scope's per-category
        // timing (see `recall_cue_timed`), not on the cue itself, so there is
        // no single-fade path. Cancel any fade still running from a prior cue.
        self.fade_controller.cancel_active().await;
        self.recall_inner(snapshot, effective_scope, palettes, ignore_scope)
            .await
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

        // Begin a fresh live-override generation: clears any prior cue's
        // automation entries so overrides can't leak across cues. `gen_id` stamps
        // every register/note_sent below.
        let registry = self.automation_override.clone();
        let gen_id = registry.as_ref().map(|r| r.begin_recall()).unwrap_or(0);

        // Fade START VALUES must be reliable, so read the live mirror with a
        // real (blocking) read — a `try_read` miss would yield `None` start
        // values, which silently demotes a fade to a discrete jump (the
        // "jumps after the pre-wait" / inconsistent-fade bug). This read is held
        // only across the in-memory diff/group build below and is DROPPED before
        // any send/fade is spawned (line `drop(state)`), so it can't starve the
        // inbound flood the way holding it across the send loop would.
        // Look-ahead fast path (see `recall_inner`): a pre-resolved
        // (snapshot, scope) entry skips both resolve passes. The live
        // diff/grouping below — start values, undo — stays at recall time.
        let cached = self
            .recall_cache
            .as_ref()
            .and_then(|c| c.get_valid(snapshot, effective_scope.id));
        debug!(cache_hit = cached.is_some(), snapshot = %snapshot.name, "Timed recall: resolution source");

        let state = Some(self.state.read().await);

        // Step 1: Resolve all values with palette overrides
        let fallback: Vec<(ParameterAddress, &ParameterValue)>;
        let (all_len, resolved): (usize, Vec<(&ParameterAddress, &ParameterValue)>) = match &cached
        {
            Some(c) => {
                let src = if ignore_scope { &c.all } else { &c.scoped };
                (c.all.len(), src.iter().map(|(a, v)| (a, v)).collect())
            }
            None => {
                let all_len = crate::model::snapshot::resolve_recall_values(
                    snapshot,
                    effective_scope,
                    palettes,
                    true,
                )
                .len();
                fallback = crate::model::snapshot::resolve_recall_values(
                    snapshot,
                    effective_scope,
                    palettes,
                    ignore_scope,
                );
                (all_len, fallback.iter().map(|(a, v)| (a, *v)).collect())
            }
        };

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

        for &(addr, effective_value) in &resolved {
            let live_value = state.as_ref().and_then(|s| s.get(addr));
            // Force: do not skip params that merely match the (lossy) live
            // mirror. A param already at target simply fades target→target.
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
        total_skipped += all_len - resolved.len();

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

        // Largest pre_wait+fade across every channel/group — the span the timed
        // progress bar fills over. Accumulated as each channel is registered.
        let mut max_span = Duration::ZERO;

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

            // Register every address this channel will automate BEFORE spawning,
            // so an operator move arriving during a pre-wait or fade cancels
            // exactly that `(channel, parameter)` and nothing else. The span
            // (pre_wait + fade) is what the progress countdown reads back as the
            // live max(pre_wait + fade). `max_span` is tracked even without a
            // registry so the timed progress bar still spans the right window.
            let mute_span = Duration::from_secs_f32(mute_timing.pre_wait_secs);
            if mute_change.is_some() {
                max_span = max_span.max(mute_span);
            }
            for (_cat, pre_wait, fade_time, _changes) in &group_tasks {
                max_span = max_span.max(*pre_wait + *fade_time);
            }
            if let Some(r) = &registry {
                if let Some(mc) = &mute_change {
                    r.register(&mc.addr, gen_id, mute_span);
                }
                for (_cat, pre_wait, fade_time, changes) in &group_tasks {
                    let span = *pre_wait + *fade_time;
                    for ch in changes {
                        r.register(&ch.addr, gen_id, span);
                    }
                }
            }

            // Clone sender for spawned tasks
            let sender_for_channel = s.clone();
            let ipad_for_channel = is.clone();
            let reg = registry.clone();
            // Shared live mirror, so each group can re-read the fader's CURRENT
            // position at fade-start (after its pre-wait) instead of using the
            // value captured at recall time — see exec_group's re-read below.
            let state_for_channel = self.state.clone();

            // Spawn the channel's execution
            let handle = tokio::spawn(async move {
                let mut sent = 0usize;

                // Helper to send one param
                #[allow(clippy::too_many_arguments)]
                async fn send_one(
                    sender: &OscSender,
                    ipad: &Option<IpadSender>,
                    state: &Arc<RwLock<ConsoleState>>,
                    addr: &ParameterAddress,
                    value: &ParameterValue,
                    reg: &Option<AutomationOverride>,
                    gen_id: u64,
                ) -> bool {
                    // Operator override: if the operator grabbed this param
                    // (e.g. during its pre-wait), it's no longer registered —
                    // don't clobber the hands-on value.
                    if let Some(r) = reg {
                        if !r.is_active(addr) {
                            return false;
                        }
                    }
                    let ok = match encode::encode_parameter(addr, value) {
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
                    };
                    if ok {
                        if let Some(r) = reg {
                            r.note_sent(addr, value, gen_id);
                        }
                        // Optimistically reflect our own send into the mirror so
                        // the next recall/fade reads the value we set (the desk
                        // doesn't echo OSC-set values back).
                        state.write().await.update(addr.clone(), value.clone());
                    }
                    ok
                }

                // Execute a single timing group
                #[allow(clippy::too_many_arguments)]
                async fn exec_group(
                    sender: &OscSender,
                    ipad: &Option<IpadSender>,
                    state: &Arc<RwLock<ConsoleState>>,
                    pre_wait: Duration,
                    fade_time: Duration,
                    changes: Vec<ParamChange>,
                    pace_us: u64,
                    reg: &Option<AutomationOverride>,
                    gen_id: u64,
                ) -> usize {
                    let mut sent = 0usize;

                    // Cancellable pre-wait: abort our upcoming send/fade for any
                    // param the operator grabs during the wait; the rest still run.
                    let addrs: Vec<ParameterAddress> =
                        changes.iter().map(|c| c.addr.clone()).collect();
                    cancellable_pre_wait(pre_wait, &addrs, reg).await;

                    // Separate discrete from continuous
                    let mut discrete = Vec::new();
                    let mut continuous_targets = Vec::new();

                    for change in changes {
                        // Re-read the live mirror NOW (after the pre-wait) so the
                        // fade starts from the fader's CURRENT position, not the
                        // value captured at recall time (pre_wait seconds ago) — a
                        // stale or moved-during-wait start makes the desk snap there
                        // before fading ("jump then fade"). Brief read lock, dropped
                        // immediately, never held across a send/await.
                        let fresh = {
                            let s = state.read().await;
                            s.get(&change.addr).cloned()
                        };
                        // `effective_fade_start` prefers the fresh live value, falls
                        // back to the recall-time capture, then (fader-family only)
                        // to −inf so the param fades IN from silence instead of
                        // jumping (a discrete send). Non-fader misses stay None and
                        // fall through to `discrete`.
                        let start = effective_fade_start(
                            &fresh.or_else(|| change.start_value.clone()),
                            &change.addr.parameter,
                        );
                        if let Some(start_value) = start
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
                        if send_one(sender, ipad, state, &d.addr, &d.value, reg, gen_id).await {
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
                            reg.clone(),
                            gen_id,
                            Some(state.clone()),
                        )
                        .await;
                        sent += result.total_steps_sent;
                    }

                    sent
                }

                // Execute a Sends group with per-send enable/disable ordering.
                #[allow(clippy::too_many_arguments)]
                async fn exec_sends_group(
                    sender: &OscSender,
                    ipad: &Option<IpadSender>,
                    state: &Arc<RwLock<ConsoleState>>,
                    pre_wait: Duration,
                    fade_time: Duration,
                    changes: Vec<ParamChange>,
                    pace_us: u64,
                    reg: &Option<AutomationOverride>,
                    gen_id: u64,
                ) -> usize {
                    let mut sent = 0usize;

                    // Cancellable pre-wait (see exec_group): a grabbed send drops
                    // out; the inner exec_group calls below pass Duration::ZERO so
                    // there is no second pre-wait.
                    let addrs: Vec<ParameterAddress> =
                        changes.iter().map(|c| c.addr.clone()).collect();
                    cancellable_pre_wait(pre_wait, &addrs, reg).await;

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
                        if send_one(sender, ipad, state, &c.addr, &c.value, reg, gen_id).await {
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
                        let st = state.clone();
                        let ft = fade_time;
                        let pu = pace_us;
                        let r = reg.clone();
                        let g = gen_id;
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
                                    sent += exec_group(
                                        &s,
                                        &i,
                                        &st,
                                        Duration::ZERO,
                                        ft,
                                        level_pan,
                                        pu,
                                        &r,
                                        g,
                                    )
                                    .await;
                                    if let Some(ec) = &enable_change {
                                        if send_one(&s, &i, &st, &ec.addr, &ec.value, &r, g).await {
                                            sent += 1;
                                        }
                                    }
                                }
                                Some(false) => {
                                    // Enable OFF: disable first, then fade level/pan
                                    if let Some(ec) = &enable_change {
                                        if send_one(&s, &i, &st, &ec.addr, &ec.value, &r, g).await {
                                            sent += 1;
                                        }
                                    }
                                    sent += exec_group(
                                        &s,
                                        &i,
                                        &st,
                                        Duration::ZERO,
                                        ft,
                                        level_pan,
                                        pu,
                                        &r,
                                        g,
                                    )
                                    .await;
                                }
                                None => {
                                    // No enable change — just fade level/pan
                                    sent += exec_group(
                                        &s,
                                        &i,
                                        &st,
                                        Duration::ZERO,
                                        ft,
                                        level_pan,
                                        pu,
                                        &r,
                                        g,
                                    )
                                    .await;
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
                        let mute_addrs: Vec<ParameterAddress> =
                            mute_change.iter().map(|mc| mc.addr.clone()).collect();
                        cancellable_pre_wait(
                            Duration::from_secs_f32(mute_timing.pre_wait_secs),
                            &mute_addrs,
                            &reg,
                        )
                        .await;
                        if let Some(mc) = &mute_change {
                            if send_one(
                                &sender_for_channel,
                                &ipad_for_channel,
                                &state_for_channel,
                                &mc.addr,
                                &mc.value,
                                &reg,
                                gen_id,
                            )
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
                            let st = state_for_channel.clone();
                            let pu = pace;
                            let r = reg.clone();
                            let g = gen_id;
                            if cat == Some(TimingCategory::Sends) {
                                handles.push(tokio::spawn(async move {
                                    exec_sends_group(
                                        &s, &i, &st, pre_wait, fade_time, changes, pu, &r, g,
                                    )
                                    .await
                                }));
                            } else {
                                handles.push(tokio::spawn(async move {
                                    exec_group(&s, &i, &st, pre_wait, fade_time, changes, pu, &r, g)
                                        .await
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
                            let st = state_for_channel.clone();
                            let pu = pace;
                            let r = reg.clone();
                            let g = gen_id;
                            if cat == Some(TimingCategory::Sends) {
                                handles.push(tokio::spawn(async move {
                                    exec_sends_group(
                                        &s, &i, &st, pre_wait, fade_time, changes, pu, &r, g,
                                    )
                                    .await
                                }));
                            } else {
                                handles.push(tokio::spawn(async move {
                                    exec_group(&s, &i, &st, pre_wait, fade_time, changes, pu, &r, g)
                                        .await
                                }));
                            }
                        }
                        for h in handles {
                            if let Ok(n) = h.await {
                                sent += n;
                            }
                        }
                        // All non-mute complete, now unmute
                        let mute_addrs: Vec<ParameterAddress> =
                            mute_change.iter().map(|mc| mc.addr.clone()).collect();
                        cancellable_pre_wait(
                            Duration::from_secs_f32(mute_timing.pre_wait_secs),
                            &mute_addrs,
                            &reg,
                        )
                        .await;
                        if let Some(mc) = &mute_change {
                            if send_one(
                                &sender_for_channel,
                                &ipad_for_channel,
                                &state_for_channel,
                                &mc.addr,
                                &mc.value,
                                &reg,
                                gen_id,
                            )
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
                            let st = state_for_channel.clone();
                            let pu = pace;
                            let r = reg.clone();
                            let g = gen_id;
                            if cat == Some(TimingCategory::Sends) {
                                handles.push(tokio::spawn(async move {
                                    exec_sends_group(
                                        &s, &i, &st, pre_wait, fade_time, changes, pu, &r, g,
                                    )
                                    .await
                                }));
                            } else {
                                handles.push(tokio::spawn(async move {
                                    exec_group(&s, &i, &st, pre_wait, fade_time, changes, pu, &r, g)
                                        .await
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

        // Drive the outbound (blue) progress bar as a time countdown over the
        // recall's max(pre_wait + fade). A lightweight driver task tracks the
        // live max remaining from the override registry, so an operator move
        // that drops a long fade pulls the bar in toward the new, nearer finish.
        // Stops as soon as a newer recall supersedes this generation or the
        // registry drains (or, with no registry, after the fixed span elapses).
        if !max_span.is_zero()
            && let Some(progress) = self.recall_progress.clone()
        {
            let progress_gen = progress.begin_timed(max_span);
            let registry_for_driver = registry.clone();
            tokio::spawn(async move {
                loop {
                    sleep(Duration::from_millis(80)).await;
                    // A newer recall bumped the generation — abandon this bar.
                    if progress.generation() != progress_gen {
                        return;
                    }
                    match &registry_for_driver {
                        Some(r) => match r.max_remaining() {
                            Some(rem) if !rem.is_zero() => progress.set_span_end(rem),
                            // Registry drained: everything finished or was taken.
                            _ => break,
                        },
                        // No registry to follow: hold the fixed span and let the
                        // bar reach its end on its own.
                        None => {
                            if progress.snapshot().time_fraction() >= 1.0 {
                                break;
                            }
                        }
                    }
                }
                if progress.generation() == progress_gen {
                    progress.finish();
                }
            });
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
                let mut sent = 0usize;
                let mut skipped = 0usize;
                let pace = self.pace_us.load(Ordering::Relaxed);

                for (addr, value) in &undo.previous_values {
                    // Force-send (no live read): like recall, undo must not trust
                    // the lossy live mirror. `send_now` sends unconditionally, so
                    // we hold no state lock across the loop — avoiding the same
                    // reader-starvation that froze the App→Console path.
                    let did_send = self.send_now(addr, value, &mut sent, &mut skipped).await;
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
        // Optimistically reflect our own send into the mirror on success — the
        // desk doesn't echo OSC-set values back, so without this the mirror goes
        // stale and a later fade would start from a stale value.
        match encode::encode_parameter(addr, value) {
            Some((path, args)) => {
                if let Err(e) = self.sender.send(&path, args).await {
                    warn!(%addr, "Failed to send recall: {e}");
                    *skipped += 1;
                    false
                } else {
                    debug!(%addr, %value, "Recall: sent discrete param");
                    *sent += 1;
                    self.state.write().await.update(addr.clone(), value.clone());
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
                                self.state.write().await.update(addr.clone(), value.clone());
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
    async fn recall_force_sends_all_in_scope_params() {
        // Recall must not trust the live mirror for correctness: even a param
        // whose stored value already matches the (lossy) mirror is re-sent, so
        // a dropped/late echo can never silently leave a channel behind.
        let (engine, state) = setup_test().await;

        {
            let mut st = state.write().await;
            // Fader differs from the snapshot (live -10, stored 0).
            st.update(
                ParameterAddress {
                    channel: ChannelId::Input(1),
                    parameter: ParameterPath::Fader,
                },
                ParameterValue::Float(-10.0),
            );
            // Mute already matches the snapshot (both false) — pre-force this
            // was skipped; with force it is still sent.
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
        // Both the changed fader and the unchanged mute are sent.
        assert_eq!(result.parameters_sent, 2);
        assert_eq!(result.parameters_skipped, 0);

        // Optimistic mirror update: the desk doesn't echo OSC-set values back,
        // so the recall must reflect what it sent into the live mirror itself —
        // otherwise the mirror stays at the stale -10 and a later fade would
        // start from there (the "jump after a cue change").
        let st = state.read().await;
        assert_eq!(
            st.get(&ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            }),
            Some(&ParameterValue::Float(0.0)),
            "recall should optimistically update the mirror to the sent value"
        );
    }

    /// Helper for the look-ahead cache tests: one Input-1 fader at 0.0.
    fn fader_snapshot() -> (ScopeTemplate, ParameterAddress, Snapshot) {
        let scope = ScopeTemplate::new(
            "Test".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::FaderMutePan]),
            )],
        );
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        let mut values = HashMap::new();
        values.insert(addr.clone(), ParameterValue::Float(0.0));
        let snapshot = Snapshot::new(
            "Snap".into(),
            scope.clone(),
            SnapshotData { values },
            SnapshotKind::ApplyOnSave,
        );
        (scope, addr, snapshot)
    }

    /// Cache entry whose value deliberately DIFFERS from the snapshot's
    /// stored 0.0, so the mirror reveals which resolution path was sent.
    fn marker_entry(
        snapshot: &Snapshot,
        scope: &ScopeTemplate,
        generation: u64,
        addr: &ParameterAddress,
    ) -> crate::console::recall_cache::CachedResolution {
        crate::console::recall_cache::CachedResolution {
            snapshot_id: snapshot.id,
            scope_id: scope.id,
            snapshot_modified_at: snapshot.modified_at,
            generation,
            all: vec![(addr.clone(), ParameterValue::Float(-7.0))],
            scoped: vec![(addr.clone(), ParameterValue::Float(-7.0))],
        }
    }

    #[tokio::test]
    async fn recall_uses_valid_lookahead_cache_entry() {
        let (mut engine, state) = setup_test().await;
        let cache = Arc::new(crate::console::recall_cache::RecallCache::new());
        engine.set_recall_cache(cache.clone());

        let (scope, addr, snapshot) = fader_snapshot();
        let entry = marker_entry(&snapshot, &scope, cache.generation(), &addr);
        assert!(cache.replace_all(cache.generation(), vec![entry]));

        let result = engine
            .recall(&snapshot, &scope, &no_palettes(), false)
            .await;
        assert_eq!(result.parameters_sent, 1);
        assert_eq!(result.parameters_skipped, 0);
        assert_eq!(
            state.read().await.get(&addr),
            Some(&ParameterValue::Float(-7.0)),
            "a valid cache entry is what the recall must send"
        );
    }

    #[tokio::test]
    async fn recall_falls_back_when_cache_generation_stale() {
        let (mut engine, state) = setup_test().await;
        let cache = Arc::new(crate::console::recall_cache::RecallCache::new());
        engine.set_recall_cache(cache.clone());

        let (scope, addr, snapshot) = fader_snapshot();
        let entry = marker_entry(&snapshot, &scope, cache.generation(), &addr);
        assert!(cache.replace_all(cache.generation(), vec![entry]));
        // Model edited after the precompute (palette tweak, cue edit, ...).
        cache.bump();

        let result = engine
            .recall(&snapshot, &scope, &no_palettes(), false)
            .await;
        assert_eq!(result.parameters_sent, 1);
        assert_eq!(
            state.read().await.get(&addr),
            Some(&ParameterValue::Float(0.0)),
            "a stale-generation entry must be ignored in favour of synchronous resolution"
        );
    }

    #[tokio::test]
    async fn recall_falls_back_when_snapshot_edited() {
        let (mut engine, state) = setup_test().await;
        let cache = Arc::new(crate::console::recall_cache::RecallCache::new());
        engine.set_recall_cache(cache.clone());

        let (scope, addr, mut snapshot) = fader_snapshot();
        let entry = marker_entry(&snapshot, &scope, cache.generation(), &addr);
        assert!(cache.replace_all(cache.generation(), vec![entry]));
        // Snapshot revised after the precompute (re-capture / auto-update
        // bump `modified_at`) — same generation, different snapshot.
        snapshot.modified_at = chrono::Utc::now() + chrono::Duration::seconds(1);

        let result = engine
            .recall(&snapshot, &scope, &no_palettes(), false)
            .await;
        assert_eq!(result.parameters_sent, 1);
        assert_eq!(
            state.read().await.get(&addr),
            Some(&ParameterValue::Float(0.0)),
            "an entry for an older snapshot revision must be ignored"
        );
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
            ChannelId::Input(1),
            &[PaletteKind::Eq],
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
    async fn recall_applies_linked_palette_outside_scope() {
        // A Fader-scoped snapshot with an EQ palette linked on its channel must
        // still apply the palette's EQ on recall — the link bypasses scope.
        // A Dyn1 value in the same palette must NOT be sent, since only the Eq
        // kind is linked.
        let (engine, _state) = setup_test().await;

        // Scope covers only Fader/Mute/Pan — no EQ, no dynamics.
        let scope = ScopeTemplate::new(
            "FaderOnly".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::FaderMutePan]),
            )],
        );

        // Empty snapshot data — the palette is the only source of values.
        let mut snapshot = Snapshot::new(
            "Snap".into(),
            scope.clone(),
            SnapshotData {
                values: HashMap::new(),
            },
            SnapshotKind::ApplyOnSave,
        );

        // Palette holds both an EQ and a Dyn1 value, linked on Eq only.
        let mut vals = HashMap::new();
        vals.insert(ParameterPath::EqBandGain(1), ParameterValue::Float(5.0));
        vals.insert(
            ParameterPath::Dyn1Threshold(1),
            ParameterValue::Float(-12.0),
        );
        let palette = ChannelPalette::new(
            "Vocal".into(),
            ChannelId::Input(1),
            &[PaletteKind::Eq, PaletteKind::Dyn1],
            vals,
        );
        let palette_id = palette.id;
        snapshot
            .palette_refs
            .insert((ChannelId::Input(1), PaletteKind::Eq), palette_id);

        let mut palettes = HashMap::new();
        palettes.insert(palette_id, palette);

        let result = engine.recall(&snapshot, &scope, &palettes, false).await;
        // Only the EQ palette param is sent; the unlinked Dyn1 value is not.
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
            ChannelId::Input(1),
            &[PaletteKind::Eq],
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
        let palette = ChannelPalette::new(
            "Test".into(),
            ChannelId::Input(1),
            &[PaletteKind::Eq],
            eq_vals,
        );
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
        let cue =
            crate::model::snapshot::Cue::new(1.0, "Test Cue".into()).with_snapshot_id(snapshot.id);

        let result = engine
            .recall_cue(&cue, Some(&snapshot), &no_palettes(), false)
            .await;
        // Both are new (no live state) → both sent
        assert_eq!(result.parameters_sent, 2);
    }

    #[tokio::test]
    async fn recall_cue_without_category_timing_is_instant() {
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
        // Fader = continuous, live value 0.0 → changes to 5.0.
        values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(5.0),
        );
        // Mute = discrete.
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
        let cue = crate::model::snapshot::Cue::new(1.0, "Instant Cue".into())
            .with_snapshot_id(snapshot.id);

        let result = engine
            .recall_cue(&cue, Some(&snapshot), &no_palettes(), false)
            .await;
        // No per-category timing → instant recall: Fader (0.0→5.0) and Mute
        // are both changes, so both are sent immediately.
        assert_eq!(result.parameters_sent, 2);
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
        let mut cue =
            crate::model::snapshot::Cue::new(1.0, "Scoped".into()).with_snapshot_id(snapshot.id);
        cue.scope_override = Some(eq_only_scope);

        let result = engine
            .recall_cue(&cue, Some(&snapshot), &no_palettes(), false)
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
        let mut cue =
            crate::model::snapshot::Cue::new(1.0, "Cue".into()).with_snapshot_id(snapshot.id);
        cue.scope_override = Some(fader_only_scope);

        // ignore_scope=false honours the override → only Fader sent.
        let result = engine
            .recall_cue(&cue, Some(&snapshot), &no_palettes(), false)
            .await;
        assert_eq!(result.parameters_sent, 1);
        assert_eq!(result.parameters_skipped, 1);

        // ignore_scope=true bypasses the override → both sent.
        let result = engine
            .recall_cue(&cue, Some(&snapshot), &no_palettes(), true)
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
        let cue = crate::model::snapshot::Cue::new(1.0, "Cue".into()).with_snapshot_id(snapshot.id);
        let _ = engine
            .recall_cue(&cue, Some(&snapshot), &no_palettes(), false)
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

        let cue = crate::model::snapshot::Cue::new(1.0, "Cue 1".into()).with_snapshot_id(snap_id);
        cue_mgr.add_cue(cue.clone());

        // Parse /cue/fire 1.0
        let src: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let event = parse_trigger_message("/cue/fire", &[OscType::Float(1.0)], src).unwrap();
        let TriggerEvent::FireCue(number) = event else {
            panic!("expected FireCue");
        };

        // Resolve cue → snapshot (mirrors main.rs dispatch)
        let fired_cue = cue_mgr.fire_cue_number(number).unwrap().clone();
        let snapshot = cue_mgr
            .snapshots
            .get(&fired_cue.snapshot_id.unwrap())
            .unwrap();
        let result = engine
            .recall_cue(&fired_cue, Some(snapshot), &no_palettes(), false)
            .await;
        assert_eq!(result.parameters_sent, 1);
    }

    // ─── cancellable_pre_wait + effective_fade_start ────────────────────

    use crate::console::automation_registry::AutomationRegistry;

    fn fader_addr(ch: u8) -> ParameterAddress {
        ParameterAddress {
            channel: ChannelId::Input(ch),
            parameter: ParameterPath::Fader,
        }
    }

    #[tokio::test]
    async fn cancellable_pre_wait_single_addr_returns_early() {
        let reg: AutomationOverride = Arc::new(AutomationRegistry::new());
        let g = reg.begin_recall();
        let a = fader_addr(1);
        reg.register(&a, g, Duration::from_secs(5));

        // Override the only address shortly after the pre-wait starts; the wait
        // must then return well before its nominal 5 s.
        let reg2 = reg.clone();
        let a2 = a.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(30)).await;
            reg2.maybe_override(&a2, &ParameterValue::Float(0.0));
        });

        let r = tokio::time::timeout(
            Duration::from_millis(500),
            cancellable_pre_wait(Duration::from_secs(5), &[a], &Some(reg)),
        )
        .await;
        assert!(r.is_ok(), "pre-wait should return early after override");
    }

    #[tokio::test]
    async fn cancellable_pre_wait_multi_addr_waits_for_all() {
        let reg: AutomationOverride = Arc::new(AutomationRegistry::new());
        let g = reg.begin_recall();
        let a1 = fader_addr(1);
        let a2 = fader_addr(2);
        reg.register(&a1, g, Duration::from_secs(5));
        reg.register(&a2, g, Duration::from_secs(5));

        // Override only one address: the group still has an active member, so
        // the pre-wait must NOT return early.
        reg.maybe_override(&a1, &ParameterValue::Float(0.0));
        let early = tokio::time::timeout(
            Duration::from_millis(120),
            cancellable_pre_wait(
                Duration::from_secs(5),
                &[a1.clone(), a2.clone()],
                &Some(reg.clone()),
            ),
        )
        .await;
        assert!(
            early.is_err(),
            "should still be waiting while one addr is active"
        );

        // Override the second too → now it returns promptly.
        reg.maybe_override(&a2, &ParameterValue::Float(0.0));
        let done = tokio::time::timeout(
            Duration::from_millis(200),
            cancellable_pre_wait(Duration::from_secs(5), &[a1, a2], &Some(reg)),
        )
        .await;
        assert!(done.is_ok(), "should return once all addrs are overridden");
    }

    #[tokio::test]
    async fn cancellable_pre_wait_no_registry_is_plain_sleep() {
        let start = Instant::now();
        cancellable_pre_wait(Duration::from_millis(60), &[fader_addr(1)], &None).await;
        assert!(
            start.elapsed() >= Duration::from_millis(55),
            "should sleep ~full pre-wait"
        );
    }

    #[tokio::test]
    async fn cancellable_pre_wait_zero_is_noop() {
        let start = Instant::now();
        cancellable_pre_wait(Duration::ZERO, &[fader_addr(1)], &None).await;
        assert!(
            start.elapsed() < Duration::from_millis(20),
            "zero pre-wait returns immediately"
        );
    }

    #[test]
    fn effective_fade_start_fader_none_defaults_to_inf() {
        assert_eq!(
            effective_fade_start(&None, &ParameterPath::Fader),
            Some(ParameterValue::Float(FADER_INF_DB))
        );
        assert_eq!(
            effective_fade_start(&None, &ParameterPath::SendLevel(1)),
            Some(ParameterValue::Float(FADER_INF_DB))
        );
    }

    #[test]
    fn effective_fade_start_non_fader_none_stays_none() {
        assert_eq!(
            effective_fade_start(&None, &ParameterPath::EqBandGain(1)),
            None
        );
    }

    #[test]
    fn effective_fade_start_some_passes_through() {
        assert_eq!(
            effective_fade_start(&Some(ParameterValue::Float(-20.0)), &ParameterPath::Fader),
            Some(ParameterValue::Float(-20.0))
        );
    }

    // ─── Fade-start selection (re-read live value after the pre-wait) ────
    //
    // exec_group needs a live OscSender to drive directly, but the load-bearing
    // new logic is the pure selection rule it applies:
    //   effective_fade_start(&fresh.or_else(|| captured.clone()), &param)
    // These pin that composition: fresh live value wins, then the recall-time
    // capture, then the fader −inf floor.

    #[test]
    fn fresh_live_value_preferred_over_recall_capture() {
        let captured = Some(ParameterValue::Float(-30.0)); // stale, recall-time
        let fresh = Some(ParameterValue::Float(-6.0)); // live, post-pre-wait
        let start =
            effective_fade_start(&fresh.or_else(|| captured.clone()), &ParameterPath::Fader);
        assert_eq!(start, Some(ParameterValue::Float(-6.0)));
    }

    #[test]
    fn falls_back_to_recall_capture_on_mirror_miss() {
        let captured = Some(ParameterValue::Float(-30.0));
        let fresh: Option<ParameterValue> = None; // mirror miss after pre-wait
        let start =
            effective_fade_start(&fresh.or_else(|| captured.clone()), &ParameterPath::Fader);
        assert_eq!(start, Some(ParameterValue::Float(-30.0)));
    }

    #[test]
    fn double_miss_fader_floors_to_inf() {
        let captured: Option<ParameterValue> = None;
        let fresh: Option<ParameterValue> = None;
        let start =
            effective_fade_start(&fresh.or_else(|| captured.clone()), &ParameterPath::Fader);
        assert_eq!(start, Some(ParameterValue::Float(FADER_INF_DB)));
    }
}
