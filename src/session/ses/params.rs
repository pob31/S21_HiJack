//! The confirmed SD/Quantum parameter table: object fields → named [`Param`]s.
//!
//! This is the only place that knows parameter IDs, scaling and type codes.
//! Every field decoded as confirmed here is tagged **[E]** in
//! `Documentation/SESSION_FILES_FIELD_NOTES.md` (checked against live OSC
//! values or a controlled save + screenshot). Fields tagged **[H]** there are
//! emitted with `confirmed: false`. Anything else stays out of the named
//! output and is only kept raw when asked for.

use std::collections::HashSet;

use super::records::Obj;
use crate::session::doc::{Param, ParamList, StripKind, Unit, Value};

/// Maps a raw strip-type code to a [`StripKind`]. The numbering changed
/// between the v9/vA layout and the vB+ layout.
pub fn strip_kind(code: u8, new_layout: bool) -> StripKind {
    if new_layout {
        match code {
            0x01 => StripKind::Input,
            0x05 => StripKind::Aux,
            0x06 => StripKind::Group,
            0x07 => StripKind::BusRight,
            0x08 => StripKind::Monitoring,
            0x0a => StripKind::ControlGroup,
            0x0b => StripKind::Solo,
            0x0c => StripKind::Talkback,
            0x0e => StripKind::Fx,
            0x0f => StripKind::MatrixInput,
            0x10 => StripKind::Matrix,
            0x11 => StripKind::GraphicEq,
            0x14 => StripKind::Node,
            0x18 => StripKind::SpiceRack,
            0x1b => StripKind::Generator,
            0x1c..=0x4c => StripKind::InputSocket,
            0x4e..=0x7f => StripKind::OutputSocket,
            _ => StripKind::Other,
        }
    } else {
        match code {
            0x01 => StripKind::Input,
            0x02 => StripKind::Talkback,
            0x03 => StripKind::Aux,
            0x04 => StripKind::Group,
            0x05 => StripKind::BusRight,
            0x06 => StripKind::Monitoring,
            0x08 => StripKind::ControlGroup,
            0x09 => StripKind::Solo,
            0x0c => StripKind::Fx,
            0x0d => StripKind::MatrixInput,
            0x0e => StripKind::Matrix,
            0x0f => StripKind::GraphicEq,
            0x28 => StripKind::Generator,
            0x29..=0x4f => StripKind::InputSocket,
            0x51..=0x7f => StripKind::OutputSocket,
            _ => StripKind::Other,
        }
    }
}

/// Resolves strip references (`type << 24 | index << 16 | …`) to names.
pub trait Resolve {
    /// Name of the strip a reference points at, `None` for "nothing".
    fn strip_ref(&self, raw: u32) -> Option<String>;
    /// Name of the bus at storage slot `k` of the given kind.
    fn bus(&self, kind: StripKind, slot: u8) -> String;
}

/// Context for decoding one strip.
pub struct Ctx<'a> {
    pub kind: StripKind,
    pub name: &'a str,
    pub quantum: bool,
    pub resolve: &'a dyn Resolve,
}

/// Records which fields were consumed, so the rest can be emitted raw.
struct Reader<'a> {
    objs: &'a [&'a Obj],
    used: HashSet<(u32, u16, u8)>,
}

impl<'a> Reader<'a> {
    fn obj(&self, typ: u32) -> Option<&'a Obj> {
        self.objs.iter().copied().find(|o| o.typ == typ)
    }

    fn f(&mut self, typ: u32, pid: u16, idx: u8) -> Option<f64> {
        let v = self.obj(typ)?.f32(pid, idx)?;
        self.used.insert((typ, pid, idx));
        Some(v as f64)
    }

    fn u(&mut self, typ: u32, pid: u16, idx: u8) -> Option<u32> {
        let v = self.obj(typ)?.u32(pid, idx)?;
        self.used.insert((typ, pid, idx));
        Some(v)
    }

    fn on(&mut self, typ: u32, pid: u16, idx: u8) -> Option<bool> {
        self.f(typ, pid, idx).map(|v| v >= 0.5)
    }
}

/// Stored filter frequencies are scaled: displayed = stored × factor.
const LOW_CUT_SCALE: f64 = 1.25;
const HIGH_CUT_SCALE: f64 = 0.8;

