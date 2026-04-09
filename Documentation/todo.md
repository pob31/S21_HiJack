# S21 HiJack — Outstanding Refinements

Snapshot of post-palette-generalization work that's worth doing but isn't blocking any current operator workflow. Items grouped by priority. UI polish work is deliberately excluded — it'll be addressed in a dedicated UI pass later.

Last updated: 2026-04-10, after commit `0ac493d` (palette generalization).

---

## 🔴 Correctness footguns

### 1. `ParameterPath::Gain` is ambiguous between GP OSC and iPad

The same enum variant maps to two physically different parameters:

- [src/model/parameter.rs:126](../src/model/parameter.rs#L126): `Gain` → GP OSC `total/gain` = post-fader+CG sum (-20..+60 dB)
- [src/model/parameter.rs:194](../src/model/parameter.rs#L194): `Gain` → iPad `Channel_Input/analog_gain` = the actual mic preamp

The parsers also point both back to `Gain`, so the live mirror's `Input(1)/Gain` value depends on which protocol delivered the most recent update. In Mode 3 (proxy) where both protocols are active, you can get inconsistent values bouncing between two physical knobs.

**Fix:**
- Rename `Gain` → `TotalGain` (GP OSC only, no iPad path).
- Add `AnalogGain` (iPad only, no GP OSC path).
- Update parsers/encoders accordingly.
- Bump ShowFile version with a serde alias migration so existing snapshots that captured the old `Gain` variant load as `TotalGain` (since the GP OSC path has been the canonical one in practice).

Estimated effort: 1-2 hours. Has to be done with care because every snapshot file in the wild needs migration.

### 2. QLab "Export to QLab" ignores palette overrides

[src/osc/qlab_cue_builder.rs `build_snapshot_cues`](../src/osc/qlab_cue_builder.rs) reads `snapshot.data.values` directly. If you link an EQ / Comp / Gate palette to a snapshot and then click "Export to QLab", QLab gets the **stored** values, not the palette overrides.

The single-trigger-cue path is fine — it just calls back into `/snapshot/recall` which goes through the engine — but the per-parameter export silently strips palettes.

**Fix:** refactor the substitution into a free function:
```rust
pub fn resolve_recall_values(
    snapshot: &Snapshot,
    scope: &ScopeTemplate,
    palettes: &HashMap<Uuid, ChannelPalette>,
    ignore_scope: bool,
) -> Vec<(ParameterAddress, ParameterValue)>
```
Both `recall_inner` and `build_snapshot_cues` call it. Cleaner than duplicating the substitution logic.

---

## 🟡 Coverage gaps

### 3. `available_for_channel` is hand-curated, no CSV cross-check

The `ParameterPath::available_for_channel` table in [src/model/parameter.rs](../src/model/parameter.rs) is hand-typed from [Documentation/DiGiCo S OSC Commandset_OSCpaths.csv](DiGiCo%20S%20OSC%20Commandset_OSCpaths.csv). There's no automated check that they stay in sync — if DiGiCo ships a new path family in the CSV, nothing in the build will catch the omission.

**Fix:** a single test that parses the CSV at compile time (`include_str!`) and walks every row, asserting `ParameterPath::from_gp_osc_suffix(...).available_for_channel(...)` matches the CSV's per-channel-type columns. ~40 lines, runs in CI, catches drift.

### 4. Mock console may be drifting from protocol changes

[src/bin/mock_console.rs](../src/bin/mock_console.rs) was useful for the connection tests early on. The recent GP OSC commit `cf990a5` changed band indexing (0-based wire / 1-based internal) and added discovery + ping/pong. The mock console may not implement the new behaviour.

**Fix:** 5-minute audit. If stale, the dev workflow ("connect to mock console, exercise the UI") quietly stops representing the real console.

### 5. Limited integration test coverage of trigger → recall

There are unit tests for each part of the chain but nothing that exercises `/snapshot/recall` end-to-end through `parse_trigger_message` → `resolve_snapshot` → `SnapshotEngine::recall`. Same for cue triggers in headless mode.

**Fix:** a couple of `#[tokio::test]` integration tests that wire the pieces together.

---

## 🟡 Cleanups & dead code

### 6. Trigger event dispatch is duplicated

The same `match TriggerEvent { ... }` block exists in two places:

- [src/main.rs:312-388](../src/main.rs#L312) (headless mode)
- [src/ui/setup_tab.rs:919-998](../src/ui/setup_tab.rs#L919) (UI mode)

Adding a new variant means editing both. Phase E added `SnapshotRecall` to both — any future variant pays the same cost.

**Fix:** consolidate into one shared async function:
```rust
async fn handle_trigger_event(
    event: TriggerEvent,
    cue_mgr: &Arc<RwLock<CueManager>>,
    palette_mgr: &Arc<RwLock<PaletteManager>>,
    macro_mgr: &Arc<RwLock<MacroManager>>,
    macro_eng: &Arc<MacroEngine>,
    engine: &Arc<SnapshotEngine>,
    reply_socket: Option<&UdpSocket>,
)
```
Called from both spawn sites.

### 7. Dead-coded helpers from earlier phases

- `ConsoleState::available_paths_for` / `has_path_for_channel` — added in Phase A for a use case that never materialized; only the per-frame static availability is used now. Marked `#[allow(dead_code)]`.
- `ConsoleState::capture_eq` — backward-compat wrapper around `capture_section(.., Eq)`. Nothing in-tree calls it any more after the palette generalization. Safe to delete.
- `ChannelGroup::color()` — used by the deleted signal-flow block view; the new matrix uses universal scope colors.

**Fix:** 10-minute pass to delete these.

### 8. Old `ShowFile` back-compat tests pile

`v1_file_loads_with_defaults`, `v2_file_loads_with_macro_defaults`, `v3_file_loads_with_monitor_defaults`, `v4_file_loads_with_gang_defaults`, `v7_file_loads_with_legacy_section_scopes`, `v8_file_loads_with_legacy_palettes_and_palette_refs` — that's 6 legacy formats (currently at v9). Useful for the recent ones, less so for v1/v2/v3/v4 which were never released to anyone outside this project.

**Fix:** prune the very old ones once you've decided what counts as "in the wild".

---

## 🟡 Asymmetries worth a design call

### 9. Trigger listener arg types are inconsistent

- `/cue/fire` accepts INT or FLOAT for the cue number ([trigger_listener.rs:117](../src/osc/trigger_listener.rs#L117))
- `/macro/fire` accepts String or Int for the identifier ([trigger_listener.rs:132](../src/osc/trigger_listener.rs#L132))
- `/snapshot/recall` accepts only String

**Fix:** ideally `/snapshot/recall` should also accept Int (for cue-number-based recall) and `/cue/fire` should accept String (for cue-name fallback). Small change, more flexible OSC API.

### 10. Per-cue scope override exists; per-cue palette override doesn't

`Cue.scope_override: Option<ScopeTemplate>` lets two cues fire the same snapshot with different scopes. There's no equivalent for palettes. If the operator wants Cue 1 to fire Verse 1 with the dry vocal palette and Cue 2 to fire the same Verse 1 with the wet vocal palette, they currently need two snapshots.

**Decision needed:** is this by design, or worth adding `Cue.palette_overrides: Option<HashMap<(ChannelId, PaletteKind), Uuid>>`?

### 11. Macros pollute the dirty tracker

When a macro fires, `MacroEngine::execute` sends OSC to the console. The console echoes the values back; the OSC dispatcher receives them and marks each cell dirty. So firing a macro then opening the scope editor's "Auto-preselect modified" view will preselect everything the macro touched.

Snapshot recalls suppress dirty correctly via `SnapshotEngine::with_dirty_suppression`. Macros don't.

**Fix:** make `MacroEngine` dirty-tracker-aware by mirroring what `SnapshotEngine` does — hold an `Option<Arc<RwLock<DirtyTracker>>>` and bracket `execute()` in begin/end suppression. Two-method addition + plumbing through the constructors. ~30 lines.

---

## 🟡 Documentation drift

### 12. README and PRD are pre-Phase-A

[README.md](../README.md) hasn't been touched since the early phases. It says nothing about:
- Per-`ParameterPath` scope granularity
- Dedicated scope window with two-level collapsing
- Snapshot kinds (`ApplyOnSave` / `ApplyOnRecall`) + recall-without-scope
- Dirty tracker (auto-preselect modified, etc.)
- QLab outbound (cue builder + sender)
- `/snapshot/recall` listener path
- Compressor / Gate palettes (only EQ palettes are mentioned)

[Documentation/PRD.md](PRD.md) is a bigger document. Probably worth a once-over to mark deferred items as done and add new sections for the post-PRD features.

**Fix:** 1-2 hour writing pass once the model is stable.

---

## 🟢 Tiny things

- `S21_HiJack.jpeg` has been sitting untracked in the repo root since around Phase D. Should be `git add`'d (if it's an asset) or `.gitignore`'d (if it's a screenshot).
- Persistence file path: confirm where the show file lives by default. Should be somewhere operator-friendly (`~/Documents/S21_HiJack/Shows/` or similar) rather than CWD.

---

## Recommended next steps

If picking just two from this list:

1. **`Gain` enum split** (#1) — real correctness bug that can cause silent value corruption in Mode 3. Fix is well-understood.
2. **Palette-aware QLab export** (#2) — closes the loop on the Phase D / palette generalization combination, before anyone notices the gap.

After those, the **trigger event dispatch consolidation** (#6) is the biggest cleanup win and pays dividends every time a new `TriggerEvent` variant is added.

The rest can wait until the UI pass — most of them touch UI tangentially anyway and would be cleaner to resolve in that context.
