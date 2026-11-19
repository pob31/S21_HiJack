//! Session reader tests.
//!
//! The Quantum fixtures are the user's playground sessions from the controlled
//! saves recorded in `Documentation/SESSION_FILES_FIELD_NOTES.md` §3b; every
//! expected value below was read off the Quantum 225 editor's screen.

use std::path::{Path, PathBuf};

use super::doc::{Format, SessionDoc, StripKind, StripState, Value};
use super::{load, report};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sessions")
        .join(name)
}

fn state<'a>(doc: &'a SessionDoc, kind: StripKind, name: &str) -> &'a StripState {
    doc.live
        .iter()
        .find(|s| {
            let strip = &doc.strips[s.strip];
            strip.kind == kind && strip.name == name
        })
        .unwrap_or_else(|| panic!("no {kind:?} strip named {name}"))
}

fn value<'a>(s: &'a StripState, key: &str) -> &'a Value {
    &s.params
        .iter()
        .find(|p| p.key == key)
        .unwrap_or_else(|| panic!("no param {key}"))
        .value
}

fn choice(s: &StripState, key: &str) -> String {
    match value(s, key) {
        Value::Choice(c) | Value::Text(c) | Value::Ref(c) => c.clone(),
        other => panic!("{key} is not text: {other:?}"),
    }
}

fn num(s: &StripState, key: &str) -> f64 {
    match value(s, key) {
        Value::Num(v, _) => *v,
        other => panic!("{key} is not a number: {other:?}"),
    }
}

fn flag(s: &StripState, key: &str) -> bool {
    match value(s, key) {
        Value::Bool(b) => *b,
        other => panic!("{key} is not a switch: {other:?}"),
    }
}

fn close(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} is not within {tolerance} of {expected}"
    );
}

fn quantum() -> SessionDoc {
    load(&fixture("quantum_test.ses")).expect("quantum fixture parses")
}

#[test]
fn quantum_header_and_strips() {
    let doc = quantum();
    assert_eq!(doc.show_name, "playground for Claude");
    assert_eq!(doc.sample_rate, Some(48000));
    match &doc.source.format {
        Format::SdSes {
            model,
            version,
            build,
        } => {
            assert_eq!(model, "Q2");
            assert_eq!(*version, 'O');
            assert_eq!(*build, 2242);
        }
        other => panic!("unexpected format {other:?}"),
    }
    let inputs = doc
        .strips
        .iter()
        .filter(|s| s.kind == StripKind::Input)
        .count();
    assert_eq!(inputs, 48);
    assert!(doc.snapshots.is_empty());
}