/// Signal-chain point scale shared by aux send points and the Mustard insert
/// position.
pub fn chain_point(v: f64) -> (&'static str, bool) {
    match v.round() as i64 {
        0 => ("post-fader", true),
        1 => ("pre-fader", true),
        2 => ("pre-mute", true),
        3 => ("mid-EQ/dyn", true),
        4 => ("pre-EQ/dyn", true),
        5 => ("pre-proc", true),
        _ => ("unknown point", false),
    }
}

/// Decodes one strip's objects into named parameters.
pub fn decode_strip(ctx: &Ctx, objs: &[&Obj], include_raw: bool) -> Vec<Param> {
    let mut r = Reader {
        objs,
        used: HashSet::new(),
    };
    let mut out = ParamList::default();
    match ctx.kind {
        StripKind::SpiceRack => spice_rack(ctx, &mut r, &mut out),
        StripKind::Node => {
            eq(&mut r, &mut out, 0x04);
            dynamics(&mut r, &mut out, 0x05);
        }
        _ => {
            input(ctx, &mut r, &mut out);
            delay_filters(&mut r, &mut out);
            eq(&mut r, &mut out, 0x04);
            dynamics(&mut r, &mut out, 0x05);
            fader(&mut r, &mut out);
            sends(ctx, &mut r, &mut out);
            groups(ctx, &mut r, &mut out);
            inserts(ctx, &mut r, &mut out);
            if ctx.quantum {
                mustard(ctx, &mut r, &mut out);
            }
        }
    }
    if include_raw {
        for obj in objs {
            let mut keys: Vec<_> = obj.params.keys().copied().collect();
            keys.sort_unstable();
            for (pid, idx) in keys {
                if r.used.contains(&(obj.typ, pid, idx)) {
                    continue;
                }
                let raw = u32::from_le_bytes(obj.params[&(pid, idx)]);
                let as_f = f32::from_bits(raw);
                let shown = if as_f.is_finite() && (as_f == 0.0 || as_f.abs() > 1e-6) {
                    format!("{as_f} ({raw:#010x})")
                } else {
                    format!("{raw:#010x}")
                };
                out.push_unconfirmed(
                    format!("raw.{:02x}.{pid:04x}.{idx}", obj.typ),
                    format!("unconfirmed {pid:04x}[{idx}]"),
                    Value::Raw(shown),
                );
            }
        }
    }
    out.0
}

fn input(ctx: &Ctx, r: &mut Reader, out: &mut ParamList) {
    const T: u32 = 0x01;
    if let Some(v) = r.f(T, 0x0101, 0) {
        out.num("input.trim", "Trim", v, Unit::Db);
    }
    if let Some(v) = r.f(T, 0x0102, 0) {
        let s = match v.round() as i64 {
            0 => "normal".to_string(),
            n => format!("inverted ({n})"),
        };
        out.push("input.phase", "Phase", Value::Choice(s));
    }
    for (idx, key, label) in [
        (0, "input.source_a", "Source A"),
        (1, "input.source_b", "Source B"),
    ] {
        if let Some(name) = r.u(T, 0x0100, idx).and_then(|v| ctx.resolve.strip_ref(v)) {
            out.push_unconfirmed(key, label, Value::Ref(name));
        }
    }
    if ctx.quantum {
        if let Some(on) = r.on(T, 0x01ff, 0) {
            out.flag("input.nodal", "Nodal processing", on);
        }
    }
}

fn delay_filters(r: &mut Reader, out: &mut ParamList) {
    if let Some(on) = r.on(0x09, 0x0907, 0) {
        out.flag("delay.on", "Delay", on);
    }
    if let Some(v) = r.f(0x09, 0x0908, 0) {
        out.num("delay.time", "Delay time", v, Unit::Seconds);
    }
    filters(r, out, 0x07, 0x0703, 0x0705, "filter", "");
}

