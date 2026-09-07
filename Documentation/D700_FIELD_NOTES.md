# D700 Field Notes — provenance log for the Asparion D700 control surface

## Purpose

The fader sidecar (`Documentation/sidecar.md`) was written and tested against a Behringer
X-Touch in MC mode. Everything it believes about control surfaces — pitch-bend faders,
touch-sense notes, relative encoder encodings — is an MCU convention that each vendor is
free to bend. This file records what an Asparion D700 actually puts on the wire, so the
constants in `src/model/sidecar.rs` can be traced to evidence rather than to analogy.

The same discipline as `OSC_FIELD_NOTES.md` applies, and the same verification vocabulary:
**Confirmed on hardware**, **Documented**, **Reported, unverified**, **Assumed by analogy**,
**Unknown**.

## Hardware measured

| | |
| --- | --- |
| Device | Asparion D700 rack, 16 motorized faders, with display modules |
| Connection | USB to the operator's Windows 11 machine |
| Ports exposed | In: `D 700`, `MIDIIN2 (D 700)` · Out: `D 700`, `MIDIOUT2 (D 700)` |
| Measured | 2026-09-07, by a throwaway `midir` probe (not committed) |
| Sample | 3582 inbound events over a 60 s capture, plus TX tests |

## At a glance

| # | Finding | Status |
| --- | --- | --- |
| 1 | Faders transmit 14-bit pitch bend, channels 1–8 per port | Confirmed on hardware |
| 2 | Fader touch on notes `0x68`–`0x6F`, MIDI channel 1 | Confirmed on hardware |
| 3 | Encoders on CC `0x10`–`0x17`, **sign-magnitude** relative | Confirmed on hardware |
| 4 | Channel-strip buttons follow the standard MCU note map | Confirmed on hardware |
| 4a | Master section is fully MCU-conformant — no vendor notes | Confirmed on hardware |
| 4b | Volume knob transmits pitch bend ch 9 (MCU master fader) | Confirmed on hardware |
| 5 | Displays accept MCU LCD SysEx | Confirmed on hardware |
| 5a | **Two** addressable rows of 56 chars (`0x00` and `0x38`) | Confirmed on hardware |
| 5b | No third row reachable via command `0x12` | Confirmed on hardware (negative) |
| 5c | Command `0x18` also writes the display buffer | Confirmed on hardware |
| 5d | **Hazard:** sweeping unknown SysEx commands wedged the whole controller | Confirmed on hardware |
| 6 | Two port pairs = two banks of 8 faders (1–8, 9–16) | Confirmed on hardware |
| 6a | LCD is a linear 56-char buffer — short writes leave stale bytes | Confirmed on hardware |
| 6b | Button LEDs light only on host note-on echo | Confirmed on hardware |
| 6c | Encoder rings painted by host via CC `0x30`–`0x37` | Confirmed on hardware |
| 7 | Display honours device IDs `0x10`, `0x11`, `0x14`, `0x15` alike | Confirmed on hardware |
| 7a | **Colour is host-controllable** via SysEx `0x72`, per strip | Confirmed on hardware |
| 7b | Colour applies to the encoder rings (dials), not just the LCD | Confirmed on hardware |
| 7c | Only the low 3 bits carry colour; the field is 3-bit RGB | Confirmed on hardware |
| 7d | Master dial colour is **not** reachable via SysEx `0x72` | Confirmed on hardware (negative) |
| 8 | Findings 1–12 match the **Mackie** preset exactly | Confirmed on hardware |
| 8a | What the Universal preset sends (never captured) | Unknown, low priority |
| 13 | Reaper preset differs from it in exactly one control: `*` | Confirmed on hardware |
| 14 | Device implements the full MCU connection handshake | Confirmed on hardware |
| 15 | Bank 1 self-identifies as `0x14`, bank 2 as `0x15` | Confirmed on hardware |
| 17 | OSC mode carries master-section buttons only — no faders | Confirmed on hardware |
| 18 | OSC button messages are bare triggers, no arguments | Confirmed on hardware |

## Findings

### 1–2. Faders and touch — Confirmed on hardware

