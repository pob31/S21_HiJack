//! Phase A: dedicated scope editor window with two-level collapsing.
//!
//! The editor presents one matrix per channel-type group (Inputs, Aux, Groups,
//! Matrix, Control Groups, Graphic EQ, Matrix Inputs). Each group is a
//! collapsible top-level section. Inside an expanded group, parameter sections
//! are themselves collapsible. Rows = individual `ParameterPath` variants
//! (maximum granularity), columns = channels in that group.
//!
//! Every header (corner / channel header / section header label / path row
//! label) is a one-click bulk toggle. Cells with no live data on the console
//! render greyed and ignore clicks. Channel column tooltips show the live
//! channel name from the console.
//!
//! Window state lives in `ScopeEditorState`. Open/Cancel/OK semantics: opening
//! takes a backup of the current selections; Cancel restores the backup, OK
//! commits the changes.
//!
//! See [Documentation/DiGiCo S OSC Commandset_OSCpaths.csv] for the source of
//! truth that drives `ParameterPath::available_for_channel`.

use std::collections::{HashMap, HashSet};

use eframe::egui;

use super::recall_scope_popup::{RecallPopupKind, RecallScopePopupState};
use super::theme;
use crate::model::channel::ChannelId;
use crate::model::config::ConsoleConfig;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::parameter::{ParameterPath, ParameterSection, TimingCategory};
use crate::model::recall_scope::ConsoleRecallConfig;
use crate::model::snapshot::{CategoryTiming, ChannelScope, ScopeTemplate};
use crate::model::state::ConsoleState;

// ── Channel-type group enum ─────────────────────────────────────────

/// Top-level channel-type grouping for the scope editor matrix layout.
/// Each group has its own matrix in the scope window with its own column count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChannelGroup {
    Inputs,
    Aux,
    Groups,
    Matrix,
    ControlGroups,
    GraphicEq,
    MatrixInputs,
}

impl ChannelGroup {
    pub fn label(&self) -> &'static str {
        match self {
            ChannelGroup::Inputs => "Inputs",
            ChannelGroup::Aux => "Aux",
            ChannelGroup::Groups => "Groups",
            ChannelGroup::Matrix => "Matrix",
            ChannelGroup::ControlGroups => "Control Groups",
            ChannelGroup::GraphicEq => "Graphic EQ",
            ChannelGroup::MatrixInputs => "Matrix Inputs",
        }
    }

    /// All channel groups in display order.
    pub fn all() -> &'static [ChannelGroup] {
        &[
            ChannelGroup::Inputs,
            ChannelGroup::Aux,
            ChannelGroup::Groups,
            ChannelGroup::Matrix,
            ChannelGroup::ControlGroups,
            ChannelGroup::GraphicEq,
            ChannelGroup::MatrixInputs,
        ]
    }

    /// Generate every channel ID in this group given the live console config.
    pub fn channels_from(&self, config: &ConsoleConfig) -> Vec<ChannelId> {
        match self {
            ChannelGroup::Inputs => (1..=config.input_channel_count)
                .map(ChannelId::Input)
                .collect(),
            ChannelGroup::Aux => (1..=config.aux_output_count).map(ChannelId::Aux).collect(),
            ChannelGroup::Groups => (1..=config.group_output_count)
                .map(ChannelId::Group)
                .collect(),
            ChannelGroup::Matrix => (1..=config.matrix_output_count)
                .map(ChannelId::Matrix)
                .collect(),
            ChannelGroup::ControlGroups => (1..=config.control_group_count)
                .map(ChannelId::ControlGroup)
                .collect(),
            ChannelGroup::GraphicEq => (1..=config.graphic_eq_count)
                .map(ChannelId::GraphicEq)
                .collect(),
            ChannelGroup::MatrixInputs => (1..=config.matrix_input_count)
                .map(ChannelId::MatrixInput)
                .collect(),
        }
    }

    /// Representative channel for the group, used by `ParameterPath::applicable_to`
    /// to derive the row catalogue. The actual row count is the same for any
    /// channel within the group.
    pub fn representative_channel(&self) -> ChannelId {
        match self {
            ChannelGroup::Inputs => ChannelId::Input(1),
            ChannelGroup::Aux => ChannelId::Aux(1),
            ChannelGroup::Groups => ChannelId::Group(1),
            ChannelGroup::Matrix => ChannelId::Matrix(1),
            ChannelGroup::ControlGroups => ChannelId::ControlGroup(1),
            ChannelGroup::GraphicEq => ChannelId::GraphicEq(1),
            ChannelGroup::MatrixInputs => ChannelId::MatrixInput(1),
        }
    }
}

// ── ScopeEditorState ────────────────────────────────────────────────

/// Which aspect of the scope the editor is currently editing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScopeEditMode {
    /// Edit which parameters are in/out of scope (default, current behavior).
    #[default]
    Scope,
    /// Edit per-category pre-wait times (section-level DragValue inputs).
    PreWait,
    /// Edit per-category fade times (section-level DragValue inputs).
    Fade,
}

/// Per-frame snapshot of the scope being edited, plus window state.
pub struct ScopeEditorState {
    /// Per-channel selected paths. The source of truth for the editor.
    pub channel_paths: HashMap<ChannelId, HashSet<ParameterPath>>,
    /// Per-channel per-category timing. Edited in PreWait/Fade modes.
    pub channel_timings: HashMap<(ChannelId, TimingCategory), CategoryTiming>,
    /// Which aspect of the scope is being edited.
    pub edit_mode: ScopeEditMode,
    /// Which channel-type groups are expanded in the window.
    /// Default: all collapsed.
    pub expanded_groups: HashSet<ChannelGroup>,
    /// Which (group, parameter-section) pairs are expanded inside an open group.
    /// Default: empty (all collapsed).
    pub expanded_sections: HashSet<(ChannelGroup, ParameterSection)>,
    /// Window open state, controlled by snapshots tab.
    pub window_open: bool,
    /// Snapshot of `channel_paths` taken at window-open time. Used by Cancel
    /// to roll back. None when the window is closed.
    backup: Option<HashMap<ChannelId, HashSet<ParameterPath>>>,
    /// Backup of timings for Cancel.
    timing_backup: Option<HashMap<(ChannelId, TimingCategory), CategoryTiming>>,
    /// Phase C: when true, the scope editor automatically replaces its
    /// selections with the dirty tracker's contents on every change.
    pub auto_preselect_modified: bool,
    /// Phase C: cached generation of the dirty tracker, so the editor knows
    /// when the dirty set has changed and it should refresh.
    pub last_dirty_generation: u64,
    /// Console recall scope & safe config (visual reference).
    pub console_recall: ConsoleRecallConfig,
    /// Popup state for the recall scope/safe editor.
    pub recall_popup: RecallScopePopupState,
}

impl Default for ScopeEditorState {
    fn default() -> Self {
        Self {
            channel_paths: HashMap::new(),
            channel_timings: HashMap::new(),
            edit_mode: ScopeEditMode::Scope,
            expanded_groups: HashSet::new(),
            expanded_sections: HashSet::new(),
            window_open: false,
            backup: None,
            timing_backup: None,
            auto_preselect_modified: false,
            last_dirty_generation: 0,
            console_recall: ConsoleRecallConfig::default(),
            recall_popup: RecallScopePopupState::default(),
        }
    }
}

impl ScopeEditorState {
    /// Open the window. Caller passes the current scope template; the editor
    /// loads it (running the v7→v8 migration if necessary), takes a backup
    /// for Cancel, and sets `window_open = true`. Aux/group/matrix counts are
    /// needed by the migration to enumerate the right paths.
    pub fn open(
        &mut self,
        template: &ScopeTemplate,
        aux_count: u8,
        group_count: u8,
        matrix_count: u8,
    ) {
        // Load selections from the template (with migration).
        let mut channel_paths: HashMap<ChannelId, HashSet<ParameterPath>> = HashMap::new();
        for cs in &template.channel_scopes {
            // Clone so we can run the migration without mutating the source.
            let mut migrated = cs.clone();
            migrated.migrate_sections_to_paths(aux_count, group_count, matrix_count);
            if !migrated.paths.is_empty() {
                channel_paths.insert(migrated.channel.clone(), migrated.paths);
            }
        }

        // Load timings from the template.
        let mut channel_timings: HashMap<(ChannelId, TimingCategory), CategoryTiming> =
            HashMap::new();
        for cs in &template.channel_scopes {
            for (cat, timing) in &cs.category_timings {
                channel_timings.insert((cs.channel.clone(), *cat), timing.clone());
            }
        }

        self.backup = Some(channel_paths.clone());
        self.timing_backup = Some(channel_timings.clone());
        self.channel_paths = channel_paths;
        self.channel_timings = channel_timings;
        self.edit_mode = ScopeEditMode::Scope;
        self.window_open = true;
    }