#[test]
fn quantum_mustard_dynamics_and_tube() {
    let doc = quantum();
    let ch6 = state(&doc, StripKind::Input, "Ch 6");
    assert_eq!(choice(ch6, "mustard.dyn1.type"), "Levelling Amp");
    close(num(ch6, "mustard.dyn1.mix"), 0.0, 0.01);
    close(num(ch6, "mustard.dyn1.gain"), 100.0, 0.01);
    assert_eq!(choice(ch6, "mustard.dyn1.mode"), "limit");
    close(num(ch6, "mustard.dyn1.filter_hi"), 6072.7, 0.5);
    assert_eq!(choice(ch6, "mustard.dyn2.type"), "MSE");
    close(num(ch6, "mustard.dyn2.depth"), 39.84, 0.01);
    assert_eq!(choice(ch6, "mustard.dyn2.release"), "slow");
    assert_eq!(choice(ch6, "mustard.tube.character"), "odd harmonics");
    assert_eq!(choice(ch6, "mustard.position"), "mid-EQ/dyn");

    let ch7 = state(&doc, StripKind::Input, "Ch 7");
    assert_eq!(choice(ch7, "mustard.dyn1.type"), "Vintage VCA");
    close(num(ch7, "mustard.dyn1.mix"), 94.9, 0.1);
    close(num(ch7, "mustard.dyn1.ratio"), 3.16, 0.01);
    assert_eq!(choice(ch7, "mustard.dyn2.type"), "Duck");
    assert_eq!(choice(ch7, "mustard.dyn2.key"), "Ch 1");
    close(num(ch7, "mustard.dyn2.hold"), 0.0856, 0.0001);

    let ch8 = state(&doc, StripKind::Input, "Ch 8");
    assert_eq!(choice(ch8, "mustard.dyn1.type"), "Optical");
    assert_eq!(choice(ch8, "mustard.dyn1.attack"), "medium");

    let ch9 = state(&doc, StripKind::Input, "Ch 9");
    assert_eq!(choice(ch9, "mustard.dyn1.type"), "FET Limiter");
    assert_eq!(choice(ch9, "mustard.dyn1.ratio"), "8:1");
    close(num(ch9, "mustard.dyn1.input"), -19.29, 0.01);
    close(num(ch9, "mustard.dyn1.attack"), 0.000_827, 0.000_001);

    let ch4 = state(&doc, StripKind::Input, "Ch 4");
    assert!(flag(ch4, "mustard.eq.on"));
    close(num(ch4, "mustard.eq.band1.freq"), 20000.0, 0.5);
    close(num(ch4, "mustard.eq.band1.gain"), -18.0, 0.01);
}

#[test]
fn quantum_sd_eq_filters_and_mutes() {
    let doc = quantum();
    let ch2 = state(&doc, StripKind::Input, "Ch 2");
    // Stored scaled: 22.75 × 1.25 and 14942 × 0.8.
    close(num(ch2, "filter.hpf"), 28.44, 0.01);
    close(num(ch2, "filter.lpf"), 11953.7, 0.5);
    close(num(ch2, "eq.band1.freq"), 5449.1, 0.5);
    close(num(ch2, "eq.band1.gain"), 1.48, 0.01);
    assert_eq!(choice(ch2, "eq.band1.mode"), "classic");
    assert_eq!(choice(ch2, "eq.band2.curve"), "shelf");
    assert!(flag(ch2, "eq.band2.dyn"));
    assert_eq!(choice(ch2, "eq.band2.dyn_mode"), "under");
    assert_eq!(choice(ch2, "eq.band3.dyn_mode"), "over");
    close(num(ch2, "eq.band3.dyn_attack"), 0.024, 0.001);
    assert_eq!(choice(ch2, "insert.b.send"), "0:Line 8");
    assert!(flag(ch2, "insert.b.on"));

    let ch1 = state(&doc, StripKind::Input, "Ch 1");
    assert!(flag(ch1, "fader.hard_mute"));
    assert_eq!(choice(ch1, "insert.a.send"), "0:Line 7");
}

#[test]
fn quantum_sends_and_nodal_processing() {
    let doc = quantum();
    let ch28 = state(&doc, StripKind::Input, "Ch 28");
    assert!(flag(ch28, "input.nodal"));
    close(num(ch28, "send.0.level"), -75.99, 0.01);
    assert_eq!(choice(ch28, "send.1.point"), "post-fader");
    assert_eq!(choice(ch28, "send.3.point"), "pre-mute");

    let node = state(&doc, StripKind::Node, "Ch 28 > Aux 2");
    assert_eq!(choice(node, "dyn.mode"), "multiband");
    close(num(node, "dyn.lo.release"), 0.7, 0.001);
    close(num(node, "dyn.hi.attack"), 0.005_02, 0.000_01);
    close(num(node, "dyn.mid.ratio"), 2.0, 0.001);
    close(num(node, "dyn.xover_lo"), 148.46, 0.01);
    assert_eq!(choice(node, "gate.mode"), "compressor");
    assert!(flag(node, "eq.band2.dyn"));
    assert_eq!(choice(node, "eq.band1.mode"), "precision");

    let ess = state(&doc, StripKind::Node, "Ch 28 > Aux 4");
    assert_eq!(choice(ess, "dyn.mode"), "de-esser");
    close(num(ess, "dyn.ess.thresh"), -24.94, 0.01);
    close(num(ess, "dyn.ess.hp"), 5000.0, 0.5);
    assert_eq!(choice(ess, "gate.mode"), "duck");
    close(num(ess, "gate.hold"), 0.08, 0.0001);
}