/// Channel-style HPF/LPF pair: `in[1]`/`freq[1]` = low cut, `[0]` = high cut.
fn filters(
    r: &mut Reader,
    out: &mut ParamList,
    t: u32,
    on: u16,
    freq: u16,
    key: &str,
    label: &str,
) {
    if let Some(v) = r.on(t, on, 1) {
        out.flag(&format!("{key}.hpf_on"), &format!("{label}HPF"), v);
    }
    if let Some(v) = r.f(t, freq, 1) {
        out.num(
            &format!("{key}.hpf"),
            &format!("{label}HPF freq"),
            v * LOW_CUT_SCALE,
            Unit::Hz,
        );
    }
    if let Some(v) = r.on(t, on, 0) {
        out.flag(&format!("{key}.lpf_on"), &format!("{label}LPF"), v);
    }
    if let Some(v) = r.f(t, freq, 0) {
        out.num(
            &format!("{key}.lpf"),
            &format!("{label}LPF freq"),
            v * HIGH_CUT_SCALE,
            Unit::Hz,
        );
    }
}

fn eq_curve(v: f64) -> String {
    match v.round() as i64 {
        1 => "bell".to_string(),
        4 => "shelf".to_string(),
        n => format!("curve {n}"),
    }
}

/// SD EQ object (`0x04`): band index 0 is the top band (console "band 1").
fn eq(r: &mut Reader, out: &mut ParamList, t: u32) {
    if r.obj(t).is_none() {
        return;
    }
    if let Some(on) = r.on(t, 0x0402, 0) {
        out.flag("eq.on", "EQ", on);
    }
    for idx in 0u8..8 {
        let Some(freq) = r.f(t, 0x0405, idx) else {
            continue;
        };
        let band = idx + 1;
        let k = |f: &str| format!("eq.band{band}.{f}");
        let l = |f: &str| format!("EQ {band} {f}");
        if let Some(on) = r.on(t, 0x0403, idx) {
            out.flag(&k("on"), &l("on"), on);
        }
        out.num(&k("freq"), &l("freq"), freq, Unit::Hz);
        if let Some(v) = r.f(t, 0x0407, idx) {
            out.num(&k("gain"), &l("gain"), v, Unit::Db);
        }
        if let Some(v) = r.f(t, 0x0406, idx) {
            out.num(&k("q"), &l("Q"), v, Unit::Plain);
        }
        if let Some(v) = r.f(t, 0x0404, idx) {
            out.push(k("curve"), l("curve"), Value::Choice(eq_curve(v)));
        }
        if let Some(v) = r.on(t, 0x047c, idx) {
            let mode = if v { "classic" } else { "precision" };
            out.push(k("mode"), l("mode"), Value::Choice(mode.into()));
        }
        let dynamic = r.on(t, 0x047d, idx).unwrap_or(false);
        out.flag(&k("dyn"), &l("dynamic"), dynamic);
        if dynamic {
            if let Some(v) = r.f(t, 0x040e, idx) {
                out.num(&k("dyn_thresh"), &l("dyn threshold"), v, Unit::Db);
            }
            if let Some(v) = r.f(t, 0x0410, idx) {
                out.num(&k("dyn_ratio"), &l("dyn ratio"), v, Unit::Ratio);
            }
            if let Some(v) = r.f(t, 0x0411, idx) {
                out.num(&k("dyn_attack"), &l("dyn attack"), v, Unit::Seconds);
            }
            if let Some(v) = r.f(t, 0x0412, idx) {
                out.num(&k("dyn_release"), &l("dyn release"), v, Unit::Seconds);
            }
            if let Some(v) = r.on(t, 0x040b, idx) {
                let s = if v { "under" } else { "over" };
                out.push(k("dyn_mode"), l("dyn over/under"), Value::Choice(s.into()));
            }
        }
    }
}