    /// Cancel: restore the backup and close the window.
    pub fn cancel(&mut self) {
        if let Some(backup) = self.backup.take() {
            self.channel_paths = backup;
        }
        if let Some(backup) = self.timing_backup.take() {
            self.channel_timings = backup;
        }
        self.window_open = false;
    }

    /// Commit: drop the backup and close the window. Caller reads
    /// `to_scope_template()` afterward.
    pub fn commit(&mut self) {
        self.backup = None;
        self.timing_backup = None;
        self.window_open = false;
    }

    /// Build a new path-granularity ScopeTemplate from the current selections,
    /// including per-category timings.
    pub fn to_scope_template(&self, name: String) -> ScopeTemplate {
        // Collect all channels that have either paths or timings.
        let mut all_channels: HashSet<ChannelId> = HashSet::new();
        for (ch, paths) in &self.channel_paths {
            if !paths.is_empty() {
                all_channels.insert(ch.clone());
            }
        }
        for (ch, _) in self.channel_timings.keys() {
            all_channels.insert(ch.clone());
        }

        let channel_scopes: Vec<ChannelScope> = all_channels
            .into_iter()
            .map(|ch| {
                let paths = self.channel_paths.get(&ch).cloned().unwrap_or_default();
                let mut timings: HashMap<TimingCategory, CategoryTiming> = HashMap::new();
                for cat in TimingCategory::all_variants() {
                    if let Some(t) = self.channel_timings.get(&(ch.clone(), *cat)) {
                        // Only store non-default timings
                        if t.pre_wait_secs != 0.0 || t.fade_time_secs != 0.0 {
                            timings.insert(*cat, t.clone());
                        }
                    }
                }
                let mut cs = ChannelScope::new(ch, paths);
                cs.category_timings = timings;
                cs
            })
            .filter(|cs| !cs.paths.is_empty() || !cs.category_timings.is_empty())
            .collect();
        ScopeTemplate::new(name, channel_scopes)
    }

    /// Load directly from a template, without opening the window. Used by
    /// snapshots tab when the user clicks a saved scope template — refreshes
    /// the editor's selections so that opening the window picks them up.
    pub fn load_template(
        &mut self,
        template: &ScopeTemplate,
        aux_count: u8,
        group_count: u8,
        matrix_count: u8,
    ) {
        let mut channel_paths: HashMap<ChannelId, HashSet<ParameterPath>> = HashMap::new();
        for cs in &template.channel_scopes {
            let mut migrated = cs.clone();
            migrated.migrate_sections_to_paths(aux_count, group_count, matrix_count);
            if !migrated.paths.is_empty() {
                channel_paths.insert(migrated.channel.clone(), migrated.paths);
            }
        }
        self.channel_paths = channel_paths;

        // Also load timings.
        let mut channel_timings: HashMap<(ChannelId, TimingCategory), CategoryTiming> =
            HashMap::new();
        for cs in &template.channel_scopes {
            for (cat, timing) in &cs.category_timings {
                channel_timings.insert((cs.channel.clone(), *cat), timing.clone());
            }
        }
        self.channel_timings = channel_timings;
    }

    /// Clear all selections.
    pub fn clear(&mut self) {
        self.channel_paths.clear();
    }

    /// Phase C: replace the editor's selections with the dirty tracker's
    /// contents. Called by the scope window when the user clicks "Select
    /// modified" (one-shot) or while "Auto-preselect modified" is enabled
    /// (every frame the dirty generation changes).
    pub fn apply_dirty_set(&mut self, dirty: &HashMap<ChannelId, HashSet<ParameterPath>>) {
        self.channel_paths.clear();
        for (ch, paths) in dirty {
            if paths.is_empty() {
                continue;
            }
            self.channel_paths.insert(ch.clone(), paths.clone());
        }
    }

    /// Phase C: additive variant — merge the dirty tracker's contents into
    /// the editor's existing selections without clearing first. Useful when
    /// the operator wants "select what I've changed plus the channels I had
    /// already picked manually".
    pub fn merge_dirty_set(&mut self, dirty: &HashMap<ChannelId, HashSet<ParameterPath>>) {
        for (ch, paths) in dirty {
            if paths.is_empty() {
                continue;
            }
            let entry = self.channel_paths.entry(ch.clone()).or_default();
            for p in paths {
                entry.insert(p.clone());
            }
        }
    }

    /// Total number of selected (channel, path) pairs.
    pub fn selection_count(&self) -> usize {
        self.channel_paths.values().map(|s| s.len()).sum()
    }

    // ─── Cell-level toggles ──────────────────────────────────────────

    /// Toggle a single (channel, path) cell. Cleans up empty entries.
    pub fn toggle_cell(&mut self, ch: &ChannelId, path: &ParameterPath) {
        let entry = self.channel_paths.entry(ch.clone()).or_default();
        if entry.contains(path) {
            entry.remove(path);
            if entry.is_empty() {
                self.channel_paths.remove(ch);
            }
        } else {
            entry.insert(path.clone());
        }
    }

    pub fn is_cell_selected(&self, ch: &ChannelId, path: &ParameterPath) -> bool {
        self.channel_paths.get(ch).is_some_and(|s| s.contains(path))
    }

    // ─── Row toggles (one path across many channels) ────────────────

    /// Toggle a row: select that one path across every channel where it is
    /// available. If every available cell is already selected, clears them
    /// instead. Unavailable cells are skipped.
    pub fn toggle_row(
        &mut self,
        path: &ParameterPath,
        channels: &[ChannelId],
        available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ) {
        let all_on = self.is_row_all_selected(path, channels, available);
        for ch in channels {
            if !cell_available(available, ch, path) {
                continue;
            }
            let entry = self.channel_paths.entry(ch.clone()).or_default();
            if all_on {
                entry.remove(path);
                if entry.is_empty() {
                    self.channel_paths.remove(ch);
                }
            } else {
                entry.insert(path.clone());
            }
        }
    }

    pub fn is_row_all_selected(
        &self,
        path: &ParameterPath,
        channels: &[ChannelId],
        available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ) -> bool {
        let mut any_available = false;
        for ch in channels {
            if !cell_available(available, ch, path) {
                continue;
            }
            any_available = true;
            if !self.is_cell_selected(ch, path) {
                return false;
            }
        }
        any_available
    }

    pub fn is_row_any_selected(
        &self,
        path: &ParameterPath,
        channels: &[ChannelId],
        available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ) -> bool {
        channels
            .iter()
            .any(|ch| cell_available(available, ch, path) && self.is_cell_selected(ch, path))
    }

    // ─── Column toggles (one channel across many paths) ─────────────

    /// Toggle a column: select every applicable path for one channel. If
    /// every available cell is already selected, clears them instead.
    pub fn toggle_column(
        &mut self,
        ch: &ChannelId,
        paths: &[ParameterPath],
        available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ) {
        let all_on = self.is_column_all_selected(ch, paths, available);
        for path in paths {
            if !cell_available(available, ch, path) {
                continue;
            }
            let entry = self.channel_paths.entry(ch.clone()).or_default();
            if all_on {
                entry.remove(path);
            } else {
                entry.insert(path.clone());
            }
        }
        if let Some(entry) = self.channel_paths.get(ch) {
            if entry.is_empty() {
                self.channel_paths.remove(ch);
            }
        }
    }

    pub fn is_column_all_selected(
        &self,
        ch: &ChannelId,
        paths: &[ParameterPath],
        available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ) -> bool {
        let mut any_available = false;
        for path in paths {
            if !cell_available(available, ch, path) {
                continue;
            }
            any_available = true;
            if !self.is_cell_selected(ch, path) {
                return false;
            }
        }
        any_available
    }

    pub fn is_column_any_selected(
        &self,
        ch: &ChannelId,
        paths: &[ParameterPath],
        available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ) -> bool {
        paths
            .iter()
            .any(|path| cell_available(available, ch, path) && self.is_cell_selected(ch, path))
    }

    // ─── Section row toggles (every path in a section, every channel) ─