#[test]
fn quantum_spice_rack() {
    let doc = quantum();
    let ch5 = state(&doc, StripKind::Input, "Ch 5");
    assert_eq!(choice(ch5, "insert.a.send"), "Spice Rack 1 (Chilli 6)");

    let chilli = state(&doc, StripKind::SpiceRack, "Chilli 6");
    assert_eq!(choice(chilli, "spice.host"), "Ch 5, insert A");
    close(num(chilli, "spice.band1.gain"), 18.0, 0.01);
    close(num(chilli, "spice.band2.gain"), -18.0, 0.01);
    close(num(chilli, "spice.xover_low"), 123.6, 0.1);
    close(num(chilli, "spice.P2.freq"), 14063.4, 0.5);

    let naga = state(&doc, StripKind::SpiceRack, "Naga 6");
    assert_eq!(choice(naga, "spice.host"), "Ch 7, insert A");
    close(num(naga, "spice.band2.freq"), 75.42, 0.01);
    close(num(naga, "spice.band5.gain"), -2.05, 0.01);
    close(num(naga, "spice.release_shape"), 0.82, 0.001);
}

#[test]
fn quantum_blank_session_has_no_mustard_or_nodes() {
    let doc = load(&fixture("quantum_blank.ses")).expect("blank fixture parses");
    assert_eq!(doc.show_name, "playground for Claude");
    for s in &doc.live {
        assert!(s.params.iter().all(|p| !p.key.starts_with("mustard.")));
    }
    assert!(!doc.strips.iter().any(|s| s.kind == StripKind::Node));
}

#[test]
fn raw_fields_are_only_kept_when_asked() {
    let path = fixture("quantum_blank.ses");
    let plain = load(&path).unwrap();
    let raw = super::load_with(&path, super::LoadOptions { include_raw: true }).unwrap();
    let count = |d: &SessionDoc| {
        d.live
            .iter()
            .flat_map(|s| &s.params)
            .filter(|p| p.section() == "raw")
            .count()
    };
    assert_eq!(count(&plain), 0);
    assert!(count(&raw) > 0);
    assert!(
        raw.live
            .iter()
            .flat_map(|s| &s.params)
            .filter(|p| p.section() == "raw")
            .all(|p| !p.confirmed)
    );
}

#[test]
fn quantum_report_covers_the_quantum_sections() {
    let html = report::render(&quantum(), "test");
    for needle in [
        "playground for Claude",
        "Levelling Amp",
        "Spice Rack",
        "Naga 6",
        "Nodal processing",
        "Ch 28 &gt; Aux 4",
        "pre-mute",
        "HARD",
    ] {
        assert!(html.contains(needle), "report lacks {needle}");
    }
}

