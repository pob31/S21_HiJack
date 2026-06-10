//! Look-ahead recall cache — pre-resolves the next few cues' recall data
//! during the idle time between cues so the recall hot path skips the
//! scope-filter + palette-resolution work (`resolve_recall_values` runs
//! twice per recall) and the first OSC packet leaves sooner. The win is a
//! few milliseconds on desktop but real on the Raspberry Pi target, where
//! resolution of a large snapshot costs 10–20 ms.
//!
//! Correctness model: a cached entry is served only when BOTH guards pass —
//! the coarse model generation (bumped by every cue / snapshot / scope /
//! palette mutation; see `bump()` callers in `CueManager` and
//! `PaletteManager`) matches the current one, and the snapshot's
//! `modified_at` matches. On any mismatch the engine silently falls back to
//! resolving synchronously — exactly the pre-cache behavior. A spurious
//! bump only costs a background recompute; a missed bump risks a stale
//! recall, so hooks are conservative.
//!
//! Anything that depends on last-moment console values — the live-state
//! reads for undo capture and fade start values, the force-send policy —
//! deliberately stays at recall time. Only the console-independent
//! resolution is precomputed here.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{Notify, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::debug;
use uuid::Uuid;

use crate::console::cue_manager::CueManager;
use crate::console::palette_manager::PaletteManager;
use crate::model::palette::ChannelPalette;
use crate::model::parameter::{ParameterAddress, ParameterValue};
use crate::model::snapshot::{ScopeTemplate, Snapshot, resolve_recall_values};

/// How many cues past the playhead to pre-resolve. The NEXT cue is the one
/// that matters; one more covers fast stepping.
const LOOKAHEAD_CUES: usize = 2;

/// Debounce after a wake before recomputing, so a burst of bumps (the
/// palette absorb loop re-absorbs every 150 ms during a knob sweep) folds
/// into one recompute.
const DEBOUNCE: Duration = Duration::from_millis(250);

/// Self-heal tick: re-targets the cache after playhead moves that have no
/// dedicated hook (cue-list row click, skip transport, follow mode).
const RETARGET_TICK: Duration = Duration::from_millis(500);

/// One pre-resolved recall, keyed by `(snapshot id, effective scope id)`.
pub struct CachedResolution {
    pub snapshot_id: Uuid,
    pub scope_id: Uuid,
    /// Secondary staleness guard: snapshot edits bump `modified_at`
    /// (re-capture, scope replace, auto-update-on-recall), invalidating
    /// this entry even within one model generation.
    pub snapshot_modified_at: DateTime<Utc>,
    /// Model generation the inputs were read at.
    pub generation: u64,
    /// `resolve_recall_values(.., ignore_scope = true)` — every stored /
    /// palette param. Also serves the skipped-parameter count
    /// (`all.len() - scoped.len()`).
    pub all: Vec<(ParameterAddress, ParameterValue)>,
    /// `resolve_recall_values(.., ignore_scope = false)`.
    pub scoped: Vec<(ParameterAddress, ParameterValue)>,
}

/// Shared look-ahead cache. Owned (via `Arc`) by the snapshot engine (reads),
/// the precompute task (writes), and both managers (generation bumps).
pub struct RecallCache {
    entries: Mutex<HashMap<(Uuid, Uuid), Arc<CachedResolution>>>,
    generation: AtomicU64,
    notify: Notify,
}

