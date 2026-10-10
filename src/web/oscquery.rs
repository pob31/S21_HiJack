//! The app's OSCQuery description, served at `GET /s21` on the web port.
//!
//! Go.dot fetches it when the app declares itself (contract
//! `docs/godot-authoring-protocol-0.1.md` in the Go.dot repository, §3 and
//! §6) and offers its commands as cascaded menus in its OSC cue editor: one
//! drop-down per part of the path, then the value from `VALS`. So the tree
//! names exactly the addresses the trigger listener takes under the `/s21`
//! root (§7.2):
//!
//! | Node | TYPE | ACCESS | VALS |
//! |---|---|---|---|
//! | `/s21/snapshot/recall` | `s` | 2 | the snapshot names |
//! | `/s21/snapshot/recall_full` | `s` | 2 | the snapshot names |
//! | `/s21/cue/go` | none | 2 | |
//! | `/s21/cue/previous` | none | 2 | |
//! | `/s21/cue/fire` | `f` | 2 | the cue numbers |
//! | `/s21/macro/fire` | `s` | 2 | the macro names |
//!
//! A command is write-only (`ACCESS` 2) and has no `VALUE`; one with no
//! `TYPE` takes no argument, and Go.dot sends it bare. Containers are
//! `ACCESS` 0. Pure: [`MenuSource`] is read from the managers by the caller.

use serde_json::{Map, Value, json};

use crate::console::cue_manager::CueManager;
use crate::console::macro_manager::MacroManager;
use crate::osc::S21_ROOT;
use crate::osc::godot_cue_builder::format_cue_number;

/// What the menus offer: the names and numbers the show holds right now.
/// Sorted and without repeats, so the same show always describes itself
/// the same way.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuSource {
    pub snapshot_names: Vec<String>,
    pub cue_numbers: Vec<f32>,
    pub macro_names: Vec<String>,
}

impl MenuSource {
    /// Gather the menu from loose lists (sorting and de-duplicating them).
    pub fn new(
        snapshot_names: impl IntoIterator<Item = String>,
        cue_numbers: impl IntoIterator<Item = f32>,
        macro_names: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut snapshot_names: Vec<String> = snapshot_names.into_iter().collect();
        snapshot_names.sort();
        snapshot_names.dedup();
        let mut cue_numbers: Vec<f32> = cue_numbers.into_iter().filter(|n| n.is_finite()).collect();
        cue_numbers.sort_by(f32::total_cmp);
        cue_numbers.dedup();
        let mut macro_names: Vec<String> = macro_names.into_iter().collect();
        macro_names.sort();
        macro_names.dedup();
        Self {
            snapshot_names,
            cue_numbers,
            macro_names,
        }
    }

    /// The snapshot names and cue numbers of `cues`.
    pub fn of_cues(cues: &CueManager) -> (Vec<String>, Vec<f32>) {
        (
            cues.snapshots.values().map(|s| s.name.clone()).collect(),
            cues.cue_list.cues.iter().map(|c| c.cue_number).collect(),
        )
    }

    /// The macro names of `macros`.
    pub fn of_macros(macros: &MacroManager) -> Vec<String> {
        macros.macros.values().map(|m| m.name.clone()).collect()
    }

    /// A value that changes whenever the menu does: the app declares itself
    /// to Go.dot again when it moves, so Go.dot fetches the new menu.
    pub fn signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.snapshot_names.hash(&mut hasher);
        for n in &self.cue_numbers {
            n.to_bits().hash(&mut hasher);
        }
        self.macro_names.hash(&mut hasher);
        hasher.finish()
    }
}