    /// Toggle every (channel, path) cell where the path is in
    /// `section_paths`. The headline interaction: click "EQ" → all of EQ
    /// across every channel in the group flips on or off in one click.
    pub fn toggle_section_row(
        &mut self,
        section_paths: &[ParameterPath],
        channels: &[ChannelId],
        available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ) {
        let all_on = self.is_section_all_selected(section_paths, channels, available);
        for ch in channels {
            for path in section_paths {
                if !cell_available(available, ch, path) {
                    continue;
                }
                let entry = self.channel_paths.entry(ch.clone()).or_default();
                if all_on {
                    entry.remove(path);
                } else {
                    entry.insert(path.clone());
                }
            }
        }
        // Clean up emptied channel entries.
        self.channel_paths.retain(|_, v| !v.is_empty());
    }

    pub fn is_section_all_selected(
        &self,
        section_paths: &[ParameterPath],
        channels: &[ChannelId],
        available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ) -> bool {
        let mut any_available = false;
        for ch in channels {
            for path in section_paths {
                if !cell_available(available, ch, path) {
                    continue;
                }
                any_available = true;
                if !self.is_cell_selected(ch, path) {
                    return false;
                }
            }
        }
        any_available
    }

    pub fn is_section_any_selected(
        &self,
        section_paths: &[ParameterPath],
        channels: &[ChannelId],
        available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ) -> bool {
        for ch in channels {
            for path in section_paths {
                if cell_available(available, ch, path) && self.is_cell_selected(ch, path) {
                    return true;
                }
            }
        }
        false
    }

    /// Toggle the cells of one section row for ONE channel — the section
    /// header's per-channel cell. Selects every applicable path in
    /// `section_paths` for `ch`, or clears them if all already selected.
    pub fn toggle_section_column(
        &mut self,
        section_paths: &[ParameterPath],
        ch: &ChannelId,
        available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ) {
        let all_on = section_paths
            .iter()
            .all(|path| !cell_available(available, ch, path) || self.is_cell_selected(ch, path))
            && section_paths
                .iter()
                .any(|path| cell_available(available, ch, path));
        for path in section_paths {
            if !cell_available(available, ch, path) {
                continue;
            }
            let entry = self.channel_paths.entry(ch.clone()).or_default();
            if all_on {
                entry.remove(path);
            } else {
                entry.insert(path.clone());
            }
        }
        if let Some(entry) = self.channel_paths.get(ch) {
            if entry.is_empty() {
                self.channel_paths.remove(ch);
            }
        }
    }

    // ─── Whole-group toggle (corner [All] button) ────────────────────

    /// Toggle every (channel, path) cell in the group. Skips unavailable cells.
    pub fn toggle_all(
        &mut self,
        channels: &[ChannelId],
        paths: &[ParameterPath],
        available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ) {
        let all_on = self.is_all_selected(channels, paths, available);
        for ch in channels {
            for path in paths {
                if !cell_available(available, ch, path) {
                    continue;
                }
                let entry = self.channel_paths.entry(ch.clone()).or_default();
                if all_on {
                    entry.remove(path);
                } else {
                    entry.insert(path.clone());
                }
            }
        }
        self.channel_paths.retain(|_, v| !v.is_empty());
    }

    pub fn is_all_selected(
        &self,
        channels: &[ChannelId],
        paths: &[ParameterPath],
        available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ) -> bool {
        let mut any_available = false;
        for ch in channels {
            for path in paths {
                if !cell_available(available, ch, path) {
                    continue;
                }
                any_available = true;
                if !self.is_cell_selected(ch, path) {
                    return false;
                }
            }
        }
        any_available
    }

    pub fn is_any_selected(
        &self,
        channels: &[ChannelId],
        paths: &[ParameterPath],
        available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ) -> bool {
        for ch in channels {
            for path in paths {
                if cell_available(available, ch, path) && self.is_cell_selected(ch, path) {
                    return true;
                }
            }
        }
        false
    }
}

/// True if the (channel, path) cell has live data on the console (i.e. the
/// availability map has the path under the channel). Centralised so the bulk
/// toggles all share one definition.
fn cell_available(
    available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ch: &ChannelId,
    path: &ParameterPath,
) -> bool {
    available.get(ch).is_some_and(|s| s.contains(path))
}

// ── Window-rendering entrypoint ─────────────────────────────────────

/// Outcome of a single `draw_scope_window` frame. Carries the window
/// open/close result plus any deferred side effects the caller needs to
/// dispatch on the dirty tracker (which the editor only has read access to
/// during render).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScopeWindowOutcome {
    pub status: ScopeWindowResult,
    /// True if the operator clicked "Clear changes" — the caller should
    /// acquire a write lock on the dirty tracker and call `clear()`.
    pub clear_dirty_requested: bool,
}

/// Result of a single `draw_scope_window` frame.
#[derive(Debug, Default, PartialEq, Eq)]
pub enum ScopeWindowResult {
    /// Window is still open and waiting for user input.
    #[default]
    StillOpen,
    /// User clicked OK; caller should read `to_scope_template`.
    Committed,
    /// User clicked Cancel or closed the window; selections were rolled back.
    Cancelled,
}

