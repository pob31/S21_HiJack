//! Pure builders for Go.dot's authoring protocol: the cues the Snapshots tab
//! captures into Go.dot, the datagrams that carry them, the declare, and the
//! firing verbs the triggers and macro steps send.
//!
//! The contract is `docs/godot-authoring-protocol-0.1.md` in the Go.dot
//! repository; the section numbers below are its. Where QLab needs a
//! positional dance of `/new` and `/cue/selected/*` per message
//! ([`crate::osc::qlab_cue_builder`]), Go.dot takes one cue in one command:
//! its identifier, name, number, notes and every message, as one undo step.
//! The app draws the identifier itself and keeps it on the snapshot, so a
//! second export updates the cue the first one made.
//!
//! [`crate::osc::godot_client`] is the IO half.

use std::collections::HashMap;

use rosc::{OscMessage, OscType};
use uuid::Uuid;

use crate::model::palette::ChannelPalette;
use crate::model::snapshot::{self, Snapshot};

use super::{
    GODOT_CAPTURE_ADDR, GODOT_DECLARE_ADDR, GODOT_FIRE_ADDR, GODOT_FIRE_NUMBER_ADDR, GODOT_GO_ADDR,
    GODOT_KILL_ALL_ADDR, GODOT_STANDBY_SET_ADDR, GODOT_STANDBY_SET_NUMBER_ADDR,
    GODOT_STOP_ALL_ADDR, S21_ROOT, SNAPSHOT_RECALL_ADDR, SNAPSHOT_RECALL_FULL_ADDR,
};

/// The family's ceiling on one datagram (contract §2): a capture longer than
/// this is cut into chunks (§4.4), so nothing is fragmented on the way.
pub const MAX_DATAGRAM_BYTES: usize = 1200;

/// Crockford base32, without I, L, O or U: the alphabet of Go.dot's
/// identifiers (contract §2).
pub const CUE_ID_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Length of a Go.dot identifier.
pub const CUE_ID_LEN: usize = 8;

/// `where` for a capture: after the standby of the focused list, unless the
/// identifier already names a cue, which is then updated in place (§4.1).
pub const WHERE_STANDBY: &str = "standby";

/// `where` for the chunks after the first: append to the cue `target` (§4.4).
pub const WHERE_MORE: &str = "more";

/// One Go.dot OSC cue as the app captures it.
#[derive(Debug, Clone, PartialEq)]
pub struct GoDotCue {
    /// The identifier to create with or to update (8 characters of
    /// [`CUE_ID_ALPHABET`]). Empty: Go.dot draws one.
    pub id: String,
    /// Written when not empty, kept when empty.
    pub name: String,
    /// Written when not empty, kept when empty; a new cue with none has none.
    pub number: String,
    /// Written when not empty, kept when empty.
    pub notes: String,
    /// The cue's messages, in order: the first is the cue's own address and
    /// value, the rest its further messages. Typed OSC arguments; spelt as
    /// atoms by [`atoms_of`] when sent.
    pub messages: Vec<(String, Vec<OscType>)>,
}

// ── The two cues the Snapshots tab captures ─────────────────────────

/// The recall cue (contract §7.2): one message to this app,
/// `/s21/snapshot/recall s:"<uuid>"`, or `recall_full` when `ignore_scope`.
///
/// It carries the snapshot's UUID rather than its name, so renaming the
/// snapshot does not break the cue: the trigger listener resolves a UUID
/// first and a name second. The identifier is left empty for the caller,
/// which reuses the one kept on the snapshot.
pub fn build_snapshot_recall_cue(snapshot: &Snapshot, ignore_scope: bool) -> GoDotCue {
    let (addr, name) = if ignore_scope {
        (
            SNAPSHOT_RECALL_FULL_ADDR,
            format!("Recall '{}' (no scope)", snapshot.name),
        )
    } else {
        (SNAPSHOT_RECALL_ADDR, format!("Recall '{}'", snapshot.name))
    };
    GoDotCue {
        id: String::new(),
        name,
        number: String::new(),
        notes: String::new(),
        messages: vec![(
            format!("{S21_ROOT}{addr}"),
            vec![OscType::String(snapshot.id.to_string())],
        )],
    }
}