/// The whole description, rooted at `/s21`.
pub fn description(menu: &MenuSource) -> Value {
    let strings = |names: &[String]| -> Vec<Value> {
        names.iter().map(|n| Value::String(n.clone())).collect()
    };
    let numbers: Vec<Value> = menu
        .cue_numbers
        .iter()
        .filter_map(|n| {
            // The number as the app writes it (12, 1.5), not as an f32
            // widens (1.100000023841858).
            let shortest: f64 = format_cue_number(*n).parse().ok()?;
            serde_json::Number::from_f64(shortest).map(Value::Number)
        })
        .collect();

    container(
        "",
        "S21_HiJack: recall its snapshots, run its cue list, fire its macros.",
        vec![
            (
                "snapshot",
                container(
                    "/snapshot",
                    "Snapshots of the DiGiCo console held by S21_HiJack.",
                    vec![
                        (
                            "recall",
                            command(
                                "/snapshot/recall",
                                Some("s"),
                                strings(&menu.snapshot_names),
                                "Recall a snapshot, within its scope. By name or by id.",
                            ),
                        ),
                        (
                            "recall_full",
                            command(
                                "/snapshot/recall_full",
                                Some("s"),
                                strings(&menu.snapshot_names),
                                "Recall a snapshot ignoring its scope: every stored parameter. \
                                 By name or by id.",
                            ),
                        ),
                    ],
                ),
            ),
            (
                "cue",
                container(
                    "/cue",
                    "S21_HiJack's cue list.",
                    vec![
                        (
                            "go",
                            command(
                                "/cue/go",
                                None,
                                vec![],
                                "GO: recall the next cue of the list.",
                            ),
                        ),
                        (
                            "previous",
                            command(
                                "/cue/previous",
                                None,
                                vec![],
                                "Step back to the previous cue of the list.",
                            ),
                        ),
                        (
                            "fire",
                            command(
                                "/cue/fire",
                                Some("f"),
                                numbers,
                                "Recall the cue with this number.",
                            ),
                        ),
                    ],
                ),
            ),
            (
                "macro",
                container(
                    "/macro",
                    "S21_HiJack's macros.",
                    vec![(
                        "fire",
                        command(
                            "/macro/fire",
                            Some("s"),
                            strings(&menu.macro_names),
                            "Run the macro with this name.",
                        ),
                    )],
                ),
            ),
        ],
    )
}

/// A container node at `path` (below the root; `""` is the root itself).
fn container(path: &str, description: &str, children: Vec<(&str, Value)>) -> Value {
    let mut contents = Map::new();
    for (name, child) in children {
        contents.insert(name.to_string(), child);
    }
    json!({
        "FULL_PATH": format!("{S21_ROOT}{path}"),
        "ACCESS": 0,
        "DESCRIPTION": description,
        "CONTENTS": Value::Object(contents),
    })
}

/// A write-only command at `path`, taking `type_tag` (none: no argument),
/// with `vals` as the values its menu offers (left out when there are
/// none).
fn command(path: &str, type_tag: Option<&str>, vals: Vec<Value>, description: &str) -> Value {
    let mut node = Map::new();
    node.insert(
        "FULL_PATH".into(),
        Value::String(format!("{S21_ROOT}{path}")),
    );
    if let Some(tag) = type_tag {
        node.insert("TYPE".into(), Value::String(tag.to_string()));
    }
    node.insert("ACCESS".into(), json!(2));
    if !vals.is_empty() {
        node.insert("RANGE".into(), json!([{ "VALS": vals }]));
    }
    node.insert("DESCRIPTION".into(), Value::String(description.to_string()));
    Value::Object(node)
}