/// Render the scope editor window. Early-returns `StillOpen` (without drawing)
/// when `state.window_open` is false. The caller should invoke this from the
/// App-level `update()` so the window floats above the central panel.
///
/// `dirty` is the live dirty tracker borrow (from `try_read()`). When `None`
/// the toolbar's three "modified" controls render greyed out — there's no
/// data to show.
pub fn draw_scope_window(
    ctx: &egui::Context,
    state: &mut ScopeEditorState,
    console_state: &ConsoleState,
    dirty: Option<&DirtyTracker>,
) -> ScopeWindowOutcome {
    if !state.window_open {
        return ScopeWindowOutcome::default();
    }

    // Phase C: if auto-preselect is on AND the dirty tracker's generation
    // has changed since we last looked, replace the selections with the
    // dirty set. Done BEFORE rendering so the matrix shows the new state.
    if state.auto_preselect_modified {
        if let Some(d) = dirty {
            if d.generation() != state.last_dirty_generation {
                state.apply_dirty_set(d.dirty_set());
                state.last_dirty_generation = d.generation();
            }
        }
    }

    // Snapshot config + channel names once per frame, BEFORE entering the
    // egui::Window closure that holds an exclusive borrow of state.
    let config = console_state.config.clone();
    let aux_count = config.aux_output_count;
    let group_count = config.group_output_count;
    let matrix_count = config.matrix_input_count;

    // Build per-group data: channels, applicable paths, availability map,
    // channel-name lookup. Done up front so the inner closures don't need
    // ConsoleState anymore.
    //
    // Availability is **static** — sourced from ParameterPath::available_for_channel
    // (the CSV table). Every cell whose path is applicable to the channel type
    // is selectable, regardless of whether the live console has pushed a value
    // for it yet. This lets the operator build a scope BEFORE connecting, and
    // it lets aux/group/matrix/CG cells be selectable from the moment the
    // window opens (the previous live-data check left them all greyed until GP
    // OSC discovery populated the parameter mirror, which made it look like
    // those channel types were missing from the scope).
    //
    // Capturing a (channel, path) cell that has no live value just produces no
    // entry in the snapshot — ConsoleState::capture skips missing parameters
    // silently. No harm.
    let groups_data: Vec<GroupRenderData> = ChannelGroup::all()
        .iter()
        .map(|g| {
            let channels = g.channels_from(&config);
            let paths = ParameterPath::applicable_to(
                &g.representative_channel(),
                aux_count,
                group_count,
                matrix_count,
            );
            // Build the availability map: every (channel, path) pair where the
            // path is statically applicable to the channel type. Within a
            // group every channel has the same type, so every channel gets the
            // same set of available paths. ParameterPath::applicable_to has
            // already filtered the rows, so the set is just `paths` cloned.
            let path_set: HashSet<ParameterPath> = paths.iter().cloned().collect();
            let available: HashMap<ChannelId, HashSet<ParameterPath>> = channels
                .iter()
                .map(|ch| (ch.clone(), path_set.clone()))
                .collect();
            let channel_names = console_state.channel_names_for(&channels);
            // Per-group dirty subset (only the channels in this group).
            let mut dirty_subset: HashMap<ChannelId, HashSet<ParameterPath>> = HashMap::new();
            if let Some(d) = dirty {
                let full = d.dirty_set();
                for ch in &channels {
                    if let Some(paths) = full.get(ch) {
                        if !paths.is_empty() {
                            dirty_subset.insert(ch.clone(), paths.clone());
                        }
                    }
                }
            }
            GroupRenderData {
                group: *g,
                channels,
                paths,
                available,
                channel_names,
                config: config.clone(),
                dirty: dirty_subset,
            }
        })
        .collect();

    let mut outcome = ScopeWindowOutcome::default();
    let mut still_open = state.window_open;
    let dirty_has_any = dirty.map(|d| d.has_any()).unwrap_or(false);

    egui::Window::new("Snapshot Scope")
        .collapsible(false)
        .resizable(true)
        .default_size([1100.0, 700.0])
        .open(&mut still_open)
        .show(ctx, |ui| {
            // ─ Toolbar ─
            ui.horizontal(|ui| {
                // ── Mode toggle ──
                let mode_btn_size = egui::Vec2::new(70.0, 28.0);
                if ui
                    .add(
                        egui::Button::new("Scope")
                            .selected(state.edit_mode == ScopeEditMode::Scope)
                            .min_size(mode_btn_size),
                    )
                    .clicked()
                {
                    state.edit_mode = ScopeEditMode::Scope;
                }
                if ui
                    .add(
                        egui::Button::new("Pre-wait")
                            .selected(state.edit_mode == ScopeEditMode::PreWait)
                            .min_size(mode_btn_size),
                    )
                    .clicked()
                {
                    state.edit_mode = ScopeEditMode::PreWait;
                }
                if ui
                    .add(
                        egui::Button::new("Fade")
                            .selected(state.edit_mode == ScopeEditMode::Fade)
                            .min_size(mode_btn_size),
                    )
                    .clicked()
                {
                    state.edit_mode = ScopeEditMode::Fade;
                }
                ui.separator();
                ui.add_space(8.0);

                let clear_btn = theme::action_button(
                    "Clear All",
                    theme::BG_ELEVATED,
                    egui::Vec2::new(80.0, 28.0),
                );
                if ui.add(clear_btn).clicked() {
                    state.clear();
                }
                ui.add_space(8.0);
                theme::colored_badge(
                    ui,
                    &format!("{} selections", state.selection_count()),
                    theme::ACCENT_BLUE,
                );
                ui.add_space(16.0);

                // ── Phase C: dirty tracker controls ──
                ui.separator();
                ui.add_space(8.0);
                let dirty_available = dirty.is_some();

                // "Auto-preselect modified" toggle.
                let auto_resp = ui.add_enabled(
                    dirty_available,
                    egui::Checkbox::new(
                        &mut state.auto_preselect_modified,
                        "Auto-preselect modified",
                    ),
                );
                if auto_resp.changed() && state.auto_preselect_modified {
                    if let Some(d) = dirty {
                        state.apply_dirty_set(d.dirty_set());
                        state.last_dirty_generation = d.generation();
                    }
                }

                // "Select modified" one-shot button (hidden when auto is on,
                // following WFS-DIY behaviour — the auto toggle subsumes it).
                if !state.auto_preselect_modified {
                    let select_btn = theme::action_button(
                        "Select modified",
                        theme::ACCENT_BLUE,
                        egui::Vec2::new(130.0, 28.0),
                    );
                    if ui
                        .add_enabled(dirty_available && dirty_has_any, select_btn)
                        .clicked()
                    {
                        if let Some(d) = dirty {
                            state.apply_dirty_set(d.dirty_set());
                            state.last_dirty_generation = d.generation();
                        }
                    }
                }

                // "Clear changes" — wipe the dirty set without sending
                // anything. The actual clear happens in the caller after
                // this frame returns (we only have a borrow of the tracker).
                let clear_changes_btn = theme::action_button(
                    "Clear changes",
                    theme::BG_ELEVATED,
                    egui::Vec2::new(120.0, 28.0),
                );
                if ui
                    .add_enabled(dirty_available && dirty_has_any, clear_changes_btn)
                    .clicked()
                {
                    outcome.clear_dirty_requested = true;
                    state.last_dirty_generation = 0;
                }

                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Click any header to bulk toggle.")
                        .color(theme::TEXT_SECONDARY)
                        .size(theme::FONT_SIZE_BADGE),
                );
            });
            ui.add_space(6.0);
            ui.separator();

            // ─ Per-group matrices (scroll area leaves room for footer) ─
            let footer_height = 60.0; // footer row + separator + margins
            let available = ui.available_height() - footer_height;
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .max_height(available.max(100.0))
                .show(ui, |ui| {
                    for data in &groups_data {
                        if data.channels.is_empty() {
                            continue;
                        }
                        draw_channel_group_block(ui, state, data);
                        ui.add_space(8.0);
                    }
                });

            // ─ Footer: Console Recall + OK/Cancel ─
            ui.separator();
            ui.horizontal(|ui| {
                // Console Recall buttons (left side)
                ui.label(
                    egui::RichText::new("Console Recall:")
                        .color(theme::TEXT_SECONDARY)
                        .small(),
                );
                let btn_size = egui::Vec2::new(100.0, 24.0);
                let scope_btn =
                    egui::Button::new(egui::RichText::new("Session Scope").size(10.0).color(
                        if state.console_recall.session_scope.active_blocks.is_empty() {
                            theme::TEXT_SECONDARY
                        } else {
                            egui::Color32::from_rgb(0, 180, 0)
                        },
                    ))
                    .fill(theme::BG_ELEVATED)
                    .min_size(btn_size);
                if ui.add(scope_btn).clicked() {
                    state.recall_popup.open = Some(RecallPopupKind::SessionScope);
                }
                for (label, kind) in [
                    ("Input Safe", RecallPopupKind::InputSafe),
                    ("Aux Safe", RecallPopupKind::AuxSafe),
                    ("Group Safe", RecallPopupKind::GroupSafe),
                    ("Matrix Safe", RecallPopupKind::MatrixSafe),
                    ("CG Safe", RecallPopupKind::CgSafe),
                ] {
                    let btn = egui::Button::new(egui::RichText::new(label).size(10.0))
                        .fill(theme::BG_ELEVATED)
                        .min_size(egui::Vec2::new(72.0, 24.0));
                    if ui.add(btn).clicked() {
                        state.recall_popup.open = Some(kind);
                        state.recall_popup.selected_channel = 1;
                    }
                }

                // OK / Cancel (right side)
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok_btn = theme::action_button(
                        "OK",
                        theme::ACCENT_GREEN,
                        egui::Vec2::new(90.0, 28.0),
                    );
                    if ui.add(ok_btn).clicked() {
                        state.commit();
                        outcome.status = ScopeWindowResult::Committed;
                    }
                    ui.add_space(8.0);
                    let cancel_btn = theme::action_button(
                        "Cancel",
                        theme::BG_ELEVATED,
                        egui::Vec2::new(90.0, 28.0),
                    );
                    if ui.add(cancel_btn).clicked() {
                        state.cancel();
                        outcome.status = ScopeWindowResult::Cancelled;
                    }
                });
            });
        });

    // Honour the X close button (which flips `still_open` to false). Treat
    // closing-via-X as Cancel — discard pending changes.
    if !still_open && state.window_open && outcome.status == ScopeWindowResult::StillOpen {
        state.cancel();
        outcome.status = ScopeWindowResult::Cancelled;
    }

    // Render the recall scope/safe popup if open
    super::recall_scope_popup::draw_recall_popup(
        ctx,
        &mut state.recall_popup,
        &mut state.console_recall,
        console_state,
    );

    outcome
}

/// Per-group data assembled once per frame, before drawing.
struct GroupRenderData {
    group: ChannelGroup,
    channels: Vec<ChannelId>,
    paths: Vec<ParameterPath>,
    available: HashMap<ChannelId, HashSet<ParameterPath>>,
    channel_names: HashMap<ChannelId, String>,
    /// Cloned snapshot of the live console config, used by
    /// `ParameterPath::label_with_config` to render bus rows with their
    /// current "Aux N" / "Group N" labels.
    config: ConsoleConfig,
    /// Phase C: per-channel dirty path set, sliced from the global dirty
    /// tracker. Empty when the dirty tracker is unavailable. Used by the
    /// matrix cell renderer to draw the golden earmark.
    dirty: HashMap<ChannelId, HashSet<ParameterPath>>,
}