/// The console capture (contract §7.2): one message per stored parameter,
/// aimed straight at the desk's GP OSC addresses (`/channel/<n>/<param>`),
/// which the operator declares in Go.dot by hand as a device with the
/// prefixes `/channel /console /digico`.
///
/// The values are what QLab's export sends ([`build_snapshot_cues`]):
/// palette overrides applied, every stored parameter whatever the scope, and
/// iPad-only parameters (no GP OSC address) left out. Sorted by GP channel
/// number then parameter, so a re-export of an unchanged snapshot sends the
/// same messages in the same order. A value no atom spells (a float that is
/// not finite) is left out too.
///
/// `mix_output_types` is the desk's aux/group bus split (`None` before
/// discovery), which the GP bus numbers depend on.
///
/// [`build_snapshot_cues`]: crate::osc::qlab_cue_builder::build_snapshot_cues
pub fn build_console_capture(
    snapshot: &Snapshot,
    palettes: &HashMap<Uuid, ChannelPalette>,
    mix_output_types: Option<&[bool]>,
) -> GoDotCue {
    let resolved = snapshot::resolve_recall_values(snapshot, &snapshot.scope, palettes, true);
    let mut keyed = Vec::with_capacity(resolved.len());
    for (addr, value) in &resolved {
        let Some((path, args)) =
            crate::osc::encode::encode_parameter_with_config(addr, value, mix_output_types)
        else {
            continue; // iPad-only or unencodable, as the QLab export
        };
        if atoms_of(&args).is_none() {
            continue;
        }
        let channel = addr
            .channel
            .to_gp_osc_number_with_config(mix_output_types)
            .unwrap_or(u8::MAX);
        keyed.push(((channel, addr.parameter.clone()), (path, args)));
    }
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    GoDotCue {
        id: String::new(),
        name: format!("Snapshot '{}'", snapshot.name),
        number: String::new(),
        notes: String::new(),
        messages: keyed.into_iter().map(|(_, message)| message).collect(),
    }
}

// ── Spelling ────────────────────────────────────────────────────────

/// Spell typed OSC arguments as Go.dot's atom list (contract §2): `i:3`,
/// `h:3`, `f:1.5`, `d:1.5`, `s:"text"`, `T`, `F`, `N`, `I`, space-separated.
/// Strings are always quoted, with `\`, `"` and newline escaped. A float is
/// written with Rust's `{}`, which Go.dot reads back as a double and narrows.
/// An empty list is an empty string (a message with no argument).
///
/// `None` when an argument has no atom: a blob, a time tag, a char, a
/// colour, a MIDI message, an array, or a float that is not finite. None of
/// the app's encoders produce one.
pub fn atoms_of(args: &[OscType]) -> Option<String> {
    let mut atoms = Vec::with_capacity(args.len());
    for arg in args {
        atoms.push(match arg {
            OscType::Int(i) => format!("i:{i}"),
            OscType::Long(h) => format!("h:{h}"),
            OscType::Float(f) if f.is_finite() => format!("f:{f}"),
            OscType::Double(d) if d.is_finite() => format!("d:{d}"),
            OscType::String(s) => format!("s:\"{}\"", escape_atom_string(s)),
            OscType::Bool(true) => "T".to_string(),
            OscType::Bool(false) => "F".to_string(),
            OscType::Nil => "N".to_string(),
            OscType::Inf => "I".to_string(),
            _ => return None,
        });
    }
    Some(atoms.join(" "))
}

/// Escape a string for the inside of an `s:"…"` atom.
fn escape_atom_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

/// A cue number as Go.dot stores it: the app's `f32` written with Rust's
/// `{}`, so `12.0` is `"12"` and `1.5` is `"1.5"`. Go.dot compares numbers
/// as exact text, and this is the text the app means by its cue numbers.
pub fn format_cue_number(number: f32) -> String {
    format!("{number}")
}

// ── Identifiers ─────────────────────────────────────────────────────

/// Draw a fresh Go.dot identifier: 8 characters of [`CUE_ID_ALPHABET`],
/// 40 random bits taken from a v4 UUID.
pub fn new_cue_id() -> String {
    let bytes = Uuid::new_v4();
    let bytes = bytes.as_bytes();
    let mut bits: u64 = 0;
    for byte in &bytes[..5] {
        bits = (bits << 8) | u64::from(*byte);
    }
    (0..CUE_ID_LEN)
        .rev()
        .map(|i| CUE_ID_ALPHABET[((bits >> (5 * i)) & 0x1F) as usize] as char)
        .collect()
}