/// The node at `full_path` (`/s21`, `/s21/cue`, `/s21/cue/go`; a trailing
/// slash is ignored) within `tree`, or `None` when there is none.
pub fn subtree<'a>(tree: &'a Value, full_path: &str) -> Option<&'a Value> {
    let rest = full_path.trim_end_matches('/').strip_prefix(S21_ROOT)?;
    if !(rest.is_empty() || rest.starts_with('/')) {
        return None;
    }
    let mut node = tree;
    for segment in rest.split('/').filter(|s| !s.is_empty()) {
        node = node.get("CONTENTS")?.get(segment)?;
    }
    Some(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    fn menu() -> MenuSource {
        MenuSource::new(
            vec![
                "Verse 1".to_string(),
                "Intro".to_string(),
                "Intro".to_string(),
            ],
            vec![2.0, 1.5, 10.0, 1.1],
            vec!["Mute all".to_string()],
        )
    }

    /// Every node: `(FULL_PATH, TYPE, ACCESS)`, walked from the root.
    fn walk(node: &Value, out: &mut Vec<(String, Option<String>, i64)>) {
        out.push((
            node["FULL_PATH"].as_str().unwrap().to_string(),
            node.get("TYPE").map(|t| t.as_str().unwrap().to_string()),
            node["ACCESS"].as_i64().unwrap(),
        ));
        if let Some(contents) = node.get("CONTENTS").and_then(Value::as_object) {
            for (name, child) in contents {
                let child_path = child["FULL_PATH"].as_str().unwrap();
                let parent_path = node["FULL_PATH"].as_str().unwrap();
                assert_eq!(child_path, format!("{parent_path}/{name}"));
                walk(child, out);
            }
        }
    }

    #[test]
    fn the_tree_is_the_contracts_table() {
        let tree = description(&menu());
        let mut nodes = Vec::new();
        walk(&tree, &mut nodes);
        let commands: Vec<(String, Option<String>, i64)> = nodes
            .into_iter()
            .filter(|(_, _, access)| *access != 0)
            .collect();
        let expected = vec![
            ("/s21/cue/fire", Some("f")),
            ("/s21/cue/go", None),
            ("/s21/cue/previous", None),
            ("/s21/macro/fire", Some("s")),
            ("/s21/snapshot/recall", Some("s")),
            ("/s21/snapshot/recall_full", Some("s")),
        ];
        let mut got: Vec<(&str, Option<&str>)> = commands
            .iter()
            .map(|(p, t, access)| {
                assert_eq!(*access, 2, "{p} is a command");
                (p.as_str(), t.as_deref())
            })
            .collect();
        got.sort();
        assert_eq!(got, expected);
        assert_eq!(tree["FULL_PATH"], "/s21");
        assert_eq!(tree["ACCESS"], 0);
    }

    #[test]
    fn commands_have_no_value_and_menus_carry_vals() {
        let tree = description(&menu());
        let recall = subtree(&tree, "/s21/snapshot/recall").unwrap();
        assert!(recall.get("VALUE").is_none());
        assert_eq!(
            recall["RANGE"][0]["VALS"],
            json!(["Intro", "Verse 1"]),
            "sorted, without repeats"
        );
        let fire = subtree(&tree, "/s21/cue/fire").unwrap();
        assert_eq!(fire["RANGE"][0]["VALS"], json!([1.1, 1.5, 2.0, 10.0]));
        assert_eq!(
            serde_json::to_string(&fire["RANGE"][0]["VALS"]).unwrap(),
            "[1.1,1.5,2.0,10.0]",
            "cue numbers as written, not as an f32 widens"
        );
        let go = subtree(&tree, "/s21/cue/go").unwrap();
        assert!(go.get("TYPE").is_none() && go.get("RANGE").is_none());
        assert_eq!(
            subtree(&tree, "/s21/macro/fire").unwrap()["RANGE"][0]["VALS"],
            json!(["Mute all"])
        );
        // An empty menu offers no VALS rather than an empty closed set.
        let empty = description(&MenuSource::default());
        assert!(
            subtree(&empty, "/s21/macro/fire")
                .unwrap()
                .get("RANGE")
                .is_none()
        );
    }

    #[test]
    fn subtrees_and_unknown_paths() {
        let tree = description(&menu());
        assert_eq!(subtree(&tree, "/s21"), Some(&tree));
        assert_eq!(subtree(&tree, "/s21/"), Some(&tree));
        assert_eq!(subtree(&tree, "/s21/cue").unwrap()["FULL_PATH"], "/s21/cue");
        assert!(subtree(&tree, "/s21/nothing").is_none());
        assert!(subtree(&tree, "/s21/cue/go/deeper").is_none());
        assert!(subtree(&tree, "/s21x").is_none());
        assert!(subtree(&tree, "/other").is_none());
    }

    /// Every command the tree offers is one the trigger listener takes, so a
    /// cue Go.dot builds from these menus fires something.
    #[test]
    fn every_command_is_a_trigger() {
        use rosc::OscType;
        let tree = description(&menu());
        let mut nodes = Vec::new();
        walk(&tree, &mut nodes);
        let src: SocketAddr = "127.0.0.1:9".parse().unwrap();
        for (path, type_tag, access) in nodes {
            if access == 0 {
                continue;
            }
            let args = match type_tag.as_deref() {
                Some("s") => vec![OscType::String("Intro".into())],
                Some("f") => vec![OscType::Float(1.5)],
                _ => vec![],
            };
            assert!(
                crate::osc::trigger_listener::parse_trigger_message(&path, &args, src).is_some(),
                "{path} is described but not a trigger"
            );
        }
    }

    #[test]
    fn the_signature_follows_the_menu() {
        let a = menu();
        assert_eq!(a.signature(), menu().signature());
        let renamed = MenuSource::new(
            vec!["Verse 2".to_string(), "Intro".to_string()],
            vec![2.0, 1.5, 10.0, 1.1],
            vec!["Mute all".to_string()],
        );
        assert_ne!(a.signature(), renamed.signature());
        let renumbered = MenuSource::new(
            vec!["Verse 1".to_string(), "Intro".to_string()],
            vec![2.0, 1.5, 10.0, 1.2],
            vec!["Mute all".to_string()],
        );
        assert_ne!(a.signature(), renumbered.signature());
    }
}
