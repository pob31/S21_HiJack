# Fader Sidecar

External MIDI control surfaces (motorized faders, endless encoders) bound to
console parameters or arbitrary outbound OSC targets. Added in v0.2.0.

## Architecture

- `src/model/sidecar.rs` — binding model (`ControlSelector`, `ControlMode`,
  `Taper`, `BindingTarget`, `SidecarConfig`) + the FaderDb taper (unity at
  ~3/4 travel, −inf detent; breakpoint table is an approximation of the S21
  law — calibrate there if A/B against the desk feels off).
- `src/console/sidecar_engine.rs` — device thread owning a midir
  input+output pair per port slot (two: a surface wider than eight strips,
  like the D700 rack or an X-Touch + Extender, has one port pair per bank).
  Bytes → `SurfaceEvent`s tagged with their port; `MotorMove` and raw `Send`
  → bytes on that port. 2 s port scans, unplug detection, auto-reconnect per
  port. Output auto-match pairs Windows' `MIDIIN2 (…)` with `MIDIOUT2 (…)`.
  Distinct from the trigger-output `midi_engine`.
- `src/console/sidecar_decode.rs` — pure decode: 14-bit CC pairing (30 ms
  window), relative encodings, pitch-bend deadband (±4 LSB, endpoints exempt).
- `src/console/sidecar_learn.rs` — debounced hardware capture (one
  accumulator per port) + phase machine. The learned control carries its port.
- `src/console/sidecar_display.rs` — pure: what each strip shows (channel
  name, value, parameter tag, colour) and the SysEx / note encodings.
- `src/console/sidecar_service.rs` — tokio runtime: taper → console via the
  shared `send_parameter` (optimistic mirror update, 15 ms/binding floor),
  generation-keyed motor poll (25 ms), triple echo protection (touch gate,
  `sent_to_console`, `sent_to_motor`), console-wins sync sweeps, display
  refresh (~10 Hz, diffed per row; colour re-asserted every second).
- `src/ui/sidecar_tab.rs` — device card, learn wizard, binding editor.

Persistence: binding table in the show file (`ShowFile::sidecar`, v18; each
binding's port slot and colour override and the display protocol, v21); MIDI
port names machine-bound (`AppPreferences::sidecar_midi`, port 2 in
`input_port_name_2` / `output_port_name_2`).

## Displays and colour

Off by default; picked per show in the device card (**Displays**). A strip is
a fader on pitch bend 1–8 or a relative V-pot on CC `0x10`–`0x17`, on the
binding's port; when both are bound, the fader's binding owns the strip.

| | Mackie Control (X-Touch) | Asparion D700 |
| --- | --- | --- |
| Row 1 | channel name, 7 chars (`0x12`) | channel name, 12 chars (`0x1A` row 1) |
| Row 2 | value, 7 chars | value, 12 chars (`0x1A` row 2) |
| Row 3 | — | parameter tag, 8 chars (`0x19`) |
| Colour | `0x72`, nearest of 8 by hue | `0x72`, then full RGB per encoder (notes on ch 2–4, blue last) |
| Device id | `0x14` port 1, `0x15` port 2 (Extender) | same; the D700 takes either |

The name is the channel's name from the console, or `In 12` / `Aux 3` until
the mirror has one. The desk's strip colours are hard-coded by channel type,
the same as its channel selector, and the app's channel pickers use them:
blue inputs, purple auxes, burgundy groups and masters, yellow control groups,
green matrices (`model::channel::*_COLOR`, one source for both). Those are
muted UI fills, so the LEDs get a hand-set full-strength version of each
(`ChannelId::led_color`): the picker's hue, except matrix green, which is pure
green because the picker hue reads teal on the D700. The 8-colour path picks
the nearest by hue, which keeps the desk's names (steel blue → blue, not
cyan). A binding's editor can override
its colour, which is then sent as picked. Switching the protocol or the
sidecar off blanks what was written.

Notes:
- TotalGain (fader + CG sum, read-only) is rejected as a binding target and
  never reaches learn (dropped in `connection::process_message`).
- A hardware move is an operator change. After sending it, the service runs
  `inbound::apply_operator_change`, the same chain as a move on the desk: it
  cancels a running fade on that parameter, propagates through gangs and pan
  link, marks the cell dirty and is recorded by macro learn. The console's
  echo can't do this: GP OSC doesn't echo, and the iPad link's echo is
  screened as our own write.
- Headless mode does not construct the sidecar (prefs aren't loaded there);
  see the comment near the MidiEngine construction in `main.rs`.

## Manual test script (X-Touch, MC mode, USB)

1. **Discovery** — plug in; the input combo lists the port within ~2 s.
   Select it: dot turns amber (enabled, console down) or green.
2. **Learn** — Learn… → move console CH12 fader → move X-Touch fader 1 →
   Confirm. Row appears: `PB ch1 (pitch bend) → Input 12 Fader`.
3. **Drive** — hardware fader moves console CH12 with unity at ~3/4 travel;
   console fader move drives the motor back. Watch the OSC log for loops
   (there must be none).
4. **Touch + recall** — recall a cue with a timed fade on CH12, then grab and
   move fader 1 while it runs: the fade releases CH12 (operator override) and
   the motor stays under the hand; release → motor snaps to console truth.
5. **Console wins** — rocker OFF; park the hardware fader somewhere silly;
   rocker ON → motor snaps back to the console value; the console never
   received the silly position.
6. **Replug** — unplug/replug USB mid-session → auto-reconnect + sync sweep.
7. **Encoder** — learn a V-pot onto a pan (relative, both directions);
   check direction + sensitivity; adjust "Full travel" ticks in the editor.
8. **Raw OSC** — bind a control to `nc -ul <port>`; verify the scaled float
   trails the fixed args.
9. **Show file** — save, reload, restart: bindings and port choice persist;
   a v17 show loads with the sidecar disabled and empty.

### Two ports and displays (D700 rack, Mackie preset)

10. **Ports** — Port 1 input `D 700`, Port 2 input `MIDIIN2 (D 700)`; both
    outputs on Auto connect (`D 700`, `MIDIOUT2 (D 700)`).
11. **Learn on bank 2** — learn fader 9: the row reads `P2 · PB ch1`. Moving
    fader 1 must not move fader 9's target, nor the reverse; motors follow
    on the right bank.
12. **Replug one bank** — pick port 2's input again (reconnect): port 1's
    faders must not twitch toward mid-travel.
13. **Displays** — Displays → Asparion D700: each bound strip shows name,
    value, tag; moving a fader updates the value row live; encoder rings take
    the channel-type colour and keep it (no idle animation). Rename a channel
    on the desk: the name follows. Displays → Off blanks the strips.