/// Whether `id` is spelt as a Go.dot identifier: 8 characters of
/// [`CUE_ID_ALPHABET`].
pub fn is_cue_id(id: &str) -> bool {
    id.len() == CUE_ID_LEN && id.bytes().all(|b| CUE_ID_ALPHABET.contains(&b))
}

// ── The datagrams ───────────────────────────────────────────────────

/// The datagrams that capture `cue` (contract §4): the first carries the
/// head — `where`, `target`, the identifier, name, number, notes and an
/// empty `messageIds` — then as many `address value` pairs as fit in
/// [`MAX_DATAGRAM_BYTES`]; each following one is `where = more`,
/// `target = <the cue's id>`, the other head fields empty, and the next
/// pairs (§4.4). Every argument is an OSC string.
///
/// A pair too long to share a datagram goes alone in one, over the ceiling.
/// A cue with no message gives one datagram with no pair, which Go.dot
/// refuses (`bad-value`); the client refuses it before sending.
pub fn capture_messages(where_: &str, target: &str, cue: &GoDotCue) -> Vec<OscMessage> {
    let pairs: Vec<(String, String)> = cue
        .messages
        .iter()
        .map(|(addr, args)| (addr.clone(), atoms_of(args).unwrap_or_default()))
        .collect();

    let first_head = [
        where_,
        target,
        cue.id.as_str(),
        cue.name.as_str(),
        cue.number.as_str(),
        cue.notes.as_str(),
        "",
    ];
    let more_head = [WHERE_MORE, cue.id.as_str(), "", "", "", "", ""];

    let mut out = Vec::new();
    let mut head: &[&str; 7] = &first_head;
    let mut chunk: Vec<&(String, String)> = Vec::new();
    for pair in &pairs {
        if !chunk.is_empty() {
            let fits = capture_size(head, chunk.iter().copied().chain(std::iter::once(pair)))
                <= MAX_DATAGRAM_BYTES;
            if !fits {
                out.push(capture_message(head, &chunk));
                chunk.clear();
                head = &more_head;
            }
        }
        chunk.push(pair);
    }
    out.push(capture_message(head, &chunk));
    out
}

/// One capture datagram: the head's seven strings, then the pairs.
fn capture_message(head: &[&str; 7], pairs: &[&(String, String)]) -> OscMessage {
    let mut args: Vec<OscType> = head
        .iter()
        .map(|s| OscType::String(s.to_string()))
        .collect();
    for &(addr, value) in pairs {
        args.push(OscType::String(addr.clone()));
        args.push(OscType::String(value.clone()));
    }
    OscMessage {
        addr: GODOT_CAPTURE_ADDR.to_string(),
        args,
    }
}

/// The encoded size of a capture datagram with `head` and `pairs`, without
/// encoding it: the address, the type tags and every string, each padded to
/// four bytes as OSC writes them.
fn capture_size<'a>(head: &[&str; 7], pairs: impl Iterator<Item = &'a (String, String)>) -> usize {
    let mut count = head.len();
    let mut size = osc_string_size(GODOT_CAPTURE_ADDR.len());
    size += head.iter().map(|s| osc_string_size(s.len())).sum::<usize>();
    for (addr, value) in pairs {
        count += 2;
        size += osc_string_size(addr.len()) + osc_string_size(value.len());
    }
    size + osc_string_size(1 + count) // ",sss…"
}

/// An OSC string of `len` bytes on the wire: the bytes, a terminating zero,
/// padded to a multiple of four.
fn osc_string_size(len: usize) -> usize {
    (len + 4) & !3
}

/// `mount.declare` (contract §3): `prefix` is the app's root, `port` where
/// it receives OSC (its cues and Go.dot's answers), `query_port` where it
/// serves its OSCQuery description (0 when it describes nothing), `name` the
/// device's name in Go.dot's Network tab. The two ports are `,i`.
pub fn declare_message(prefix: &str, port: u16, query_port: u16, name: &str) -> OscMessage {
    OscMessage {
        addr: GODOT_DECLARE_ADDR.to_string(),
        args: vec![
            OscType::String(prefix.to_string()),
            OscType::Int(i32::from(port)),
            OscType::Int(i32::from(query_port)),
            OscType::String(name.to_string()),
        ],
    }
}

