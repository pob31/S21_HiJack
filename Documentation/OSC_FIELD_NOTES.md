# OSC Field Notes — provenance log for the DiGiCo SD / Quantum surfaces

## Purpose

This application is a live state mirror for DiGiCo consoles, driven entirely over OSC.
DiGiCo publishes some of what we rely on and none of the rest. The two PDFs committed in
this directory are real, primary documents — they define the `/sd/` "Other OSC" command
set and the console-side setup for the DiGiCo Pad device — but they are silent on
feedback, on query syntax, on value scaling laws, and on every wire quirk the Pad protocol
carries. Everything past the edge of those documents was reverse-engineered on hardware, or
inherited from the wider live-sound community, or simply assumed by analogy with the S21
we can actually put our hands on.

Those three things look identical once they are compiled into a Rust constant. That is the
problem this file exists to solve. Every constant in the codebase that encodes a belief
about a console's behaviour should be traceable from here to the evidence behind it — who
established it, how, on what console, on what firmware, and how much weight it can carry.
When a hypothesis is wrong, the symptom is usually not a crash: it is a fader that recalls
to the wrong level or an EQ edit that lands on the mirror-image band, silently, in front of
an audience. Knowing which constants are guesses tells you where to look first.

Provenance is also perishable. Much of what is written down here cost the community days of
sleuthing on forums and in Facebook groups, and it has been lost and re-derived more than
once. Recording the attribution keeps the credit where it belongs and keeps the finding
recoverable.

## Sources