All eight faders on a port transmit pitch bend on MIDI channels 1–8, full 14-bit. Touch
sense arrives as note on/off `0x68`–`0x6F` on channel 1, one per fader, all eight observed.

This matches `mcu_default_touch_note` in `src/model/sidecar.rs` exactly — the D700 needs no
special casing for faders. Bank 2 numbers its own faders 1–8 locally, so pitch-bend channel
alone does not identify a physical fader; the port does.

### 3. Encoders are sign-magnitude — Confirmed on hardware

**This is the one that bites.** Observed CC values were `1, 2, 3, 4` clockwise and
`65, 66, 67, 68` counter-clockwise — bit 6 as the sign, magnitude in the low bits.

`RelativeMode::TwosComplement` is documented in the code as *"X-Touch V-pots, most MCU
pots"*, which makes it the natural pick in the learn wizard. It decodes `65` as `v - 128`
= **−63**. A single click counter-clockwise would move the target 63 steps the wrong way.

`RelativeMode::SignMagnitude` decodes it correctly as −1 and is already implemented. It
simply has to be the mode selected for this surface. Worth defaulting or flagging in the UI
when the bound port looks like a D700.

### 4. Button map — Confirmed on hardware (partial)

| Control | Notes (channel 1) | Observed |
| --- | --- | --- |
| Rec / Arm | `0x00`–`0x07` | `0x00`–`0x03` |
| Solo | `0x08`–`0x0F` | `0x08`–`0x09` |
| Mute | `0x10`–`0x17` | `0x10`–`0x13` |
| Select | `0x18`–`0x1F` | `0x19` |
| V-Pot press | `0x20`–`0x27` | all eight |
| Transport | `0x5B`–`0x5F` | `0x5D`–`0x5F` (Stop/Play/Record) |

Every button observed fell exactly where the standard MCU map predicts. The master section
was captured separately — see finding 9.

Note that the sidecar cannot currently bind any of these: `sidecar_learn.rs` discards note
events outright, and `is_valid_console_target` requires a continuous parameter, so Mute is
unreachable even by a hand-authored binding.

### 5. Displays speak MCU LCD SysEx — Confirmed on hardware

```
F0 00 00 66 <device_id> 12 <offset> <ascii...> F7
```

Row 1 is eight strips of seven characters at offsets `0x00`, `0x07`, `0x0E`, `0x15`, `0x1C`,
`0x23`, `0x2A`, `0x31`. Text written this way rendered on the display modules.

The display is a **linear 56-character buffer, not eight independent fields**. A write
only replaces the bytes actually sent, so a 6-character label at offset 0 leaves character 7
holding whatever was there before. This was observed directly: after writing `S21 HIJACK`
(which spans strips 1–2 as `S21 HIJ` + `ACK`) a later 6-char write of `PORT-1` rendered as
`PORT-1J`. **Always pad a strip write to the full 7 characters.**

A 16-fader rack drives its two display modules from separate ports: text sent to port 1
appeared on the left module and port 2 on the right, confirming finding 6 independently of
the device spec.

The sidecar has **no SysEx path at all** — `SidecarCmd` carries `MotorMove` and nothing
else — so lighting these up is new work, not a configuration change.

### 6. Two banks — Confirmed (device spec)

The rack carries 16 faders and exposes two MIDI port pairs; port 1 addresses faders 1–8 and
port 2 faders 9–16, each numbering locally. Standard MCU extender behaviour.

`SidecarMidiSettings` holds exactly one input and one output port name, and the engine owns
a single `midir` pair, so **S21_HiJack can reach only 8 of the 16 faders**. Supporting the
full surface means making the sidecar multi-device, not merely multi-port.

### 7. Display device IDs — Confirmed on hardware

A per-strip probe wrote `ID-<id>` to strip *n* using device id *n*, after clearing both rows.
All four rendered exactly:

| Device ID | Sent | Rendered |
| --- | --- | --- |
| `0x10` Logic Control | `ID-10` | `ID-10` |
| `0x11` Logic Control XT | `ID-11` | `ID-11` |
| `0x14` Mackie Control | `ID-14` | `ID-14` |
| `0x15` Mackie Control XT | `ID-15` | `ID-15` |