// ── Firing Go.dot (contract §5) ─────────────────────────────────────

/// A bare command: `go`, `run.stopAll`, `run.killAll`.
fn bare(addr: &str) -> OscMessage {
    OscMessage {
        addr: addr.to_string(),
        args: vec![],
    }
}

/// A command with one string argument.
fn with_string(addr: &str, arg: &str) -> OscMessage {
    OscMessage {
        addr: addr.to_string(),
        args: vec![OscType::String(arg.to_string())],
    }
}

/// GO: fires the focused list's standby and moves it on.
pub fn go_message() -> OscMessage {
    bare(GODOT_GO_ADDR)
}

/// Fires the cue `id`, the standby untouched.
pub fn fire_message(id: &str) -> OscMessage {
    with_string(GODOT_FIRE_ADDR, id)
}

/// Fires the cue numbered `number` (exact text), the standby untouched.
pub fn fire_number_message(number: &str) -> OscMessage {
    with_string(GODOT_FIRE_NUMBER_ADDR, number)
}

/// Esc: everything stops gracefully, footers run.
pub fn stop_all_message() -> OscMessage {
    bare(GODOT_STOP_ALL_ADDR)
}

/// Double Esc: everything is dropped, footers skipped.
pub fn kill_all_message() -> OscMessage {
    bare(GODOT_KILL_ALL_ADDR)
}

/// "GO at" the cue `id`: park the standby on it, then GO, as one bundle so
/// Go.dot applies both in order in one tick.
pub fn go_at_id_bundle(id: &str) -> Vec<OscMessage> {
    vec![with_string(GODOT_STANDBY_SET_ADDR, id), go_message()]
}