/// Draw one channel-type group: collapsible header + (when expanded) matrix.
fn draw_channel_group_block(
    ui: &mut egui::Ui,
    state: &mut ScopeEditorState,
    data: &GroupRenderData,
) {
    // Group header row: triangle + label + tristate cell + count.
    let group_expanded = state.expanded_groups.contains(&data.group);
    let group_all = state.is_all_selected(&data.channels, &data.paths, &data.available);
    let group_any = state.is_any_selected(&data.channels, &data.paths, &data.available);

    egui::Frame::new()
        .fill(theme::BG_ELEVATED)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Triangle: expand/collapse the group.
                let triangle = if group_expanded { "▼" } else { "▶" };
                let tri_resp = ui.add(
                    egui::Label::new(
                        egui::RichText::new(triangle)
                            .color(theme::TEXT_PRIMARY)
                            .size(theme::FONT_SIZE_BODY),
                    )
                    .sense(egui::Sense::click()),
                );
                if tri_resp.clicked() {
                    if group_expanded {
                        state.expanded_groups.remove(&data.group);
                    } else {
                        state.expanded_groups.insert(data.group);
                    }
                }
                ui.add_space(4.0);

                // Group label: clicking it bulk-toggles every cell in the group.
                let label_resp = ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "{} ({})",
                            data.group.label(),
                            data.channels.len()
                        ))
                        .strong()
                        .color(theme::TEXT_PRIMARY)
                        .size(theme::FONT_SIZE_SECTION),
                    )
                    .sense(egui::Sense::click()),
                );
                if label_resp.clicked() {
                    state.toggle_all(&data.channels, &data.paths, &data.available);
                }

                ui.add_space(8.0);

                // Group tristate cell.
                let tri_cell = matrix_cell(
                    ui,
                    egui::Vec2::new(28.0, 20.0),
                    group_all,
                    group_any,
                    true,
                    /* dirty */ false,
                    "",
                );
                if tri_cell.clicked() {
                    state.toggle_all(&data.channels, &data.paths, &data.available);
                }
            });
        });

    // Body: matrix only if expanded.
    if group_expanded {
        egui::Frame::new()
            .fill(theme::BG_PANEL)
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 6))
            .show(ui, |ui| {
                draw_group_matrix(ui, state, data);
            });
    }
}

const CELL_SIZE: egui::Vec2 = egui::Vec2 { x: 22.0, y: 18.0 };
const CELL_SPACING: f32 = 3.0;
const ROW_LABEL_WIDTH: f32 = 170.0;

/// Draw the matrix for one expanded channel-type group.
fn draw_group_matrix(ui: &mut egui::Ui, state: &mut ScopeEditorState, data: &GroupRenderData) {
    if data.paths.is_empty() {
        ui.label(
            egui::RichText::new("(no parameters)")
                .color(theme::TEXT_SECONDARY)
                .small(),
        );
        return;
    }

    // Group paths by section, in section enum order. Rely on
    // ParameterPath::applicable_to returning paths in signal-flow order;
    // walking it once and grouping consecutive same-section paths gives a
    // stable ordering.
    let paths_by_section = group_paths_by_section(&data.paths);

    egui::ScrollArea::horizontal()
        .id_salt(("scope_group_hscroll", data.group))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Zero out horizontal item spacing — all spacing is manual via add_space.
            ui.spacing_mut().item_spacing.x = 0.0;

            // ── Header row: corner [All] + per-channel header cells ──
            ui.horizontal(|ui| {
                // Corner [All] cell — width = row label + spacing.
                let corner_all =
                    state.is_all_selected(&data.channels, &data.paths, &data.available);
                let corner_any =
                    state.is_any_selected(&data.channels, &data.paths, &data.available);
                let corner_resp = matrix_cell(
                    ui,
                    egui::Vec2::new(ROW_LABEL_WIDTH, CELL_SIZE.y),
                    corner_all,
                    corner_any,
                    true,
                    /* dirty */ false,
                    "All",
                );
                if corner_resp.clicked() {
                    state.toggle_all(&data.channels, &data.paths, &data.available);
                }
                ui.add_space(CELL_SPACING);

                // Per-channel column headers.
                for ch in &data.channels {
                    let col_all = state.is_column_all_selected(ch, &data.paths, &data.available);
                    let col_any = state.is_column_any_selected(ch, &data.paths, &data.available);
                    let label = channel_short_label(ch);
                    // Channel header is "dirty" if any cell under it is dirty.
                    let col_dirty = data.dirty.get(ch).is_some_and(|s| !s.is_empty());
                    let resp =
                        matrix_cell(ui, CELL_SIZE, col_all, col_any, true, col_dirty, &label);
                    if resp.clicked() {
                        state.toggle_column(ch, &data.paths, &data.available);
                    }
                    let tooltip = match data.channel_names.get(ch) {
                        Some(n) => format!("{ch} — {n}"),
                        None => format!("{ch}"),
                    };
                    let _ = resp.on_hover_text(tooltip);
                    ui.add_space(CELL_SPACING);
                }
            });

            // ── Section blocks ──
            for (section, section_paths) in &paths_by_section {
                let key = (data.group, section.clone());
                let expanded = state.expanded_sections.contains(&key);

                let sec_all =
                    state.is_section_all_selected(section_paths, &data.channels, &data.available);
                let sec_any =
                    state.is_section_any_selected(section_paths, &data.channels, &data.available);

                // Section header row: triangle + label + per-channel section cells.
                ui.horizontal(|ui| {
                    let row = section_header_row(
                        ui,
                        if expanded { "▼" } else { "▶" },
                        &section_display_name(section),
                        sec_all,
                        sec_any,
                    );
                    match row {
                        SectionHeaderClick::Triangle => {
                            if expanded {
                                state.expanded_sections.remove(&key);
                            } else {
                                state.expanded_sections.insert(key.clone());
                            }
                        }
                        SectionHeaderClick::Label => {
                            state.toggle_section_row(
                                section_paths,
                                &data.channels,
                                &data.available,
                            );
                        }
                        SectionHeaderClick::None => {}
                    }
                    ui.add_space(CELL_SPACING);

                    // Per-channel cells for the section header row.
                    if state.edit_mode == ScopeEditMode::Scope {
                        // Scope mode: tristate toggle cells (existing behavior)
                        for ch in &data.channels {
                            let mut any_available = false;
                            let mut all_on = true;
                            let mut any_on = false;
                            for path in section_paths {
                                if !cell_available(&data.available, ch, path) {
                                    continue;
                                }
                                any_available = true;
                                if state.is_cell_selected(ch, path) {
                                    any_on = true;
                                } else {
                                    all_on = false;
                                }
                            }
                            let cell_all = any_available && all_on;
                            let cell_any = any_on;
                            let cell_dirty = data
                                .dirty
                                .get(ch)
                                .is_some_and(|set| section_paths.iter().any(|p| set.contains(p)));
                            // Check if this section+channel has any timing configured
                            let has_timing =
                                TimingCategory::for_section(section).iter().any(|cat| {
                                    let t = state
                                        .channel_timings
                                        .get(&(ch.clone(), *cat))
                                        .cloned()
                                        .unwrap_or_default();
                                    t.pre_wait_secs != 0.0 || t.fade_time_secs != 0.0
                                });
                            let resp = matrix_cell(
                                ui,
                                CELL_SIZE,
                                cell_all,
                                cell_any,
                                any_available,
                                cell_dirty || has_timing, // show timing indicator via earmark
                                "",
                            );
                            if resp.clicked() && any_available {
                                state.toggle_section_column(section_paths, ch, &data.available);
                            }
                            ui.add_space(CELL_SPACING);
                        }
                    } else {
                        // PreWait / Fade mode: render dimmed placeholder cells
                        // to keep column alignment with the timing rows.
                        for ch in &data.channels {
                            let mut any_available = false;
                            let mut all_on = true;
                            let mut any_on = false;
                            for path in section_paths {
                                if !cell_available(&data.available, ch, path) {
                                    continue;
                                }
                                any_available = true;
                                if state.is_cell_selected(ch, path) {
                                    any_on = true;
                                } else {
                                    all_on = false;
                                }
                            }
                            let cell_all = any_available && all_on;
                            let cell_any = any_on;
                            matrix_cell(
                                ui, CELL_SIZE, cell_all, cell_any,
                                false, // not interactive in timing mode
                                false, "",
                            );
                            ui.add_space(CELL_SPACING);
                        }
                    }
                });

                // Per-category timing rows (only in PreWait/Fade modes).
                if state.edit_mode != ScopeEditMode::Scope {
                    let categories = TimingCategory::for_section(section);
                    for cat in categories {
                        // Skip Mute in Fade mode (Mute has no fade)
                        if state.edit_mode == ScopeEditMode::Fade && !cat.supports_fade() {
                            continue;
                        }
                        ui.horizontal(|ui| {
                            // Row label: indented, showing category + mode.
                            // Uses allocate_exact_size + manual paint to match
                            // path_row_label alignment exactly.
                            let label_text = match state.edit_mode {
                                ScopeEditMode::PreWait => format!("  {} pre-wait (s)", cat.label()),
                                ScopeEditMode::Fade => format!("  {} fade (s)", cat.label()),
                                _ => unreachable!(),
                            };
                            let (rect, _resp) = ui.allocate_exact_size(
                                egui::Vec2::new(ROW_LABEL_WIDTH, CELL_SIZE.y),
                                egui::Sense::hover(),
                            );
                            let galley = ui.painter().layout_no_wrap(
                                label_text,
                                egui::FontId::proportional(theme::FONT_SIZE_BADGE),
                                theme::TEXT_SECONDARY,
                            );
                            let text_pos = egui::pos2(
                                rect.left() + 4.0,
                                rect.center().y - galley.size().y / 2.0,
                            );
                            ui.painter().galley(text_pos, galley, theme::TEXT_SECONDARY);
                            ui.add_space(CELL_SPACING);

                            // Per-channel timing inputs — paint value text inside
                            // allocate_exact_size cells to match matrix_cell alignment.
                            for ch in &data.channels {
                                let key = (ch.clone(), *cat);
                                let timing = state
                                    .channel_timings
                                    .entry(key.clone())
                                    .or_default();
                                let val = match state.edit_mode {
                                    ScopeEditMode::PreWait => &mut timing.pre_wait_secs,
                                    ScopeEditMode::Fade => &mut timing.fade_time_secs,
                                    _ => unreachable!(),
                                };

                                let (rect, resp) = ui
                                    .allocate_exact_size(CELL_SIZE, egui::Sense::click_and_drag());

                                // Background
                                let bg = if *val != 0.0 {
                                    theme::SCOPE_ACTIVE
                                } else {
                                    theme::SCOPE_INACTIVE
                                };
                                let bg = if resp.hovered() {
                                    theme::lighten(bg, 25)
                                } else {
                                    bg
                                };
                                ui.painter().rect_filled(rect, 3.0, bg);

                                // Display value
                                let text = format!("{:.1}", val);
                                let galley = ui.painter().layout_no_wrap(
                                    text,
                                    egui::FontId::proportional(8.0),
                                    theme::TEXT_PRIMARY,
                                );
                                let text_pos = egui::pos2(
                                    rect.center().x - galley.size().x / 2.0,
                                    rect.center().y - galley.size().y / 2.0,
                                );
                                ui.painter().galley(text_pos, galley, theme::TEXT_PRIMARY);

                                // Drag interaction
                                if resp.dragged() {
                                    let delta = resp.drag_delta().x * 0.05;
                                    *val = (*val + delta).clamp(0.0, 30.0);
                                }

                                ui.add_space(CELL_SPACING);
                            }
                        });
                    }
                }

                // Path rows (only if expanded, and only in Scope mode or when section is expanded).
                if expanded {
                    for path in section_paths {
                        ui.horizontal(|ui| {
                            // Row label (clickable: bulk-toggle this path
                            // across the whole group).
                            let row_all =
                                state.is_row_all_selected(path, &data.channels, &data.available);
                            let row_any =
                                state.is_row_any_selected(path, &data.channels, &data.available);
                            let row_resp = path_row_label(
                                ui,
                                &path.label_with_config(&data.config),
                                row_all,
                                row_any,
                            );
                            if row_resp.clicked() {
                                state.toggle_row(path, &data.channels, &data.available);
                            }
                            ui.add_space(CELL_SPACING);

                            // Per-channel cells.
                            for ch in &data.channels {
                                let avail = cell_available(&data.available, ch, path);
                                let sel = state.is_cell_selected(ch, path);
                                let cell_dirty =
                                    data.dirty.get(ch).is_some_and(|s| s.contains(path));
                                if state.edit_mode == ScopeEditMode::Scope {
                                    let conflict =
                                        sel && state.console_recall.is_console_recalled(ch, path);
                                    let resp = matrix_cell_ex(
                                        ui, CELL_SIZE, sel, sel, avail, cell_dirty, conflict, "",
                                    );
                                    if resp.clicked() && avail {
                                        state.toggle_cell(ch, path);
                                    }
                                    if !avail {
                                        let _ = resp.on_hover_text("no live parameter");
                                    }
                                } else {
                                    // In timing modes, path cells are dimmed/read-only
                                    matrix_cell(
                                        ui, CELL_SIZE, sel, sel,
                                        false, // not available = greyed
                                        false, "",
                                    );
                                }
                                ui.add_space(CELL_SPACING);
                            }
                        });
                    }
                }
            }
        });
}