impl RecallCache {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
            notify: Notify::new(),
        }
    }

    /// Record that recall-relevant model data changed (cue list, snapshots,
    /// scopes, palettes incl. the in-session working overlay): invalidates
    /// every entry and wakes the precompute task.
    pub fn bump(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        self.notify.notify_one();
    }

    /// Wake the precompute task WITHOUT invalidating entries. Used after a
    /// recall completes — the playhead moved, so the cache should re-target,
    /// but the existing entries are still valid.
    pub fn poke(&self) {
        self.notify.notify_one();
    }

    /// Current model generation.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Look up a pre-resolved recall for `(snapshot, scope_id)`. Returns
    /// `None` — caller resolves synchronously — unless the entry was computed
    /// at the current model generation AND against this exact snapshot
    /// revision (`modified_at`).
    pub fn get_valid(&self, snapshot: &Snapshot, scope_id: Uuid) -> Option<Arc<CachedResolution>> {
        let entries = self.entries.lock().unwrap();
        let entry = entries.get(&(snapshot.id, scope_id))?;
        if entry.generation != self.generation() {
            return None;
        }
        if entry.snapshot_modified_at != snapshot.modified_at {
            return None;
        }
        Some(entry.clone())
    }

    /// Swap in a freshly computed entry set. No-op when the model generation
    /// moved during the compute — the inputs are already stale and the next
    /// wake recomputes. (Even if a stale set slipped in, `get_valid`'s
    /// generation check would reject every entry; this just skips the
    /// pointless swap.) Returns whether the set was installed.
    pub fn replace_all(&self, computed_at: u64, entries: Vec<CachedResolution>) -> bool {
        if self.generation() != computed_at {
            return false;
        }
        *self.entries.lock().unwrap() = entries
            .into_iter()
            .map(|e| ((e.snapshot_id, e.scope_id), Arc::new(e)))
            .collect();
        true
    }
}

impl Default for RecallCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve one `(snapshot, scope)` pair into an owned cache entry. Pure —
/// shared by the precompute pass and the unit tests.
fn resolve_for_cache(
    snapshot: &Snapshot,
    scope: &ScopeTemplate,
    palettes: &HashMap<Uuid, ChannelPalette>,
    generation: u64,
) -> CachedResolution {
    let own = |pairs: Vec<(ParameterAddress, &ParameterValue)>| {
        pairs
            .into_iter()
            .map(|(a, v)| (a, v.clone()))
            .collect::<Vec<_>>()
    };
    CachedResolution {
        snapshot_id: snapshot.id,
        scope_id: scope.id,
        snapshot_modified_at: snapshot.modified_at,
        generation,
        all: own(resolve_recall_values(snapshot, scope, palettes, true)),
        scoped: own(resolve_recall_values(snapshot, scope, palettes, false)),
    }
}

/// One precompute pass: read the upcoming cues' `(snapshot, effective
/// scope)` pairs, resolve them against the current palettes, and install
/// the result. Each lock is taken briefly and dropped before the next —
/// never two at once, and never the console-state lock at all, so this
/// task cannot participate in the palette<->state ABBA order (see
/// palette_tracker.rs).
async fn precompute_pass(
    cache: &RecallCache,
    cue_manager: &Arc<RwLock<CueManager>>,
    palette_manager: &Arc<RwLock<PaletteManager>>,
) {
    let generation = cache.generation();

    let targets: Vec<(Snapshot, ScopeTemplate)> = {
        let mgr = cue_manager.read().await;
        let mut seen: HashSet<(Uuid, Uuid)> = HashSet::new();
        let mut targets = Vec::new();
        for cue in mgr.upcoming_cues(LOOKAHEAD_CUES) {
            let Some(snap) = cue.snapshot_id.and_then(|id| mgr.get_snapshot(&id)) else {
                continue;
            };
            // The cue path recalls against the override-or-snapshot scope;
            // the direct path (follow dispatcher, trigger listener) recalls
            // against the snapshot's own scope. Cache both when they differ.
            let effective = cue.scope_override.as_ref().unwrap_or(&snap.scope);
            for scope in [effective, &snap.scope] {
                if seen.insert((snap.id, scope.id)) {
                    targets.push((snap.clone(), scope.clone()));
                }
            }
        }
        targets
    };

    let palettes = palette_manager.read().await.palettes.clone();

    let entries: Vec<CachedResolution> = targets
        .iter()
        .map(|(snap, scope)| resolve_for_cache(snap, scope, &palettes, generation))
        .collect();
    let installed = cache.replace_all(generation, entries);
    debug!(
        installed,
        targets = targets.len(),
        generation,
        "Recall look-ahead: precompute pass"
    );
}

