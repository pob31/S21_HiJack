# Console session files — field notes

What we know about DiGiCo session files on disk, gathered for three planned
features: **import** a console session into S21 HiJack as a new show, produce
**readable/printable HTML** documents of a session, and eventually **convert**
an SD `.ses` into an S21/S31 `.session`.

Every claim is tagged:

- **[E]** evidence — observed in the sample files, or confirmed by a parser run
  over them.
- **[H]** hypothesis — fits the data, not yet confirmed. Code must not rely on
  an [H] field without confirming it first (same rule as `OSC_FIELD_NOTES.md`:
  no guessed constants).

Gathered 2026-10-19 by read-only analysis; nothing in this repo parses these
files yet.

---

## 0. Sample files and tools on the dev machine

| What | Where |
|---|---|
| S21 sessions (37 distinct, schema versions 82–100) | `E:\Régie\**\*.session` |
| SD sessions (SD8, SD9, SD10, SD11, SD12; 2015–2026) | `D:\Projects\SD*\*.ses` |
| Same show in both formats (Coup Fatal) | SD12: `D:\Projects\SD12\coupfatal_*.ses` · S21 hand port: `E:\Régie\Coup Fatal\CoupFatal_Virtual*.session` |
| S-Series offline editor | `C:\Program Files\DiGiCo\S-Series\OfflineEditor.exe` |
| SD12 offline software v22.0.2242 | `C:\SD12\SD12.exe` (settings `C:\SD12\SD12.INI`) |
| DiGiCo Pad OSC command sets for SD8/9/11/12 | `C:\SD12\IPAD SD8-9-11-12.dosc`, `C:\SD12\IPADV2sd8-9-11-12.dosc` |

The Coup Fatal S21 port is **not** a 1:1 conversion: it was rebuilt by hand
(60 inputs, 2 snapshots, renamed channels). Use it as a guide to how the show
was mapped onto an S21, not as converter ground truth.

---

## 1. S21 / S31 `.session` — SQLite 3

A plain SQLite database. Open read-only with
`file:<path>?mode=ro` (URI mode). **[E]**

### 1.1 Versions

- `PRAGMA user_version` seen: 82, 86, 88, 89, 92, 94, 99, 100. **[E]**
- Firmware, not the console model, decides the channel count: uv ≤ 94 files
  have 48 inputs / 16 buses (85 channels); uv 99/100 have 60 / 24 (105). The
  same show exists at both sizes. **[E]**
- Columns are added between versions **and column order differs between files
  of the same version** — always address columns by name. **[E]**
  - →86: Channel `automixerMode/automixerWeight/faderCrossfade`; SocketInfo `socketControlMode`.
  - →88: extra Effect columns, Output `position`, Preset table, Snapshot `gpoSnapshotBehaviour`.
  - →89: EqualiserBand `dynamics*` (dynamic EQ).
  - →92/94: PluginData table.
  - →99: Channel `stereoMode`; Pan `lcrBlend/lrmSendGain`; Session `gpiSnapshotTrigger`.
  - →100: Channel `pannerLinkMode`; Snapshot `skipNextPrevious`; SocketInfo
    `socketHardwareMode`; Session `currentUserLayoutId/showClockInScreen/screenLayersInterlocked`;
    new `UiLayouts` table + `layoutId` on UiBanks/UiChannels; Macro becomes
    `(macroId, modelIndex, parameter_1)`.
- All samples are S21 (identical local I/O, 12 UI banks). No S31 sample yet. **[E]**

### 1.2 Snapshot model

- `snapshotId 0` = the **live desk state**; `x == x_saved` everywhere. **[E]**
- `snapshotId 9999` = Solo channels only (or empty). Purpose **[H]**.
- User snapshots: ids ≥ 10000 (one old file also has id 1). `number` is the
  recall/display order and is renumbered on insert. **[E]**
- Every snapshot is a **full copy** of the per-channel row set, minus the Solo
  channels, input `InsertData` type 2, and ~132 internal sockets. **[E]**
- `x` vs `x_saved`: `x` is the edited content, `x_saved` the last committed
  value; `Snapshot.unsavedChanges` flags the difference (not always
  consistently). **[E]** Which one the desk recalls: **[H]** (`x_saved`).