The D700 accepts any of the four interchangeably, so the device-ID byte needs no discovery
step and no per-unit configuration. **Use `0x14`** — the canonical Mackie Control id — for
no reason other than that it is the least surprising choice to a reader.

### 8. Active preset — Unknown

The Configurator screenshot taken before the session showed preset **1 Universal** active,
not **2 Mackie**, and it was not confirmed whether the preset changed before the capture.
Every byte observed was MCU-conformant, which suggests either that Universal is
MCU-compatible or that the preset was switched. This matters for reproducibility: the note
and CC maps above are only valid for whichever preset was live. Confirm before relying on
them.

### 6b–6c. The surface has no local feedback — Confirmed on hardware

Pressing a button transmits a note-on and does nothing else: the LED stays dark until the
host echoes a note-on back at the same note number. Encoder rings behave the same way — the
host paints them with CC `0x30`–`0x37`, value `(mode << 4) | position`, position 1..11,
bit 6 lighting the centre LED. Ring modes offered are 0 single dot, 1 boost/cut from centre,
2 wrap from left, 3 spread.

Both were confirmed by driving them from the probe: button LEDs lit in banks and the rings
animated through all four modes. The operator sees nothing without this echo, which makes it
a functional requirement of button support rather than a refinement.

### 9. Master section — Confirmed on hardware

A dedicated 90 s capture of the right-hand section produced **zero unmapped note numbers and
zero CC events**. Every control landed on a note the standard MCU map already defines, which
is unusual: the master section is normally where vendors improvise.

| Physical control | Note | MCU meaning |
| --- | --- | --- |
| Pan | `0x2A` | Assign Pan |
| EQ | `0x2C` | Assign EQ |
| Send | `0x29` | Assign Send |
| FX | `0x2B` | Assign Plug-In |
| `*` | `0x36` | F1 |
| Icon button 1 | `0x59` | Click |
| Icon button 2 | `0x56` | Cycle |
| Record | `0x5F` | Record |
| Play | `0x5E` | Play |
| Stop | `0x5D` | Stop |
| Arrow left | `0x2E` | Bank Left |
| Arrow right | `0x2F` | Bank Right |
| Knob press | `0x38` | F3 |
| Volume knob | **PB ch 9** | Master fader |

The four labelled buttons align semantically with their MCU meanings — Pan/EQ/Send/FX map to
Assign Pan/EQ/Send/Plug-In — so their intent needs no invention.

**Caveat on attribution.** The note numbers are confirmed; *which physical button produced
which note* is reconstructed from the order the operator pressed them during the capture, not
observed directly. The four labelled buttons and the transport are unambiguous. The `*`,
icon-button and knob-press rows (`0x36`, `0x59`, `0x56`, `0x38`) rest on press order alone —
re-press individually to confirm before relying on them.

Absent, and therefore unavailable on this unit: no jog wheel (`CC 0x3C` never seen), no
rewind or fast-forward (`0x5B`, `0x5C`), and **no master-fader touch** (`0x70`). That last
one matters: `mcu_default_touch_note` returns `0x70` for pitch-bend channel 9, so binding the
volume knob would install a touch gate that can never fire. Harmless, but it means the knob
gets no touch protection.

The volume knob transmitting absolute 14-bit pitch bend rather than relative CC makes it a
legal sidecar target today — it looks exactly like a fader to the existing decode path.

### 10. Colour is host-controllable — Confirmed on hardware

MCU defines no colour protocol at all, so this is a vendor extension — the same one Behringer
uses on the X-Touch, and the D700 implements it:

```
F0 00 00 66 <device_id> 72 <c1> <c2> <c3> <c4> <c5> <c6> <c7> <c8> F7
```

Eight colour bytes, one per strip, applied together. Confirmed values:

| Value | Colour |
| --- | --- |
| 0 | black / off |
| 1 | red |
| 2 | green |
| 3 | yellow |
| 4 | blue |
| 5 | magenta |
| 6 | cyan |
| 7 | white |

Each strip takes its own value — a rainbow across the eight rendered correctly, as did
setting all eight to one colour in turn.