#[test]
fn unknown_files_are_rejected() {
    let dir = std::env::temp_dir().join("s21_hijack_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("not_a_session.ses");
    std::fs::write(&path, b"hello").unwrap();
    assert!(load(&path).is_err());
    std::fs::write(&path, b"DiGiCo     SDC .SES vN   truncated").unwrap();
    assert!(load(&path).is_err());
}

/// The S21 reader addresses columns by name: build a small database whose
/// column order differs from real files and read it back.
#[test]
fn s21_reader_uses_column_names_not_positions() {
    let dir = std::env::temp_dir().join("s21_hijack_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("columns.session");
    let _ = std::fs::remove_file(&path);
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "PRAGMA user_version = 99;
             CREATE TABLE Session (sampleRate INTEGER, id INTEGER, name TEXT);
             INSERT INTO Session VALUES (96000, 1, 'Session');
             CREATE TABLE Snapshot (name TEXT, note TEXT, id INTEGER, unsavedChanges INTEGER, number INTEGER);
             INSERT INTO Snapshot VALUES (NULL, NULL, 0, 0, 65535);
             INSERT INTO Snapshot VALUES ('Opening', 'GO on the blackout', 10000, 0, 1);
             CREATE TABLE Channel (mute_saved INTEGER, gain REAL, name TEXT, type INTEGER,
                                   snapshotId INTEGER, channelNumber INTEGER, mute INTEGER,
                                   gain_saved REAL, busMode INTEGER, stereoMode INTEGER);
             INSERT INTO Channel VALUES (0, -6.5, 'Kick', 0, 0, 1, 0, -6.5, 0, 0);
             INSERT INTO Channel VALUES (0, 0.0, 'Kick', 0, 10000, 1, 1, -6.5, 0, 0);
             INSERT INTO Channel VALUES (0, -150.0, 'MON 1', 1, 0, 1, 0, -150.0, 0, 0);
             CREATE TABLE EqualiserBand (gain REAL, bandNumber INTEGER, snapshotId INTEGER,
                                         channelType INTEGER, channelNumber INTEGER,
                                         frequency REAL, qvalue REAL, bandType INTEGER);
             INSERT INTO EqualiserBand VALUES (-3.0, 0, 0, 0, 1, 100.0, 0.71, 0);
             CREATE TABLE ChannelBusSend (busId INTEGER, gainInAuxMode REAL, snapshotId INTEGER,
                                          enabledInAuxMode INTEGER, channelType INTEGER,
                                          channelNumber INTEGER, enabledInGroupMode INTEGER);
             INSERT INTO ChannelBusSend VALUES (0, -10.0, 0, 1, 0, 1, 0);",
        )
        .unwrap();
    }
    let doc = load(&path).expect("synthetic S21 session parses");
    assert!(matches!(
        doc.source.format,
        Format::S21Session { user_version: 99 }
    ));
    assert_eq!(doc.sample_rate, Some(96000));
    let kick = state(&doc, StripKind::Input, "Kick");
    close(num(kick, "fader.level"), -6.5, 1e-9);
    close(num(kick, "eq.band1.freq"), 100.0, 1e-9);
    close(num(kick, "send.0.level"), -10.0, 1e-9);
    assert_eq!(value(kick, "send.0.on"), &Value::Bool(true));
    assert_eq!(doc.snapshots.len(), 1);
    let snap = &doc.snapshots[0];
    assert_eq!(snap.name, "Opening");
    assert_eq!(snap.notes, "GO on the blackout");
    // gain/mute differ from their _saved twins → unsaved edits.
    assert!(snap.unsaved);
}

/// Opt-in sweep over real show files: set `S21H_SESSION_SAMPLES` to one or
/// more folders (separated by `;`) and every `.ses` / `.session` below them
/// must parse and render.
#[test]
fn every_local_sample_parses() {
    let Ok(roots) = std::env::var("S21H_SESSION_SAMPLES") else {
        return;
    };
    let mut files = Vec::new();
    for root in roots.split(';').filter(|r| !r.is_empty()) {
        collect(Path::new(root), &mut files);
    }
    assert!(!files.is_empty(), "no session files under {roots}");
    for f in &files {
        let doc = load(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        let html = report::render(&doc, "test");
        assert!(html.contains("Cue sheet"), "{}", f.display());
    }
    eprintln!("parsed and rendered {} session files", files.len());
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if matches!(
            p.extension()
                .and_then(|x| x.to_str())
                .map(|x| x.to_ascii_lowercase())
                .as_deref(),
            Some("ses" | "session")
        ) {
            out.push(p);
        }
    }
}
