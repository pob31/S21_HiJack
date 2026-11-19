//! Printable HTML report of a console session.
//!
//! One self-contained page (inline CSS, no scripts, no external assets) laid
//! out for A4 landscape when printed. Sections: cue sheet, setup sheet,
//! processing, sends and routing, snapshot changes, gaps. Values decoded from
//! fields that are not yet confirmed are marked with a dagger (†).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use super::doc::{Param, SessionDoc, SnapshotDoc, Strip, StripKind, StripState};

/// Strip kinds in report order. Others are left out.
const ORDER: [StripKind; 10] = [
    StripKind::Input,
    StripKind::Aux,
    StripKind::Group,
    StripKind::Master,
    StripKind::MatrixInput,
    StripKind::Matrix,
    StripKind::ControlGroup,
    StripKind::Fx,
    StripKind::Talkback,
    StripKind::Solo,
];

/// Renders the full report. `generated` is shown in the header (pass the
/// current local time; tests pass a fixed string).
pub fn render(doc: &SessionDoc, generated: &str) -> String {
    let mut h = String::with_capacity(256 * 1024);
    let title = if doc.show_name.trim().is_empty() {
        doc.source.file_name.clone()
    } else {
        doc.show_name.clone()
    };
    let _ = write!(
        h,
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{} — session report</title><style>{CSS}</style></head><body>",
        esc(&title)
    );
    header(&mut h, doc, &title, generated);
    cue_sheet(&mut h, doc);
    setup_sheet(&mut h, doc);
    processing(&mut h, doc);
    routing(&mut h, doc);
    changes(&mut h, doc);
    gaps(&mut h, doc);
    h.push_str("</body></html>\n");
    h
}

const CSS: &str = "
:root{--ink:#1d1d1f;--muted:#6b6b70;--line:#d4d4d8;--head:#f2f2f4;--accent:#7a2e8e;}
*{box-sizing:border-box}
body{margin:0 auto;max-width:1400px;padding:16px;font:13px/1.35 system-ui,-apple-system,'Segoe UI',sans-serif;color:var(--ink);background:#fff}
h1{font-size:22px;margin:0 0 4px}
h2{font-size:17px;margin:28px 0 8px;padding-bottom:4px;border-bottom:2px solid var(--accent)}
h3{font-size:14px;margin:14px 0 4px}
.meta{color:var(--muted);margin:0 0 12px}
.meta b{color:var(--ink);font-weight:600}
table{border-collapse:collapse;width:100%;margin:4px 0 10px}
th,td{border:1px solid var(--line);padding:3px 6px;text-align:left;vertical-align:top}
th{background:var(--head);font-weight:600}
td.num{text-align:right;white-space:nowrap}
td.notes{white-space:pre-wrap;max-width:560px}
.strip{break-inside:avoid;margin:0 0 12px}
.kv{display:flex;flex-wrap:wrap;gap:2px 14px;margin:2px 0 6px}
.kv span{white-space:nowrap}
.kv i{font-style:normal;color:var(--muted)}
.off{color:var(--muted)}
.u{color:#a14000}
.flag{color:#a14000;font-weight:600}
nav ol{columns:3;margin:6px 0 0;padding-left:20px}
.scroll{overflow-x:auto}
@media print{
 @page{size:A4 landscape;margin:10mm}
 body{max-width:none;padding:0;font-size:9pt}
 h2{break-before:page}
 h2:first-of-type{break-before:auto}
 nav{display:none}
 thead{display:table-header-group}
 tr{break-inside:avoid}
 .scroll{overflow:visible}
}
";

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            _ => o.push(c),
        }
    }
    o
}

/// A param's value, with the unconfirmed marker.
fn val(p: &Param) -> String {
    if p.confirmed {
        esc(&p.value.display())
    } else {
        format!("<span class=\"u\">{}†</span>", esc(&p.value.display()))
    }
}

fn get<'a>(state: &'a StripState, key: &str) -> Option<&'a Param> {
    state.params.iter().find(|p| p.key == key)
}

fn cell(state: &StripState, key: &str) -> String {
    get(state, key).map(val).unwrap_or_default()
}

fn is_on(state: &StripState, key: &str) -> bool {
    matches!(
        get(state, key).map(|p| &p.value),
        Some(super::doc::Value::Bool(true))
    )
}