/// SD dynamics object (`0x05`): compressor/multiband/de-esser bands 0–2 and
/// the gate/duck/comp section at index 3.
fn dynamics(r: &mut Reader, out: &mut ParamList, t: u32) {
    if r.obj(t).is_none() {
        return;
    }
    let mode = r.f(t, 0x053a, 0).map(|v| v.round() as i64).unwrap_or(0);
    let (mode_name, bands): (&str, &[(u8, &str, &str)]) = match mode {
        1 => (
            "multiband",
            &[(0, "lo", "low "), (2, "mid", "mid "), (1, "hi", "high ")],
        ),
        2 => ("de-esser", &[(2, "ess", "")]),
        _ => ("compressor", &[(0, "comp", "")]),
    };
    out.push("dyn.mode", "Dynamics mode", Value::Choice(mode_name.into()));
    if let Some(on) = r.on(t, 0x0502, 0) {
        out.flag("dyn.on", "Dynamics", on);
    }
    for &(idx, key, label) in bands {
        let k = |f: &str| format!("dyn.{key}.{f}");
        let l = |f: &str| format!("Dyn {label}{f}");
        if mode == 1 {
            if let Some(on) = r.on(t, 0x0503, idx) {
                out.flag(&k("on"), &l("band on"), on);
            }
        }
        if let Some(v) = r.f(t, 0x050e, idx) {
            out.num(&k("thresh"), &l("threshold"), v, Unit::Db);
        }
        if let Some(v) = r.f(t, 0x0510, idx) {
            out.num(&k("ratio"), &l("ratio"), v, Unit::Ratio);
        }
        if let Some(v) = r.f(t, 0x0511, idx) {
            out.num(&k("attack"), &l("attack"), v, Unit::Seconds);
        }
        if let Some(v) = r.f(t, 0x0512, idx) {
            out.num(&k("release"), &l("release"), v, Unit::Seconds);
        }
        if let Some(v) = r.f(t, 0x0509, idx) {
            out.num(&k("gain"), &l("gain"), v, Unit::Db);
        }
        if let Some(v) = r.f(t, 0x050f, idx) {
            let (s, confirmed) = match v.round() as i64 {
                0 => ("hard", true),
                1 => ("medium", true),
                2 => ("soft", false),
                _ => ("unknown", false),
            };
            let p = Value::Choice(s.into());
            if confirmed {
                out.push(k("knee"), l("knee"), p);
            } else {
                out.push_unconfirmed(k("knee"), l("knee"), p);
            }
        }
        if let Some(on) = r.on(t, 0x053b, idx) {
            out.flag(&k("auto_gain"), &l("auto gain"), on);
        }
    }
    match mode {
        1 => {
            if let Some(v) = r.f(t, 0x0505, 0) {
                out.num("dyn.xover_lo", "Dyn crossover low", v, Unit::Hz);
            }
            if let Some(v) = r.f(t, 0x0505, 1) {
                out.num("dyn.xover_hi", "Dyn crossover high", v, Unit::Hz);
            }
        }
        2 => {
            if let Some(v) = r.f(t, 0x0505, 0) {
                out.num("dyn.ess.hp", "De-esser band low", v, Unit::Hz);
            }
            if let Some(v) = r.f(t, 0x0505, 1) {
                out.num("dyn.ess.lp", "De-esser band high", v, Unit::Hz);
            }
        }
        _ => {}
    }

    // Gate / duck / compressor section.
    let gmode = r.f(t, 0x050b, 3).map(|v| v.round() as i64);
    if let Some(m) = gmode {
        let s = match m {
            0 => "gate",
            1 => "duck",
            2 => "compressor",
            _ => "unknown",
        };
        out.push("gate.mode", "Gate mode", Value::Choice(s.into()));
    }
    if let Some(on) = r.on(t, 0x0502, 3) {
        out.flag("gate.on", "Gate", on);
    }
    if let Some(v) = r.f(t, 0x050e, 3) {
        out.num("gate.thresh", "Gate threshold", v, Unit::Db);
    }
    if let Some(v) = r.f(t, 0x0511, 3) {
        out.num("gate.attack", "Gate attack", v, Unit::Seconds);
    }
    if let Some(v) = r.f(t, 0x0513, 3) {
        out.num("gate.hold", "Gate hold", v, Unit::Seconds);
    }
    if let Some(v) = r.f(t, 0x0512, 3) {
        out.num("gate.release", "Gate release", v, Unit::Seconds);
    }
    if let Some(v) = r.f(t, 0x050a, 3) {
        out.num("gate.range", "Gate range", v, Unit::Db);
    }
    if gmode == Some(2) {
        if let Some(v) = r.f(t, 0x0510, 3) {
            out.num("gate.ratio", "Gate ratio", v, Unit::Ratio);
        }
    }
    if let Some(v) = r.f(t, 0x0505, 3) {
        out.num("gate.key_hp", "Gate key HPF", v, Unit::Hz);
    }
    if let Some(v) = r.f(t, 0x0505, 2) {
        out.num("gate.key_lp", "Gate key LPF", v, Unit::Hz);
    }
}

