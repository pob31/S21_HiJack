//! Selection / filter / lifecycle state for the scope editor.
//!
//! Pure logic — no `egui` dependency. Unit-testable in isolation. The matrix
//! rendering layer lives in [`super::channel_grid`] and reads / mutates this
//! state through public methods.

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::model::channel::ChannelId;
use crate::model::config::ConsoleConfig;
use crate::model::parameter::{ParameterPath, ParameterSection, TimingCategory};
use crate::model::recall_scope::ConsoleRecallConfig;
use crate::model::snapshot::{CategoryTiming, ChannelScope, ScopeTemplate};
use crate::ui::recall_scope_popup::RecallScopePopupState;

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
    /// Timing-cell selection (PreWait/Fade modes). Scoped to the CURRENT
    /// `edit_mode` — cleared on mode change and window open/close, so a
    /// selection never leaks between pre-wait and fade. Mode is intentionally
    /// NOT part of the key; `apply_timing_value_to_selection` picks the field.
    pub timing_selection: HashSet<(ChannelId, TimingCategory)>,
    /// Anchor for shift-click rectangle selection: the last plainly-clicked
    /// cell. `None` until the first click of the current selection session.
    pub timing_anchor: Option<(ChannelId, TimingCategory)>,
    /// Draft text for the numeric entry box (seconds). Synced to the
    /// first-selected cell's value when the box is not being edited.
    pub timing_value_draft: String,
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
    /// In-window template controls — `(select)` placeholder when None.
    /// Cleared on `open(...)`.
    pub selected_template_id: Option<Uuid>,
    /// In-window "Save as" name buffer. Cleared on `open(...)` and after
    /// a successful Save.
    pub template_name_buf: String,
    /// Per-group last-frame body scroll offset (x, y), fed forward to the
    /// frozen header strip (x) and label column (y) so they track the body.
    /// A plain tuple keeps this module egui-free. Keyed by channel group.
    pub matrix_scroll_offset: HashMap<ChannelGroup, (f32, f32)>,
}