| Source | Kind | Weight | Notes |
| --- | --- | --- | --- |
| `Documentation/DiGiCo_OTHER_OSC_List_17_11_14.pdf` (704 KB, 17 Nov 2014) | Primary, DiGiCo | Authoritative **for what it covers** | The general-purpose "Other OSC" command set: `/sd/` prefix, 577 commands, normalized `0..1` levels. Dated 2014 and predates the interface actually shipping in the form we meet today, so absence of a feature from this list is weak evidence of absence on current firmware. Documents no query syntax and no feedback of any kind. |
| `Documentation/DiGiCo_SD_App_User_Guide_Issue_D.pdf` (3.8 MB, Issue D, Nov 2014) | Primary, DiGiCo | Authoritative for console-side setup | External Control setup for the "DiGiCo Pad" device, the one-Pad-device-at-a-time limit, the manually-loaded command set, and the "4 band EQ (or 8 band where applicable)" statement. Says nothing about the OSC wire format. |
| Bitfocus Companion DiGiCo module | Secondhand | Weak — **not** independent corroboration | The module bundles the same 2014 "Other OSC" document as its own reference. Where it agrees with that PDF it is repeating it, not confirming it. Its fader table is the only published normalized-level mapping we have, which is why we use it — but it is one unverified source, not two. |
| Community field reports (forums, Facebook groups) | Secondhand, attributed | Treat as a lead, not a fact | Useful precisely because they cover what DiGiCo does not document. Always recorded here with the reporter's name and date, and always marked unverified until someone in this project reproduces it. |
| Our own hardware measurements | Primary, ours | Strongest | Today this means one S21 (the operator's own desk). No SD or Quantum desk has been measured yet — every SD/Quantum constant below is documentation, hearsay, or analogy. |

### Verification vocabulary

Used consistently in the findings below:

- **Confirmed on hardware** — someone in this project observed it on a real desk. Names the console and, where known, the firmware.
- **Documented** — stated by DiGiCo in one of the committed PDFs. Strong, but bounded by the document's age and scope.
- **Reported, unverified** — a named community field report. Nobody here has reproduced it.
- **Assumed by analogy** — no evidence beyond "the S21 does it this way and the protocols share a lineage". The weakest category; these are the ones that bite.
- **Unknown** — an open question with a decision riding on it.

## Findings

### At a glance

| # | Finding | Status | Family |
| --- | --- | --- | --- |
| 1 | Two SD/Quantum surfaces address one TitleCase parameter tree | Documented | SD / Quantum |
| 2 | `/?` pull queries work on the `/sd/` surface | Reported, unverified | SD / Quantum |
| 3 | Does either surface **push** unsolicited surface moves? | **Unknown** | SD / Quantum |
| 4 | 8-band EQ on Aux/Group/Matrix outputs, 4 on inputs | Documented (both PDFs) | SD / Quantum |
| 5 | Normalized level breakpoint table | Assumed (secondhand) | SD / Quantum |
| 6 | `PadQuirks::SD_HYPOTHESIS` — five wire quirks | Assumed by analogy | SD / Quantum |
| 7 | Send-level range clamp, −140…+10 dB | Assumed | SD / Quantum |
| 8 | Console-side setup, one-device limit, ports 8000/9000 | Documented | SD / Quantum |
| 9 | EQ band indices run backwards on the Pad wire | Confirmed on hardware | S21 |
| 10 | Multiband Dyn1 Mid/High swapped on the Pad wire | Confirmed on hardware | S21 |
| 11 | Enumeration pacing and heartbeat timings | Assumed (our choice) | SD / Quantum |
| 12 | An S21 volunteers aux/group counts that were never asked for | Confirmed on hardware (S21); unknown elsewhere | S21 |

---

### 1. The two SD/Quantum OSC surfaces address the same TitleCase parameter tree

**Claim.** SD and Quantum consoles expose two distinct OSC surfaces, not one. The `/sd/`
"Other OSC" surface is DiGiCo's general-purpose interface — the analogue of the S-series GP
OSC dialect — and the "DiGiCo Pad" external-control device is the reverse-engineered iPad
protocol. They are *not* different parameter trees. Both address the same TitleCase tree
(`Input_Channels/1/fader`, `EQ/eq_gain_2`, …) that our Pad codec already emits. The two
differences that matter are:

- **Path prefix.** `/sd/` prepends the literal `/sd`; the Pad surface uses the bare tree.
- **Level scaling.** `/sd/` carries levels as a normalized `0.0 ..= 1.0` position
  (`/sd/Input_Channels/1/fader, f, 1` = maximum); the Pad surface carries dB directly.

**Source and attribution.** The committed 2014 "Other OSC" command list. All 577 commands
were extracted from the PDF and compared leaf-by-leaf against the Pad codec's parameter
leaves; the trees line up.

**Status.** Documented. The correspondence is established from a primary DiGiCo source, not
from hardware — but it is a structural fact about the published address space, which is the
kind of thing a 2014 document is still reliable about. Caveat: the list is over a decade
old, so commands added since will be missing from it, and absence there does not prove
absence on the desk.

**Firmware / console.** Not applicable — documentary.

**Code that relies on it.**

- `src/model/family.rs` — `ConsoleSurface` (the `SdOther` / `Pad` split),
  `ConsoleSurface::path_prefix` (`"/sd"` vs `""`), `ConsoleSurface::level_wire`
  (`LevelWire::Normalized01` vs `LevelWire::Db`), `ConsoleSurface::uses_pad_tree`.
- `src/model/parameter.rs` — `ParameterPath::to_pad_suffix` / `from_pad_suffix`: one codec
  serves all three surfaces precisely because of this finding.

---

### 2. `/?` pull queries reportedly work on the `/sd/` surface

**Claim.** Appending `/?` to an address on the `/sd/` surface causes the console to reply
with that parameter's current value — even though DiGiCo's published command list documents
no query syntax and no feedback at all, and describes the interface as one-way.

**Source and attribution.** A field report posted **1 August 2026 by Kyra Soko**, crediting
**David Lim** of the Facebook Bitfocus Companion Users Group. The example given was querying
an input channel's name.

**Status.** Reported, unverified. Nobody in this project has reproduced it. Neither Kyra Soko
nor David Lim is a participant in this project; they are credited as the origin of the
report, not as validators of our implementation.

Note plainly what makes this plausible: `/?` is exactly the query convention the Pad
protocol uses, and both surfaces sit on the same parameter tree (finding 1). If the query
handling lives below the surface split inside the console's OSC dispatcher, `/?` working on
`/sd/` is not a surprise — it is the expected consequence. That is an argument for
plausibility, not evidence.

**Firmware / console.** Unstated in the report. This is a real gap: without a model and a
firmware version, a failure to reproduce tells us nothing about whether the report was wrong
or the behaviour was version-specific.

**Code that relies on it.**

- `src/model/family.rs` — the `ConsoleSurface::SdOther` doc comment records this as the
  reason `/sd/` is kept as a candidate surface at all;
  `EnumerationStrategy::PacedPadQueries`.
- `src/console/pad_connection.rs` — the whole enumeration pump is built on per-parameter
  `/?` queries (`send_next_query`, `build_enumeration_queue`). On the Pad surface this is
  well-founded; on `/sd/` it rests on this report alone.

---

### 3. THE OPEN QUESTION — does either surface push unsolicited updates?

**Claim (unresolved).** When an engineer moves a fader, hits a mute, or turns an EQ knob
**on the console surface itself**, does the console emit an OSC message about it to a
connected external-control device — on the `/sd/` surface, on the Pad surface, on both, or
on neither?

**Status.** **Unknown.** This is the single most consequential gap in this file.

**Why it is decisive.** This app is a live state mirror, and its headline features are
liveness features:

- **Ganging** — moving channel 3 must move channel 7. If the move originates on the desk and
  the desk never tells us, the gang never fires.
- **Pan link** — same argument, same failure.
- **Personal monitoring** — a musician's mix must track what the engineer just did.

Pull-only would still be worth something: `/?` queries solve *initial enumeration*, so the
mirror could be populated at connect time and stay correct as long as every change
originates from us. That is not the product. An engineer touching their own desk is the
normal case, not the exception.

**Why polling cannot substitute.** Gang propagation needs sub-100 ms end-to-end latency to
feel like the desk did it rather than like a glitch. Covering even 60 faders at 20 Hz is
1200 queries per second, before mutes, pans, sends and EQ — against a desk that community
reports say drops query bursts, which is exactly why our enumeration pump is paced
one-query-in-flight (`src/console/pad_connection.rs`, `send_next_query`). Polling hard
enough to be usable is polling hard enough to be dropped, and a dropped poll is a stale
mirror with no error to report.

**What settles it.** The Protocol Probe: connect, move a control on the console surface, and
watch whether anything arrives on either surface. It is a five-minute test on real hardware
and it decides the surface ordering, the feature set, and whether SD/Quantum support is a
mirror or a remote control.

**Code that depends on the answer.**

- `src/model/family.rs` — `ConsoleProfile::for_family`, the SD/Quantum arm's
  `surfaces: &[ConsoleSurface::Pad, ConsoleSurface::SdOther]`. Pad leads *only* because it
  has positive evidence of push feedback on S-series. `SdOther` would otherwise be
  preferable: DiGiCo-documented, no reverse engineering, no one-device limit. A probe result
  may reorder these.
- `src/console/pad_connection.rs` — the entire `run_loop` mirror design assumes inbound
  traffic arrives unsolicited; `handle_message` routes any `ParameterUpdate` through
  `inbound::apply_inbound_parameter` whether or not it answers an outstanding query.

---

### 4. Aux, Group and Matrix outputs carry 8 EQ bands; inputs carry 4

**Claim.** On SD and Quantum, bus outputs (Aux, Group, Matrix) expose an eight-band
parametric EQ — `eq_freq_1` … `eq_freq_8` and the parallel gain/Q/curve/dynamic-EQ leaves —
while input channels stop at four. S-series is four bands throughout.

**Source and attribution.** Confirmed by **both** committed PDFs, independently: the SD App
user guide states "4 band EQ (or 8 band where applicable)", and the `/sd/` command list
enumerates `eq_*_1` through `eq_*_8` on Aux/Group/Matrix while inputs stop at 4.

**Status.** Documented, and the only SD/Quantum finding here with two primary sources
agreeing. Still unmeasured on hardware, but this is as solid as documentation gets.

**Firmware / console.** SD range as of the Issue D guide; assumed to carry forward to
Quantum, which inherits the SD processing model.

**Code that relies on it.**

- `src/model/parameter.rs` — `ParameterPath::eq_band_range(channel, family)` returns `1..=8`
  for `Aux`/`Group`/`Matrix` on `SdRange`/`Quantum` and `EQ_BAND_RANGE` (1..=4) otherwise.
  This is the single enforcement point.
- `src/model/parameter.rs` — `pad_eq_band_map` accepts unreversed indices across
  `EQ_BAND_RANGE_MAX` specifically to admit the eighth band.
- `src/console/pad_connection.rs` — `build_enumeration_queue` queries band 8 on a Quantum
  aux (pinned by `enumeration_queue_covers_the_extra_sd_output_eq_bands`).

---

### 5. The normalized level breakpoint table

**Claim.** The `/sd/` surface's normalized `0.0 ..= 1.0` level position maps to dB by a
piecewise-linear law: 0.025 per dB above −10 dB, half that from −10 to −30 dB, and half
again below.

| dB | normalized |
| --- | --- |
| `FADER_INF_DB` (−∞ sentinel) | 0.0 |
| −50.0 | 0.125 |
| −30.0 | 0.25 |
| −10.0 | 0.5 |
| 0.0 | 0.75 |
| +10.0 | 1.0 |

**Source and attribution.** The Bitfocus Companion DiGiCo module's fader table — the only
published mapping we have. **Not measured**, and not independent of the 2014 document (see
Sources: the module bundles it).

**Status.** Assumed (secondhand). Marked `HYPOTHESIS` in the code. If the real law turns out
to be smooth rather than piecewise, the table is replaced and nothing else changes — the
interpolation is shared by both directions so they cannot drift apart.

**How to settle it.** Send a known normalized value on `/sd/`, read the dB the desk displays,
repeat across the range. Half a dozen points would either confirm the breakpoints or expose
the real curve.

**Consequence if wrong.** A recalled fader lands at the wrong level. This is the highest-risk
hypothesis in the file for that reason: it fails quietly and in the one place an audience
hears it.

**Firmware / console.** None — never measured on any desk.

**Code that relies on it.**

- `src/osc/ipad_values.rs` — `NORMALIZED_LEVEL_TABLE`, and the `db_to_normalized` /
  `normalized_to_db` pair built on it.

---

### 6. `PadQuirks::SD_HYPOTHESIS` — five wire quirks, none measured

**Claim.** The SD/Quantum Pad dialect differs from the S21 Pad dialect in two respects and
matches it in three.

| Field | SD/Quantum value | Reasoning | Status |
| --- | --- | --- | --- |
| `eq_bands_reversed` | `false` | The reversal looks like an S21 firmware artifact; no other DiGiCo implementation reports it, so it is assumed absent elsewhere. | Assumed by analogy |
| `dyn1_mid_high_swapped` | `false` | Same reasoning — an S21 artifact, assumed absent. | Assumed by analogy |
| `bool_wire` | `BoolWire::Float01` | Shared Pad-app lineage; the S21 sends floats 1.0/0.0 rather than OSC `T`/`F` type tags, and that convention is assumed to carry over. | Assumed by analogy |
| `pan_wire` | `PanWire::ZeroToOne` | Same lineage argument: pan as `0..1` with 0.5 centre. | Assumed by analogy |
| `control_groups_zero_based` | `true` | Same lineage argument: Control Groups numbered from 0 on the wire while everything else is 1-based. | Assumed by analogy |

**Source and attribution.** None. There is no source. Every one of these five is an inference
from the S21's measured behaviour plus the observation that the Pad protocol has a single
lineage across the ranges. Two of them are inferences that the S21's quirk is *not* shared,
which is a strictly weaker argument than the three that assume it is.

**Status.** Assumed by analogy — all five. The constant is named `SD_HYPOTHESIS` for exactly
this reason, and the name should not be changed until the fields it holds have been measured.

**Firmware / console.** None.

**Code that relies on it.**

- `src/model/family.rs` — `PadQuirks::SD_HYPOTHESIS`, wired in by
  `ConsoleProfile::for_family` for both `ConsoleFamily::SdRange` and `ConsoleFamily::Quantum`.
- `src/model/parameter.rs` — consumed by `to_pad_suffix` / `from_pad_suffix` via
  `pad_eq_band_map` and `pad_dyn1_band_map`.

**Escape hatch.** Every field is overridable at runtime without a rebuild, via
`ConsoleConfig::pad_quirk_overrides` (`src/model/config.rs`) in the show file or preferences.
One trap: the override **replaces** the quirks wholesale rather than merging field-wise, and
`PadQuirks`'s serde defaults are the *S21* values — so a hand-written partial override on an
SD/Quantum show silently inherits S21 values for the fields it omits. Build overrides from
the family constant: `PadQuirks { eq_bands_reversed: true, ..PadQuirks::SD_HYPOTHESIS }`.

---

### 7. Send-level range, −140 … +10 dB

**Claim.** SD/Quantum send levels lie within roughly −90 … +10 dB.

**Source and attribution.** Community Pad-protocol implementations, unattributed to any named
individual. The low end is widened in our code to −140 dB so the app's −∞ sentinel region
survives the clamp and a legitimate "off" is not rounded up into audibility.

**Status.** Assumed. Marked `HYPOTHESIS` in the code. Note the clamp exists *because* the
level scaling is a hypothesis: a value outside the range would otherwise be a silent
mis-scale rather than an obvious failure. S-series carries no clamp at all
(`send_level_db_range: None`) — the desk's own range is trusted there, because it has been
measured.

**Firmware / console.** None.

**Code that relies on it.**

- `src/model/family.rs` — `ConsoleProfile::for_family`, field `send_level_db_range:
  Some((-140.0, 10.0))` on the SD/Quantum arm.
- `src/model/parameter.rs` — `ParameterPath::clamp_value_with_profile`.

---

### 8. Console-side setup, the one-device limit, and ports 8000 / 9000

**Claim.** On the console: **External Control → Add Device → "DiGiCo Pad"**, with the console
configured **Send = 9000** and **Receive = 8000** — so we send to `console:8000` and listen
on `9000`. Two further constraints from the same guide:

- The Pad command set (`ipad_Q`) must be **manually Loaded** on the console. A desk with
  External Control enabled but no command set loaded will simply not answer, with no error.
- The console accepts **only one Pad device at a time**. If an iPad is already connected,
  we are not getting in. (The `/sd/` surface has no such limit, which is one of the reasons
  it would be the preferable surface if finding 3 resolves in its favour.)

**Source and attribution.** `DiGiCo_SD_App_User_Guide_Issue_D.pdf`.

**Status.** Documented for the procedure and the limits. The **port numbers** are a weaker
claim than the procedure: they are the guide's configuration, but they are
operator-configurable on the desk, so our constants are only first-run defaults and are
marked `HYPOTHESIS` in the code accordingly. Do not treat a connection failure on 8000/9000
as evidence against anything else in this file — check the desk's own port fields first.

**Firmware / console.** SD range as of Issue D; assumed unchanged on Quantum.

**Code that relies on it.**

- `src/model/family.rs` — `ConsoleProfile::for_family`, fields `default_pad_send_port: 8000`
  and `default_pad_receive_port: 9000` on the SD/Quantum arm.
- `src/console/pad_connection.rs` — `connect_pad` warns explicitly about all three failure
  modes when the handshake completes with zero replies ("may not have External Control
  enabled, may be pointed at a different port, or may need its Pad command set loaded").

---

### 9. S21: EQ band indices run backwards on the Pad wire — MEASURED

**Claim.** The S21 numbers its four parametric EQ bands in the reverse of the internal /
GP-OSC order. Internal band `b` is wire band `5 - b`: 1↔4, 2↔3. The mapping is its own
inverse, so one function converts in both directions.

**Source and attribution.** Measured by the project operator on their own S21, by comparing
the Pad-surface value against the GP-OSC value for the same band on a live desk.

**Status.** **Confirmed on hardware.** Recorded here as the deliberate contrast to findings
5, 6 and 7: this is what an established finding looks like, and it is the reason the
SD/Quantum assumptions are held to a different standard rather than quietly promoted.

**Firmware / console.** DiGiCo S21, the operator's own desk. Exact firmware version not
recorded at the time — a gap worth closing, and a reason the template below asks for it.

**Consequence if unhandled.** Without the reversal, iPad-sourced EQ updates land on the
mirror-image band, and in Mode 3 they collide with the correctly-decoded GP-OSC mirror
writes — one edit corrupts two bands at once.

**Code that relies on it.**

- `src/model/parameter.rs` — `pad_eq_band_map(band, reversed)`; all ten `EqBand*` arms of
  `to_pad_suffix` and their `parse_pad_eq_suffix` counterparts.
- `src/model/family.rs` — `PadQuirks::S21.eq_bands_reversed = true`.

**Known limit.** The `5 - band` arithmetic is only meaningful on a four-band strip, so a
reversed index outside 1..=4 is rejected rather than mapped to nonsense. That costs nothing
today because reversal is an S21 artifact and no S-series channel has more than four bands.
Should a probe ever find an eight-band console that *also* reverses, `pad_eq_band_map` needs
the strip width passed in so it can use `width + 1 - band`.

---

### 10. S21: multiband Dynamics 1 Mid and High are swapped on the Pad wire — MEASURED

**Claim.** The S21 multiband compressor numbers its three bands Low = 1, **High = 2, Mid =
3** — swapping Mid and High relative to the internal / GP-OSC order (1 = Low, 2 = Mid,
3 = High). Band 1 is unaffected. The swap is its own inverse.

**Source and attribution.** Measured by the project operator on their own S21: Pad
`comp_thresh_3` and GP `dyn1/1` (internal band 2, Mid) carry the same value, as do Pad
`comp_thresh_2` and GP `dyn1/2` (internal band 3, High).

**Status.** **Confirmed on hardware** — a direct cross-surface comparison on a live desk,
which is the strongest form of evidence available to this project.

**Firmware / console.** DiGiCo S21, the operator's own desk. Firmware version not recorded.

**Consequence if unhandled.** iPad-sourced multiband updates land on the swapped band and
collide with the correct GP-OSC mirror writes, collapsing bands 2↔3.

**Code that relies on it.**

- `src/model/parameter.rs` — `pad_dyn1_band_map(band, swapped)`; the `Dyn1*` arms of
  `to_pad_suffix` and `parse_pad_dyn1_suffix`.
- `src/model/family.rs` — `PadQuirks::S21.dyn1_mid_high_swapped = true`.

**Scope note.** The band-1 *bare path* convention (`comp_thresh` with no index) is
deliberately **not** quirk-parameterized — it is a path-shape question, not an index
mapping, and multiband dynamics are marked `Unsupported` on non-S families until a hardware
probe settles the real SD/Quantum shape.

---

### 11. Enumeration pacing and heartbeat timings

**Claim.** None about the console. These are *our* engineering choices, recorded here so
they are not later mistaken for measurements of desk behaviour.

| Constant | Value | Rationale |
| --- | --- | --- |
| `HANDSHAKE_TIMEOUT` | 5 s | Per collection phase. |
| `QUERY_TIMEOUT` | 250 ms | Per enumeration query, before retry. |
| `QUERY_RETRIES` | 2 | Then the parameter is skipped and logged rather than stalling the pump. |
| `HEARTBEAT_INTERVAL` | 2 s | `/Console/Name/?` once enumeration is done. |
| `HEARTBEAT_MISSES_STALE` | 3 | ≈ 6 s to `Stale`. |
| `HEARTBEAT_MISSES_LOST` | 6 | ≈ 12 s to `Lost`. |
| `TICK` | 100 ms | Loop tick driving both timeouts and heartbeat. |

**Source and attribution.** Chosen by this project. The one-query-in-flight pacing is
informed by community reports that some desks drop query floods — itself an unverified
community claim, and the reason enumeration is a pump rather than a burst.

**Status.** Assumed. Real round-trip latency on an SD or Quantum has never been measured, so
`QUERY_TIMEOUT` in particular is a guess; a desk slower than 250 ms per reply will retry
every parameter twice and skip a lot, turning a slow enumeration into an incomplete one.
Worth measuring in the same session that settles finding 3, since the probe is already
timing round trips.

**Firmware / console.** None.

**Code that relies on it.**

- `src/console/pad_connection.rs` — the constants block at the top of the file, consumed by
  `run_loop` and `send_next_query`.

---

### 12. An S21 volunteers the aux and group counts alongside the "modes" reply

**Claim.** The handshake's `BASE_QUERIES` ask `/Console/Aux_Outputs/modes/?` and
`/Console/Group_Outputs/modes/?` — the *modes* of those buses. It never asks either bus for
its *count*. An S21 nonetheless answers with the count message as well, and that is the only
reason `config.aux_output_count` and `config.group_output_count` are ever populated on the
Pad surface. The query set was captured from a real iPad session, so this courtesy was baked
in from the start without anyone noticing it was load-bearing.

**Source and attribution.** Found by inspection during Phase 2a of the SD/Quantum work
(13 Aug 2026), while writing `mock_pad_console`: the mock had to volunteer the counts
unasked in order for the app to learn them, which is what exposed the dependency. The
underlying S21 behaviour is evidenced by the captured handshake and by the fact that the
S-series path has always reported correct bus counts.

**Status.** Confirmed on hardware for the S21 (implicitly — the counts do arrive). **Unknown
for SD and Quantum**: there is no reason a different console generation must repeat an
undocumented courtesy.

**Firmware / console.** S21, firmware not recorded.

**Date.** 13 Aug 2026.

**Code that relies on it.** Nothing, now — `HandshakeOptions::PAD_ONLY_COUNT_QUERIES` asks
for `/Console/Aux_Outputs/?` and `/Console/Group_Outputs/?` outright, so the Pad path no
longer depends on the courtesy. The S-series default query set is deliberately unchanged
and still relies on it, because that path is hardware-verified and its regression run is
outstanding.

**Consequence if wrong.** Before the fix, a console that answered only what was asked would
leave both counts at their defaults: the app would mirror, scope and enumerate the wrong
number of aux and group buses, with no error anywhere — the counts would simply look
plausible and be wrong. Worth an explicit check in the first hardware session: confirm the
reported aux/group counts match the desk.

---

## How to add a finding

Append a new numbered subsection under **Findings**, add its row to the at-a-glance table,
and fill in every field of this template. Empty fields are informative: an unknown firmware
version is a fact about the finding's strength, so write "not recorded" rather than deleting
the line.

```markdown
### N. <One-sentence claim, in the present tense>

**Claim.** What the console does. Be specific enough that someone could disprove it —
name the address, the value, the channel type.

**Source and attribution.** Who established it and how. Name people exactly as they gave
their name. For a document, name the file. For a measurement, name who took it.

**Status.** One of: Confirmed on hardware / Documented / Reported, unverified /
Assumed by analogy / Unknown.

**Firmware / console.** Model and firmware version, or "not recorded". If a report does
not state them, say so — it is the difference between "we failed to reproduce it" and
"it does not happen on this firmware".

**Date.** When it was established or reported.

**Code that relies on it.** File path plus the constant, function or field name. If
nothing relies on it yet, write "nothing yet" — that is worth knowing too.

**Consequence if wrong.** What the operator sees when this belief is false.
```

### When a measurement confirms a hypothesis

Updating this file is only half the job. A confirmed measurement must also **remove the
corresponding `HYPOTHESIS` marker in the code** — otherwise the codebase keeps warning about
something we have since settled, and the markers stop meaning anything. Concretely:

1. Change the status here to **Confirmed on hardware**, and record the console model,
   firmware version, date and who measured it.
2. Delete or rewrite the `HYPOTHESIS` comment at the constant named under "Code that relies
   on it".
3. If the measurement *disproves* the value, change the constant too — and if it is a
   `PadQuirks` field, consider whether `SD_HYPOTHESIS` still deserves that name.
4. Leave the reasoning behind the original guess in this file. A hypothesis that turned out
   wrong is more useful to the next contributor than one that was quietly deleted.

Grep for `HYPOTHESIS` across `src/` to see what is still outstanding.