/// Group an `applicable_to` slice into per-section runs while preserving the
/// signal-flow order. Returns a Vec of (section, paths-in-that-section).
fn group_paths_by_section(paths: &[ParameterPath]) -> Vec<(ParameterSection, Vec<ParameterPath>)> {
    let mut out: Vec<(ParameterSection, Vec<ParameterPath>)> = Vec::new();
    for path in paths {
        let s = path.section();
        if let Some((cur_section, cur_paths)) = out.last_mut() {
            if *cur_section == s {
                cur_paths.push(path.clone());
                continue;
            }
        }
        out.push((s, vec![path.clone()]));
    }
    out
}

/// Friendly section name shown in the section header row.
fn section_display_name(s: &ParameterSection) -> String {
    match s {
        ParameterSection::Name => "Identity".into(),
        ParameterSection::FaderMutePan => "Fader / Mute / Pan".into(),
        ParameterSection::InputGain => "Input".into(),
        ParameterSection::Delay => "Delay".into(),
        ParameterSection::Digitube => "DiGiTube".into(),
        ParameterSection::Eq => "EQ".into(),
        ParameterSection::Dyn1 => "Dynamics 1".into(),
        ParameterSection::Dyn2 => "Dynamics 2".into(),
        ParameterSection::Sends => "Aux Sends".into(),
        ParameterSection::GroupRouting => "Group Routing".into(),
        ParameterSection::Inserts => "Inserts".into(),
        ParameterSection::CgMembership => "CG Membership".into(),
        ParameterSection::GraphicEq => "Graphic EQ".into(),
        ParameterSection::MatrixSends => "Matrix Sends".into(),
    }
}

/// Compact channel column-header text. Shows just the index ("1", "2"…).
fn channel_short_label(ch: &ChannelId) -> String {
    match ch {
        ChannelId::Input(n)
        | ChannelId::Aux(n)
        | ChannelId::Group(n)
        | ChannelId::Matrix(n)
        | ChannelId::ControlGroup(n)
        | ChannelId::GraphicEq(n)
        | ChannelId::MatrixInput(n) => n.to_string(),
    }
}

// ── Cell rendering primitives ──────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
enum SectionHeaderClick {
    None,
    Triangle,
    Label,
}

/// Render a section header row's leading triangle + label as one combined
/// horizontal item. Returns which sub-area was clicked, if any.
fn section_header_row(
    ui: &mut egui::Ui,
    triangle: &str,
    label: &str,
    all_selected: bool,
    any_selected: bool,
) -> SectionHeaderClick {
    let mut click = SectionHeaderClick::None;

    let total_width = ROW_LABEL_WIDTH;
    let triangle_width: f32 = 16.0;
    let label_width = total_width - triangle_width;

    // Triangle area
    let (tri_rect, tri_resp) = ui.allocate_exact_size(
        egui::Vec2::new(triangle_width, CELL_SIZE.y),
        egui::Sense::click(),
    );
    let tri_galley = ui.painter().layout_no_wrap(
        triangle.to_string(),
        egui::FontId::proportional(theme::FONT_SIZE_BODY),
        theme::TEXT_PRIMARY,
    );
    let tri_pos = egui::pos2(
        tri_rect.left() + 2.0,
        tri_rect.center().y - tri_galley.size().y / 2.0,
    );
    ui.painter()
        .galley(tri_pos, tri_galley, theme::TEXT_PRIMARY);
    if tri_resp.clicked() {
        click = SectionHeaderClick::Triangle;
    }

    // Label area (clickable bulk-toggle target).
    let (label_rect, label_resp) = ui.allocate_exact_size(
        egui::Vec2::new(label_width, CELL_SIZE.y),
        egui::Sense::click(),
    );

    // Background tint based on tristate selection.
    let bg = if all_selected {
        theme::SCOPE_ACTIVE
    } else if any_selected {
        theme::SCOPE_PARTIAL
    } else {
        theme::BG_ELEVATED
    };
    let bg = if label_resp.hovered() {
        theme::lighten(bg, 25)
    } else {
        bg
    };
    ui.painter().rect_filled(label_rect, 3.0, bg);

    let text_color = if all_selected || any_selected {
        theme::TEXT_PRIMARY
    } else {
        theme::TEXT_SECONDARY
    };
    let label_galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(theme::FONT_SIZE_BADGE),
        text_color,
    );
    let label_pos = egui::pos2(
        label_rect.left() + 6.0,
        label_rect.center().y - label_galley.size().y / 2.0,
    );
    ui.painter().galley(label_pos, label_galley, text_color);

    if label_resp.clicked() {
        click = SectionHeaderClick::Label;
    }

    click
}