/// Run the look-ahead precompute loop until `cancel` fires. Spawned at
/// connect time alongside the palette absorb loop.
pub async fn run_precompute_loop(
    cache: Arc<RecallCache>,
    cue_manager: Arc<RwLock<CueManager>>,
    palette_manager: Arc<RwLock<PaletteManager>>,
    cancel: CancellationToken,
) {
    let mut ticker = tokio::time::interval(RETARGET_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Tracks the last (generation, playhead target) we computed for, so the
    // 500 ms tick is a cheap no-op while nothing changes.
    let mut last_computed: Option<(u64, Option<Uuid>)> = None;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("Recall look-ahead loop shutting down");
                break;
            }
            _ = cache.notify.notified() => {
                // Fold a burst of bumps (knob-sweep absorption) into one
                // recompute of the final state.
                tokio::time::sleep(DEBOUNCE).await;
            }
            _ = ticker.tick() => {}
        }

        let generation = cache.generation();
        let next_cue = {
            let mgr = cue_manager.read().await;
            mgr.next_cue().map(|c| c.id)
        };
        if last_computed == Some((generation, next_cue)) {
            continue;
        }
        precompute_pass(&cache, &cue_manager, &palette_manager).await;
        last_computed = Some((generation, next_cue));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::parameter::ParameterPath;
    use crate::model::snapshot::{ChannelScope, SnapshotData, SnapshotKind};

    fn make_snapshot() -> Snapshot {
        let channel = ChannelId::Input(1);
        let mut paths = HashSet::new();
        paths.insert(ParameterPath::Fader);
        let scope = ScopeTemplate::new("test".into(), vec![ChannelScope::new(channel.clone(), paths)]);
        let mut data = SnapshotData::new();
        data.values.insert(
            ParameterAddress {
                channel,
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(-10.0),
        );
        Snapshot::new("snap".into(), scope, data, SnapshotKind::ApplyOnSave)
    }

    #[test]
    fn hit_requires_matching_generation() {
        let cache = RecallCache::new();
        let snap = make_snapshot();
        let scope_id = snap.scope.id;

        let generation = cache.generation();
        let entry = resolve_for_cache(&snap, &snap.scope, &HashMap::new(), generation);
        assert!(cache.replace_all(generation, vec![entry]));
        assert!(cache.get_valid(&snap, scope_id).is_some());

        cache.bump();
        assert!(
            cache.get_valid(&snap, scope_id).is_none(),
            "a model-generation bump must invalidate every entry"
        );
    }

    #[test]
    fn hit_requires_matching_modified_at() {
        let cache = RecallCache::new();
        let mut snap = make_snapshot();
        let scope_id = snap.scope.id;

        let generation = cache.generation();
        let entry = resolve_for_cache(&snap, &snap.scope, &HashMap::new(), generation);
        assert!(cache.replace_all(generation, vec![entry]));

        snap.modified_at = Utc::now() + chrono::Duration::seconds(1);
        assert!(
            cache.get_valid(&snap, scope_id).is_none(),
            "a snapshot edit (modified_at) must invalidate its entry"
        );
    }

    #[test]
    fn replace_all_rejects_stale_compute() {
        let cache = RecallCache::new();
        let snap = make_snapshot();

        let generation = cache.generation();
        let entry = resolve_for_cache(&snap, &snap.scope, &HashMap::new(), generation);
        cache.bump(); // inputs changed mid-compute
        assert!(!cache.replace_all(generation, vec![entry]));
        assert!(cache.get_valid(&snap, snap.scope.id).is_none());
    }

    #[test]
    fn cached_resolution_matches_synchronous() {
        let snap = make_snapshot();
        let palettes = HashMap::new();
        let entry = resolve_for_cache(&snap, &snap.scope, &palettes, 0);

        let sync_all = resolve_recall_values(&snap, &snap.scope, &palettes, true);
        let sync_scoped = resolve_recall_values(&snap, &snap.scope, &palettes, false);
        assert_eq!(entry.all.len(), sync_all.len());
        assert_eq!(entry.scoped.len(), sync_scoped.len());
        for (addr, value) in &entry.scoped {
            assert!(
                sync_scoped
                    .iter()
                    .any(|(a, v)| a == addr && *v == value),
                "cached pair missing from synchronous resolution: {addr}"
            );
        }
    }
}
