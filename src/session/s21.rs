//! DiGiCo S21/S31 `.session` reader (SQLite, opened read-only).
//!
//! Column order differs between files of the same schema version, so every
//! read goes by column name. Snapshot 0 is the live desk state; user
//! snapshots have ids ≥ 10000 and a 1-based `number`. See
//! `Documentation/SESSION_FILES_FIELD_NOTES.md` §1.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::types::Value as Sql;
use rusqlite::{Connection, OpenFlags};

use super::SessionError;
use super::doc::{
    Format, ParamList, SessionDoc, SnapshotDoc, Source, Strip, StripKind, StripState, Unit, Value,
};

type Row = HashMap<String, Sql>;

/// (channelType, channelNumber)
type ChKey = (i64, i64);

pub fn read(path: &Path) -> Result<SessionDoc, SessionError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let user_version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let db = Db { conn };

    let session = db.rows("Session", None)?;
    let sample_rate = session
        .first()
        .and_then(|r| int(r, "sampleRate"))
        .and_then(|v| u32::try_from(v).ok());

    let channel_rows = db.rows("Channel", None)?;
    let mut by_snapshot: HashMap<i64, Vec<Row>> = HashMap::new();
    for r in channel_rows {
        if let Some(id) = int(&r, "snapshotId") {
            by_snapshot.entry(id).or_default().push(r);
        }
    }

    // Strips come from the live state (snapshot 0).
    let live_channels = by_snapshot.get(&0).cloned().unwrap_or_default();
    let mut strips = Vec::new();
    let mut strip_of: HashMap<ChKey, usize> = HashMap::new();
    let mut bus_mode: HashMap<i64, i64> = HashMap::new();
    let mut sorted = live_channels.clone();
    sorted.sort_by_key(|r| (int(r, "type"), int(r, "channelNumber")));
    for r in &sorted {
        let (Some(ty), Some(num)) = (int(r, "type"), int(r, "channelNumber")) else {
            continue;
        };
        let mode = int(r, "busMode").unwrap_or(0);
        if ty == 1 {
            bus_mode.insert(num, mode);
        }
        let kind = match ty {
            0 => StripKind::Input,
            1 if mode == 1 => StripKind::Group,
            1 => StripKind::Aux,
            2 => StripKind::ControlGroup,
            3 => StripKind::Master,
            4 => StripKind::Matrix,
            5 => StripKind::Solo,
            _ => StripKind::Other,
        };
        let stereo = int(r, "stereoMode")
            .or_else(|| int(r, "stereo"))
            .unwrap_or(0)
            == 1;
        strip_of.insert((ty, num), strips.len());
        strips.push(Strip {
            kind,
            // Control groups are numbered from 0 in the file, everything else from 1.
            index: if ty == 2 { num } else { num - 1 }.max(0) as u16,
            name: text(r, "name").unwrap_or_default(),
            stereo,
        });
    }

    let ctx = Ctx {
        strips: &strips,
        strip_of: &strip_of,
        bus_mode: &bus_mode,
        patch: db.input_patch()?,
        sockets: db.socket_names()?,
    };

    let live = db.snapshot_state(
        0,
        by_snapshot.get(&0).map(Vec::as_slice).unwrap_or(&[]),
        &ctx,
    )?;

    let mut snapshots = Vec::new();
    let mut snap_rows = db.rows("Snapshot", None)?;
    snap_rows.sort_by_key(|r| int(r, "number"));
    for r in &snap_rows {
        let (Some(id), Some(number)) = (int(r, "id"), int(r, "number")) else {
            continue;
        };
        // 0 is the live state; 9999 holds the solo channels; numbers ≤ 0 or
        // ≥ 65534 are those same internal rows in the two file lineages.
        if id == 0 || id == 9999 || number <= 0 || number >= 65534 {
            continue;
        }
        let channels = by_snapshot.get(&id).map(Vec::as_slice).unwrap_or(&[]);
        let unsaved = int(r, "unsavedChanges").unwrap_or(0) != 0
            || channels.iter().any(|c| {
                c.get("gain") != c.get("gain_saved") || c.get("mute") != c.get("mute_saved")
            });
        snapshots.push(SnapshotDoc {
            number: number as f64,
            name: text(r, "name").unwrap_or_default(),
            group: None,
            crossfade_s: None,
            notes: text(r, "note").unwrap_or_default().replace("\r\n", "\n"),
            unsaved,
            states: db.snapshot_state(id, channels, &ctx)?,
        });
    }

    let show_name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(SessionDoc {
        source: Source {
            file_name: path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            format: Format::S21Session { user_version },
        },
        show_name,
        sample_rate,
        strips,
        live,
        snapshots,
        snapshot_groups: Vec::new(),
        warnings: vec![
            "Recall scope is session-wide on the S21 and is not printed per snapshot.".to_string(),
            "EQ shelf names (low/high shelf) are not yet confirmed against the desk.".to_string(),
        ],
    })
}