/// Live states of the strips in report order.
fn ordered(doc: &SessionDoc) -> Vec<(&Strip, &StripState)> {
    let mut v: Vec<_> = doc
        .live
        .iter()
        .filter_map(|s| doc.strips.get(s.strip).map(|strip| (strip, s)))
        .filter(|(strip, _)| ORDER.contains(&strip.kind))
        .collect();
    v.sort_by_key(|(strip, _)| {
        (
            ORDER
                .iter()
                .position(|k| *k == strip.kind)
                .unwrap_or(usize::MAX),
            strip.index,
        )
    });
    v
}

fn header(h: &mut String, doc: &SessionDoc, title: &str, generated: &str) {
    let _ = write!(h, "<h1>{}</h1><p class=\"meta\">", esc(title));
    let _ = write!(h, "<b>{}</b>", esc(&doc.source.format.describe()));
    let _ = write!(h, " · file <b>{}</b>", esc(&doc.source.file_name));
    if let Some(sr) = doc.sample_rate {
        let _ = write!(h, " · <b>{} kHz</b>", sr / 1000);
    }
    let mut counts: BTreeMap<usize, (StripKind, usize)> = BTreeMap::new();
    for s in &doc.strips {
        if let Some(i) = ORDER.iter().position(|k| *k == s.kind) {
            counts.entry(i).or_insert((s.kind, 0)).1 += 1;
        }
    }
    let list: Vec<String> = counts
        .values()
        .map(|(k, n)| format!("{n} {}", k.label().to_lowercase()))
        .collect();
    let _ = write!(
        h,
        "<br>{} snapshots · {}<br>Report generated {} by S21 HiJack {}. \
         Values marked <span class=\"u\">†</span> come from fields that are not yet confirmed.</p>",
        doc.snapshots.len(),
        esc(&list.join(", ")),
        esc(generated),
        env!("CARGO_PKG_VERSION"),
    );
    h.push_str(
        "<nav><b>Contents</b><ol><li><a href=\"#cues\">Cue sheet</a></li>\
         <li><a href=\"#setup\">Setup sheet</a></li><li><a href=\"#processing\">Processing</a></li>\
         <li><a href=\"#routing\">Sends and routing</a></li>\
         <li><a href=\"#changes\">Snapshot changes</a></li><li><a href=\"#gaps\">Gaps</a></li></ol></nav>",
    );
}

fn group_name(doc: &SessionDoc, snap: &SnapshotDoc) -> String {
    snap.group
        .and_then(|g| doc.snapshot_groups.get(g - 1))
        .cloned()
        .unwrap_or_default()
}

fn number(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{n:.0}")
    } else {
        format!("{n}")
    }
}

fn cue_sheet(h: &mut String, doc: &SessionDoc) {
    h.push_str("<h2 id=\"cues\">Cue sheet</h2>");
    if doc.snapshots.is_empty() {
        h.push_str("<p class=\"off\">No snapshots in this session.</p>");
        return;
    }
    let has_groups = !doc.snapshot_groups.is_empty();
    let has_fade = doc.snapshots.iter().any(|s| s.crossfade_s.is_some());
    h.push_str("<table><thead><tr><th>No.</th><th>Name</th>");
    if has_groups {
        h.push_str("<th>Group</th>");
    }
    if has_fade {
        h.push_str("<th>Crossfade</th>");
    }
    h.push_str("<th>Notes</th></tr></thead><tbody>");
    for s in &doc.snapshots {
        let _ = write!(
            h,
            "<tr><td class=\"num\">{}</td><td>{}",
            number(s.number),
            esc(&s.name)
        );
        if s.unsaved {
            h.push_str(" <span class=\"flag\">(unsaved edits)</span>");
        }
        h.push_str("</td>");
        if has_groups {
            let _ = write!(h, "<td>{}</td>", esc(&group_name(doc, s)));
        }
        if has_fade {
            let fade = match s.crossfade_s {
                Some(f) if f > 0.0 => format!("{f:.2} s"),
                _ => "—".to_string(),
            };
            let _ = write!(h, "<td class=\"num\">{fade}</td>");
        }
        let _ = write!(
            h,
            "<td class=\"notes\">{}</td></tr>",
            esc(s.notes.trim_end())
        );
    }
    h.push_str("</tbody></table>");
}