fn fader(r: &mut Reader, out: &mut ParamList) {
    if let Some(v) = r.f(0x0c, 0x0c21, 0) {
        out.num("fader.level", "Fader", v, Unit::Db);
    }
    if let Some(on) = r.on(0x0c, 0x0c22, 0) {
        out.flag("fader.mute", "Mute", on);
    }
    if let Some(on) = r.on(0x0c, 0x0c02, 0) {
        out.flag("fader.hard_mute", "Hard mute", on);
    }
    if let Some(v) = r.f(0x0d, 0x0d23, 0) {
        out.num("fader.pan", "Pan", v, Unit::Pan);
    }
}

/// Aux sends (`0x02`): slot `k` is the aux stored at position `k`.
fn sends(ctx: &Ctx, r: &mut Reader, out: &mut ParamList) {
    let Some(obj) = r.obj(0x02) else {
        return;
    };
    let mut slots: Vec<u8> = obj
        .params
        .keys()
        .filter(|(pid, _)| *pid == 0x0201)
        .map(|&(_, i)| i)
        .collect();
    slots.sort_unstable();
    for k in slots {
        let level = r.f(0x02, 0x0201, k).unwrap_or(-150.0);
        let on = r.on(0x02, 0x0202, k).unwrap_or(false);
        let pan = r.f(0x02, 0x0203, k);
        let point = r.f(0x02, 0x0204, k);
        if !on && level <= -149.0 {
            continue;
        }
        let bus = ctx.resolve.bus(StripKind::Aux, k);
        let key = |f: &str| format!("send.{k}.{f}");
        out.flag(&key("on"), &format!("Send {bus}"), on);
        out.num(&key("level"), &format!("Send {bus} level"), level, Unit::Db);
        if let Some(p) = pan {
            out.num(&key("pan"), &format!("Send {bus} pan"), p, Unit::Pan);
        }
        if let Some(p) = point {
            let (s, ok) = chain_point(p);
            let v = Value::Choice(s.into());
            if ok {
                out.push(key("point"), format!("Send {bus} point"), v);
            } else {
                out.push_unconfirmed(key("point"), format!("Send {bus} point"), v);
            }
        }
    }
}

/// Group routing (`0x03`): slot `k` is the group stored at position `k`.
fn groups(ctx: &Ctx, r: &mut Reader, out: &mut ParamList) {
    let Some(obj) = r.obj(0x03) else {
        return;
    };
    let mut slots: Vec<u8> = obj
        .params
        .keys()
        .filter(|(pid, _)| *pid == 0x0309)
        .map(|&(_, i)| i)
        .collect();
    slots.sort_unstable();
    for k in slots {
        if r.on(0x03, 0x0309, k) == Some(true) {
            let bus = ctx.resolve.bus(StripKind::Group, k);
            out.flag(&format!("group.{k}"), &format!("Routed to {bus}"), true);
        }
    }
}

fn inserts(ctx: &Ctx, r: &mut Reader, out: &mut ParamList) {
    for (idx, slot) in [(0u8, "a"), (1u8, "b")] {
        let on = r.on(0x08, 0x0802, idx).unwrap_or(false);
        let send = r
            .u(0x08, 0x08fe, idx)
            .and_then(|v| ctx.resolve.strip_ref(v));
        let ret = r
            .u(0x08, 0x0800, idx)
            .and_then(|v| ctx.resolve.strip_ref(v));
        if !on && send.is_none() && ret.is_none() {
            continue;
        }
        let up = slot.to_uppercase();
        out.flag(&format!("insert.{slot}.on"), &format!("Insert {up}"), on);
        if let Some(s) = send {
            out.push(
                format!("insert.{slot}.send"),
                format!("Insert {up} send"),
                Value::Ref(s),
            );
        }
        if let Some(s) = ret {
            out.push(
                format!("insert.{slot}.return"),
                format!("Insert {up} return"),
                Value::Ref(s),
            );
        }
    }
}

const TUBE_CHARACTER: [&str; 6] = [
    "odd harmonics",
    "even harmonics",
    "overdrive",
    "distortion",
    "crunch",
    "hi distortion",
];