struct Ctx<'a> {
    strips: &'a [Strip],
    strip_of: &'a HashMap<ChKey, usize>,
    /// Bus channel number (1-based) → busMode (0 aux, 1 group).
    bus_mode: &'a HashMap<i64, i64>,
    /// Input channel number → (receiver socket id) for the patch lookup.
    patch: HashMap<i64, i64>,
    /// Socket id → readable description (port / device / socket numbers).
    sockets: HashMap<i64, String>,
}

impl Ctx<'_> {
    fn bus_name(&self, bus_id: i64) -> String {
        // busId is 0-based over the bus pool; the last id is the master.
        if let Some(&i) = self.strip_of.get(&(1, bus_id + 1)) {
            return self.strips[i].title();
        }
        "Master".to_string()
    }
}

struct Db {
    conn: Connection,
}

impl Db {
    fn rows(&self, table: &str, snapshot: Option<i64>) -> Result<Vec<Row>, SessionError> {
        let exists: i64 = self.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |r| r.get(0),
        )?;
        if exists == 0 {
            return Ok(Vec::new());
        }
        let sql = match snapshot {
            Some(_) => format!("SELECT * FROM \"{table}\" WHERE snapshotId = ?1"),
            None => format!("SELECT * FROM \"{table}\""),
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let names: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();
        let map = |r: &rusqlite::Row| {
            let mut row = Row::with_capacity(names.len());
            for (i, n) in names.iter().enumerate() {
                row.insert(n.clone(), r.get::<_, Sql>(i)?);
            }
            Ok(row)
        };
        let rows = match snapshot {
            Some(id) => stmt.query_map([id], map)?.collect::<Result<Vec<_>, _>>()?,
            None => stmt.query_map([], map)?.collect::<Result<Vec<_>, _>>()?,
        };
        Ok(rows)
    }

    /// Input channel → receiver socket id, via the static socket map
    /// (verified on real sessions: channel input socket `(ch−1)·30`).
    fn input_patch(&self) -> Result<HashMap<i64, i64>, SessionError> {
        let mut out = HashMap::new();
        for r in self.rows("SocketInfo", None)? {
            let is_channel_input = int(&r, "portType") == Some(0)
                && int(&r, "deviceType") == Some(3)
                && int(&r, "socketInfoType") == Some(2);
            let (Some(num), Some(hw), Some(id)) = (
                int(&r, "socketNumber"),
                int(&r, "socketHWNumber"),
                int(&r, "id"),
            ) else {
                continue;
            };
            if is_channel_input && num >= 1 && hw == (num - 1) * 30 {
                out.insert(num, id);
            }
        }
        Ok(out)
    }

    /// Socket id → "port P · device D · socket S" (the numbering is only
    /// partly decoded, so the label stays close to the raw values).
    fn socket_names(&self) -> Result<HashMap<i64, String>, SessionError> {
        let mut out = HashMap::new();
        for r in self.rows("SocketInfo", None)? {
            let Some(id) = int(&r, "id") else { continue };
            let port = match int(&r, "portType") {
                Some(0) => "internal".to_string(),
                Some(1) => "local".to_string(),
                Some(2) => "rack".to_string(),
                Some(n) => format!("port {n}"),
                None => "port ?".to_string(),
            };
            let device = int(&r, "deviceNumber").unwrap_or(0);
            let socket = int(&r, "socketNumber").unwrap_or(0);
            out.insert(id, format!("{port} {device}, socket {socket}"));
        }
        Ok(out)
    }