fn setup_sheet(h: &mut String, doc: &SessionDoc) {
    h.push_str("<h2 id=\"setup\">Setup sheet</h2><p class=\"meta\">Desk state when the file was saved.</p>");
    let rows = ordered(doc);
    let mut current: Option<StripKind> = None;
    for (strip, state) in rows {
        if current != Some(strip.kind) {
            if current.is_some() {
                h.push_str("</tbody></table></div>");
            }
            current = Some(strip.kind);
            let _ = write!(
                h,
                "<h3>{}</h3><div class=\"scroll\"><table><thead><tr><th>#</th><th>Name</th>\
                 <th>Source</th><th>Preamp</th><th>Trim</th><th>Phase</th><th>Delay</th>\
                 <th>HPF</th><th>LPF</th><th>Inserts</th><th>Fader</th><th>Mute</th><th>Pan</th>\
                 </tr></thead><tbody>",
                esc(strip.kind.label())
            );
        }
        let preamp = {
            let mut s = cell(state, "input.preamp");
            if is_on(state, "input.phantom") {
                s.push_str(" 48V");
            }
            if is_on(state, "input.pad") {
                s.push_str(" pad");
            }
            s
        };
        let delay = if is_on(state, "delay.on") {
            cell(state, "delay.time")
        } else {
            String::new()
        };
        let filter = |on: &str, f: &str| {
            if is_on(state, on) {
                cell(state, f)
            } else {
                String::new()
            }
        };
        let mut inserts = Vec::new();
        for slot in ["a", "b"] {
            if get(state, &format!("insert.{slot}.on")).is_some() {
                let mut s = format!("{}:", slot.to_uppercase());
                if !is_on(state, &format!("insert.{slot}.on")) {
                    s.push_str(" (off)");
                }
                let send = cell(state, &format!("insert.{slot}.send"));
                let ret = cell(state, &format!("insert.{slot}.return"));
                if !send.is_empty() && send == ret {
                    let _ = write!(s, " {send}");
                } else {
                    if !send.is_empty() {
                        let _ = write!(s, " → {send}");
                    }
                    if !ret.is_empty() {
                        let _ = write!(s, " ← {ret}");
                    }
                }
                inserts.push(s);
            }
        }
        let mute = if is_on(state, "fader.hard_mute") {
            "HARD".to_string()
        } else if is_on(state, "fader.mute") {
            "MUTE".to_string()
        } else {
            String::new()
        };
        let stereo = if strip.stereo {
            " <i class=\"off\">(st)</i>"
        } else {
            ""
        };
        let _ = write!(
            h,
            "<tr><td class=\"num\">{}</td><td>{}{stereo}</td><td>{}</td><td class=\"num\">{preamp}</td>\
             <td class=\"num\">{}</td><td>{}</td><td class=\"num\">{delay}</td>\
             <td class=\"num\">{}</td><td class=\"num\">{}</td><td>{}</td>\
             <td class=\"num\">{}</td><td>{mute}</td><td class=\"num\">{}</td></tr>",
            strip.index + 1,
            esc(&strip.title()),
            cell(state, "input.source_a"),
            cell(state, "input.trim"),
            cell(state, "input.phase"),
            filter("filter.hpf_on", "filter.hpf"),
            filter("filter.lpf_on", "filter.lpf"),
            inserts.join("<br>"),
            cell(state, "fader.level"),
            cell(state, "fader.pan"),
        );
    }
    if current.is_some() {
        h.push_str("</tbody></table></div>");
    }
}

/// `label value` chips for every param of the given sections.
fn chips(state: &StripState, sections: &[&str], skip: &[&str]) -> String {
    let mut out = String::new();
    for p in &state.params {
        let sect = p.key.split('.').take(2).collect::<Vec<_>>().join(".");
        let top = p.section();
        if !(sections.contains(&top) || sections.contains(&sect.as_str())) {
            continue;
        }
        if skip.iter().any(|s| p.key.starts_with(s)) {
            continue;
        }
        let _ = write!(out, "<span><i>{}</i> {}</span>", esc(&p.label), val(p));
    }
    if out.is_empty() {
        out
    } else {
        format!("<div class=\"kv\">{out}</div>")
    }
}