- Recall scope: `ChannelLoadScope.snapshotId` is NULL everywhere → one
  session-wide scope, not per snapshot. `scopeType 0` = per channel,
  `1` = per category (`channelNumber 0`). Solo uses channelType **80** here. **[E]**
  `Scope.scopeEnabled` meaning **[H]**.

### 1.3 Channels

`Channel.type`: **[E]**

| type | meaning | count (60-in layout) |
|---|---|---|
| 0 | input | 60 (48 older) |
| 1 | bus — `busMode` 0 = aux, 1 = group | 24 (16) |
| 2 | control group (numbered 0–9) | 10 |
| 3 | master (number 25; 17 older) | 1 |
| 4 | matrix | 8 |
| 5 | solo | 2 |

`stereoMode` 0 mono / 1 stereo (master = 2, **[H]** LRM/LCR). Legacy `stereo`
is NULL from uv 100. **[E]**

### 1.4 Tables, units

| Table | Meaning | Units / notes |
|---|---|---|
| Channel | 1 row per channel per snapshot (`UNIQUE(snapshotId,channelNumber,type)`) | `gain` = **fader dB, −150 = −∞** (1/16 dB steps); `mute` bool; `gangId` → Gang.id (0 = none) **[E]** |
| `controlGroupMemberIds` | BLOB of 4-byte records `u16le channelNumber, u8 channelType, u8 flag` | `flag` meaning **[H]** |
| Gang | `(id, gangType=1, color '#rrggbb')` | **[E]** |
| ChannelBusSend | per source × 25 buses; `busId` **0-based**, 0..23 = bus 1..24, 24 = master (16 in older) | `gainInAuxMode` dB (−150 off); separate `enabledInAuxMode` / `enabledInGroupMode`, the bus's `busMode` decides which applies **[E]**; `sendPosition` **[H]** |
| Pan | panType 0 = channel pan; 1 = per-send pan (`parentId` = bus) | `balance` −1..+1 **[E]** |
| Input | `trim` dB, `delay` **seconds**, `balance`, `width`, `polarity` | **[E]** |
| Output | `trim` dB | **[E]** |
| Passband | 3 rows/channel; type 0 = channel HPF/LPF in Hz | types 1/2 **[H]** dynamics side-chain filters |
| Equaliser / EqualiserBand | 4 bands (`bandNumber` 0..3): `frequency` Hz (int), `gain` dB, `qvalue` (0.71 default), `bandType` 0 bell / 1 low shelf / 2 high shelf; `dynamics*` dynamic EQ (s, dB) | band types **[H]** names, **[E]** placement |
| Dynamics | type 0 = DYN1 multiband comp (`mode` single/multi **[H]**), type 1 = DYN2 keyed (gate/duck) | **[E]**/**[H]** |
| DynamicProcessor | DYN1 processor 0–2 (bands), DYN2 processor 3 | `threashold` (sic) dB, `ratio`, `range` dB, `attack/release/hold` s, `knee` 0/1/2, `gain` make-up dB |
| InsertData | insertType 0/1 = insert A/B | **[E]** |
| DigiTube | `drive`, `bias` | **[E]** |
| MatrixBusSend | 10 matrix inputs × 8 matrix buses | `gain` dB, −90 = off **[E]** |
| GraphicEqualiser(+Band) | 16 GEQs × 32 bands, gain dB | **[E]** |
| Effect | 8 FX units, `effectType` enum, times in seconds | **[E]**/**[H]** |
| SocketInfo | static I/O map (not per snapshot); `socketInfoType` 1 source / 2 destination; `portType` 0 internal DSP, 1 local/DMI, 2 remote rack | most **[E]** |
| SenderSocket | per snapshot per source: `gain` = **preamp gain dB** (−20..+37.5), `phantomPower`, `padEnabled` | **[E]** (phantom off exactly on the SM58s in Lovedoll) |
| ReceiverSocket | `routedSocketId` → SocketInfo.id of the source; 0 = unpatched | **[E]** |
| Preset | channel presets as JSON text in `data` | **[E]** |
| PluginData | pluginId 9 = JSON list of OSC mappings | **[E]** |
| UiBanks / UiChannels | 12 banks; channel → bank + `indexInBank` (−1 unassigned) | **[E]** |
| Session | sample rate, master-fader assignment, prefs | **[E]** |
| Automixer | 1 row per snapshot, floors dB | **[E]** |