The value→colour mapping above was verified directly rather than from recollection: each
strip was labelled with the name of the colour it was simultaneously being set to, and the
operator confirmed every strip matched its own label. This rules out the plausible failure
mode of a reversed bit order (which would swap red↔blue and yellow↔cyan while leaving green,
magenta, white and black looking correct).

**The colour reaches the encoder rings, not only the LCD backlight.** This was the operator's
own observation: the knob colours changed together with the strips. That makes colour a
surface-wide visual channel rather than a display detail.

**The field is 3-bit RGB, not a lookup table.** Bit 0 is red, bit 1 green, bit 2 blue, which
is why the order runs black, red, green, yellow (r+g), blue, magenta (r+b), cyan (g+b),
white (r+g+b). Useful because a colour can be composed rather than looked up.

**Values above 7 yield no further colours.** Walking 0–31 across the strips gave:

| Value range | Bits 4,3 | Rendered |
| --- | --- | --- |
| 0–7 | `00` | the eight colours |
| 8–15 | `01` | black |
| 16–23 | `10` | black |
| 24–31 | `11` | white |

So the vocabulary is exactly eight and **code should mask to `& 0x07`**. The X-Touch uses the
spare bits for row inversion; the D700 does not reproduce that behaviour — the out-of-range
values collapse to black or white rather than inverting. No reason to send anything above 7.

### 11. The two banks are independently addressable — Confirmed on hardware

Writing different text to each output port put `S21` across all eight strips of the left
module and `HIJACK` across the right. Combined with finding 6, the two ports are fully
independent surfaces for both input and output: separate faders, separate displays, separate
LEDs. Nothing is mirrored.

The operator's description of a full-surface chase — faders, LEDs, rings and displays driven
together across both banks — as "super responsive" is worth recording: there is no observed
need to pace or throttle output to this surface, unlike the console's ARM chip.

### 12. The master dial takes no colour — Confirmed on hardware (negative)

`0x72` defines eight colour bytes, one per strip. The obvious extension hypothesis — that
Asparion added a ninth byte for the master dial — was tested and **disproved**:

- 16 colour bytes, all red: strips 1–8 turned red, nothing else on the panel changed.
- Strips held black while a single byte 9…16 was set green, one at a time, on both ports:
  the master dial never lit.

A second pass widened the search and also came back empty:

| Probe | Result |
| --- | --- |
| `0x72` with 9–16 colour bytes, each later byte isolated | No effect beyond strip 8 |
| Ring CCs `0x38`–`0x3F` (above the eight strip rings) | No effect |
| SysEx command bytes `0x70`–`0x7F`, nine colour bytes each | No effect |
| `0x72` with 9 and 16 bytes **after completing the MCU handshake** | No effect |

**What is ruled out:** the vendor-extension region around the known colour command, the
ring-CC range immediately above the eight strips, and the hypothesis that the surface gates
colour behind a live host connection — the handshake was completed and accepted (finding 14)
and the master dial still did not respond.

**What is not ruled out:** the low MCU command range (`0x00`–`0x6F`). That was deliberately
left unswept — it contains "go offline" (`0x0F`) and the fader/LED/global reset commands
(`0x61`–`0x63`), and sweeping blind through those to hunt for a colour command is a bad
trade. It also remains possible the master dial's colour is a Configurator-only preference;
the Configurator has a **Colors** tab, so the hardware can certainly colour it locally.

**Recommendation: stop probing.** Asparion's MIDI implementation chart, or their support,
will answer this in minutes. If the answer is "Configurator only", that is very likely
sufficient — the master dial is one fixed control that does not change meaning as banks
scroll, while the 16 strips that *do* need to track console state are already colourable.

Incidental robustness finding: the D700 accepted a 16-byte payload on an 8-byte command
without garbling the display or hanging. Over-length SysEx is ignored, not misparsed.

### 13. Reaper preset — Confirmed on hardware

The operator switched the Configurator to preset **12 Reaper** and the full control sweep was
repeated. Everything below is byte-identical to the earlier preset:

- Faders: pitch bend ch 1–8, touch notes `0x68`+
- Encoders: CC `0x10`–`0x17`, **sign-magnitude** (values 1, 2, 65, 66 observed — same encoding)
- Channel buttons: Rec `0x00`, Solo `0x08`, Mute `0x10`, Select `0x18`, V-Pot press `0x20`
- Pan `0x2A`, EQ `0x2C`, Send `0x29`, FX `0x2B`
- Icon buttons `0x59` / `0x56`, transport `0x5D`–`0x5F`, arrows `0x2E` / `0x2F`
- Knob press `0x38`, volume knob pitch bend ch 9

**Exactly one control moved:**

| Control | Earlier preset | Reaper preset |
| --- | --- | --- |
| `*` | `0x36` (F1) | `0x5A` (Solo, global) |

This is the important result, and it is more useful than a wholesale remap would have been.
The presets are *almost* interchangeable, which is precisely the condition under which a
silent failure hides: a binding learned under one preset keeps working for every control
except one, and that one quietly addresses something else.

**Therefore: pin a recommended preset rather than auto-detecting.** A surface that is 97%
identical across presets cannot be reliably fingerprinted from traffic, and the cost of
guessing wrong is one mis-bound button rather than an obvious failure. Document the expected
preset and let the operator match it.

### 8. Preset provenance — Confirmed on hardware

The operator switched to preset **2 Mackie** and the sweep was repeated. The `*` button sent
`0x36`, matching findings 1–12 exactly, with `0x5A` absent. Every other control also matched:
encoders on CC `0x10` (values 1, 2, 3, 65, 66, 67 — sign-magnitude), Mute `0x10`, Solo `0x08`,
Select `0x18`, Rec `0x00`, V-Pot press `0x20`, Play `0x5E`, Stop `0x5D`, fader touch `0x68`+,
faders PB ch 1, volume knob PB ch 9.

**Findings 1–12 are therefore the Mackie map.** Strictly this does not prove the session
*opened* on Mackie rather than Universal — Universal could emit the same bytes — but the
distinction stops mattering: the recorded map is confirmed to be what Mackie produces, and
Mackie is the preset to recommend.

### Preset comparison

| Control | Mackie | Reaper | Universal |
| --- | --- | --- | --- |
| `*` | `0x36` (F1) | `0x5A` (Solo, global) | not captured |
| everything else | identical | identical | not captured |

Only the `*` button is known to move between presets. The Universal preset was never captured
(finding 8a) and is low priority — there is no reason to prefer it over Mackie, which is now
a measured reference.

### 16. Display rows, and a hazard — Confirmed on hardware

**There are two addressable rows, not one.** Offset `0x00` writes the upper row and `0x38`
the lower, 56 characters each — 112 per bank, 224 across a 16-fader rack. Everything earlier
in this file used only the upper row; the lower one works identically and was simply never
tried. For a scribble strip this is the difference between showing a channel name *or* its
value and showing both, which is the whole point of the strip.

**No third row via `0x12`.** The command addresses a 128-character buffer (SysEx data bytes
are 7-bit), and two rows of 56 consume 112 of it. Writes to offsets `0x70`–`0x7F` produced
nothing visible. The display modules physically show more than two lines, so those extra
lines exist — they are simply not reachable through this command.

**Command `0x18` also writes the display buffer**, landing in the same place as `0x12`. Its
full behaviour is not characterised.

### ⚠ Hazard: do not sweep unknown SysEx command bytes

A sweep of command bytes `0x10`–`0x7F` (excluding the known-destructive `0x0A`–`0x0F` and
`0x61`–`0x63`) **put the display modules into a logo-only state and required a full restart of
the controller to recover.** Replugging the display modules alone was not enough.

No configuration was lost and nothing persisted across the restart, so the damage was
recoverable — but it took the surface out of service, which on a show day would be
unacceptable.

The exclusions were not sufficient. Some command in the swept range takes the displays out of
host-write mode, and identifying which would mean repeating the experiment — i.e. wedging the
controller again. Not worth it. Treat the entire undocumented command space as hazardous on
this hardware.

**Only four commands are established as safe here:** `0x12` (write text), `0x72` (set colour),
`0x00` (device query) and `0x02` (handshake reply). Everything in this document was established
using those four plus ordinary channel-voice messages. Use nothing else.