fn eq_table(state: &StripState, prefix: &str) -> String {
    let mut bands: BTreeMap<u32, HashMap<String, &Param>> = BTreeMap::new();
    for p in &state.params {
        let Some(rest) = p.key.strip_prefix(prefix) else {
            continue;
        };
        let Some((band, field)) = rest.split_once('.') else {
            continue;
        };
        let Some(n) = band
            .strip_prefix("band")
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        bands.entry(n).or_default().insert(field.to_string(), p);
    }
    if bands.is_empty() {
        return String::new();
    }
    let mut t = String::from(
        "<table><thead><tr><th>Band</th><th>Freq</th><th>Gain</th><th>Q</th><th>Curve</th>\
         <th>Mode</th><th>Dynamic</th></tr></thead><tbody>",
    );
    for (n, f) in bands {
        let c = |k: &str| f.get(k).map(|p| val(p)).unwrap_or_default();
        let dynamic = if f
            .get("dyn")
            .is_some_and(|p| p.value == super::doc::Value::Bool(true))
        {
            format!(
                "{} {} {} / {} {}",
                c("dyn_thresh"),
                c("dyn_ratio"),
                c("dyn_attack"),
                c("dyn_release"),
                c("dyn_mode")
            )
        } else {
            String::new()
        };
        let off = if f
            .get("on")
            .is_some_and(|p| p.value == super::doc::Value::Bool(false))
        {
            " class=\"off\""
        } else {
            ""
        };
        let _ = write!(
            t,
            "<tr{off}><td class=\"num\">{n}</td><td class=\"num\">{}</td><td class=\"num\">{}</td>\
             <td class=\"num\">{}</td><td>{}</td><td>{}</td><td>{dynamic}</td></tr>",
            c("freq"),
            c("gain"),
            c("q"),
            c("curve"),
            c("mode")
        );
    }
    t.push_str("</tbody></table>");
    t
}

fn heading_state(state: &StripState, key: &str, label: &str) -> String {
    match get(state, key) {
        Some(p) if p.value == super::doc::Value::Bool(false) => {
            format!("<b>{label}</b> <span class=\"off\">(off)</span>")
        }
        _ => format!("<b>{label}</b>"),
    }
}

fn processing(h: &mut String, doc: &SessionDoc) {
    h.push_str("<h2 id=\"processing\">Processing</h2>");
    for (strip, state) in ordered(doc) {
        let mut body = String::new();
        let eq = eq_table(state, "eq.");
        if !eq.is_empty() {
            let _ = write!(body, "{}{}", heading_state(state, "eq.on", "EQ"), eq);
        }
        let dynamics = chips(state, &["dyn"], &["dyn.on"]);
        if !dynamics.is_empty() {
            let _ = write!(
                body,
                "{}{}",
                heading_state(state, "dyn.on", "Dynamics"),
                dynamics
            );
        }
        let gate = chips(state, &["gate"], &["gate.on"]);
        if !gate.is_empty() {
            let _ = write!(body, "{}{}", heading_state(state, "gate.on", "Gate"), gate);
        }
        let mustard_eq = eq_table(state, "mustard.eq.");
        let mustard = chips(state, &["mustard"], &["mustard.eq.band"]);
        if !mustard.is_empty() || !mustard_eq.is_empty() {
            let _ = write!(body, "<b>Mustard</b>{mustard}{mustard_eq}");
        }
        if body.is_empty() {
            continue;
        }
        let _ = write!(
            h,
            "<div class=\"strip\"><h3>{} {} — {}</h3>{body}</div>",
            esc(strip.kind.label()),
            strip.index + 1,
            esc(&strip.title())
        );
    }

    // Spice Rack slots in use.
    let racks: Vec<_> = doc
        .live
        .iter()
        .filter_map(|s| doc.strips.get(s.strip).map(|strip| (strip, s)))
        .filter(|(strip, s)| strip.kind == StripKind::SpiceRack && get(s, "spice.host").is_some())
        .collect();
    if !racks.is_empty() {
        h.push_str("<h3>Spice Rack</h3>");
        for (strip, state) in racks {
            let _ = write!(
                h,
                "<div class=\"strip\"><b>Slot {} — {}</b> on {}{}</div>",
                strip.index + 1,
                esc(&strip.name),
                cell(state, "spice.host"),
                chips(state, &["spice"], &["spice.model", "spice.host"])
            );
        }
    }

    // Nodal processing, with the host channel's master switch.
    let nodes: Vec<_> = doc
        .live
        .iter()
        .filter_map(|s| doc.strips.get(s.strip).map(|strip| (strip, s)))
        .filter(|(strip, _)| strip.kind == StripKind::Node)
        .collect();
    if !nodes.is_empty() {
        h.push_str("<h3>Nodal processing</h3>");
        for (strip, state) in nodes {
            let host = strip.name.split(" > ").next().unwrap_or("");
            let active = doc
                .live
                .iter()
                .filter_map(|s| doc.strips.get(s.strip).map(|st| (st, s)))
                .find(|(st, _)| st.kind == StripKind::Input && st.title() == host)
                .map(|(_, s)| is_on(s, "input.nodal"));
            let status = match active {
                Some(true) => "active",
                Some(false) => "present, inactive (channel nodal switch off)",
                None => "host channel not found",
            };
            let _ = write!(
                h,
                "<div class=\"strip\"><b>{}</b> <span class=\"off\">({status})</span>{}{}{}</div>",
                esc(&strip.name),
                eq_table(state, "eq."),
                chips(state, &["dyn"], &[]),
                chips(state, &["gate"], &[])
            );
        }
    }
}

