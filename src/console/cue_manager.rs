use std::collections::HashMap;

use tracing::{info, warn};
use uuid::Uuid;

use crate::model::snapshot::{Cue, CueList, ScopeTemplate, Snapshot};

/// Manages the cue list, snapshots, and scope templates.
pub struct CueManager {
    pub cue_list: CueList,
    pub snapshots: HashMap<Uuid, Snapshot>,
    pub scope_templates: HashMap<Uuid, ScopeTemplate>,
    current_cue_index: Option<usize>,
    /// UUID of the most recently successfully recalled snapshot. Used by
    /// the auto-update-on-recall feature to know which snapshot to merge
    /// the dirty parameters into when the next recall fires.
    last_recalled_snapshot_id: Option<Uuid>,
}

impl CueManager {
    pub fn new(cue_list: CueList) -> Self {
        Self {
            cue_list,
            snapshots: HashMap::new(),
            scope_templates: HashMap::new(),
            current_cue_index: None,
            last_recalled_snapshot_id: None,
        }
    }

    /// Record that this snapshot was just recalled successfully.
    pub fn set_last_recalled(&mut self, id: Uuid) {
        self.last_recalled_snapshot_id = Some(id);
    }

    /// UUID of the most recently recalled snapshot, if any.
    pub fn last_recalled(&self) -> Option<Uuid> {
        self.last_recalled_snapshot_id
    }

    /// Advance to the next cue and return it.
    pub fn go_next(&mut self) -> Option<&Cue> {
        if self.cue_list.cues.is_empty() {
            warn!("No cues in cue list");
            return None;
        }

        let next = match self.current_cue_index {
            None => 0,
            Some(i) => {
                if i + 1 >= self.cue_list.cues.len() {
                    warn!("Already at last cue");
                    return None;
                }
                i + 1
            }
        };

        self.current_cue_index = Some(next);
        let cue = &self.cue_list.cues[next];
        info!(
            cue_number = cue.cue_number,
            cue_name = %cue.name,
            index = next,
            "Advanced to cue"
        );
        Some(cue)
    }

    /// Go back to the previous cue and return it.
    pub fn go_previous(&mut self) -> Option<&Cue> {
        if self.cue_list.cues.is_empty() {
            warn!("No cues in cue list");
            return None;
        }

        let prev = match self.current_cue_index {
            None => {
                warn!("No current cue to go back from");
                return None;
            }
            Some(0) => {
                warn!("Already at first cue");
                return None;
            }
            Some(i) => i - 1,
        };

        self.current_cue_index = Some(prev);
        let cue = &self.cue_list.cues[prev];
        info!(
            cue_number = cue.cue_number,
            cue_name = %cue.name,
            index = prev,
            "Went back to cue"
        );
        Some(cue)
    }

    /// Fire a specific cue by number. Finds the closest matching cue.
    pub fn fire_cue_number(&mut self, number: f32) -> Option<&Cue> {
        let idx = self
            .cue_list
            .cues
            .iter()
            .position(|c| (c.cue_number - number).abs() < 0.001);

        match idx {
            Some(i) => {
                self.current_cue_index = Some(i);
                let cue = &self.cue_list.cues[i];
                info!(cue_number = cue.cue_number, cue_name = %cue.name, "Fired cue by number");
                Some(cue)
            }
            None => {
                warn!(number, "No cue found with number");
                None
            }
        }
    }

    /// Fire a specific cue by id, making it the current cue. Returns the cue
    /// (so the caller can recall its snapshot). Used by the cue-list popup's
    /// per-cue Fire buttons, where firing by id is unambiguous.
    pub fn fire_cue_id(&mut self, id: Uuid) -> Option<&Cue> {
        let idx = self.cue_list.cues.iter().position(|c| c.id == id)?;
        self.current_cue_index = Some(idx);
        let cue = &self.cue_list.cues[idx];
        info!(cue_number = cue.cue_number, cue_name = %cue.name, "Fired cue by id");
        Some(cue)
    }

    /// Make the cue with this id the current cue WITHOUT recalling anything
    /// (the cue-list popup's row click). Repositions the playhead so the next
    /// GO fires the following cue. Returns the cue number set, if found.
    pub fn set_current_cue_id(&mut self, id: Uuid) -> Option<f32> {
        let idx = self.cue_list.cues.iter().position(|c| c.id == id)?;
        self.current_cue_index = Some(idx);
        let cue = &self.cue_list.cues[idx];
        info!(cue_number = cue.cue_number, cue_name = %cue.name, "Set current cue (no recall)");
        Some(cue.cue_number)
    }