/// Quantum Mustard processing (`0x1a`–`0x1e`). Only blocks that are on.
fn mustard(ctx: &Ctx, r: &mut Reader, out: &mut ParamList) {
    let tube = r.on(0x1a, 0x1a02, 0).unwrap_or(false);
    let eq_on = r.on(0x1c, 0x1c02, 0).unwrap_or(false);
    let d1 = r.on(0x1d, 0x1d02, 0).unwrap_or(false);
    let d2 = r.on(0x1e, 0x1e02, 0).unwrap_or(false);
    if !(tube || eq_on || d1 || d2) {
        return;
    }
    if let Some(v) = r.f(0x1a, 0x1ae1, 0) {
        let (s, ok) = chain_point(v);
        let p = Value::Choice(s.into());
        if ok && matches!(v.round() as i64, 0 | 3 | 5) {
            out.push("mustard.position", "Mustard position", p);
        } else {
            out.push_unconfirmed("mustard.position", "Mustard position", p);
        }
    }
    if tube {
        out.flag("mustard.tube.on", "Tube", true);
        if let Some(v) = r.f(0x1a, 0x1a1b, 0) {
            out.num("mustard.tube.drive", "Tube drive", v, Unit::Plain);
        }
        if let Some(v) = r.f(0x1a, 0x1a45, 0) {
            let s = TUBE_CHARACTER
                .get(v.round() as usize)
                .copied()
                .unwrap_or("unknown");
            out.push(
                "mustard.tube.character",
                "Tube character",
                Value::Choice(s.into()),
            );
        }
        if let Some(v) = r.f(0x1a, 0x1a3c, 0) {
            out.num("mustard.tube.output", "Tube output", v, Unit::Db);
        }
    }
    if eq_on {
        out.flag("mustard.eq.on", "Mustard EQ", true);
        filters(r, out, 0x1b, 0x1b03, 0x1b05, "mustard.eq", "Mustard EQ ");
        for idx in 0u8..4 {
            let Some(freq) = r.f(0x1c, 0x1c05, idx) else {
                continue;
            };
            let band = idx + 1;
            let k = |f: &str| format!("mustard.eq.band{band}.{f}");
            let l = |f: &str| format!("Mustard EQ {band} {f}");
            out.num(&k("freq"), &l("freq"), freq, Unit::Hz);
            if let Some(v) = r.f(0x1c, 0x1c07, idx) {
                out.num(&k("gain"), &l("gain"), v, Unit::Db);
            }
            if let Some(v) = r.f(0x1c, 0x1c06, idx) {
                out.num(&k("q"), &l("Q"), v, Unit::Plain);
            }
            if let Some(v) = r.f(0x1c, 0x1c04, idx) {
                out.push(k("curve"), l("curve"), Value::Choice(eq_curve(v)));
            }
        }
    }
    if d1 {
        mustard_dyn1(r, out);
    }
    if d2 {
        mustard_dyn2(ctx, r, out);
    }
}

