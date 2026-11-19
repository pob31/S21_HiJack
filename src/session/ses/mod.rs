//! SD-range and Quantum `.ses` reader.

mod container;
pub mod params;
mod records;

use std::collections::HashMap;

use container::Container;
use params::{Ctx, Resolve};
use records::{Obj, StripRec};

use super::SessionError;
use super::doc::{Format, SessionDoc, SnapshotDoc, Source, Strip, StripKind, StripState};

/// Parses an SD/Quantum session held in memory.
pub fn parse(data: &[u8], file_name: &str, include_raw: bool) -> Result<SessionDoc, SessionError> {
    let c = Container::parse(data)?;
    let new_layout = c.header.new_layout;
    let quantum = c.header.model == "Q2";

    let (db, db_end) = records::parse_object_db(&c)?;
    let (live_start, _) = c.section(0xd3)?;
    let (live, _) = records::parse_strip_list(c.bytes, live_start)?;
    let (groups, snaps) = records::parse_snapshots(&c, db_end)?;

    let mut index = StripIndex::default();
    for rec in &live {
        index.add(rec, new_layout);
    }
    for snap in &snaps {
        for rec in &snap.strips {
            index.add(rec, new_layout);
        }
    }

    let decode = |recs: &[StripRec]| {
        let mut out = Vec::new();
        for rec in recs {
            let Some(&strip) = index.by_code.get(&(rec.typ, rec.idx)) else {
                continue;
            };
            let s = &index.strips[strip];
            if !s.kind.is_mixing() {
                continue;
            }
            let ctx = Ctx {
                kind: s.kind,
                name: &s.name,
                quantum,
                resolve: &index,
            };
            let objs: Vec<&Obj> = rec.objects(&db);
            let mut params = params::decode_strip(&ctx, &objs, include_raw);
            if !new_layout {
                // Parameter IDs were confirmed on SD12/Quantum files only.
                for p in &mut params {
                    p.confirmed = false;
                }
            }
            out.push(StripState { strip, params });
        }
        out
    };

    let live_states = decode(&live);
    let snapshots = snaps
        .iter()
        .map(|s| SnapshotDoc {
            number: s.number,
            name: s.name.clone(),
            group: (s.group > 0).then_some(s.group as usize),
            crossfade_s: Some(s.crossfade_s),
            notes: s.notes.clone(),
            unsaved: false,
            states: decode(&s.strips),
        })
        .collect();

    let mut warnings = vec![
        "Per-snapshot recall scope and recall safes are not decoded yet.".to_string(),
        "Input sources (patch) are decoded from a field that is not yet confirmed.".to_string(),
    ];
    if !new_layout {
        warnings.push(
            "Older file layout (v9/vA): parameter meanings were confirmed on SD12 and Quantum \
             files only, so every value is marked unconfirmed."
                .to_string(),
        );
    }
    if quantum {
        warnings.push("Nodal processing bypass is not found in the session file.".to_string());
    }

    Ok(SessionDoc {
        source: Source {
            file_name: file_name.to_string(),
            format: Format::SdSes {
                model: c.header.model.clone(),
                version: c.header.version,
                build: c.header.build,
            },
        },
        show_name: c.show_name.clone(),
        sample_rate: c.header.sample_rate,
        strips: index.strips,
        live: live_states,
        snapshots,
        snapshot_groups: groups,
        warnings,
    })
}

/// Strips by raw (type code, index), plus name resolution for references.
#[derive(Default)]
struct StripIndex {
    strips: Vec<Strip>,
    by_code: HashMap<(u8, u8), usize>,
}

impl StripIndex {
    fn add(&mut self, rec: &StripRec, new_layout: bool) {
        if self.by_code.contains_key(&(rec.typ, rec.idx)) {
            return;
        }
        self.by_code.insert((rec.typ, rec.idx), self.strips.len());
        self.strips.push(Strip {
            kind: params::strip_kind(rec.typ, new_layout),
            index: rec.idx as u16,
            name: rec.name.clone(),
            stereo: rec.width == 2,
        });
    }

    fn title(&self, strip: &Strip) -> String {
        match strip.kind {
            StripKind::SpiceRack => format!("Spice Rack {} ({})", strip.index + 1, strip.name),
            _ => strip.title(),
        }
    }
}

impl Resolve for StripIndex {
    fn strip_ref(&self, raw: u32) -> Option<String> {
        if raw == 0 {
            return None;
        }
        let typ = (raw >> 24) as u8;
        let idx = (raw >> 16) as u8;
        let mut name = match self.by_code.get(&(typ, idx)) {
            Some(&i) => self.title(&self.strips[i]),
            None => format!("strip {typ:#04x}/{idx}"),
        };
        // Back-links into an insert carry the insert object type (0x08) in the
        // low byte and the insert (0x00 = A, 0x10 = B) in the byte above.
        if raw & 0xff == 0x08 {
            let insert = if (raw >> 8) & 0xff == 0x10 { "B" } else { "A" };
            name.push_str(&format!(", insert {insert}"));
        }
        Some(name)
    }

    fn bus(&self, kind: StripKind, slot: u8) -> String {
        self.strips
            .iter()
            .find(|s| s.kind == kind && s.index == slot as u16)
            .map(|s| s.title())
            .unwrap_or_else(|| format!("{} slot {}", kind.label(), slot + 1))
    }
}