    /// Advance the current-cue pointer to the next cue WITHOUT recalling it
    /// (the "skip" transport action). Returns the now-current cue, or None at
    /// the end of the list / when empty.
    pub fn skip_next(&mut self) -> Option<&Cue> {
        self.go_next()
    }

    /// Get the current cue (if any).
    pub fn current_cue(&self) -> Option<&Cue> {
        self.current_cue_index.map(|i| &self.cue_list.cues[i])
    }

    /// Get the current cue number (for QLab /cue/current response).
    pub fn current_cue_number(&self) -> Option<f32> {
        self.current_cue().map(|c| c.cue_number)
    }

    /// Look up a snapshot by ID.
    pub fn get_snapshot(&self, id: &Uuid) -> Option<&Snapshot> {
        self.snapshots.get(id)
    }

    /// Phase E: resolve a snapshot identifier to a snapshot reference.
    /// Tries UUID parsing first; if the string doesn't parse as a UUID (or
    /// the UUID isn't found), falls back to a case-insensitive name match.
    /// Used by the `/snapshot/recall` trigger listener so QLab cues can
    /// reference snapshots by either their stable id or their human name.
    pub fn resolve_snapshot(&self, identifier: &str) -> Option<&Snapshot> {
        if let Ok(uuid) = Uuid::parse_str(identifier) {
            if let Some(s) = self.snapshots.get(&uuid) {
                return Some(s);
            }
        }
        self.snapshots
            .values()
            .find(|s| s.name.eq_ignore_ascii_case(identifier))
    }

    /// Add a snapshot.
    pub fn add_snapshot(&mut self, snapshot: Snapshot) {
        info!(name = %snapshot.name, id = %snapshot.id, "Added snapshot");
        self.snapshots.insert(snapshot.id, snapshot);
    }

