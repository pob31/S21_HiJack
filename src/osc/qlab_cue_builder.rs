//! Phase D: pure OSC sequence builders for QLab cue creation.
//!
//! QLab's OSC API for creating cues is positional: you fire `/new <type>` to
//! create a cue, then immediately address it with `/cue/selected/<property>`.
//! The "currently selected cue" pointer moves to the most-recently-created
//! cue. To put a child cue inside a group, you create the group first,
//! capture its unique id (via a query reply), then create each child and
//! issue `/cue/{child_id}/moveInto /cue/{group_id}` once the child's id is
//! known.
//!
//! This module is the **pure** half of the integration: it produces an
//! ordered list of messages and metadata describing where each child needs
//! to land, without doing any IO. The [`crate::osc::qlab_client`] module is
//! the IO half — it sends the messages, queries unique ids, and issues the
//! moveInto commands.
//!
//! Mirrors [WFS-DIY's `QLabCueBuilder.h`](../../../Documents/WFS-DIY_JUCE/WFS-DIY/Source/Network/QLabCueBuilder.h).
//!
//! See also [`crate::osc::SNAPSHOT_RECALL_ADDR`] — the trigger-cue builder
//! embeds it in network-cue customStrings so QLab can recall snapshots back
//! through the Phase E listener.

use rosc::{OscMessage, OscType};

use std::collections::HashMap;
use uuid::Uuid;

use crate::model::palette::ChannelPalette;
use crate::model::snapshot::{self, Snapshot};

use super::{SNAPSHOT_RECALL_ADDR, SNAPSHOT_RECALL_FULL_ADDR};

// ── Types ───────────────────────────────────────────────────────────

/// One QLab network cue plus where it should land in the group order.
///
/// `messages` is the ordered OSC stream that creates and configures the cue
/// (`/new network`, then a series of `/cue/selected/*` setters). The client
/// fires this list, captures the resulting cue's id, then issues a moveInto
/// using `move_position` (1-based — `1` means "first child").
///
/// `move_position == 0` means "standalone cue" — don't move it into any group.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkCue {
    pub messages: Vec<OscMessage>,
    pub move_position: u32,
}

/// A whole QLab cue sequence: an optional group cue plus its children.
///
/// `group_messages` creates and configures the group (`/new group`,
/// `/cue/selected/name`, `/cue/selected/mode`, …). Empty when the sequence
/// has no group at all.
///
/// `network_cues` are the child cues, in display order. Each has its own
/// `move_position` so the client can put them in the right slot inside the
/// group after creation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QLabCueSequence {
    pub group_messages: Vec<OscMessage>,
    pub network_cues: Vec<NetworkCue>,
}

// ── Builders ────────────────────────────────────────────────────────

/// Build the QLab sequence to create a **single trigger cue** that recalls a
/// snapshot back through the daemon's Phase E `/snapshot/recall` listener.
///
/// This is the preferred shape for large snapshots: instead of pushing every
/// stored parameter as its own QLab network cue (which can stall QLab when
/// the parameter count is in the hundreds), the operator gets one network
/// cue whose customString is `/snapshot/recall <name>`. Firing it from QLab
/// triggers the daemon to load the snapshot from its own internal store.
///
/// Layout — mirrors WFS-DIY's `buildSnapshotLoadCue`:
///
/// ```text
/// Group "Snapshot 'Verse 1'" (mode 2: start first, then continue)
///   ├─ Memo (instructions for the operator)
///   ├─ Network: /snapshot/recall "Verse 1"   ("Reload 'Verse 1'")
///   └─ Network: /snapshot/recall_full "Verse 1"   ("Reload (no scope) 'Verse 1'")
/// ```
///
/// The third child is the Phase B "recall without scope" trigger — only
/// useful for `SnapshotKind::ApplyOnRecall` snapshots, but harmless on
/// others (the listener just skips paths outside the saved data).
pub fn build_snapshot_load_cue(snapshot_name: &str, qlab_patch: i32) -> QLabCueSequence {
    let mut sequence = QLabCueSequence::default();

    // ── Group cue ──
    sequence
        .group_messages
        .push(new_cue("group"));
    sequence
        .group_messages
        .push(cue_selected_string("/cue/selected/name", &format!("Snapshot '{snapshot_name}'")));
    // Mode 2 = "start first and go to next" (matches WFS-DIY).
    sequence
        .group_messages
        .push(cue_selected_int("/cue/selected/mode", 2));

    // ── Memo child ──
    sequence.network_cues.push(NetworkCue {
        messages: vec![
            new_cue("memo"),
            cue_selected_string("/cue/selected/name", "Snapshot recall"),
            cue_selected_string(
                "/cue/selected/notes",
                "Reload triggers /snapshot/recall on the daemon.\n\
                 The 'no scope' variant restores every stored parameter.",
            ),
        ],
        move_position: 1,
    });

    // ── Reload child ──
    sequence.network_cues.push(NetworkCue {
        messages: vec![
            new_cue("network"),
            cue_selected_int("/cue/selected/patch", qlab_patch),
            cue_selected_string(
                "/cue/selected/customString",
                &format!("{SNAPSHOT_RECALL_ADDR} \"{snapshot_name}\""),
            ),
            cue_selected_string("/cue/selected/name", &format!("Reload '{snapshot_name}'")),
        ],
        move_position: 2,
    });

    // ── Reload-without-scope child ──
    sequence.network_cues.push(NetworkCue {
        messages: vec![
            new_cue("network"),
            cue_selected_int("/cue/selected/patch", qlab_patch),
            cue_selected_string(
                "/cue/selected/customString",
                &format!("{SNAPSHOT_RECALL_FULL_ADDR} \"{snapshot_name}\""),
            ),
            cue_selected_string(
                "/cue/selected/name",
                &format!("Reload (no scope) '{snapshot_name}'"),
            ),
        ],
        move_position: 3,
    });

    sequence
}