/// Render a single ParameterPath row label as a clickable bulk-toggle target.
fn path_row_label(
    ui: &mut egui::Ui,
    label: &str,
    all_selected: bool,
    any_selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::Vec2::new(ROW_LABEL_WIDTH, CELL_SIZE.y),
        egui::Sense::click(),
    );
    let bg = if all_selected {
        theme::SCOPE_ACTIVE
    } else if any_selected {
        theme::SCOPE_PARTIAL
    } else {
        theme::BG_PANEL
    };
    let bg = if response.hovered() {
        theme::lighten(bg, 25)
    } else {
        bg
    };
    ui.painter().rect_filled(rect, 3.0, bg);

    let text_color = if all_selected || any_selected {
        theme::TEXT_PRIMARY
    } else {
        theme::TEXT_SECONDARY
    };
    let galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(11.0),
        text_color,
    );
    let pos = egui::pos2(rect.left() + 16.0, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, text_color);
    response
}

/// Render a single matrix cell. Available cells respond to clicks; unavailable
/// cells render dim and ignore interaction.
#[allow(clippy::too_many_arguments)]
fn matrix_cell(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    all_selected: bool,
    any_selected: bool,
    available: bool,
    dirty: bool,
    label: &str,
) -> egui::Response {
    matrix_cell_ex(
        ui,
        size,
        all_selected,
        any_selected,
        available,
        dirty,
        false,
        label,
    )
}