    /// Add a cue to the cue list.
    pub fn add_cue(&mut self, cue: Cue) {
        info!(cue_number = cue.cue_number, name = %cue.name, "Added cue");
        self.cue_list.cues.push(cue);
        // Keep cues sorted by cue number
        self.cue_list.cues.sort_by(|a, b| {
            a.cue_number
                .partial_cmp(&b.cue_number)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// Remove a cue by ID.
    pub fn remove_cue(&mut self, cue_id: Uuid) -> bool {
        let before = self.cue_list.cues.len();
        self.cue_list.cues.retain(|c| c.id != cue_id);
        let removed = self.cue_list.cues.len() < before;
        if removed {
            // Reset current index if it's now invalid
            if let Some(idx) = self.current_cue_index {
                if idx >= self.cue_list.cues.len() {
                    self.current_cue_index = if self.cue_list.cues.is_empty() {
                        None
                    } else {
                        Some(self.cue_list.cues.len() - 1)
                    };
                }
            }
        }
        removed
    }

    /// Peek at the next cue (without advancing).
    pub fn next_cue(&self) -> Option<&Cue> {
        match self.current_cue_index {
            None => self.cue_list.cues.first(),
            Some(i) => self.cue_list.cues.get(i + 1),
        }
    }

    /// Remove a snapshot by ID.
    pub fn remove_snapshot(&mut self, id: Uuid) -> bool {
        let removed = self.snapshots.remove(&id).is_some();
        if removed {
            info!(%id, "Removed snapshot");
        }
        removed
    }

    /// Update a snapshot's data (re-capture with fresh values).
    pub fn update_snapshot(&mut self, id: Uuid, data: crate::model::snapshot::SnapshotData) {
        if let Some(snapshot) = self.snapshots.get_mut(&id) {
            snapshot.data = data;
            snapshot.modified_at = chrono::Utc::now();
            info!(name = %snapshot.name, %id, "Updated snapshot data");
        }
    }

    /// Add a scope template.
    pub fn add_scope_template(&mut self, template: ScopeTemplate) {
        info!(name = %template.name, id = %template.id, "Added scope template");
        self.scope_templates.insert(template.id, template);
    }

    /// Replace the scope template with the given id. Returns `false` when
    /// no template with that id exists. The replacement keeps the id so
    /// any references (cue `scope_override`) stay valid.
    pub fn update_scope_template(&mut self, id: Uuid, mut template: ScopeTemplate) -> bool {
        if !self.scope_templates.contains_key(&id) {
            warn!(%id, "Scope template not found for update");
            return false;
        }
        template.id = id;
        info!(name = %template.name, %id, "Updated scope template");
        self.scope_templates.insert(id, template);
        true
    }

    /// Remove a scope template. Returns whether the template existed.
    pub fn remove_scope_template(&mut self, id: Uuid) -> bool {
        let removed = self.scope_templates.remove(&id).is_some();
        if removed {
            info!(%id, "Removed scope template");
        }
        removed
    }

    /// Update cue properties. `cue_number`, when provided, replaces the
    /// existing order key and the cue list is re-sorted so the display
    /// reflects the new position. `snapshot_id` and `console_snapshot` are
    /// passed through unchanged (Option == None clears the link).
    #[allow(clippy::too_many_arguments)]
    pub fn update_cue(
        &mut self,
        cue_id: Uuid,
        cue_number: Option<f32>,
        snapshot_id: Option<Uuid>,
        console_snapshot: Option<i32>,
        scope_override: Option<ScopeTemplate>,
        notes: String,
    ) -> bool {
        let updated = if let Some(cue) = self.cue_list.cues.iter_mut().find(|c| c.id == cue_id) {
            if let Some(n) = cue_number {
                cue.cue_number = n;
            }
            cue.snapshot_id = snapshot_id;
            cue.console_snapshot = console_snapshot;
            cue.scope_override = scope_override;
            cue.notes = notes;
            info!(cue_number = cue.cue_number, name = %cue.name, "Updated cue");
            true
        } else {
            warn!(%cue_id, "Cue not found for update");
            false
        };
        if updated && cue_number.is_some() {
            self.cue_list.cues.sort_by(|a, b| {
                a.cue_number
                    .partial_cmp(&b.cue_number)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        updated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::snapshot::{Cue, CueList};

    fn make_cue(number: f32, name: &str) -> Cue {
        Cue::new(number, name.into()).with_snapshot_id(Uuid::new_v4())
    }

    #[test]
    fn go_next_advances() {
        let mut mgr = CueManager::new(CueList::default());
        mgr.add_cue(make_cue(1.0, "Cue 1"));
        mgr.add_cue(make_cue(2.0, "Cue 2"));
        mgr.add_cue(make_cue(3.0, "Cue 3"));

        assert!(mgr.current_cue().is_none());

        let cue = mgr.go_next().unwrap();
        assert!((cue.cue_number - 1.0).abs() < 0.001);

        let cue = mgr.go_next().unwrap();
        assert!((cue.cue_number - 2.0).abs() < 0.001);

        let cue = mgr.go_next().unwrap();
        assert!((cue.cue_number - 3.0).abs() < 0.001);

        // At the end
        assert!(mgr.go_next().is_none());
    }

    #[test]
    fn go_previous_goes_back() {
        let mut mgr = CueManager::new(CueList::default());
        mgr.add_cue(make_cue(1.0, "Cue 1"));
        mgr.add_cue(make_cue(2.0, "Cue 2"));

        // Advance to cue 2
        mgr.go_next();
        mgr.go_next();

        let cue = mgr.go_previous().unwrap();
        assert!((cue.cue_number - 1.0).abs() < 0.001);

        // At the beginning
        assert!(mgr.go_previous().is_none());
    }

    #[test]
    fn fire_cue_number_finds_cue() {
        let mut mgr = CueManager::new(CueList::default());
        mgr.add_cue(make_cue(1.0, "Cue 1"));
        mgr.add_cue(make_cue(1.5, "Cue 1.5"));
        mgr.add_cue(make_cue(2.0, "Cue 2"));

        let cue = mgr.fire_cue_number(1.5).unwrap();
        assert_eq!(cue.name, "Cue 1.5");
        assert_eq!(mgr.current_cue_number(), Some(1.5));

        // Non-existent
        assert!(mgr.fire_cue_number(99.0).is_none());
    }

    #[test]
    fn cues_stay_sorted() {
        let mut mgr = CueManager::new(CueList::default());
        mgr.add_cue(make_cue(3.0, "Cue 3"));
        mgr.add_cue(make_cue(1.0, "Cue 1"));
        mgr.add_cue(make_cue(2.0, "Cue 2"));

        let numbers: Vec<f32> = mgr.cue_list.cues.iter().map(|c| c.cue_number).collect();
        assert_eq!(numbers, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn empty_cue_list() {
        let mut mgr = CueManager::new(CueList::default());
        assert!(mgr.go_next().is_none());
        assert!(mgr.go_previous().is_none());
        assert!(mgr.fire_cue_number(1.0).is_none());
        assert!(mgr.current_cue().is_none());
    }

    #[test]
    fn update_cue_modifies_fields() {
        let mut mgr = CueManager::new(CueList::default());
        let cue = make_cue(1.0, "Cue 1");
        let cue_id = cue.id;
        mgr.add_cue(cue);

        assert!(mgr.update_cue(
            cue_id,
            None,
            mgr.cue_list.cues[0].snapshot_id,
            None,
            None,
            "Scene change".into(),
        ));

        let updated = mgr.cue_list.cues.iter().find(|c| c.id == cue_id).unwrap();
        assert!(updated.scope_override.is_none());
        assert_eq!(updated.notes, "Scene change");
    }

    #[test]
    fn update_cue_renumber_resorts_list() {
        let mut mgr = CueManager::new(CueList::default());
        mgr.add_cue(make_cue(1.0, "A"));
        mgr.add_cue(make_cue(2.0, "B"));
        mgr.add_cue(make_cue(3.0, "C"));
        let b_id = mgr.cue_list.cues[1].id;

        // Renumber B from 2.0 → 0.5; it should now be first.
        assert!(mgr.update_cue(
            b_id,
            Some(0.5),
            mgr.cue_list.cues[1].snapshot_id,
            None,
            None,
            String::new(),
        ));
        assert_eq!(mgr.cue_list.cues[0].id, b_id);
        assert!((mgr.cue_list.cues[0].cue_number - 0.5).abs() < 0.001);
    }

    #[test]
    fn update_cue_nonexistent_returns_false() {
        let mut mgr = CueManager::new(CueList::default());
        assert!(!mgr.update_cue(Uuid::new_v4(), None, None, None, None, String::new(),));
    }

    // ─── Phase E: resolve_snapshot ──────────────────────────────────

    fn make_snapshot(name: &str) -> Snapshot {
        use crate::model::snapshot::{ScopeTemplate, SnapshotData, SnapshotKind};
        Snapshot::new(
            name.into(),
            ScopeTemplate::new("S".into(), vec![]),
            SnapshotData::new(),
            SnapshotKind::ApplyOnSave,
        )
    }

    #[test]
    fn resolve_snapshot_by_uuid_first() {
        let mut mgr = CueManager::new(CueList::default());
        let snap = make_snapshot("Verse 1");
        let id = snap.id;
        mgr.add_snapshot(snap);

        let resolved = mgr.resolve_snapshot(&id.to_string());
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap().id, id);
    }

    #[test]
    fn resolve_snapshot_by_name_case_insensitive() {
        let mut mgr = CueManager::new(CueList::default());
        mgr.add_snapshot(make_snapshot("Verse 1"));

        // Exact match.
        assert!(mgr.resolve_snapshot("Verse 1").is_some());
        // Case-insensitive match.
        assert!(mgr.resolve_snapshot("verse 1").is_some());
        assert!(mgr.resolve_snapshot("VERSE 1").is_some());
    }

    #[test]
    fn resolve_snapshot_returns_none_for_unknown_identifier() {
        let mut mgr = CueManager::new(CueList::default());
        mgr.add_snapshot(make_snapshot("Verse 1"));
        assert!(mgr.resolve_snapshot("Chorus").is_none());
        // Random UUID that doesn't exist.
        assert!(mgr.resolve_snapshot(&Uuid::new_v4().to_string()).is_none());
    }

    #[test]
    fn resolve_snapshot_uuid_takes_precedence_over_name() {
        // If a snapshot's name happens to look like a UUID and ANOTHER
        // snapshot's id matches that UUID, the UUID lookup should win.
        let mut mgr = CueManager::new(CueList::default());
        let target = make_snapshot("Real");
        let target_id = target.id;
        let mut decoy = make_snapshot(&target_id.to_string());
        decoy.name = target_id.to_string();
        mgr.add_snapshot(target);
        mgr.add_snapshot(decoy);

        // Looking up by the UUID string should hit the target by id, not
        // the decoy by name.
        let resolved = mgr.resolve_snapshot(&target_id.to_string()).unwrap();
        assert_eq!(resolved.id, target_id);
        assert_eq!(resolved.name, "Real");
    }
}