fn routing(h: &mut String, doc: &SessionDoc) {
    h.push_str("<h2 id=\"routing\">Sends and routing</h2>");
    h.push_str(
        "<table><thead><tr><th>Strip</th><th>Aux sends</th><th>Routed to</th></tr></thead><tbody>",
    );
    for (strip, state) in ordered(doc) {
        let mut sends: BTreeMap<String, (String, String, String, String)> = BTreeMap::new();
        let mut groups = Vec::new();
        for p in &state.params {
            let mut parts = p.key.split('.');
            match (parts.next(), parts.next(), parts.next()) {
                (Some("send"), Some(slot), Some(field)) => {
                    let e = sends.entry(slot.to_string()).or_default();
                    match field {
                        "on" => e.0 = p.label.trim_start_matches("Send ").to_string(),
                        "level" => e.1 = val(p),
                        "point" => e.2 = val(p),
                        "pan" => e.3 = val(p),
                        _ => {}
                    }
                }
                (Some("group"), Some(_), None) => {
                    groups.push(esc(p.label.trim_start_matches("Routed to ")));
                }
                _ => {}
            }
        }
        if sends.is_empty() && groups.is_empty() {
            continue;
        }
        let list: Vec<String> = sends
            .values()
            .map(|(name, level, point, _pan)| {
                let point = if point.is_empty() {
                    String::new()
                } else {
                    format!(" <i class=\"off\">{point}</i>")
                };
                format!("{} {level}{point}", esc(name))
            })
            .collect();
        let _ = write!(
            h,
            "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
            esc(&strip.title()),
            list.join("<br>"),
            groups.join(", ")
        );
    }
    h.push_str("</tbody></table>");
}

fn changes(h: &mut String, doc: &SessionDoc) {
    h.push_str(
        "<h2 id=\"changes\">Snapshot changes</h2><p class=\"meta\">Each snapshot against the one \
         before it (the first against the desk state at save). Only values that differ are listed.</p>",
    );
    let mut prev: &[StripState] = &doc.live;
    for snap in &doc.snapshots {
        let diff = diff_states(doc, prev, &snap.states);
        let _ = write!(h, "<h3>{} {}</h3>", number(snap.number), esc(&snap.name));
        if diff.is_empty() {
            h.push_str("<p class=\"off\">No change.</p>");
        } else {
            h.push_str(
                "<table class=\"changes\"><thead><tr><th>Strip</th><th>Changes (before → after)</th>\
                 </tr></thead><tbody>",
            );
            // One row per strip; the diff is already grouped by strip.
            let mut i = 0;
            while i < diff.len() {
                let strip = &diff[i].0;
                let mut items = Vec::new();
                while i < diff.len() && &diff[i].0 == strip {
                    let (_, label, before, after) = &diff[i];
                    items.push(format!(
                        "<span><i>{}</i> {before} → {after}</span>",
                        esc(label)
                    ));
                    i += 1;
                }
                let _ = write!(
                    h,
                    "<tr><td>{}</td><td><div class=\"kv\">{}</div></td></tr>",
                    esc(strip),
                    items.join("")
                );
            }
            h.push_str("</tbody></table>");
        }
        prev = &snap.states;
    }
}