/// Build the QLab sequence to **export every parameter** of a snapshot as
/// individual QLab network cues. Use this for small snapshots where the
/// operator wants the cue list visible inside QLab itself; for larger
/// snapshots prefer [`build_snapshot_load_cue`] (one trigger cue) so QLab
/// doesn't stall on a hundreds-of-cues group.
///
/// Layout:
///
/// ```text
/// Group "Snapshot 'Verse 1'" (playlist mode)
///   ├─ Network: <path1> <args1>
///   ├─ Network: <path2> <args2>
///   └─ ...
/// ```
///
/// Each child carries the GP OSC path and the value as a customString. The
/// daemon listens on the network cue's address space — the operator points
/// the QLab network patch at the daemon's GP OSC port and lets QLab fire
/// each cue directly to the console.
///
/// NOTE: only parameters with a GP OSC encoding are exported. iPad-only
/// parameters (Phantom, GroupSendOn, MatrixSend*, etc.) are skipped because
/// QLab can't easily represent the iPad protocol's bare-path framing.
/// Parameters that fail to encode (out-of-range bands, unknown variants)
/// are also skipped silently.
pub fn build_snapshot_cues(
    snapshot: &Snapshot,
    palettes: &HashMap<Uuid, ChannelPalette>,
    qlab_patch: i32,
) -> QLabCueSequence {
    let mut sequence = QLabCueSequence::default();

    // ── Group cue ──
    sequence.group_messages.push(new_cue("group"));
    sequence.group_messages.push(cue_selected_string(
        "/cue/selected/name",
        &format!("Snapshot '{}'", snapshot.name),
    ));
    // Mode 6 = playlist (start each cue in turn). Matches WFS-DIY's
    // buildSnapshotCues.
    sequence
        .group_messages
        .push(cue_selected_int("/cue/selected/mode", 6));

    // ── Per-parameter children ──
    // Use resolve_recall_values so palette overrides are applied.
    let resolved = snapshot::resolve_recall_values(snapshot, &snapshot.scope, palettes, true);
    let mut position: u32 = 0;
    for (addr, value) in &resolved {
        let Some((path, args)) = crate::osc::encode::encode_parameter(addr, value) else {
            // Skip iPad-only or unencodable parameters (see doc above).
            continue;
        };
        position += 1;
        let custom = format_custom_string(&path, &args);
        let label = format_cue_label(addr, value);
        sequence.network_cues.push(NetworkCue {
            messages: vec![
                new_cue("network"),
                cue_selected_int("/cue/selected/patch", qlab_patch),
                cue_selected_string("/cue/selected/customString", &custom),
                cue_selected_string("/cue/selected/name", &label),
            ],
            move_position: position,
        });
    }

    sequence
}

// ── Helpers ─────────────────────────────────────────────────────────

/// `OscMessage::new("/new", [String("group" | "network" | "memo")])`.
fn new_cue(cue_type: &str) -> OscMessage {
    OscMessage {
        addr: "/new".into(),
        args: vec![OscType::String(cue_type.into())],
    }
}

fn cue_selected_string(addr: &str, value: &str) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args: vec![OscType::String(value.into())],
    }
}

fn cue_selected_int(addr: &str, value: i32) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args: vec![OscType::Int(value)],
    }
}

