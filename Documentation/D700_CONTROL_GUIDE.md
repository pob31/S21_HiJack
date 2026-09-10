# Asparion D700 — control guide

**How to drive every parameter on the surface.** This is the recipe book;
`D700_FIELD_NOTES.md` is the evidence behind it. Where they disagree, the field
notes are the record of what was measured and this is the distillation.

Written to be read two ways: an operator can follow the **Setup** and
**Gotchas** sections without touching code, and an implementer — human or
agent — can work straight from the byte tables without re-deriving anything.

Everything here is confirmed on hardware: an Asparion D700 Rack, 16 faders, two
display modules, over USB on Windows 11. Anything unverified is marked
*(unverified)*.

---

## 1. Setup — before anything works

### 1.1 Ports

The D700 presents **two MIDI port pairs**, one per bank of eight:

| Bank | Faders | Windows port name |
| --- | --- | --- |
| 1 | 1–8 | `D 700` |
| 2 | 9–16 | `MIDIIN2 (D 700)` / `MIDIOUT2 (D 700)` |

**The bank is selected by which port you send to**, never by the message
content. Within a bank, elements are numbered 0–7. There is no message that
addresses fader 12 directly — you address element 3 on bank 2's port.

Open both output ports at startup. Match by name substring `D 700`, and treat
the one containing `MIDIIN2`/`MIDIOUT2` as bank 2; do **not** rely on WinMM port
numbering, which renumbers when other USB devices come and go.

### 1.2 Preset

Set the Configurator to **Mackie** (preset 2). Everything in this guide is
confirmed under Mackie, and colour, metering, rings and displays behave
identically under Universal.

**Pin the preset in your documentation and say so to the operator.** Presets are
*almost* identical, which is the dangerous kind of difference: between Mackie and
Reaper exactly one control moves — the `*` button, note `0x36` under Mackie and
`0x5A` under Reaper. A binding learned under one preset keeps working for every
control except that one, silently. A surface 97% identical across presets cannot
be fingerprinted from traffic, so auto-detection is not available.

### 1.3 What must not be running

| Software | Effect |
| --- | --- |
| **Asparion Connector** | Holds the vendor HID interface exclusively. Harmless for MIDI. |
| **Asparion Configurator** | Consumes HID input; has been observed overriding colour writes. |
| Any DAW or Max patch with the ports open | Windows MIDI input is classically exclusive — it will take the input ports and you will see nothing. |

For the MIDI paths in this guide, close DAWs and Max. The Asparion apps only
matter if you also want the HID interface.

### 1.4 Sanity check

Move a fader. If you see pitch bend on channel 1–8, the ports and preset are
right. If you see nothing, something else has the port open.

---

## 2. Addressing model

Two rules cover the whole surface:

1. **The bank is the port.** Elements are 0–7 within a bank.
2. **An element's identity is its button note.** Encoder 3's V-Pot press,
   ring, and colour all key off the same number.

| Element | Base note / CC | Element *n* |
| --- | --- | --- |
| Fader | pitch bend channel | channel *n*+1 |
| Fader touch | `0x68` | `0x68 + n` |
| Encoder (input) | CC `0x10` | CC `0x10 + n` |
| Encoder ring (output) | CC `0x30` | CC `0x30 + n` |
| V-Pot press / **colour** | `0x20` | `0x20 + n` |
| Rec / Solo / Mute / Select | `0x00` / `0x08` / `0x10` / `0x18` | `+ n` |
| Master dial press / **colour** | `0x38` | — |
| Volume knob | pitch bend channel 9 | — |

---

## 3. Reading the surface

All inbound messages arrive on the bank's input port.

### 3.1 Faders

```
E<n> <lsb> <msb>          n = 0..7, 14-bit, value = (msb << 7) | lsb
```

Range `0`–`16383`. Touch sense arrives separately as note `0x68 + n`,
velocity `127` on touch and `0` on release.

**Touch does not mean "being adjusted".** Faders adjacent to buttons register
genuine touches when the operator reaches past them — 58 of 81 touch events in
one capture landed within 150 ms of a nearby button press. Gating motor updates
on touch alone will freeze those faders whenever someone reaches for the master
section.