fn matrix_cell_ex(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    all_selected: bool,
    any_selected: bool,
    available: bool,
    dirty: bool,
    conflict: bool,
    label: &str,
) -> egui::Response {
    let sense = if available {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);

    let scope_conflict = egui::Color32::from_rgb(50, 100, 200); // blue
    let base = if !available {
        theme::SCOPE_UNAVAILABLE
    } else if all_selected && conflict {
        scope_conflict
    } else if all_selected {
        theme::SCOPE_ACTIVE
    } else if any_selected {
        theme::SCOPE_PARTIAL
    } else {
        theme::SCOPE_INACTIVE
    };
    let fill = if available && response.hovered() {
        theme::lighten(base, 25)
    } else {
        base
    };
    ui.painter().rect_filled(rect, 3.0, fill);

    if !available {
        // Outline-only treatment for unavailable cells.
        ui.painter().rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.0, theme::TEXT_DISABLED),
            egui::StrokeKind::Inside,
        );
    }

    if !label.is_empty() {
        let color = if !available {
            theme::TEXT_DISABLED
        } else if all_selected || any_selected {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        };
        let galley =
            ui.painter()
                .layout_no_wrap(label.to_string(), egui::FontId::proportional(9.0), color);
        let pos = rect.center() - galley.size() / 2.0;
        ui.painter().galley(pos, galley, color);
    }

    // Phase C: golden earmark in the top-right corner when dirty.
    // Triangle ~35% of the cell width — visible without obscuring the label.
    if dirty {
        let earmark_size = (size.x.min(size.y) * 0.35).max(4.0);
        let cx = rect.right() - 1.0;
        let cy = rect.top() + 1.0;
        let points = vec![
            egui::pos2(cx - earmark_size, cy),
            egui::pos2(cx, cy),
            egui::pos2(cx, cy + earmark_size),
        ];
        ui.painter().add(egui::Shape::convex_polygon(
            points,
            theme::SCOPE_DIRTY,
            egui::Stroke::NONE,
        ));
    }

    response
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_available(
        channels: &[ChannelId],
        paths: &[ParameterPath],
    ) -> HashMap<ChannelId, HashSet<ParameterPath>> {
        let mut map: HashMap<ChannelId, HashSet<ParameterPath>> = HashMap::new();
        for ch in channels {
            map.insert(ch.clone(), paths.iter().cloned().collect());
        }
        map
    }

    #[test]
    fn default_state_has_all_groups_collapsed() {
        let s = ScopeEditorState::default();
        assert!(s.expanded_groups.is_empty());
        assert!(s.expanded_sections.is_empty());
        assert!(!s.window_open);
    }

    #[test]
    fn toggle_cell_adds_then_removes_and_cleans_up() {
        let mut s = ScopeEditorState::default();
        let ch = ChannelId::Input(1);
        let p = ParameterPath::Fader;
        s.toggle_cell(&ch, &p);
        assert!(s.is_cell_selected(&ch, &p));
        s.toggle_cell(&ch, &p);
        assert!(!s.is_cell_selected(&ch, &p));
        // Empty set cleaned up.
        assert!(!s.channel_paths.contains_key(&ch));
    }

    #[test]
    fn toggle_row_skips_unavailable_cells() {
        let mut s = ScopeEditorState::default();
        let channels = vec![ChannelId::Input(1), ChannelId::Input(2)];
        // Only Input(1) has the path.
        let mut available = HashMap::new();
        available.insert(ChannelId::Input(1), HashSet::from([ParameterPath::Fader]));
        available.insert(ChannelId::Input(2), HashSet::new());

        s.toggle_row(&ParameterPath::Fader, &channels, &available);
        assert!(s.is_cell_selected(&ChannelId::Input(1), &ParameterPath::Fader));
        assert!(!s.is_cell_selected(&ChannelId::Input(2), &ParameterPath::Fader));
    }

    #[test]
    fn toggle_column_skips_unavailable_cells() {
        let mut s = ScopeEditorState::default();
        let ch = ChannelId::Input(1);
        let paths = vec![ParameterPath::Fader, ParameterPath::Mute];
        // Only Fader is available.
        let mut available = HashMap::new();
        available.insert(ch.clone(), HashSet::from([ParameterPath::Fader]));

        s.toggle_column(&ch, &paths, &available);
        assert!(s.is_cell_selected(&ch, &ParameterPath::Fader));
        assert!(!s.is_cell_selected(&ch, &ParameterPath::Mute));
    }

    #[test]
    fn toggle_section_row_selects_every_path_in_section_for_every_channel() {
        let mut s = ScopeEditorState::default();
        let channels = vec![
            ChannelId::Input(1),
            ChannelId::Input(2),
            ChannelId::Input(3),
        ];
        let section_paths = vec![
            ParameterPath::EqEnabled,
            ParameterPath::EqBandFrequency(1),
            ParameterPath::EqBandGain(1),
        ];
        let available = make_available(&channels, &section_paths);

        s.toggle_section_row(&section_paths, &channels, &available);

        for ch in &channels {
            for path in &section_paths {
                assert!(
                    s.is_cell_selected(ch, path),
                    "{ch} — {path:?} should be selected",
                );
            }
        }
    }

    #[test]
    fn toggle_section_row_clears_when_already_full() {
        let mut s = ScopeEditorState::default();
        let channels = vec![ChannelId::Input(1), ChannelId::Input(2)];
        let section_paths = vec![ParameterPath::EqEnabled, ParameterPath::EqBandFrequency(1)];
        let available = make_available(&channels, &section_paths);

        // Fill.
        s.toggle_section_row(&section_paths, &channels, &available);
        assert!(s.is_section_all_selected(&section_paths, &channels, &available));

        // Clear.
        s.toggle_section_row(&section_paths, &channels, &available);
        assert!(!s.is_section_any_selected(&section_paths, &channels, &available));
        assert_eq!(s.selection_count(), 0);
    }

    #[test]
    fn toggle_all_respects_availability() {
        let mut s = ScopeEditorState::default();
        let channels = vec![ChannelId::Input(1), ChannelId::Input(2)];
        let paths = vec![ParameterPath::Fader, ParameterPath::Mute];
        let mut available = HashMap::new();
        // Input(1) has both, Input(2) has only Fader.
        available.insert(
            ChannelId::Input(1),
            HashSet::from([ParameterPath::Fader, ParameterPath::Mute]),
        );
        available.insert(ChannelId::Input(2), HashSet::from([ParameterPath::Fader]));

        s.toggle_all(&channels, &paths, &available);
        assert!(s.is_cell_selected(&ChannelId::Input(1), &ParameterPath::Fader));
        assert!(s.is_cell_selected(&ChannelId::Input(1), &ParameterPath::Mute));
        assert!(s.is_cell_selected(&ChannelId::Input(2), &ParameterPath::Fader));
        // Unavailable cell stays off.
        assert!(!s.is_cell_selected(&ChannelId::Input(2), &ParameterPath::Mute));
    }

    #[test]
    fn is_row_all_selected_ignores_unavailable_cells() {
        let mut s = ScopeEditorState::default();
        let channels = vec![ChannelId::Input(1), ChannelId::Input(2)];
        // Only Input(1) has the path.
        let mut available = HashMap::new();
        available.insert(ChannelId::Input(1), HashSet::from([ParameterPath::Fader]));
        available.insert(ChannelId::Input(2), HashSet::new());

        s.toggle_cell(&ChannelId::Input(1), &ParameterPath::Fader);
        // Even though Input(2) doesn't have it, the row is "all selected"
        // because every AVAILABLE cell is selected.
        assert!(s.is_row_all_selected(&ParameterPath::Fader, &channels, &available));
    }

    #[test]
    fn cancel_restores_backup() {
        let mut s = ScopeEditorState::default();
        // Pre-populate with {Input(1) → {Fader}}.
        s.channel_paths
            .insert(ChannelId::Input(1), HashSet::from([ParameterPath::Fader]));
        // Open with the current selections.
        let template = s.to_scope_template("test".into());
        s.open(&template, 8, 8, 10);
        assert!(s.window_open);

        // Modify: add Mute.
        s.toggle_cell(&ChannelId::Input(1), &ParameterPath::Mute);
        assert!(s.is_cell_selected(&ChannelId::Input(1), &ParameterPath::Mute));

        // Cancel: should drop the Mute change.
        s.cancel();
        assert!(!s.window_open);
        assert!(s.is_cell_selected(&ChannelId::Input(1), &ParameterPath::Fader));
        assert!(!s.is_cell_selected(&ChannelId::Input(1), &ParameterPath::Mute));
    }

    #[test]
    fn commit_clears_backup_and_keeps_changes() {
        let mut s = ScopeEditorState::default();
        let template = ScopeTemplate::new("empty".into(), vec![]);
        s.open(&template, 8, 8, 10);
        s.toggle_cell(&ChannelId::Input(1), &ParameterPath::Fader);
        s.commit();
        assert!(!s.window_open);
        assert!(s.is_cell_selected(&ChannelId::Input(1), &ParameterPath::Fader));
    }

    #[test]
    fn open_migrates_legacy_section_scope_into_paths() {
        use crate::model::parameter::ParameterSection;
        let mut s = ScopeEditorState::default();
        // v7-style legacy template: only sections, no paths.
        let template = ScopeTemplate::new(
            "Legacy".into(),
            vec![ChannelScope::from_sections(
                ChannelId::Input(1),
                HashSet::from([ParameterSection::FaderMutePan]),
            )],
        );
        s.open(&template, 8, 8, 10);

        // After open, the Input(1) entry should contain every applicable
        // FaderMutePan-section path (Fader, Mute, Solo, Pan).
        let entry = s
            .channel_paths
            .get(&ChannelId::Input(1))
            .expect("Input(1) should have selections after migration");
        assert!(entry.contains(&ParameterPath::Fader));
        assert!(entry.contains(&ParameterPath::Mute));
        assert!(entry.contains(&ParameterPath::Solo));
        assert!(entry.contains(&ParameterPath::Pan));
    }

    #[test]
    fn to_scope_template_writes_paths_only() {
        let mut s = ScopeEditorState::default();
        s.toggle_cell(&ChannelId::Input(1), &ParameterPath::Fader);
        let template = s.to_scope_template("Test".into());
        assert_eq!(template.channel_scopes.len(), 1);
        let cs = &template.channel_scopes[0];
        assert_eq!(cs.channel, ChannelId::Input(1));
        assert!(cs.sections.is_empty());
        assert_eq!(cs.paths.len(), 1);
        assert!(cs.paths.contains(&ParameterPath::Fader));
    }

    #[test]
    fn group_paths_by_section_preserves_order_and_groups_runs() {
        let paths = vec![
            ParameterPath::Name,
            ParameterPath::Fader,
            ParameterPath::Mute,
            ParameterPath::Pan,
            ParameterPath::EqEnabled,
            ParameterPath::EqBandFrequency(1),
        ];
        let grouped = group_paths_by_section(&paths);
        // Three sections expected: Name, FaderMutePan, Eq.
        assert_eq!(grouped.len(), 3);
        assert_eq!(grouped[0].0, ParameterSection::Name);
        assert_eq!(grouped[0].1.len(), 1);
        assert_eq!(grouped[1].0, ParameterSection::FaderMutePan);
        assert_eq!(grouped[1].1.len(), 3);
        assert_eq!(grouped[2].0, ParameterSection::Eq);
        assert_eq!(grouped[2].1.len(), 2);
    }

    #[test]
    fn channel_groups_from_config_match_counts() {
        let config = ConsoleConfig::default();
        assert_eq!(
            ChannelGroup::Inputs.channels_from(&config).len(),
            config.input_channel_count as usize,
        );
        assert_eq!(
            ChannelGroup::Aux.channels_from(&config).len(),
            config.aux_output_count as usize,
        );
        assert_eq!(
            ChannelGroup::ControlGroups.channels_from(&config).len(),
            config.control_group_count as usize,
        );
    }

    // ─── Phase C: dirty-set apply / merge ────────────────────────────

    fn dirty_map(
        entries: &[(ChannelId, &[ParameterPath])],
    ) -> HashMap<ChannelId, HashSet<ParameterPath>> {
        entries
            .iter()
            .map(|(ch, paths)| (ch.clone(), paths.iter().cloned().collect()))
            .collect()
    }

    #[test]
    fn apply_dirty_set_replaces_selections() {
        let mut s = ScopeEditorState::default();
        // Pre-populate with something OTHER than what the dirty set has.
        s.toggle_cell(&ChannelId::Input(99), &ParameterPath::Mute);

        let dirty = dirty_map(&[(ChannelId::Input(1), &[ParameterPath::Fader])]);
        s.apply_dirty_set(&dirty);

        // The pre-existing Input(99) selection is gone.
        assert!(!s.is_cell_selected(&ChannelId::Input(99), &ParameterPath::Mute));
        // The dirty cell is now selected.
        assert!(s.is_cell_selected(&ChannelId::Input(1), &ParameterPath::Fader));
        assert_eq!(s.selection_count(), 1);
    }

    #[test]
    fn apply_dirty_set_drops_empty_channel_entries() {
        let mut s = ScopeEditorState::default();
        let mut dirty: HashMap<ChannelId, HashSet<ParameterPath>> = HashMap::new();
        // Channel with empty path set should NOT show up in selections.
        dirty.insert(ChannelId::Input(1), HashSet::new());
        dirty.insert(ChannelId::Input(2), HashSet::from([ParameterPath::Fader]));

        s.apply_dirty_set(&dirty);
        assert!(!s.channel_paths.contains_key(&ChannelId::Input(1)));
        assert!(s.is_cell_selected(&ChannelId::Input(2), &ParameterPath::Fader));
    }

    #[test]
    fn merge_dirty_set_adds_to_existing_selections() {
        let mut s = ScopeEditorState::default();
        s.toggle_cell(&ChannelId::Input(1), &ParameterPath::Fader);

        let dirty = dirty_map(&[
            (ChannelId::Input(1), &[ParameterPath::Mute]),
            (ChannelId::Input(2), &[ParameterPath::EqEnabled]),
        ]);
        s.merge_dirty_set(&dirty);

        // Both the original Fader AND the merged-in Mute are present.
        assert!(s.is_cell_selected(&ChannelId::Input(1), &ParameterPath::Fader));
        assert!(s.is_cell_selected(&ChannelId::Input(1), &ParameterPath::Mute));
        // The new Input(2) selection appeared.
        assert!(s.is_cell_selected(&ChannelId::Input(2), &ParameterPath::EqEnabled));
    }

    #[test]
    fn auto_preselect_modified_default_is_false() {
        let s = ScopeEditorState::default();
        assert!(!s.auto_preselect_modified);
        assert_eq!(s.last_dirty_generation, 0);
    }
}