    fn snapshot_state(
        &self,
        id: i64,
        channels: &[Row],
        ctx: &Ctx,
    ) -> Result<Vec<StripState>, SessionError> {
        let keyed = |rows: Vec<Row>| -> HashMap<ChKey, Vec<Row>> {
            let mut m: HashMap<ChKey, Vec<Row>> = HashMap::new();
            for r in rows {
                if let (Some(t), Some(n)) = (int(&r, "channelType"), int(&r, "channelNumber")) {
                    m.entry((t, n)).or_default().push(r);
                }
            }
            m
        };
        let input = keyed(self.rows("Input", Some(id))?);
        let passband = keyed(self.rows("Passband", Some(id))?);
        let eq = keyed(self.rows("Equaliser", Some(id))?);
        let bands = keyed(self.rows("EqualiserBand", Some(id))?);
        let dynamics = keyed(self.rows("Dynamics", Some(id))?);
        let processors = keyed(self.rows("DynamicProcessor", Some(id))?);
        let pans = keyed(self.rows("Pan", Some(id))?);
        let sends = keyed(self.rows("ChannelBusSend", Some(id))?);
        let inserts = keyed(self.rows("InsertData", Some(id))?);
        let receivers: HashMap<i64, i64> = self
            .rows("ReceiverSocket", Some(id))?
            .iter()
            .filter_map(|r| Some((int(r, "socketId")?, int(r, "routedSocketId")?)))
            .collect();
        let senders: HashMap<i64, Row> = self
            .rows("SenderSocket", Some(id))?
            .into_iter()
            .filter_map(|r| Some((int(&r, "socketId")?, r)))
            .collect();

        let empty = Vec::new();
        let mut out = Vec::new();
        for ch in channels {
            let (Some(ty), Some(num)) = (int(ch, "type"), int(ch, "channelNumber")) else {
                continue;
            };
            let Some(&strip) = ctx.strip_of.get(&(ty, num)) else {
                continue;
            };
            let key = (ty, num);
            let mut p = ParamList::default();

            if let Some(i) = input.get(&key).and_then(|v| v.first()) {
                if let Some(v) = float(i, "trim") {
                    p.num("input.trim", "Trim", v, Unit::Db);
                }
                if let Some(v) = int(i, "polarity") {
                    let s = if v == 0 { "normal" } else { "inverted" };
                    p.push("input.phase", "Phase", Value::Choice(s.into()));
                }
            }
            if ty == 0 {
                patch(&mut p, num, ctx, &receivers, &senders);
            }
            if let Some(i) = input.get(&key).and_then(|v| v.first()) {
                if let Some(v) = int(i, "delayEnabled") {
                    p.flag("delay.on", "Delay", v != 0);
                }
                if let Some(v) = float(i, "delay") {
                    p.num("delay.time", "Delay time", v, Unit::Seconds);
                }
            }
            if let Some(pb) = passband
                .get(&key)
                .and_then(|v| v.iter().find(|r| int(r, "passbandType") == Some(0)))
            {
                if let Some(v) = int(pb, "highPassEnabled") {
                    p.flag("filter.hpf_on", "HPF", v != 0);
                }
                if let Some(v) = float(pb, "highPassFrequency") {
                    p.num("filter.hpf", "HPF freq", v, Unit::Hz);
                }
                if let Some(v) = int(pb, "lowPassEnabled") {
                    p.flag("filter.lpf_on", "LPF", v != 0);
                }
                if let Some(v) = float(pb, "lowPassFrequency") {
                    p.num("filter.lpf", "LPF freq", v, Unit::Hz);
                }
            }
            if let Some(v) = eq
                .get(&key)
                .and_then(|v| v.first())
                .and_then(|r| int(r, "enabled"))
            {
                p.flag("eq.on", "EQ", v != 0);
            }
            let mut band_rows: Vec<&Row> = bands.get(&key).unwrap_or(&empty).iter().collect();
            band_rows.sort_by_key(|r| int(r, "bandNumber"));
            for b in band_rows {
                let n = int(b, "bandNumber").unwrap_or(0) + 1;
                let k = |f: &str| format!("eq.band{n}.{f}");
                let l = |f: &str| format!("EQ {n} {f}");
                if let Some(v) = float(b, "frequency") {
                    p.num(&k("freq"), &l("freq"), v, Unit::Hz);
                }
                if let Some(v) = float(b, "gain") {
                    p.num(&k("gain"), &l("gain"), v, Unit::Db);
                }
                if let Some(v) = float(b, "qvalue") {
                    p.num(&k("q"), &l("Q"), v, Unit::Plain);
                }
                if let Some(v) = int(b, "bandType") {
                    let s = match v {
                        0 => "bell",
                        1 => "low shelf",
                        2 => "high shelf",
                        _ => "unknown",
                    };
                    if v == 0 {
                        p.push(k("curve"), l("curve"), Value::Choice(s.into()));
                    } else {
                        p.push_unconfirmed(k("curve"), l("curve"), Value::Choice(s.into()));
                    }
                }
                let dynamic = int(b, "dynamicsEnabled").unwrap_or(0) != 0;
                if b.contains_key("dynamicsEnabled") {
                    p.flag(&k("dyn"), &l("dynamic"), dynamic);
                }
                if dynamic {
                    for (col, f, label, unit) in [
                        ("dynamicsThreshold", "dyn_thresh", "dyn threshold", Unit::Db),
                        ("dynamicsRatio", "dyn_ratio", "dyn ratio", Unit::Ratio),
                        ("dynamicsAttack", "dyn_attack", "dyn attack", Unit::Seconds),
                        (
                            "dynamicsRelease",
                            "dyn_release",
                            "dyn release",
                            Unit::Seconds,
                        ),
                    ] {
                        if let Some(v) = float(b, col) {
                            p.num(&k(f), &l(label), v, unit);
                        }
                    }
                }
            }
            let dyn_rows = dynamics.get(&key).unwrap_or(&empty);
            let proc_rows = processors.get(&key).unwrap_or(&empty);
            let processor = |n: i64| {
                proc_rows
                    .iter()
                    .find(|r| int(r, "processorNumber") == Some(n))
            };
            if let Some(d1) = dyn_rows.iter().find(|r| int(r, "dynamicsType") == Some(0)) {
                if let Some(v) = int(d1, "enabled") {
                    p.flag("dyn.on", "Dynamics", v != 0);
                }
                let multi = int(d1, "mode") == Some(1);
                let mode = if multi { "multiband" } else { "compressor" };
                p.push_unconfirmed("dyn.mode", "Dynamics mode", Value::Choice(mode.into()));
                let used: &[(i64, &str, &str)] = if multi {
                    &[
                        (0, "b1", "band 1 "),
                        (1, "b2", "band 2 "),
                        (2, "b3", "band 3 "),
                    ]
                } else {
                    &[(0, "comp", "")]
                };
                for &(n, key, label) in used {
                    if let Some(r) = processor(n) {
                        let k = |f: &str| format!("dyn.{key}.{f}");
                        let l = |f: &str| format!("Dyn {label}{f}");
                        for (col, f, lab, unit) in [
                            ("threashold", "thresh", "threshold", Unit::Db),
                            ("ratio", "ratio", "ratio", Unit::Ratio),
                            ("attack", "attack", "attack", Unit::Seconds),
                            ("release", "release", "release", Unit::Seconds),
                            ("gain", "gain", "gain", Unit::Db),
                        ] {
                            if let Some(v) = float(r, col) {
                                p.num(&k(f), &l(lab), v, unit);
                            }
                        }
                    }
                }
            }
            if let Some(d2) = dyn_rows.iter().find(|r| int(r, "dynamicsType") == Some(1)) {
                if let Some(v) = int(d2, "enabled") {
                    p.flag("gate.on", "Gate", v != 0);
                }
                if let Some(r) = processor(3) {
                    for (col, f, lab, unit) in [
                        ("threashold", "thresh", "Gate threshold", Unit::Db),
                        ("attack", "attack", "Gate attack", Unit::Seconds),
                        ("hold", "hold", "Gate hold", Unit::Seconds),
                        ("release", "release", "Gate release", Unit::Seconds),
                        ("range", "range", "Gate range", Unit::Db),
                    ] {
                        if let Some(v) = float(r, col) {
                            p.num(&format!("gate.{f}"), lab, v, unit);
                        }
                    }
                }
            }
            if let Some(v) = float(ch, "gain") {
                p.num("fader.level", "Fader", v, Unit::Db);
            }
            if let Some(v) = int(ch, "mute") {
                p.flag("fader.mute", "Mute", v != 0);
            }
            if let Some(v) = pans
                .get(&key)
                .and_then(|v| v.iter().find(|r| int(r, "panType") == Some(0)))
                .and_then(|r| float(r, "balance"))
            {
                p.num("fader.pan", "Pan", v, Unit::Pan);
            }
            let mut send_rows: Vec<&Row> = sends.get(&key).unwrap_or(&empty).iter().collect();
            send_rows.sort_by_key(|r| int(r, "busId"));
            for s in send_rows {
                let Some(bus) = int(s, "busId") else { continue };
                let name = ctx.bus_name(bus);
                let group = ctx.bus_mode.get(&(bus + 1)) == Some(&1);
                if group {
                    if int(s, "enabledInGroupMode") == Some(1) {
                        p.flag(&format!("group.{bus}"), &format!("Routed to {name}"), true);
                    }
                    continue;
                }
                let on = int(s, "enabledInAuxMode") == Some(1);
                let level = float(s, "gainInAuxMode").unwrap_or(-150.0);
                if !on && level <= -149.0 {
                    continue;
                }
                p.flag(&format!("send.{bus}.on"), &format!("Send {name}"), on);
                p.num(
                    &format!("send.{bus}.level"),
                    &format!("Send {name} level"),
                    level,
                    Unit::Db,
                );
            }
            for i in inserts.get(&key).unwrap_or(&empty) {
                let slot = match int(i, "insertType") {
                    Some(0) => "a",
                    Some(1) => "b",
                    _ => continue,
                };
                if int(i, "enabled") == Some(1) {
                    let up = slot.to_uppercase();
                    p.flag(&format!("insert.{slot}.on"), &format!("Insert {up}"), true);
                }
            }
            out.push(StripState { strip, params: p.0 });
        }
        Ok(out)
    }
}