When something beyond them is needed, ask Asparion rather than probing. This is the third
finding pointing that way — after the master dial colour and the extra display lines — and it
is the one that cost hardware downtime.

### 17–18. OSC mode via the Asparion Connector — Confirmed on hardware

The Connector app bridges the D700 to OSC over UDP. Configured at `127.0.0.1`, Rx 7000
(commands in), Tx 7001 (surface events out).

**What it emits — the complete set observed:**

| Address | Args |
| --- | --- |
| `/play` | none |
| `/stop` | none |
| `/record` | none |
| `/click` | none |
| `/repeat` | none |
| `/device/track/bank/-` | none |
| `/device/track/bank/+` | none |

**What it does not emit: anything continuous.** Separate captures of faders, encoders and the
volume knob in isolation produced **zero packets**. A hands-off control capture also produced
zero, confirming the Connector transmits only on user action rather than on a timer — so the
silence during fader moves is a real negative, not a missed window.

OSC mode is therefore a **transport remote, not a control surface**. It cannot carry a fader
position, so it cannot replace the MIDI path for the sidecar's core job.

**But the two protocols cover each other's gaps exactly.** Faders and encoders work over MIDI
and are unavailable over OSC; buttons emit clean semantic OSC addresses and are the one thing
the MIDI path cannot bind, because `sidecar_learn.rs` discards note events.

**And the message shapes already match.** S21_HiJack's trigger listener accepts `/cue/go`,
`/cue/previous` and `/cue/current` as bare argument-less messages — precisely the form the
D700's buttons take. Only the address *names* differ. Two consequences:

- If the Connector's address map is **editable**, mapping Play → `/cue/go` and
  Stop → `/cue/previous` makes the D700 fire cues today with **no code at all**, by pointing
  the Connector's Tx at the app's trigger port.
- If it is **fixed**, an alias table in `parse_trigger_message` is a few lines and achieves
  the same thing.

Either way this is the cheapest route to a working button on this surface — cheaper than the
learn-plus-discrete-target-plus-LED-echo work the MIDI path needs, though it does not replace
it (OSC gives no LED feedback, so the button stays dark).

**Open question:** whether the Connector's OSC address map is user-configurable. That decides
between "no code" and "a few lines", and it is a look at the Connector's UI.

## Implications for the sidecar

1. **Encoder default.** Bind a D700 encoder with the obvious `TwosComplement` and it will
   fly 63 steps on one click. Fix by defaulting to `SignMagnitude` for this surface.
2. **Buttons.** The note map is known, but button support is a three-part job, not two: a
   learn path that stops discarding notes, a discrete-target path so Mute/Solo become
   bindable, **and** an LED echo driven from console state. Without the echo the operator
   presses Mute and gets no confirmation the desk heard them — unusable on a live surface.
3. **Second bank.** Half the operator's faders are unaddressable until the sidecar can hold
   more than one device.
3a. **Free real estate.** `0x36`, `0x38`, `0x56` and `0x59` (F1, F3, Cycle, Click) carry no
   meaning on a mixing console. They are the natural home for functions with no DAW
   equivalent — cue Go, snapshot fire, gang enable — once buttons are bindable at all.
4. **Displays, rings and colour.** All confirmed; what is missing is an outbound path. Note
   the display gives **two** rows per strip (finding 16) — enough for name over value.
   `SidecarCmd` carries `MotorMove` and nothing else, so text, LEDs, rings and colour all
   need new command variants. Rings are the cheapest — same "console state changed → push to
   hardware" machinery as motor feedback, just a different message.
5. **Pin the preset.** Findings 13 shows presets differ in at least one control while looking
   otherwise identical. Whatever S21_HiJack ends up recommending, it should say so explicitly
   in the sidecar docs, because the failure mode is one silently mis-bound button.
6. **Colour as a channel-type map.** Colour is the one capability here with no equivalent
   anywhere else in the application. Colouring strips by console channel type — inputs,
   auxes, groups, matrices — makes a 16-fader surface readable at a glance in a way seven
   characters of text cannot. It costs one SysEx per bank on layout change, not per parameter
   update, so it is nearly free at runtime.