### 3.2 Encoders

```
B0 <0x10+n> <value>
```

**Sign-magnitude, not two's complement.** Bit 6 is the sign, bits 0–5 the
magnitude:

```
value  1, 2, 3 …    →  +1, +2, +3      (clockwise)
value 65, 66, 67 …  →  −1, −2, −3      (counter-clockwise)
```

A two's-complement decoder reads `65` as **−63**. This is the single most likely
bug in a new integration, because two's complement is the usual MCU assumption.

### 3.3 Buttons

```
9<ch> <note> <velocity>       velocity 127 = press, 0 = release
```

Notes per §2. Velocity is always `127` on press — it carries no information.

### 3.4 Volume knob

Pitch bend on **channel 9**, 14-bit, absolute. It reports as a fader, not as an
encoder, and has **no touch sense** — MCU convention would place it at note
`0x70`, which the device never sends.

---

## 4. Driving the surface

### 4.1 Motor faders

```
E<n> <lsb> <msb>          n = 0..7, value 0..16383
```

**Never command full travel in one message.** Driving a fader to either end from
the far end hits the physical stop at full speed with no deceleration. The bottom
of the range is not an edge case — a parked channel lives there — so any recall
moving a fader from unity to −∞ commands exactly that.

**Interpolate large jumps** over roughly 20 steps, which was smooth on this
hardware. Do **not** clamp short of the endpoints to avoid the stop: a fader that
cannot reach −∞ misrepresents the desk, which is worse than the wear.

### 4.2 Button LEDs

```
90 <note> <127|0>         channel 1
```

**The surface has no local feedback.** Pressing a button sends a note-on and
nothing else — the LED lights only when the host echoes one back. An unlit button
after a press means the host didn't answer, not that the press was missed.

All buttons except the encoders and master dial are **single-colour**.

### 4.3 Encoder rings

```
B0 <0x30+n> <value>       mode NONE
B1 <0x30+n> <value>       mode PAN     — fills outward from centre
B2 <0x30+n> <value>       mode NORMAL  — fills from the left
```

**The MIDI channel selects the display mode**, and the value is **0–127** — not
the 11 positions the MCU convention implies. Use `B2` for level-style fills and
`B1` for pan.

### 4.4 Colour — encoders and master dial

```
91 <note> <r>             channel 2, red
92 <note> <g>             channel 3, green
93 <note> <b>             channel 4, blue — triggers the refresh
```

- `<note>` is the element's own button note: `0x20 + n` for encoders,
  `0x38` for the master dial.
- Components are **0–127**. Scale 8-bit colour by halving.
- **Blue must be sent last** — the ring only updates when it arrives.
- Note-on on **channel 1** at the same note is the ordinary LED on/off, so
  colour and lit-state are independent.

**Only 17 elements have RGB**: the 16 encoders and the master dial. Every other
button is a single-colour LED.

Colour is **preset-independent** and needs no provisioning — unlike the HID
colour path, which addresses configuration slots and reaches only elements the
Configurator has assigned.

**The firmware reclaims the LEDs.** An idle animation resumes when nothing is
driving the surface, so a host that paints a colour and stops will lose it. Keep
asserting, or find whatever disables the animation *(unknown; likely a config
setting)*.

### 4.5 Displays — native protocol

```
rows 0,1:   F0 00 00 66 14 1A <pos> <row+1> <12 chars> F7      pos = strip * 12
row 2:      F0 00 00 66 14 19 <pos> <8 chars>          F7      pos = strip * 8
track no:   F0 00 00 66 14 17 00 <8 bytes>             F7      one per strip
```

**Three rows of 12 / 12 / 8 characters — 32 per strip.** The row byte on `0x1a`
is **1-based on the wire** (`row + 1`): send `01` for the top row, `02` for the
middle.

Pad every write to the full field width. Text is ASCII.

### 4.6 Displays — MCU compatibility path

```
F0 00 00 66 14 12 <offset> <text> F7
```

Two rows of 56 characters: offset `0x00` upper, `0x38` lower, eight strips of
seven characters at offsets `0x00`, `0x07`, `0x0E`, `0x15`, `0x1C`, `0x23`,
`0x2A`, `0x31`.