fn patch(
    p: &mut ParamList,
    num: i64,
    ctx: &Ctx,
    receivers: &HashMap<i64, i64>,
    senders: &HashMap<i64, Row>,
) {
    let Some(source) = ctx.patch.get(&num).and_then(|r| receivers.get(r)).copied() else {
        return;
    };
    if source == 0 {
        p.push("input.source_a", "Source", Value::Text("unpatched".into()));
        return;
    }
    let label = ctx
        .sockets
        .get(&source)
        .cloned()
        .unwrap_or_else(|| format!("socket {source}"));
    p.push_unconfirmed("input.source_a", "Source", Value::Text(label));
    if let Some(s) = senders.get(&source) {
        if let Some(v) = float(s, "gain") {
            p.num("input.preamp", "Preamp gain", v, Unit::Db);
        }
        if let Some(v) = int(s, "phantomPower") {
            p.flag("input.phantom", "48V", v != 0);
        }
        if let Some(v) = int(s, "padEnabled") {
            p.flag("input.pad", "Pad", v != 0);
        }
    }
}

fn int(r: &Row, col: &str) -> Option<i64> {
    match r.get(col)? {
        Sql::Integer(i) => Some(*i),
        Sql::Real(f) => Some(*f as i64),
        _ => None,
    }
}

fn float(r: &Row, col: &str) -> Option<f64> {
    match r.get(col)? {
        Sql::Real(f) => Some(*f),
        Sql::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

fn text(r: &Row, col: &str) -> Option<String> {
    match r.get(col)? {
        Sql::Text(s) => Some(s.clone()),
        _ => None,
    }
}