fn mustard_dyn1(r: &mut Reader, out: &mut ParamList) {
    const T: u32 = 0x1d;
    let ty = r.f(T, 0x1d0b, 0).map(|v| v.round() as i64).unwrap_or(0);
    let name = match ty {
        0 => "Classic",
        3 => "Vintage VCA",
        4 => "FET Limiter",
        5 => "Optical",
        8 => "Levelling Amp",
        _ => "unknown type",
    };
    out.push(
        "mustard.dyn1.type",
        "Mustard Dyn 1",
        Value::Choice(name.into()),
    );
    if let Some(v) = r.f(T, 0x1d47, 0) {
        out.num(
            "mustard.dyn1.mix",
            "Mustard Dyn 1 mix",
            v * 100.0,
            Unit::Percent,
        );
    }
    let mut num = |r: &mut Reader, pid: u16, key: &str, label: &str, unit: Unit| {
        if let Some(v) = r.f(T, pid, 0) {
            out.num(
                &format!("mustard.dyn1.{key}"),
                &format!("Mustard Dyn 1 {label}"),
                v,
                unit,
            );
        }
    };
    match ty {
        0 | 3 => {
            num(r, 0x1d0e, "thresh", "threshold", Unit::Db);
            num(r, 0x1d10, "ratio", "ratio", Unit::Ratio);
            num(r, 0x1d11, "attack", "attack", Unit::Seconds);
            num(r, 0x1d12, "release", "release", Unit::Seconds);
            num(r, 0x1d09, "gain", "gain", Unit::Db);
        }
        4 => {
            num(r, 0x1d41, "input", "input", Unit::Db);
            num(r, 0x1d11, "attack", "attack", Unit::Seconds);
            num(r, 0x1d12, "release", "release", Unit::Seconds);
            num(r, 0x1d4d, "output", "output", Unit::Db);
        }
        5 => {
            num(r, 0x1d0e, "thresh", "threshold", Unit::Db);
            num(r, 0x1d10, "ratio", "ratio", Unit::Ratio);
            num(r, 0x1d09, "gain", "gain", Unit::Db);
        }
        8 => {
            num(r, 0x1d51, "gain", "gain", Unit::Plain);
            num(r, 0x1d52, "peak_reduction", "peak reduction", Unit::Plain);
        }
        _ => {}
    }
    if ty == 4 {
        if let Some(v) = r.f(T, 0x1d4a, 0) {
            let s = ["ALL", "4:1", "8:1", "12:1", "20:1"]
                .get(v.round() as usize)
                .copied()
                .unwrap_or("unknown");
            out.push(
                "mustard.dyn1.ratio",
                "Mustard Dyn 1 ratio",
                Value::Choice(s.into()),
            );
        }
    }
    if ty == 5 {
        if let Some(v) = r.f(T, 0x1d48, 0) {
            let s = speed(v);
            out.push(
                "mustard.dyn1.attack",
                "Mustard Dyn 1 attack",
                Value::Choice(s.into()),
            );
        }
        if let Some(v) = r.f(T, 0x1d49, 0) {
            let s = speed(v);
            out.push_unconfirmed(
                "mustard.dyn1.recovery",
                "Mustard Dyn 1 recovery",
                Value::Choice(s.into()),
            );
        }
    }
    if ty == 8 {
        if let Some(v) = r.on(T, 0x1d50, 0) {
            let s = if v { "limit" } else { "compress" };
            out.push(
                "mustard.dyn1.mode",
                "Mustard Dyn 1 mode",
                Value::Choice(s.into()),
            );
        }
    }
    side_chain_filter(r, out, T, 0x1d03, 0x1d05, "mustard.dyn1", "Mustard Dyn 1");
}

fn mustard_dyn2(ctx: &Ctx, r: &mut Reader, out: &mut ParamList) {
    const T: u32 = 0x1e;
    let ty = r.f(T, 0x1e0b, 0).map(|v| v.round() as i64).unwrap_or(0);
    let name = match ty {
        0 => "Gate",
        1 => "Duck",
        2 => "MSE",
        _ => "unknown type",
    };
    out.push(
        "mustard.dyn2.type",
        "Mustard Dyn 2",
        Value::Choice(name.into()),
    );
    let mut num = |r: &mut Reader, pid: u16, key: &str, label: &str, unit: Unit| {
        if let Some(v) = r.f(T, pid, 0) {
            out.num(
                &format!("mustard.dyn2.{key}"),
                &format!("Mustard Dyn 2 {label}"),
                v,
                unit,
            );
        }
    };
    num(r, 0x1e0e, "thresh", "threshold", Unit::Db);
    if ty == 2 {
        num(r, 0x1e0a, "depth", "depth", Unit::Db);
    } else {
        num(r, 0x1e11, "attack", "attack", Unit::Seconds);
        num(r, 0x1e13, "hold", "hold", Unit::Seconds);
        num(r, 0x1e12, "release", "release", Unit::Seconds);
        num(r, 0x1e0a, "range", "range", Unit::Db);
    }
    if ty == 2 {
        if let Some(v) = r.f(T, 0x1e4c, 0) {
            let s = speed(v);
            let val = Value::Choice(s.into());
            if v.round() as i64 == 2 {
                out.push_unconfirmed("mustard.dyn2.release", "Mustard Dyn 2 release", val);
            } else {
                out.push("mustard.dyn2.release", "Mustard Dyn 2 release", val);
            }
        }
    }
    if let Some(name) = r.u(T, 0x1e16, 0).and_then(|v| ctx.resolve.strip_ref(v)) {
        out.push(
            "mustard.dyn2.key",
            "Mustard Dyn 2 external key",
            Value::Ref(name),
        );
    }
    side_chain_filter(r, out, T, 0x1e03, 0x1e05, "mustard.dyn2", "Mustard Dyn 2");
}