/// Render a customString from an encoded GP OSC `(path, args)` pair —
/// the format QLab will fire when its network cue runs.
///
/// QLab's customString is space-separated: `"/path arg1 arg2 ..."`. We use
/// the same conventions as the existing daemon encoder so a round-trip
/// (QLab → console → daemon mirror) yields the same value.
fn format_custom_string(path: &str, args: &[OscType]) -> String {
    let mut out = String::from(path);
    for arg in args {
        out.push(' ');
        match arg {
            OscType::Int(n) => out.push_str(&n.to_string()),
            OscType::Float(f) => {
                // Always include a decimal point so QLab parses as float32.
                if f.fract() == 0.0 {
                    out.push_str(&format!("{f:.1}"));
                } else {
                    out.push_str(&format!("{f:.6}"));
                }
            }
            OscType::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            OscType::String(s) => {
                // Quote strings with spaces; otherwise pass bare.
                if s.contains(' ') {
                    out.push('"');
                    out.push_str(s);
                    out.push('"');
                } else {
                    out.push_str(s);
                }
            }
            // Other arg types are unusual on the GP OSC wire — render
            // a Debug placeholder so the operator can spot them.
            other => out.push_str(&format!("{other:?}")),
        }
    }
    out
}

/// Friendly label for a network cue, e.g. "Input 5: Fader = -10.0".
fn format_cue_label(
    addr: &crate::model::parameter::ParameterAddress,
    value: &crate::model::parameter::ParameterValue,
) -> String {
    format!("{}: {} = {}", addr.channel, addr.parameter.label(), value)
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
    use crate::model::snapshot::{ScopeTemplate, Snapshot, SnapshotData, SnapshotKind};

    fn first_string_arg(msg: &OscMessage) -> Option<&str> {
        msg.args.first().and_then(|a| match a {
            OscType::String(s) => Some(s.as_str()),
            _ => None,
        })
    }

    // ── build_snapshot_load_cue ──

    #[test]
    fn load_cue_creates_group_with_three_children() {
        let seq = build_snapshot_load_cue("Verse 1", 1);
        // Group: /new group + name + mode → 3 messages.
        assert_eq!(seq.group_messages.len(), 3);
        assert_eq!(seq.group_messages[0].addr, "/new");
        assert_eq!(first_string_arg(&seq.group_messages[0]), Some("group"));
        // Three children: memo, reload, reload_full.
        assert_eq!(seq.network_cues.len(), 3);
    }

    #[test]
    fn load_cue_group_mode_is_2() {
        let seq = build_snapshot_load_cue("Test", 1);
        let mode_msg = &seq.group_messages[2];
        assert_eq!(mode_msg.addr, "/cue/selected/mode");
        match &mode_msg.args[0] {
            OscType::Int(n) => assert_eq!(*n, 2),
            _ => panic!("expected int mode"),
        }
    }

    #[test]
    fn load_cue_group_name_includes_snapshot_name() {
        let seq = build_snapshot_load_cue("Verse 1", 1);
        let name_msg = &seq.group_messages[1];
        assert_eq!(name_msg.addr, "/cue/selected/name");
        let s = first_string_arg(name_msg).unwrap();
        assert!(s.contains("Verse 1"));
    }

    #[test]
    fn load_cue_reload_child_uses_snapshot_recall_addr() {
        let seq = build_snapshot_load_cue("Verse 1", 7);
        // Children are [memo, reload, reload_full] in that order.
        let reload = &seq.network_cues[1];
        assert_eq!(reload.move_position, 2);
        // The customString message is the third one (after /new network +
        // /cue/selected/patch).
        let custom_msg = reload
            .messages
            .iter()
            .find(|m| m.addr == "/cue/selected/customString")
            .expect("reload should set customString");
        let custom = first_string_arg(custom_msg).unwrap();
        assert!(custom.starts_with(SNAPSHOT_RECALL_ADDR));
        assert!(custom.contains("Verse 1"));
        // Patch is set to the requested patch number.
        let patch_msg = reload
            .messages
            .iter()
            .find(|m| m.addr == "/cue/selected/patch")
            .expect("reload should set patch");
        match &patch_msg.args[0] {
            OscType::Int(n) => assert_eq!(*n, 7),
            _ => panic!("expected int patch"),
        }
    }

    #[test]
    fn load_cue_reload_full_child_uses_recall_full_addr() {
        let seq = build_snapshot_load_cue("Verse 1", 1);
        let reload_full = &seq.network_cues[2];
        assert_eq!(reload_full.move_position, 3);
        let custom_msg = reload_full
            .messages
            .iter()
            .find(|m| m.addr == "/cue/selected/customString")
            .unwrap();
        let custom = first_string_arg(custom_msg).unwrap();
        assert!(custom.starts_with(SNAPSHOT_RECALL_FULL_ADDR));
        assert!(custom.contains("Verse 1"));
    }

    #[test]
    fn load_cue_memo_is_first_child() {
        let seq = build_snapshot_load_cue("Verse 1", 1);
        let memo = &seq.network_cues[0];
        assert_eq!(memo.move_position, 1);
        // First message is /new memo.
        assert_eq!(memo.messages[0].addr, "/new");
        assert_eq!(first_string_arg(&memo.messages[0]), Some("memo"));
    }

    // ── build_snapshot_cues ──

    fn small_snapshot() -> Snapshot {
        let mut data = SnapshotData::new();
        data.values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(-10.0),
        );
        data.values.insert(
            ParameterAddress {
                channel: ChannelId::Input(2),
                parameter: ParameterPath::Mute,
            },
            ParameterValue::Bool(true),
        );
        Snapshot::new(
            "Test".into(),
            ScopeTemplate::new("S".into(), vec![]),
            data,
            SnapshotKind::ApplyOnSave,
        )
    }

    #[test]
    fn snapshot_cues_one_network_cue_per_param() {
        let snap = small_snapshot();
        let seq = build_snapshot_cues(&snap, &HashMap::new(), 1);
        // 2 stored params → 2 network cues.
        assert_eq!(seq.network_cues.len(), 2);
        // Group exists.
        assert_eq!(seq.group_messages[0].addr, "/new");
        assert_eq!(first_string_arg(&seq.group_messages[0]), Some("group"));
    }

    #[test]
    fn snapshot_cues_handles_empty_snapshot() {
        let snap = Snapshot::new(
            "Empty".into(),
            ScopeTemplate::new("S".into(), vec![]),
            SnapshotData::new(),
            SnapshotKind::ApplyOnSave,
        );
        let seq = build_snapshot_cues(&snap, &HashMap::new(), 1);
        // Group still created, no children.
        assert!(!seq.group_messages.is_empty());
        assert!(seq.network_cues.is_empty());
    }

    #[test]
    fn snapshot_cues_skip_ipad_only_parameters() {
        // Phantom is iPad-only, no GP OSC encoding.
        let mut data = SnapshotData::new();
        data.values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Phantom,
            },
            ParameterValue::Bool(true),
        );
        data.values.insert(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::Fader,
            },
            ParameterValue::Float(0.0),
        );
        let snap = Snapshot::new(
            "Mixed".into(),
            ScopeTemplate::new("S".into(), vec![]),
            data,
            SnapshotKind::ApplyOnSave,
        );
        let seq = build_snapshot_cues(&snap, &HashMap::new(), 1);
        // Phantom skipped, only Fader exported.
        assert_eq!(seq.network_cues.len(), 1);
        let custom = seq.network_cues[0]
            .messages
            .iter()
            .find(|m| m.addr == "/cue/selected/customString")
            .unwrap();
        let s = first_string_arg(custom).unwrap();
        assert!(s.contains("fader"));
    }

    #[test]
    fn snapshot_cues_move_positions_are_sequential_starting_at_one() {
        let snap = small_snapshot();
        let seq = build_snapshot_cues(&snap, &HashMap::new(), 1);
        let positions: Vec<u32> = seq.network_cues.iter().map(|c| c.move_position).collect();
        // Two cues, positions 1 and 2 in some order (HashMap iteration is
        // unordered, but `position += 1` guarantees the SET is {1, 2}).
        assert_eq!(positions.len(), 2);
        let mut sorted = positions.clone();
        sorted.sort();
        assert_eq!(sorted, vec![1, 2]);
    }

    // ── format_custom_string ──

    #[test]
    fn format_custom_string_renders_float_with_decimal() {
        let s = format_custom_string(
            "/channel/1/fader",
            &[OscType::Float(-10.0)],
        );
        assert_eq!(s, "/channel/1/fader -10.0");
    }

    #[test]
    fn format_custom_string_renders_int_without_decimal() {
        let s = format_custom_string(
            "/channel/1/dyn1/0/knee",
            &[OscType::Int(2)],
        );
        assert_eq!(s, "/channel/1/dyn1/0/knee 2");
    }

    #[test]
    fn format_custom_string_renders_bool_lowercase() {
        let s = format_custom_string("/channel/1/mute", &[OscType::Bool(true)]);
        assert_eq!(s, "/channel/1/mute true");
    }

    #[test]
    fn format_custom_string_quotes_strings_with_spaces() {
        let s = format_custom_string("/cue/selected/name", &[OscType::String("Verse 1".into())]);
        assert_eq!(s, "/cue/selected/name \"Verse 1\"");
    }

    #[test]
    fn format_custom_string_passes_bare_strings_unquoted() {
        let s = format_custom_string("/channel/1/name", &[OscType::String("Kick".into())]);
        assert_eq!(s, "/channel/1/name Kick");
    }
}