impl Default for ScopeEditorState {
    fn default() -> Self {
        Self {
            channel_paths: HashMap::new(),
            channel_timings: HashMap::new(),
            timing_selection: HashSet::new(),
            timing_anchor: None,
            timing_value_draft: String::new(),
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
            selected_template_id: None,
            template_name_buf: String::new(),
            matrix_scroll_offset: HashMap::new(),
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
        // Fresh template-picker state per editing session.
        self.selected_template_id = None;
        self.template_name_buf.clear();
        self.clear_timing_selection();
    }

    /// Cancel: restore the backup and close the window.
    pub fn cancel(&mut self) {
        if let Some(backup) = self.backup.take() {
            self.channel_paths = backup;
        }
        if let Some(backup) = self.timing_backup.take() {
            self.channel_timings = backup;
        }
        self.clear_timing_selection();
        self.window_open = false;
    }

    /// Commit: drop the backup and close the window. Caller reads
    /// `to_scope_template()` afterward.
    pub fn commit(&mut self) {
        self.backup = None;
        self.timing_backup = None;
        self.clear_timing_selection();
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

    // ─── Timing-cell selection (PreWait / Fade modes) ────────────────

    /// Clear the timing selection, anchor, and draft text. Called on mode
    /// change and window open/close so a selection never leaks across modes
    /// or editing sessions.
    pub fn clear_timing_selection(&mut self) {
        self.timing_selection.clear();
        self.timing_anchor = None;
        self.timing_value_draft.clear();
    }

    /// Set the edit mode. Switching to a different mode clears the timing
    /// selection (each mode is "one set of timings"); a no-op same-mode call
    /// leaves the selection alone.
    pub fn set_edit_mode(&mut self, mode: ScopeEditMode) {
        if self.edit_mode != mode {
            self.edit_mode = mode;
            self.clear_timing_selection();
        }
    }

    /// Toggle a single timing cell in/out of the selection (plain click).
    /// Re-clicking a selected cell removes it. The clicked cell becomes the
    /// anchor for a subsequent shift-click rectangle.
    pub fn toggle_timing_cell(&mut self, ch: &ChannelId, cat: TimingCategory) {
        let key = (ch.clone(), cat);
        if !self.timing_selection.insert(key.clone()) {
            self.timing_selection.remove(&key);
        }
        self.timing_anchor = Some((ch.clone(), cat));
    }

    pub fn is_timing_selected(&self, ch: &ChannelId, cat: TimingCategory) -> bool {
        self.timing_selection.contains(&(ch.clone(), cat))
    }

    /// Shift-click rectangle selection. Selects every timing cell in the block
    /// spanning the rows (categories) and channels between the anchor and the
    /// clicked cell, within one channel group. Additive — adds to the existing
    /// selection without clearing — and leaves the anchor unchanged so repeated
    /// shift-clicks grow from the same start.
    ///
    /// `ordered_channels` / `ordered_cats` give the group's display order (the
    /// only source of ordering, since `ChannelId` is not `Ord`). Falls back to
    /// a plain toggle when there is no anchor or either endpoint is outside the
    /// supplied lists (e.g. the anchor lives in another group).
    pub fn select_timing_rect(
        &mut self,
        ch: &ChannelId,
        cat: TimingCategory,
        ordered_channels: &[ChannelId],
        ordered_cats: &[TimingCategory],
    ) {
        let Some((anchor_ch, anchor_cat)) = self.timing_anchor.clone() else {
            self.toggle_timing_cell(ch, cat);
            return;
        };
        let ch_a = ordered_channels.iter().position(|c| *c == anchor_ch);
        let ch_b = ordered_channels.iter().position(|c| c == ch);
        let cat_a = ordered_cats.iter().position(|c| *c == anchor_cat);
        let cat_b = ordered_cats.iter().position(|c| *c == cat);
        let (Some(ch_a), Some(ch_b), Some(cat_a), Some(cat_b)) = (ch_a, ch_b, cat_a, cat_b) else {
            // Anchor (or target) not in this group's lists — treat as a plain
            // toggle so the click is never silently ignored.
            self.toggle_timing_cell(ch, cat);
            return;
        };
        let (ch_lo, ch_hi) = (ch_a.min(ch_b), ch_a.max(ch_b));
        let (cat_lo, cat_hi) = (cat_a.min(cat_b), cat_a.max(cat_b));
        for c in &ordered_channels[ch_lo..=ch_hi] {
            for k in &ordered_cats[cat_lo..=cat_hi] {
                self.timing_selection.insert((c.clone(), *k));
            }
        }
        // Anchor intentionally left unchanged.
    }

    /// Number of selected timing cells.
    pub fn timing_selection_count(&self) -> usize {
        self.timing_selection.len()
    }

    /// Value of the first selected cell in render order (top-left-most), for
    /// the given mode — what the numeric box displays. `ordered_keys` is the
    /// caller's render-order key list (the `HashSet` itself has no order).
    /// `None` when the selection is empty.
    pub fn first_timing_value(
        &self,
        mode: ScopeEditMode,
        ordered_keys: &[(ChannelId, TimingCategory)],
    ) -> Option<f32> {
        let (ch, cat) = ordered_keys
            .iter()
            .find(|k| self.timing_selection.contains(k))?;
        let t = self
            .channel_timings
            .get(&(ch.clone(), *cat))
            .cloned()
            .unwrap_or_default();
        Some(match mode {
            ScopeEditMode::PreWait => t.pre_wait_secs,
            ScopeEditMode::Fade => t.fade_time_secs,
            ScopeEditMode::Scope => return None,
        })
    }

    /// Apply an already-parsed/clamped seconds value to every selected cell's
    /// per-mode field. `Scope` mode is a no-op.
    pub fn apply_timing_value_to_selection(&mut self, mode: ScopeEditMode, secs: f32) {
        if mode == ScopeEditMode::Scope {
            return;
        }
        let sel = &self.timing_selection;
        let timings = &mut self.channel_timings;
        for (ch, cat) in sel {
            let t = timings.entry((ch.clone(), *cat)).or_default();
            match mode {
                ScopeEditMode::PreWait => t.pre_wait_secs = secs,
                ScopeEditMode::Fade => t.fade_time_secs = secs,
                ScopeEditMode::Scope => {}
            }
        }
    }

    /// Parse a seconds value from the numeric box: `.`-decimal, clamped to the
    /// cell range `0.0..=30.0`. `None` on unparseable input.
    pub fn parse_timing_secs(s: &str) -> Option<f32> {
        s.trim().parse::<f32>().ok().map(|v| v.clamp(0.0, 30.0))
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
/// toggles all share one definition. Visible to sibling submodules so
/// `channel_grid` can reuse the same predicate for cell rendering.
pub(super) fn cell_available(
    available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ch: &ChannelId,
    path: &ParameterPath,
) -> bool {
    available.get(ch).is_some_and(|s| s.contains(path))
}

/// Group an `applicable_to` slice into one block per section. Each section
/// appears exactly once — paths belonging to the same section are merged even
/// when they aren't contiguous in `paths` (e.g. the `InputGain` paths are
/// split by Trim / Balance / Polarity in signal-flow order, which would
/// otherwise render as two separate "Input" headers). Section order is fixed
/// by first appearance; path order within a section is preserved. Pure helper
/// — lives here so the test module can access it without pulling egui in.
pub(super) fn group_paths_by_section(
    paths: &[ParameterPath],
) -> Vec<(ParameterSection, Vec<ParameterPath>)> {
    let mut out: Vec<(ParameterSection, Vec<ParameterPath>)> = Vec::new();
    for path in paths {
        let s = path.section();
        if let Some((_, existing)) = out.iter_mut().find(|(sec, _)| *sec == s) {
            existing.push(path.clone());
        } else {
            out.push((s, vec![path.clone()]));
        }
    }
    out
}

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
    fn group_paths_by_section_merges_non_contiguous_runs() {
        // InputGain paths are split by Trim/Polarity in signal-flow order;
        // they must still collapse into a single "Input" block (one header),
        // positioned by first appearance, with path order preserved.
        let paths = vec![
            ParameterPath::AnalogGain, // InputGain
            ParameterPath::Trim,       // Trim
            ParameterPath::Polarity,   // Polarity
            ParameterPath::Phantom,    // InputGain again
            ParameterPath::StereoMode, // InputGain again
        ];
        let grouped = group_paths_by_section(&paths);
        assert_eq!(grouped.len(), 3, "InputGain must not appear twice");
        assert_eq!(grouped[0].0, ParameterSection::InputGain);
        assert_eq!(
            grouped[0].1,
            vec![
                ParameterPath::AnalogGain,
                ParameterPath::Phantom,
                ParameterPath::StereoMode
            ]
        );
        assert_eq!(grouped[1].0, ParameterSection::Trim);
        assert_eq!(grouped[2].0, ParameterSection::Polarity);
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

    // ─── Timing-cell selection ───────────────────────────────────────

    #[test]
    fn toggle_timing_cell_adds_then_removes_and_sets_anchor() {
        let mut s = ScopeEditorState::default();
        let ch = ChannelId::Input(1);
        s.toggle_timing_cell(&ch, TimingCategory::Fader);
        assert!(s.is_timing_selected(&ch, TimingCategory::Fader));
        assert_eq!(s.timing_anchor, Some((ch.clone(), TimingCategory::Fader)));
        s.toggle_timing_cell(&ch, TimingCategory::Fader);
        assert!(!s.is_timing_selected(&ch, TimingCategory::Fader));
        // Anchor still points at the last clicked cell.
        assert_eq!(s.timing_anchor, Some((ch, TimingCategory::Fader)));
    }

    #[test]
    fn toggle_timing_cell_is_additive_across_cells() {
        let mut s = ScopeEditorState::default();
        s.toggle_timing_cell(&ChannelId::Input(1), TimingCategory::Fader);
        s.toggle_timing_cell(&ChannelId::Input(2), TimingCategory::Eq);
        assert_eq!(s.timing_selection_count(), 2);
    }

    #[test]
    fn select_timing_rect_selects_inclusive_block() {
        let mut s = ScopeEditorState::default();
        let channels: Vec<ChannelId> = (1..=8).map(ChannelId::Input).collect();
        let cats = [
            TimingCategory::Fader,
            TimingCategory::Eq,
            TimingCategory::Dyn1,
        ];
        // Anchor at (ch2, Fader), rectangle to (ch5, Eq) → 4 channels × 2 cats.
        s.toggle_timing_cell(&ChannelId::Input(2), TimingCategory::Fader);
        s.select_timing_rect(&ChannelId::Input(5), TimingCategory::Eq, &channels, &cats);
        assert_eq!(s.timing_selection_count(), 8);
        for n in 2..=5 {
            assert!(s.is_timing_selected(&ChannelId::Input(n), TimingCategory::Fader));
            assert!(s.is_timing_selected(&ChannelId::Input(n), TimingCategory::Eq));
            assert!(!s.is_timing_selected(&ChannelId::Input(n), TimingCategory::Dyn1));
        }
        assert!(!s.is_timing_selected(&ChannelId::Input(1), TimingCategory::Fader));
        assert!(!s.is_timing_selected(&ChannelId::Input(6), TimingCategory::Fader));
    }

    #[test]
    fn select_timing_rect_handles_reversed_endpoints() {
        let mut s = ScopeEditorState::default();
        let channels: Vec<ChannelId> = (1..=8).map(ChannelId::Input).collect();
        let cats = [TimingCategory::Fader, TimingCategory::Eq];
        s.toggle_timing_cell(&ChannelId::Input(5), TimingCategory::Eq);
        // Click "before" the anchor on both axes.
        s.select_timing_rect(
            &ChannelId::Input(2),
            TimingCategory::Fader,
            &channels,
            &cats,
        );
        assert_eq!(s.timing_selection_count(), 8); // ch2..=5 × Fader,Eq
        assert!(s.is_timing_selected(&ChannelId::Input(2), TimingCategory::Fader));
        assert!(s.is_timing_selected(&ChannelId::Input(5), TimingCategory::Eq));
    }

    #[test]
    fn select_timing_rect_keeps_anchor_for_repeated_shift_clicks() {
        let mut s = ScopeEditorState::default();
        let channels: Vec<ChannelId> = (1..=8).map(ChannelId::Input).collect();
        let cats = [TimingCategory::Fader];
        s.toggle_timing_cell(&ChannelId::Input(2), TimingCategory::Fader);
        s.select_timing_rect(
            &ChannelId::Input(4),
            TimingCategory::Fader,
            &channels,
            &cats,
        );
        // A second shift-click should grow from the ORIGINAL anchor (ch2).
        s.select_timing_rect(
            &ChannelId::Input(6),
            TimingCategory::Fader,
            &channels,
            &cats,
        );
        for n in 2..=6 {
            assert!(s.is_timing_selected(&ChannelId::Input(n), TimingCategory::Fader));
        }
        assert_eq!(
            s.timing_anchor,
            Some((ChannelId::Input(2), TimingCategory::Fader))
        );
    }

    #[test]
    fn select_timing_rect_falls_back_without_anchor() {
        let mut s = ScopeEditorState::default();
        let channels: Vec<ChannelId> = (1..=8).map(ChannelId::Input).collect();
        let cats = [TimingCategory::Fader];
        s.select_timing_rect(
            &ChannelId::Input(3),
            TimingCategory::Fader,
            &channels,
            &cats,
        );
        // No anchor → behaves as a single-cell toggle.
        assert_eq!(s.timing_selection_count(), 1);
        assert!(s.is_timing_selected(&ChannelId::Input(3), TimingCategory::Fader));
    }

    #[test]
    fn select_timing_rect_cross_group_falls_back_to_single_cell() {
        let mut s = ScopeEditorState::default();
        // Anchor in the Aux group.
        s.toggle_timing_cell(&ChannelId::Aux(1), TimingCategory::Fader);
        // Shift-click in the Inputs group: anchor channel isn't in this list.
        let channels: Vec<ChannelId> = (1..=8).map(ChannelId::Input).collect();
        let cats = [TimingCategory::Fader];
        s.select_timing_rect(
            &ChannelId::Input(4),
            TimingCategory::Fader,
            &channels,
            &cats,
        );
        // Only the new single cell got added (plus the original anchor).
        assert!(s.is_timing_selected(&ChannelId::Input(4), TimingCategory::Fader));
        assert!(s.is_timing_selected(&ChannelId::Aux(1), TimingCategory::Fader));
        assert_eq!(s.timing_selection_count(), 2);
    }

    #[test]
    fn first_timing_value_returns_render_order_first() {
        let mut s = ScopeEditorState::default();
        // Seed distinct pre-wait values.
        s.channel_timings.insert(
            (ChannelId::Input(1), TimingCategory::Fader),
            CategoryTiming {
                pre_wait_secs: 1.0,
                fade_time_secs: 0.0,
            },
        );
        s.channel_timings.insert(
            (ChannelId::Input(3), TimingCategory::Fader),
            CategoryTiming {
                pre_wait_secs: 3.0,
                fade_time_secs: 0.0,
            },
        );
        // Select ch3 first, ch1 second — insertion order must not matter.
        s.toggle_timing_cell(&ChannelId::Input(3), TimingCategory::Fader);
        s.toggle_timing_cell(&ChannelId::Input(1), TimingCategory::Fader);
        let order = vec![
            (ChannelId::Input(1), TimingCategory::Fader),
            (ChannelId::Input(2), TimingCategory::Fader),
            (ChannelId::Input(3), TimingCategory::Fader),
        ];
        assert_eq!(
            s.first_timing_value(ScopeEditMode::PreWait, &order),
            Some(1.0)
        );
    }

    #[test]
    fn first_timing_value_empty_selection_is_none() {
        let s = ScopeEditorState::default();
        let order = vec![(ChannelId::Input(1), TimingCategory::Fader)];
        assert_eq!(s.first_timing_value(ScopeEditMode::PreWait, &order), None);
    }

    #[test]
    fn apply_timing_value_writes_correct_field_per_mode() {
        let mut s = ScopeEditorState::default();
        s.toggle_timing_cell(&ChannelId::Input(1), TimingCategory::Fader);
        s.toggle_timing_cell(&ChannelId::Input(2), TimingCategory::Fader);

        s.apply_timing_value_to_selection(ScopeEditMode::PreWait, 1.5);
        for n in 1..=2 {
            let t = &s.channel_timings[&(ChannelId::Input(n), TimingCategory::Fader)];
            assert_eq!(t.pre_wait_secs, 1.5);
            assert_eq!(t.fade_time_secs, 0.0); // untouched
        }

        s.apply_timing_value_to_selection(ScopeEditMode::Fade, 2.0);
        for n in 1..=2 {
            let t = &s.channel_timings[&(ChannelId::Input(n), TimingCategory::Fader)];
            assert_eq!(t.pre_wait_secs, 1.5); // untouched
            assert_eq!(t.fade_time_secs, 2.0);
        }
    }

    #[test]
    fn parse_timing_secs_parses_and_clamps() {
        assert_eq!(ScopeEditorState::parse_timing_secs("1.5"), Some(1.5));
        assert_eq!(ScopeEditorState::parse_timing_secs("  .5 "), Some(0.5));
        assert_eq!(ScopeEditorState::parse_timing_secs("99"), Some(30.0));
        assert_eq!(ScopeEditorState::parse_timing_secs("-4"), Some(0.0));
        assert_eq!(ScopeEditorState::parse_timing_secs("abc"), None);
        assert_eq!(ScopeEditorState::parse_timing_secs(""), None);
    }

    #[test]
    fn clear_timing_selection_resets_all_three() {
        let mut s = ScopeEditorState::default();
        s.toggle_timing_cell(&ChannelId::Input(1), TimingCategory::Fader);
        s.timing_value_draft = "1.5".into();
        s.clear_timing_selection();
        assert_eq!(s.timing_selection_count(), 0);
        assert!(s.timing_anchor.is_none());
        assert!(s.timing_value_draft.is_empty());
    }

    #[test]
    fn set_edit_mode_clears_selection_only_on_change() {
        let mut s = ScopeEditorState::default();
        s.set_edit_mode(ScopeEditMode::PreWait);
        s.toggle_timing_cell(&ChannelId::Input(1), TimingCategory::Fader);
        // Same-mode call leaves selection alone.
        s.set_edit_mode(ScopeEditMode::PreWait);
        assert_eq!(s.timing_selection_count(), 1);
        // Switching to Fade clears it.
        s.set_edit_mode(ScopeEditMode::Fade);
        assert_eq!(s.timing_selection_count(), 0);
    }

    #[test]
    fn cancel_clears_timing_selection() {
        let mut s = ScopeEditorState::default();
        let template = ScopeTemplate::new("empty".into(), vec![]);
        s.open(&template, 8, 8, 10);
        s.set_edit_mode(ScopeEditMode::PreWait);
        s.toggle_timing_cell(&ChannelId::Input(1), TimingCategory::Fader);
        s.cancel();
        assert_eq!(s.timing_selection_count(), 0);
    }
}