fn speed(v: f64) -> &'static str {
    match v.round() as i64 {
        0 => "slow",
        1 => "medium",
        2 => "fast",
        _ => "unknown",
    }
}

/// Dynamics side-chain filter: stored in plain Hz (unlike channel filters).
fn side_chain_filter(
    r: &mut Reader,
    out: &mut ParamList,
    t: u32,
    on: u16,
    freq: u16,
    key: &str,
    label: &str,
) {
    if let Some(v) = r.on(t, on, 0) {
        out.flag(&format!("{key}.filter"), &format!("{label} filter"), v);
        if v {
            if let Some(f) = r.f(t, freq, 1) {
                out.num(
                    &format!("{key}.filter_lo"),
                    &format!("{label} filter low"),
                    f,
                    Unit::Hz,
                );
            }
            if let Some(f) = r.f(t, freq, 0) {
                out.num(
                    &format!("{key}.filter_hi"),
                    &format!("{label} filter high"),
                    f,
                    Unit::Hz,
                );
            }
        }
    }
}

/// Spice Rack slot (`0x18` strip, object `0x1f`). Band layout depends on the
/// plugin model, which the strip is named after.
fn spice_rack(ctx: &Ctx, r: &mut Reader, out: &mut ParamList) {
    const T: u32 = 0x1f;
    out.push("spice.model", "Plugin", Value::Text(ctx.name.to_string()));
    if let Some(host) = r.u(T, 0x1f00, 0).and_then(|v| ctx.resolve.strip_ref(v)) {
        out.push("spice.host", "Inserted on", Value::Ref(host));
    }
    let naga = ctx.name.contains("Naga");
    let chilli = ctx.name.contains("Chilli");
    if !(naga || chilli) {
        return;
    }
    let bands: Vec<(u8, String)> = if naga {
        (0u8..6).map(|i| (i, format!("band {}", i + 1))).collect()
    } else {
        let mut v: Vec<(u8, String)> = (0u8..4).map(|i| (i, format!("band {}", i + 1))).collect();
        v.push((4, "P1".into()));
        v.push((5, "P2".into()));
        v
    };
    for (idx, label) in bands {
        let key = label.replace(' ', "");
        let k = |f: &str| format!("spice.{key}.{f}");
        let l = |f: &str| format!("{label} {f}");
        // Chilli 6 bands 1–4 are split by crossovers, not by a frequency.
        let has_freq = naga || idx >= 4;
        if has_freq {
            if let Some(v) = r.f(T, 0x1f05, idx) {
                out.num(&k("freq"), &l("freq"), v, Unit::Hz);
            }
            if let Some(v) = r.f(T, 0x1f06, idx) {
                out.num(&k("q"), &l("Q"), v, Unit::Plain);
            }
        }
        if let Some(v) = r.f(T, 0x1f07, idx) {
            out.num(&k("gain"), &l("gain"), v, Unit::Db);
        }
        if let Some(v) = r.f(T, 0x1f0e, idx) {
            out.num(&k("thresh"), &l("threshold"), v, Unit::Db);
        }
        if let Some(v) = r.f(T, 0x1f0a, idx) {
            out.num(&k("range"), &l("range"), v, Unit::Db);
        }
        if let Some(v) = r.f(T, 0x1f11, idx) {
            out.num(&k("attack"), &l("attack"), v, Unit::Seconds);
        }
        if let Some(v) = r.f(T, 0x1f12, idx) {
            out.num(&k("release"), &l("release"), v, Unit::Seconds);
        }
    }
    if chilli {
        for (idx, name) in [(1u8, "low"), (2, "mid"), (3, "high")] {
            if let Some(v) = r.f(T, 0x1f05, idx) {
                out.num(
                    &format!("spice.xover_{name}"),
                    &format!("Crossover {name}"),
                    v,
                    Unit::Hz,
                );
            }
        }
    }
    if naga {
        if let Some(v) = r.f(T, 0x1f4d, 0) {
            out.num("spice.release_shape", "Release shape", v, Unit::Plain);
        }
    }
}