**Input patch lookup** (verified on Lovedoll) **[E]**:
SocketInfo row with `portType=0, deviceType=3, socketInfoType=2,
socketNumber=channel, socketHWNumber=(ch−1)*30` (+1 = right leg) →
`ReceiverSocket(snapshotId, socketId).routedSocketId` = source socket →
`SenderSocket(snapshotId, source)` gives preamp gain / phantom / pad.

### 1.5 What a writer must keep consistent

- Foreign keys: every per-channel table references
  `Channel(snapshotId, channelNumber, type) ON DELETE CASCADE`; per-snapshot
  tables reference `Snapshot(id)`; sockets reference `SocketInfo(id)`.
  `PRAGMA foreign_key_check` is clean on the samples. No triggers, no views. **[E]**
- Only `Channel` has a UNIQUE index — the writer must prevent duplicate rows
  elsewhere itself. **[E]**
- Full row set per snapshot (60-in layout): Channel 103, Pan 1918,
  ChannelBusSend 2125, 4 EqualiserBand + 4 DynamicProcessor per channel,
  MatrixBusSend 80, GEQ 16 (+512 bands), Effect 8, Automixer 1,
  SenderSocket 802 / ReceiverSocket 821. **[E]**
- Keep SocketInfo ids stable (routing and Scope point at them). **[E]**
- Plan: **never synthesise a session from nothing** — copy a real session of
  the target firmware as a template and update rows in place, then open the
  result in `OfflineEditor.exe` to validate.

---

## 2. SD-range `.ses` — DiGiCo binary

Proprietary, but neither compressed nor encrypted. Little-endian throughout.
Decoded across 38 sample files (SD8 v9 → SD12 vN, 2015–2026). **[E]** unless
tagged.

### 2.1 Header

| Field | Old layout (SD8/9/10/11, v9–vA) | SD12 layout (`SDC`, vB–vN) |
|---|---|---|
| Magic @0x00 | `DiGiCo     SD8 .SES vA` (or `SD9 `) | `DiGiCo     SDC .SES vN` |
| Version letter @0x15 | 9, A | B, J, K, N |
| u16 @0x20 first section offset | 0x148 | 0x1c8 |
| u16 @0x22 software build | 726, 760, 821 | 925 (vB), 1742 (vJ), 1926 (vK), 2126 (vN) |
| u16 @0x26 input channel count | 60 / 48 / 72 | 90, 92, 60 |
| Aux / group counts | u8 @0x2e/0x2f mono/stereo aux; @0x36/0x37 groups | u16 @0x36/0x38 aux; @0x46/0x48 groups |
| f32 sample rate | @0x5f | @0x77 |
| Bus counts (5 × u8) | @0x7b | @0x93: matrix outs, CGs, ? (0/2), matrix inputs, GEQs (order **[H]**) |
| Section offset table (u32, last = file size) | @0x80 | @0x100 |
| Section tag list (1 byte per section) | before first section | before first section |

Look sections up **by tag**, not by index — the count changes per version.

| Tag | Content |
|---|---|
| (section 0) | u16 length + show name |
| `db` | FX preset library |
| `c9` | I/O racks |
| `d2` | **object database + snapshot list** |
| `d3` | live (current) state |
| `f8` | macros |
| `f9` | Sets |
| `fa` | MIDI devices |
| `f3` | not identified (starts "Grp 6-17") |
| others | plain blobs: u16 length + data |

Two section kinds: *tagged* (u16 tag + u32 count) and *blob* (u16 = length − 2).

### 2.2 Primitive encodings

- Record stream, each record starts with a u16 tag; some carry a u16 length
  (`da 00 3a 00`).
- Strings: u16-length (show name, snapshot name, notes); u8-length padded to
  32 bytes (object/channel names, font); u32-length (I/O list).
- **Text is CP1252** — decodes "entrée", "Prélude", "Système" correctly.
- Timestamps: f64 OLE date (days since 1899-12-30).

### 2.3 Object database (section `d2`)

`d2 00 d2 00`, u32 N (14,828 in the orleans file), then N objects:

```
da 00 3a 00 | u32 objType | name[32] (u8 len + text) | f64 date | u32 nParams |
u8 ? | u16 xfade (1/100 s) | u16 ? | u8 ? | u32 refCount |
nParams × { u16 paramId (high byte = objType), u8 index, u8 flags, 4-byte value } |
12-byte trailer: u32 link (ffffffff none), u32 snapshot index, u32 0
```

Snapshots store **u32 indexes into this table**; objects shared by several
snapshots are stored once (`refCount`). That is why snapshot records are only
~54 KB.

| objType | Meaning | Key parameters |
|---|---|---|
| 0x0C | fader block | `0c21` fader dB f32 (−150 = −∞); `0c22` mute 0.0/1.0 |
| 0x0D | pan | `0d23` −1..+1 |
| 0x04 | 4-band EQ + dynamic EQ (index = band) | `0405` freq Hz, `0406` Q, `0407` gain dB, `040e` dyn threshold, `0410` ratio, `0411` attack, `0412` release |
| 0x05 | dynamics (0–2 comp bands, 3 gate) | `050e` threshold, `0510` ratio, `0511` attack, `0512` release, `0509` gain |
| 0x07 | filters | `0705[0]` LPF Hz, `0705[1]` HPF Hz, `0703` enables |
| 0x09 | delay | `0908` seconds |
| 0x01 | channel input | `0101` digital trim dB; `0100[0/1]` input A/B source (rack index + rack type) |
| 0x11 | input socket | `1186` preamp gain dB |
| 0x02 | aux sends (index = aux) | `0201[n]` level dB, `0202[n]` on |
| 0x03 | group routing | `0309[n]` assigned |
| 0x10 | control-group master | `1021` level dB, `1022` mute |
| 0x0F / 0x15 / 0x12 / 0x0E | GEQ / FX unit / output socket / insert **[H]** | — |

**Parameter IDs are DiGiCo's own command IDs.** The Pad command-set file
(`.dosc`, §3.1) uses the same `paramId[index]` numbering and names every one,
e.g. `0405[n]` = "eq freq n+1", `0c21` = "fader". The full ID → name → range
list is in `DiGiCo SD iPad V2 OSC Commandset.md`. **[E]**

**Confirmed against the live desk** (SD12 editor, 30 input channels: OSC
value == file value, one unique field per parameter) **[E]**: trim `0101`,
phase `0102`, delay `0908` / on `0907`, filter in `0703[0/1]`, compressor
`050e/050f/0511/0512/0510/0509[band]` + in `0502[0]` + crossovers
`0505[0/1]` + mode `053a`, gate `0502[3]/050e[3]/0511[3]/0512[3]`, EQ
`0403–0407`, `040b`, `040e`, `0410–0412`, `047d` per band, fader `0c21`, mute
`0c22`, pan `0d23`, aux sends `0201/0202/0203[slot]`, group routing
`0309[slot]`.

- Send/group `slot` is a **shared bus-slot index**, not the aux/group number
  (aux 1 → slot 2, aux 10 → slot 1, group 1 → slot 6 in the test session).
  It has to be mapped through the session's bus table.
- Pan and send pan are stored −1…+1. OSC carries 0…1.
- Filter frequency `0705[0]` (high cut) / `0705[1]` (low cut) is stored
  **scaled**. Displayed = OSC = file × 0.8 (high cut), file × 1.25 (low cut),
  on every channel tested. **[E]** Confirmed on screen 2026-10-20: Ngoma1 low
  cut shows 81.8 Hz (OSC 81.81, file 65.45).
- The gate/de-esser key filter is stored in Hz (`0505[2/3]`); OSC uses a
  0–255 code (`057e/057f`).
- **Object names are not channel names.** The compressor holding Ngoma1's live
  values is an object named "Congas1". Resolve objects through the strip's
  object list, never by name. **[E]**

Unknown: the 0x04 flag on fader params, `0c04/0c05` bitfields, objTypes
0x08/0x0A/0x13.

### 2.4 Snapshot list (inside `d2`, after the objects)

u32 0 · *(vJ+)* u32 group count + groups (u16 length + blob starting with the
name — e.g. PRESTI OMAI, EKUNDA, LASCIA) · u32 snapshot count · records:

- Strip list: `d3 00` + u32 strip count, then per strip
  `d5 00 01 00 <0 by-ref / 1 inline> u32 n, n × u32 object index, 27 00 <idx> <stripType>, name[32], ffffffff, u8 width (1 mono / 2 stereo)`.
- u16 + snapshot name, f64 date.
- u16 + 992-byte blob (mostly 0xFF — recall scope **[H]**).
- `02 00 xx xx` + u32 length + MIDI events (Program Change **[H]**), 10 bytes.
- u16 + 807-byte blob (same size as section `d4`) **[H]**.
- u16 property length (77 vA/vJ, 81 vK/vN): **u32 number × 100** (750 = 7.5),
  **u16 crossfade in 1/100 s**, **u8 group (1-based)**, …, font colour/size/name,
  u32 unique id.
- u16 + note text; *(vJ+)* u16 + second string (empty so far).

The crossfade field matches the operator's notes (17.0 "pan 12s" → 12.00 s;
39.0 → 10.00 s). **[E]**

Input strips have 13 object slots; slot **order differs between snapshots and
the live section** — find objects by objType, never by slot. **[E]**

Strip types (SD12): 01 input, 05 aux, 06 group, 07 right leg of stereo
aux/group, 08 monitoring, 0a control group, 0b solo, 0c talkback, 0e FX,
0f matrix input, 10 matrix output, 11 GEQ, 1b generators, 1c–21 / 4d–53 rack
sockets. Older vA files use different codes (03 aux, 04 group, 05 R legs,
08 CG, 0d matrix in, 0e matrix out, 0f GEQ).

### 2.5 Live state (section `d3`)

`d3 00` + u32 strip count (839 in the test session), then per strip:
`d5 00 01 00 01` + u32 n + **n inline objects** (same layout as §2.3), then a
41-byte tail `27 00 <u8 idx> <u8 stripType> name[32] ffffffff <u8 width>`.
Input strips carry 13 objects of types 1, 9, 7, 8, 5, 4, C, 2, D, 3, E, 13, A.
This is the state the desk is in, and what the OSC live check above was
compared against. **[E]**

Newer file version seen: `session001.ses` is **vO, build 2242** (SD12 software
v22.0.2242) with one extra section tag `0x73`. The parser above reads it
unchanged. **[E]**

### 2.6 Known gaps

- The 992- and 807-byte blobs (scope/safes?) not decoded.
- Files with 0 objects / 0 snapshots (`requiem_base1`, `requiem_foh_test1`,
  the `lascondes` files) trip the group-detection step — need a special case.
- Cross-file check: `coupfatal_amiens020426` vs `…030426` differ only in
  snapshot 18 (275 parameter changes — the snapshot was re-stored). **[E]**

### 2.7 Reference parser (Python, validated on all SD12 files)

```python
import struct
u16=lambda d,p: struct.unpack_from('<H',d,p)[0]; u32=lambda d,p: struct.unpack_from('<I',d,p)[0]
def sections(d):
    first=u16(d,0x20); j=0x80 if first==0x148 else 0x100; offs=[]
    while not offs or offs[-1]!=len(d): offs.append(u32(d,j)); j+=4
    k=first-1
    while d[k]==0: k-=1
    e=k+1
    while d[k]!=0: k-=1
    tags=d[k+1:e]; return {tags[i]:(offs[i+1],offs[i+2]) for i in range(len(tags))}
def parse_objects(d,p):
    n=u32(d,p+4); p+=8; objs=[]
    for _ in range(n):
        typ,np_=u32(d,p+4),u32(d,p+48)
        prm={struct.unpack_from('<HB',d,p+62+8*k):d[p+66+8*k:p+70+8*k] for k in range(np_)}
        objs.append((typ,d[p+9:p+9+d[p+8]].decode('cp1252'),prm)); p+=62+8*np_+12
    return objs,p
# objs,_ = parse_objects(d, sections(d)[0xd2][0])
# fader of an 0x0C object: struct.unpack('<f', prm[(0x0c21,0)]); mute: prm[(0x0c22,0)]
```

Re-checked on `coupfatal_orleans270326.ses`: header (build 2126, 90 inputs,
48 kHz), 26 tagged sections, 14,828 objects parsed exactly to the end of the
object list. **[E]**

---

## 3. SD12 offline editor as a live test console