/// (strip title, parameter label, before, after) for every changed value.
fn diff_states(
    doc: &SessionDoc,
    before: &[StripState],
    after: &[StripState],
) -> Vec<(String, String, String, String)> {
    let index = |states: &[StripState]| -> HashMap<usize, HashMap<String, Param>> {
        states
            .iter()
            .map(|s| {
                let m = s
                    .params
                    .iter()
                    .filter(|p| p.section() != "raw")
                    .map(|p| (p.key.clone(), p.clone()))
                    .collect();
                (s.strip, m)
            })
            .collect()
    };
    let a = index(before);
    let b = index(after);
    let mut out = Vec::new();
    let mut strips: Vec<usize> = b.keys().copied().collect();
    strips.sort_by_key(|i| {
        let s = &doc.strips[*i];
        (
            ORDER
                .iter()
                .position(|k| *k == s.kind)
                .unwrap_or(usize::MAX),
            s.index,
        )
    });
    let empty = HashMap::new();
    for i in strips {
        let strip = &doc.strips[i];
        if !strip.kind.is_mixing() {
            continue;
        }
        let old = a.get(&i).unwrap_or(&empty);
        let new = &b[&i];
        let mut keys: Vec<&String> = old.keys().chain(new.keys()).collect();
        keys.sort();
        keys.dedup();
        for k in keys {
            let (o, n) = (old.get(k), new.get(k));
            if o.map(|p| &p.value) == n.map(|p| &p.value) {
                continue;
            }
            let label = n.or(o).map(|p| p.label.clone()).unwrap_or_default();
            let show = |p: Option<&Param>| p.map(val).unwrap_or_else(|| "—".to_string());
            out.push((strip.title(), label, show(o), show(n)));
        }
    }
    out
}

fn gaps(h: &mut String, doc: &SessionDoc) {
    h.push_str("<h2 id=\"gaps\">Gaps and warnings</h2><ul>");
    for w in &doc.warnings {
        let _ = write!(h, "<li>{}</li>", esc(w));
    }
    let unconfirmed = doc
        .live
        .iter()
        .flat_map(|s| s.params.iter())
        .filter(|p| !p.confirmed && p.section() != "raw")
        .count();
    if unconfirmed > 0 {
        let _ = write!(
            h,
            "<li>{unconfirmed} printed value(s) come from fields that are not yet confirmed (marked †).</li>"
        );
    }
    h.push_str(
        "<li>Field-by-field evidence: Documentation/SESSION_FILES_FIELD_NOTES.md in the S21 HiJack \
         repository.</li></ul>",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::doc::{Format, ParamList, Source, Unit, Value};

    fn doc_with(name: &str, notes: &str) -> SessionDoc {
        let mut p = ParamList::default();
        p.num("fader.level", "Fader", -6.0, Unit::Db);
        let mut q = ParamList::default();
        q.num("fader.level", "Fader", 0.0, Unit::Db);
        q.push_unconfirmed("input.source_a", "Source A", Value::Ref("Dante 1".into()));
        SessionDoc {
            source: Source {
                file_name: "x.ses".into(),
                format: Format::SdSes {
                    model: "SDC".into(),
                    version: 'N',
                    build: 2126,
                },
            },
            show_name: "Show".into(),
            sample_rate: Some(48000),
            strips: vec![Strip {
                kind: StripKind::Input,
                index: 0,
                name: name.into(),
                stereo: false,
            }],
            live: vec![StripState {
                strip: 0,
                params: p.0,
            }],
            snapshots: vec![SnapshotDoc {
                number: 7.5,
                name: "Yale".into(),
                group: Some(1),
                crossfade_s: Some(2.88),
                notes: notes.into(),
                unsaved: false,
                states: vec![StripState {
                    strip: 0,
                    params: q.0,
                }],
            }],
            snapshot_groups: vec!["PRESTI OMAI".into()],
            warnings: vec![],
        }
    }

    #[test]
    fn text_from_the_file_is_escaped() {
        let html = render(&doc_with("<script>alert(1)</script>", "a & b <c>"), "now");
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("a &amp; b &lt;c&gt;"));
    }

    #[test]
    fn cue_sheet_and_changes_show_the_snapshot() {
        let html = render(&doc_with("Ngoma1", "GO>8.0: après"), "now");
        assert!(html.contains("<td class=\"num\">7.5</td>"));
        assert!(html.contains("PRESTI OMAI"));
        assert!(html.contains("2.88 s"));
        assert!(html.contains("GO&gt;8.0: après"));
        // The fader move from the live state shows in the changes table.
        assert!(html.contains("-6.0 dB"));
        assert!(html.contains("0.0 dB"));
        // An unconfirmed value carries the dagger.
        assert!(html.contains("Dante 1†"));
    }
}
