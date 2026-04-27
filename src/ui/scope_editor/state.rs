//! Selection / filter / lifecycle state for the scope editor.
//!
//! Pure logic — no `egui` dependency. Unit-testable in isolation. The matrix
//! rendering layer lives in [`super::channel_grid`] and reads / mutates this
//! state through public methods.

use std::collections::{HashMap, HashSet};

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
/// toggles all share one definition. Visible to sibling submodules so
/// `channel_grid` can reuse the same predicate for cell rendering.
pub(super) fn cell_available(
    available: &HashMap<ChannelId, HashSet<ParameterPath>>,
    ch: &ChannelId,
    path: &ParameterPath,
) -> bool {
    available.get(ch).is_some_and(|s| s.contains(path))
}

/// Group an `applicable_to` slice into per-section runs while preserving the
/// signal-flow order. Pure helper — used by the matrix renderer to lay out
/// sections in row-blocks; lives here so the test module can access it
/// without pulling egui into scope.
pub(super) fn group_paths_by_section(
    paths: &[ParameterPath],
) -> Vec<(ParameterSection, Vec<ParameterPath>)> {
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