The offline software exposes External Control like a real SD12, so the app's
existing Pad connection (`src/console/pad_connection.rs`) should be able to
drive it. That turns it into ground truth for §2: recall a snapshot, read every
parameter over OSC, find those values in the `.ses` bytes.

State observed:

- External Control is a global switch: `ExternalControlEnabled` in
  `C:\SD12\SD12.INI` (it was 0, the user turned it on). **[E]**
- With the switch on but **no device added, the editor opens no OSC port** —
  it only holds its own TCP 2028 / UDP 2029. **[E]**
- With a **DiGiCo Pad** device added (V2 command set Loaded, editor Send 9000 /
  Receive 8000), the editor listens on UDP **8000** on all interfaces and
  answers `/?` queries from `127.0.0.1` exactly like a desk. It identifies as
  `SD9-12`. **[E]**
- Full command set, wire conventions and quirks (strip dumps, unknown-path
  fallback, pan 0…1 on the wire): `DiGiCo SD iPad V2 OSC Commandset.md`. **[E]**
- **Push confirmed** (2026-10-20): surface changes in the editor are sent
  unasked to the Pad device: fader and EQ gain stream while dragged, mutes
  send one message each, control-group mutes send the CG `mute` and each
  member's `CGs_mute`. With nothing touched, nothing is sent. **[E]**
  This was observed on the offline software, not on hardware.

### 3.1 `.dosc` command-set files

Binary. Header: u8 length + `"DiGiCo OSC Commands 1502"`. Then records, each a
24-byte header followed by a name block. **[E]** (778 records parsed from
`IPADV2sd8-9-11-12.dosc`.)

```
header: u16 section | u16 paramId | u8 index | u8 sub | u8 ? | u8 valueType |
        f32 min | f32 max | 8 bytes 0
name:   u16 marker (1 = plain, 5 = takes an argument / wildcard) | u8 len | text, 0-padded
then:   optional display block (units "dB", "Hz"/"kHz", "S"/"mS"/"uS", ": 1", "%", "samples")
```

- `section`: strip type for strip commands (01 input, 05 aux, 06 group, 0a CG,
  0f matrix input, 10 matrix output, 11 GEQ, 12 solo; same codes as the `.ses`
  strip types). Console areas: `ce` /Console, `708` /Snapshots, `1fe` /Macros,
  `d0` /Presets, `ca` /Layout, plus `c9`, `cc`, `cf` (prefix unknown).
- `valueType`: 0 int/enum/bool, 1 float, 3 meter, 4 string.
- OSC path = strip folder (from the desk, not in the file) + name with `_` for
  spaces.

---

## 3b. Quantum (Quantum 225 offline software, 2026-10-20)

Same container as the SD12: header `DiGiCo     Q2  .SES vO`, build 2242.
`sdparse.py` / the §2.5 live-state parser read it unchanged. **[E]**
Test session `D:\Projects\Q2\session001.ses` (48 inputs, no snapshots).
Controlled save: `C:\projects\session002.ses`.

**OSC (Pad surface):**
- The editor identifies as `SD7Q-Q2`. Its command set is
  `C:\Quantum2\IPAD_Q2.DOSC` (header `DiGiCo OSC Commands 1012`, 845
  records). Compared with the SD V2 set, it adds EQ bands 5–8 and
  `eq pre-ins` on aux/group/matrix outputs, and drops the output-strip
  HPF/LPF. It has **no Mustard, Spice Rack or nodal commands**. **[E]**
- **Push confirmed**: fader and EQ gain stream while dragged, mute sends one
  message, and a Save As pushes `/Console/Session/Filename`. Mustard changes
  were **not** pushed (consistent with the command set). **[E]**

**File: Quantum-only objects.** A Quantum input strip carries 18 objects:
the SD 13, then `0x1a`–`0x1e`. Mapped by a controlled save (input 4: Mustard
EQ band 1 → 20 kHz / Q 0.1 / −18 dB, Dyn 1 threshold → −60, Dyn 2 threshold
→ −60), plus the editor's screens, which match every default:

| objType | Mustard section | Fields |
|---|---|---|
| `0x1a` | Mustard main (type, mix, mode, order?) | not identified **[H]** |
| `0x1b` | Mustard EQ filters | `1b05[0]` LPF, `1b05[1]` HPF, `1b03[n]` in. Scaled like SD filters: displayed = file × 0.8 (LPF), × 1.25 (HPF) **[E]** |
| `0x1c` | Mustard EQ, 4 bands | `1c05[n]` freq Hz, `1c06[n]` Q, `1c07[n]` gain dB, `1c04[n]` curve (1 = bell) **[E]** |
| `0x1d` | Mustard Dynamics 1 (CLASSIC comp in the test) | `1d0e` thresh, `1d11` attack s, `1d12` release s, `1d10` ratio, `1d09` gain, `1d05[1]/[0]` s/c filter low/high Hz (unscaled) **[E]**; mix/knee/RMS **[H]** |
| `0x1e` | Mustard Dynamics 2 (GATE in the test) | `1e0e` thresh, `1e11` attack s, `1e13` hold s, `1e12` release s, `1e0a` range dB, `1e05[1]/[0]` key filter Hz **[E]** |

**Mustard types and modes** (controlled save on input 6, checked against the
type-picker screens):

- `1d0b` = Mustard Dynamics 1 type. 0 = Classic (DYN Blue), 8 = Levelling Amp
  (DYN Silver) **[E]**. VCA / Opto / FET codes are not yet seen; 0/2/4/6/8 is
  only a guess **[H]**.
- Levelling Amp (second controlled save, every on-screen value changed and
  matched) **[E]**: `1d51` = gain (25 → 100), `1d52` = peak reduction (20 →
  100), `1d50` = compress 0 / limit 1, `1d47` = mix 0…1 (100 % → 0 %).
- Dyn 1 common fields **[E]**: `1d02` = on, `1d03` = side-chain filter on,
  `1d05[0]` / `1d05[1]` = filter high / low in plain Hz (6.07 kHz / 205 Hz).
- `1e0b` = Mustard Dynamics 2 type. 0 = Gate, 2 = MSE (Mustard Source
  Expander) **[E]**; 1 = Duck by elimination **[H]**.
- MSE **[E]**: `1e0e` = threshold dB, `1e0a` = depth dB (the gate's range
  field; 10 → 39.84), `1e4c` = release slow 0 / medium 1 (/ fast 2 **[H]**).
- Dyn 2 common fields **[E]**: `1e02` = on, `1e03` = key filter on,
  `1e05[0]` / `1e05[1]` = key filter high / low in plain Hz.
- Not yet mapped: object `0x1a`, the SD/M selector, and the Mustard on
  switches.

**Spice Rack** (controlled save: Chilli 6 on input 5 insert A, checked
against the Spice Rack screen) **[E]**:

- Strip type `0x18` = Spice Rack slot (6 in this session, each with one
  object `0x1f`, named after the plugin, "Chilli 6").
- Host input strip, insert object `0x08`: `0802[0]` insert A in = 1 (the same
  ID as the Pad command `insert A in`). `0800[0]` and `08fe[0]` =
  `0x18000000`: insert A routed to strip type `0x18`, slot index 0
  (send/return **[H]**).
- Slot back-link `1f00[0]` = `0x01040008`: strip type 01 (input), index 4
  (input 5), insert A, object type 0x08 **[H]** for the byte meanings.
- Chilli 6 band index: 0–3 = bands 1–4, 4 = P1, 5 = P2. `1f07[n]` = band
  gain dB, `1f05[1/2/3]` = crossover low/mid/high Hz, `1f05[5]` / `1f07[5]` =
  P2 freq / gain. Q, range, attack, release, threshold, side chain and output
  are not yet exercised.
- Nodal processing: nothing seen in a fresh session. **[H]**
- Consequence: Mustard, Spice Rack and nodal settings can only be **read from
  the file**. Live OSC can't reach them, and an S21 has no equivalent, so a
  conversion report must list them per channel as not converted.

## 4. Open questions

S21: which of `x` / `x_saved` the desk recalls; `controlGroupMemberIds` flag;
`Scope.scopeEnabled`; snapshot 9999; `sendPosition`; Passband types 1/2;
full `socketHWNumber` layout.

SD: confirm every parameter ID in §2.3 against the editor; the scope/safe
blobs; section `d3`; MIDI event block; `.dosc` record layout.
