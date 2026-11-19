//! SD/Quantum records: the parameter-object database, strip lists, snapshots.
//!
//! Layout notes: `Documentation/SESSION_FILES_FIELD_NOTES.md` §2.3–2.5.

use std::collections::HashMap;

use super::container::{Bytes, Container};
use crate::session::SessionError;

/// One parameter object (`da 00 3a 00` record).
#[derive(Debug, Clone)]
pub struct Obj {
    pub typ: u32,
    pub params: HashMap<(u16, u8), [u8; 4]>,
}

impl Obj {
    pub fn f32(&self, pid: u16, idx: u8) -> Option<f32> {
        self.params.get(&(pid, idx)).map(|b| f32::from_le_bytes(*b))
    }

    pub fn u32(&self, pid: u16, idx: u8) -> Option<u32> {
        self.params.get(&(pid, idx)).map(|b| u32::from_le_bytes(*b))
    }
}

const OBJ_FIXED: usize = 62;
const OBJ_TRAILER: usize = 12;

fn parse_object(b: Bytes, at: usize) -> Result<(Obj, usize), SessionError> {
    if b.u16(at)? != 0xda {
        return Err(SessionError::format(format!("object expected at {at:#x}")));
    }
    let typ = b.u32(at + 4)?;
    let count = b.u32(at + 48)? as usize;
    if count > 4096 {
        return Err(SessionError::format(format!(
            "implausible object at {at:#x}"
        )));
    }
    let mut params = HashMap::with_capacity(count);
    for k in 0..count {
        let p = at + OBJ_FIXED + 8 * k;
        let pid = b.u16(p)?;
        let idx = b.u8(p + 2)?;
        let raw = b.slice(p + 4, 4)?;
        params.insert((pid, idx), [raw[0], raw[1], raw[2], raw[3]]);
    }
    Ok((
        Obj { typ, params },
        at + OBJ_FIXED + 8 * count + OBJ_TRAILER,
    ))
}

/// The shared object database (section `d2`). Returns it and its end offset.
pub fn parse_object_db(c: &Container) -> Result<(Vec<Obj>, usize), SessionError> {
    let (start, _) = c.section(0xd2)?;
    let b = c.bytes;
    let count = b.u32(start + 4)? as usize;
    let mut at = start + 8;
    let mut objs = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        let (obj, next) = parse_object(b, at)?;
        objs.push(obj);
        at = next;
    }
    Ok((objs, at))
}

/// Where a strip's objects live.
#[derive(Debug, Clone)]
pub enum ObjSource {
    Inline(Vec<Obj>),
    /// Indexes into the object database.
    Refs(Vec<u32>),
}

#[derive(Debug, Clone)]
pub struct StripRec {
    /// Raw strip-type code (layout-dependent; see `params::strip_kind`).
    pub typ: u8,
    pub idx: u8,
    pub name: String,
    pub width: u8,
    pub objs: ObjSource,
}

impl StripRec {
    /// Resolves this strip's objects against the database.
    pub fn objects<'a>(&'a self, db: &'a [Obj]) -> Vec<&'a Obj> {
        match &self.objs {
            ObjSource::Inline(v) => v.iter().collect(),
            ObjSource::Refs(r) => r.iter().filter_map(|&i| db.get(i as usize)).collect(),
        }
    }
}

const STRIP_TAIL: usize = 41;

/// A strip list (`d3 00` + count + strips). Returns the strips and the end offset.
pub fn parse_strip_list(b: Bytes, at: usize) -> Result<(Vec<StripRec>, usize), SessionError> {
    if b.u16(at)? != 0xd3 {
        return Err(SessionError::format(format!(
            "strip list expected at {at:#x}"
        )));
    }
    let count = b.u32(at + 2)? as usize;
    let mut p = at + 6;
    let mut strips = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        if b.slice(p, 4)? != [0xd5, 0x00, 0x01, 0x00] {
            return Err(SessionError::format(format!("strip expected at {p:#x}")));
        }
        let inline = b.u8(p + 4)? != 0;
        let n = b.u32(p + 5)? as usize;
        p += 9;
        let objs = if inline {
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                let (obj, next) = parse_object(b, p)?;
                v.push(obj);
                p = next;
            }
            ObjSource::Inline(v)
        } else {
            let mut v = Vec::with_capacity(n);
            for k in 0..n {
                v.push(b.u32(p + 4 * k)?);
            }
            p += 4 * n;
            ObjSource::Refs(v)
        };
        if b.u16(p)? != 0x27 {
            return Err(SessionError::format(format!(
                "strip tail expected at {p:#x}"
            )));
        }
        strips.push(StripRec {
            idx: b.u8(p + 2)?,
            typ: b.u8(p + 3)?,
            name: b.short_name(p + 4)?,
            width: b.u8(p + 40)?,
            objs,
        });
        p += STRIP_TAIL;
    }
    Ok((strips, p))
}

#[derive(Debug, Clone)]
pub struct SnapshotRec {
    pub name: String,
    pub number: f64,
    pub crossfade_s: f64,
    pub group: u8,
    pub notes: String,
    pub strips: Vec<StripRec>,
}

/// Snapshot groups and snapshots, which follow the object database.
pub fn parse_snapshots(
    c: &Container,
    db_end: usize,
) -> Result<(Vec<String>, Vec<SnapshotRec>), SessionError> {
    let b = c.bytes;
    let mut p = db_end;
    if b.u32(p)? != 0 {
        return Err(SessionError::format(format!(
            "snapshot list expected at {p:#x}"
        )));
    }
    p += 4;
    let groups_present = c.has_snapshot_groups();
    let mut groups = Vec::new();
    if groups_present {
        let n = b.u32(p)? as usize;
        p += 4;
        for _ in 0..n {
            let len = b.u16(p)? as usize;
            groups.push(b.short_name(p + 2)?);
            p += 2 + len;
        }
    }
    let count = b.u32(p)? as usize;
    p += 4;
    let mut snaps = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        let (strips, next) = parse_strip_list(b, p)?;
        p = next;
        let (name, next) = b.string16(p)?;
        p = next + 8; // f64 save date
        p += 2 + b.u16(p)? as usize; // recall-scope blob (not decoded)
        if b.u16(p)? != 2 {
            return Err(SessionError::format(format!(
                "MIDI block expected at {p:#x}"
            )));
        }
        let midi_len = b.u32(p + 4)? as usize;
        p += 8 + midi_len + 10;
        p += 2 + b.u16(p)? as usize; // second blob (not decoded)
        let props_len = b.u16(p)? as usize;
        let props = p + 2;
        let number = b.u32(props)? as f64 / 100.0;
        let crossfade_s = b.u16(props + 4)? as f64 / 100.0;
        let group = b.u8(props + 6)?;
        p = props + props_len;
        let (notes, next) = b.string16(p)?;
        p = next;
        if groups_present {
            p += 2 + b.u16(p)? as usize;
        }
        snaps.push(SnapshotRec {
            name,
            number,
            crossfade_s,
            group,
            notes: notes.replace("\r\n", "\n"),
            strips,
        });
    }
    Ok((groups, snaps))
}
