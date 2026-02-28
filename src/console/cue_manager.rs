use std::collections::HashMap;

use uuid::Uuid;
use tracing::{info, warn};

use crate::model::snapshot::{Cue, CueList, ScopeTemplate, Snapshot};

/// Manages the cue list, snapshots, and scope templates.
pub struct CueManager {
    pub cue_list: CueList,
    pub snapshots: HashMap<Uuid, Snapshot>,
    pub scope_templates: HashMap<Uuid, ScopeTemplate>,
    current_cue_index: Option<usize>,
}

impl CueManager {
    pub fn new(cue_list: CueList) -> Self {
        Self {
            cue_list,
            snapshots: HashMap::new(),
            scope_templates: HashMap::new(),
            current_cue_index: None,
        }
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
        let idx = self.cue_list.cues.iter().position(|c| {
            (c.cue_number - number).abs() < 0.001
        });

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
            a.cue_number.partial_cmp(&b.cue_number).unwrap_or(std::cmp::Ordering::Equal)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::snapshot::{CueList, Cue};

    fn make_cue(number: f32, name: &str) -> Cue {
        Cue::new(number, name.into(), Uuid::new_v4())
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
}