/// "GO cue N": park the standby on the cue numbered `number`, then GO, as
/// one bundle. QLab's `/go "N"` does the same.
pub fn go_at_number_bundle(number: &str) -> Vec<OscMessage> {
    vec![
        with_string(GODOT_STANDBY_SET_NUMBER_ADDR, number),
        go_message(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
    use crate::model::snapshot::{ScopeTemplate, SnapshotData, SnapshotKind};

    fn snapshot_with(values: Vec<(ChannelId, ParameterPath, ParameterValue)>) -> Snapshot {
        let mut data = SnapshotData::new();
        for (channel, parameter, value) in values {
            data.values
                .insert(ParameterAddress { channel, parameter }, value);
        }
        Snapshot::new(
            "Verse 1".into(),
            ScopeTemplate::new("S".into(), vec![]),
            data,
            SnapshotKind::ApplyOnSave,
        )
    }

    fn strings(msg: &OscMessage) -> Vec<&str> {
        msg.args
            .iter()
            .map(|a| match a {
                OscType::String(s) => s.as_str(),
                other => panic!("every capture argument is a string, got {other:?}"),
            })
            .collect()
    }

    fn encoded_len(msg: &OscMessage) -> usize {
        rosc::encoder::encode(&rosc::OscPacket::Message(msg.clone()))
            .unwrap()
            .len()
    }

    #[test]
    fn recall_cue_carries_the_uuid_under_the_root() {
        let snap = snapshot_with(vec![]);
        let cue = build_snapshot_recall_cue(&snap, false);
        assert_eq!(cue.name, "Recall 'Verse 1'");
        assert_eq!(cue.id, "");
        assert_eq!(
            cue.messages,
            vec![(
                "/s21/snapshot/recall".to_string(),
                vec![OscType::String(snap.id.to_string())]
            )]
        );

        let full = build_snapshot_recall_cue(&snap, true);
        assert_eq!(full.messages[0].0, "/s21/snapshot/recall_full");
        assert_eq!(full.name, "Recall 'Verse 1' (no scope)");
    }

    #[test]
    fn console_capture_is_sorted_typed_and_skips_ipad_only() {
        let snap = snapshot_with(vec![
            (
                ChannelId::Input(2),
                ParameterPath::Mute,
                ParameterValue::Bool(true),
            ),
            (
                ChannelId::Input(1),
                ParameterPath::Fader,
                ParameterValue::Float(-10.0),
            ),
            // iPad-only: no GP OSC address, left out as QLab's export does.
            (
                ChannelId::Input(1),
                ParameterPath::Phantom,
                ParameterValue::Bool(true),
            ),
        ]);
        let cue = build_console_capture(&snap, &HashMap::new(), None);
        assert_eq!(cue.name, "Snapshot 'Verse 1'");
        assert_eq!(
            cue.messages,
            vec![
                ("/channel/1/fader".to_string(), vec![OscType::Float(-10.0)]),
                ("/channel/2/mute".to_string(), vec![OscType::Bool(true)]),
            ]
        );
    }

    #[test]
    fn atoms_spell_every_type_and_escape_strings() {
        assert_eq!(atoms_of(&[]).as_deref(), Some(""));
        assert_eq!(
            atoms_of(&[
                OscType::Int(3),
                OscType::Long(4),
                OscType::Float(1.5),
                OscType::Float(-10.0),
                OscType::Double(0.25),
                OscType::Bool(true),
                OscType::Bool(false),
                OscType::Nil,
                OscType::Inf,
            ])
            .as_deref(),
            Some("i:3 h:4 f:1.5 f:-10 d:0.25 T F N I")
        );
        assert_eq!(
            atoms_of(&[OscType::String("say \"hi\"\\\nbye".into())]).as_deref(),
            Some(r#"s:"say \"hi\"\\\nbye""#)
        );
        // Strings are quoted even when plain, so a word never reads bare.
        assert_eq!(
            atoms_of(&[OscType::String("Kick".into())]).as_deref(),
            Some(r#"s:"Kick""#)
        );
        assert_eq!(atoms_of(&[OscType::Float(f32::NAN)]), None);
        assert_eq!(atoms_of(&[OscType::Char('x')]), None);
    }

    #[test]
    fn a_float_atom_reads_back_as_the_same_f32() {
        for f in [0.1f32, -10.04, 1e-7, 3.4e38, 12.0, -0.0] {
            let atom = atoms_of(&[OscType::Float(f)]).unwrap();
            let back: f64 = atom.strip_prefix("f:").unwrap().parse().unwrap();
            assert_eq!(back as f32, f, "{atom}");
        }
    }

    #[test]
    fn cue_numbers_have_no_trailing_zeros() {
        assert_eq!(format_cue_number(12.0), "12");
        assert_eq!(format_cue_number(1.5), "1.5");
        assert_eq!(format_cue_number(0.25), "0.25");
    }

    #[test]
    fn identifiers_are_eight_crockford_characters() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let id = new_cue_id();
            assert!(is_cue_id(&id), "{id}");
            assert!(!id.contains(['I', 'L', 'O', 'U']), "{id}");
            seen.insert(id);
        }
        assert!(seen.len() > 190, "identifiers should not repeat");
        assert!(!is_cue_id("ABCDEFGI"), "I is not in the alphabet");
        assert!(!is_cue_id("abcdefgh"), "lower case is not the spelling");
        assert!(!is_cue_id("ABC"));
    }

    #[test]
    fn a_short_capture_is_one_datagram_with_the_full_head() {
        let cue = GoDotCue {
            id: "0123ABCD".into(),
            name: "Recall 'Verse 1'".into(),
            number: "12".into(),
            notes: "a note".into(),
            messages: vec![
                (
                    "/s21/snapshot/recall".into(),
                    vec![OscType::String("x".into())],
                ),
                ("/s21/cue/go".into(), vec![]),
            ],
        };
        let msgs = capture_messages(WHERE_STANDBY, "", &cue);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].addr, "/godot/cmd/cue/capture");
        assert_eq!(
            strings(&msgs[0]),
            vec![
                "standby",
                "",
                "0123ABCD",
                "Recall 'Verse 1'",
                "12",
                "a note",
                "",
                "/s21/snapshot/recall",
                r#"s:"x""#,
                "/s21/cue/go",
                "",
            ]
        );
        // Seven head strings, then pairs: an odd count never leaves.
        assert_eq!((msgs[0].args.len() - 7) % 2, 0);
    }

    #[test]
    fn a_long_capture_is_cut_into_chunks_under_the_ceiling() {
        let messages: Vec<(String, Vec<OscType>)> = (1..=120)
            .map(|n| (format!("/channel/{n}/fader"), vec![OscType::Float(-12.5)]))
            .collect();
        let cue = GoDotCue {
            id: "0123ABCD".into(),
            name: "Snapshot 'Big'".into(),
            number: String::new(),
            notes: String::new(),
            messages: messages.clone(),
        };
        let msgs = capture_messages(WHERE_STANDBY, "", &cue);
        assert!(msgs.len() > 1, "120 pairs do not fit in one datagram");

        let mut pairs_seen = Vec::new();
        for (i, msg) in msgs.iter().enumerate() {
            let len = encoded_len(msg);
            assert!(len <= MAX_DATAGRAM_BYTES, "chunk {i} is {len} bytes");
            // The estimate is the encoder's size, to the byte.
            let s = strings(msg);
            let head: [&str; 7] = s[..7].try_into().unwrap();
            let pairs: Vec<(String, String)> = s[7..]
                .chunks(2)
                .map(|p| (p[0].to_string(), p[1].to_string()))
                .collect();
            assert_eq!(capture_size(&head, pairs.iter()), len);
            assert_eq!((s.len() - 7) % 2, 0, "pair parity in chunk {i}");
            if i == 0 {
                assert_eq!(
                    &s[..7],
                    &["standby", "", "0123ABCD", "Snapshot 'Big'", "", "", ""]
                );
            } else {
                assert_eq!(&s[..7], &["more", "0123ABCD", "", "", "", "", ""]);
            }
            pairs_seen.extend(pairs);
        }
        // Every pair once, in order.
        let expected: Vec<(String, String)> = messages
            .iter()
            .map(|(a, v)| (a.clone(), atoms_of(v).unwrap()))
            .collect();
        assert_eq!(pairs_seen, expected);
        // Full chunks: the next pair would not have fitted.
        let first_pairs = (msgs[0].args.len() - 7) / 2;
        let s0 = strings(&msgs[0]);
        let head0: [&str; 7] = s0[..7].try_into().unwrap();
        assert!(
            capture_size(&head0, expected[..=first_pairs].iter()) > MAX_DATAGRAM_BYTES,
            "the first chunk was cut early"
        );
    }

    #[test]
    fn an_oversized_pair_goes_alone() {
        let huge = "x".repeat(1500);
        let cue = GoDotCue {
            id: "0123ABCD".into(),
            name: String::new(),
            number: String::new(),
            notes: String::new(),
            messages: vec![
                ("/s21/a".into(), vec![]),
                ("/s21/b".into(), vec![OscType::String(huge)]),
                ("/s21/c".into(), vec![]),
            ],
        };
        let msgs = capture_messages(WHERE_STANDBY, "", &cue);
        let counts: Vec<usize> = msgs.iter().map(|m| (m.args.len() - 7) / 2).collect();
        assert_eq!(counts, vec![1, 1, 1]);
    }

    #[test]
    fn declare_is_siis() {
        let msg = declare_message("/s21", 53001, 8080, "S21_HiJack");
        assert_eq!(msg.addr, "/godot/cmd/mount/declare");
        assert_eq!(
            msg.args,
            vec![
                OscType::String("/s21".into()),
                OscType::Int(53001),
                OscType::Int(8080),
                OscType::String("S21_HiJack".into()),
            ]
        );
    }

    #[test]
    fn firing_verbs_spell_the_contract() {
        assert_eq!(go_message().addr, "/godot/cmd/go");
        assert!(go_message().args.is_empty());
        assert_eq!(
            fire_message("0123ABCD").args,
            vec![OscType::String("0123ABCD".into())]
        );
        assert_eq!(fire_number_message("12").addr, "/godot/cmd/cue/fireNumber");
        assert_eq!(stop_all_message().addr, "/godot/cmd/run/stopAll");
        assert_eq!(kill_all_message().addr, "/godot/cmd/run/killAll");
        let at_id = go_at_id_bundle("0123ABCD");
        assert_eq!(at_id[0].addr, "/godot/cmd/standby/set");
        assert_eq!(at_id[1].addr, "/godot/cmd/go");
        let at_n = go_at_number_bundle("12");
        assert_eq!(at_n[0].addr, "/godot/cmd/standby/setNumber");
        assert_eq!(at_n[0].args, vec![OscType::String("12".into())]);
        assert_eq!(at_n[1].addr, "/godot/cmd/go");
    }
}