**The buffer is flat, not per-field.** A write replaces only the bytes sent, so a
6-character label at offset 0 leaves character 7 holding whatever was there
before. **Always pad to exactly 7 characters.**

Prefer §4.5 unless you need MCU compatibility — it gives more than twice the
space and a separate number field.

### 4.7 Metering

```
D0 <(strip << 4) | level>     level 0..11
D0 <(strip << 4) | 0x0F>      resets peak hold
```

Asparion's own implementation rate-limits to **5 fps** and gates on transport
play. Higher rates work — 18 fps was smooth — but 5 is a sensible default.

---

## 5. Recipes

### 5.1 A channel strip

For strip *n* on the appropriate bank's port:

```
F0 00 00 66 14 1A <n*12> 01 "Kick In     " F7      name
F0 00 00 66 14 1A <n*12> 02 "-6.2 dB     " F7      value
F0 00 00 66 14 19 <n*8>     "GATE    "     F7      tag
F0 00 00 66 14 17 00 <8 track numbers>     F7      once per bank
E<n> <lsb> <msb>                                   fader position
91 <0x20+n> <r>  92 <0x20+n> <g>  93 <0x20+n> <b>  colour by channel type
B2 <0x30+n> <value>                                ring
```

### 5.2 Colour as a channel-type map

Colour is the one capability with no equivalent elsewhere in a mixing UI. On a
16-fader surface, colouring encoders by channel type — inputs, auxes, groups,
matrices — makes the layout readable at a glance in a way seven characters of
text cannot. It costs three MIDI messages per element, sent only when the layout
changes rather than per parameter update.

### 5.3 Double-click

Off by default; enabled **per button** by a checkbox in the Configurator. When
on, the button emits a *different note* on a double click — on `*`, F1 (`0x36`)
single and F2 (`0x37`) double.

**It is suppressive.** The single click is withheld until the gesture resolves,
so every single press on that button pays the detection window. Three regimes:

| Case | What is precise |
| --- | --- |
| Double-click **off** | the **down stroke** — real switch timing |
| Double-click **on**, deliberate press | the **release** — resolves early, real duration |
| Everything else | cooked by firmware, with latency |

**Bind on release** where timing matters, and use double-click for **escalation,
never alternation** — Panic → Hard Panic works; Go → Undo Go would fire the cue
and then revert it, because the single has already fired.

Keep it off anything where a late single click matters.

---

## 6. Gotchas

Ranked by how much they will cost you.

1. **Encoders are sign-magnitude.** `65` means −1, not −63.
2. **Never sweep undocumented SysEx command bytes.** A sweep of `0x10`–`0x7F`
   put the displays into a logo-only state and required a **full restart of the
   controller**. Only `0x12`, `0x17`, `0x19`, `0x1A`, `0x72`, `0x00` and `0x02`
   are established as safe.
3. **Never command full fader travel** — see §4.1.
4. **Pad every display write** to the field width, or inherit stale characters.
5. **Blue last** on colour, or the ring will not refresh.
6. **Pin the preset**, and say which one — see §1.2.
7. **Touch ≠ adjusting** — see §3.1.
8. **The surface has no local feedback** — echo LEDs yourself.
9. **Pace HID writes ~25 ms apart** if you use that interface; bursts stall the
   device. MIDI needs no such pacing.

---

## 7. What is not covered here

- **The vendor HID interface** (`VID_04D8`/`PID_E44E`, interface 0). It carries
  the raw surface protocol and 8-bit colour, but addresses configuration slots
  rather than physical positions and needs provisioning. The MIDI paths above are
  better in every respect except one bit of colour resolution. See the field
  notes, findings 24–35.
- **The OSC path via the Connector.** Works, but the template ships empty, the
  Connector holds the HID interface, and it does not exist on Linux, Android or
  iOS.
- **The configuration block.** 2048 bytes, and Asparion's published `.aPres`
  files are exactly that block in hex — so it can be studied offline with no
  hardware at risk. Whatever disables the idle animation is probably in there.
