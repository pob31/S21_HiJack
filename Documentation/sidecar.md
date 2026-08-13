# Fader Sidecar

External MIDI control surfaces (motorized faders, endless encoders) bound to
console parameters or arbitrary outbound OSC targets. Added in v0.2.0.

## Architecture

- `src/model/sidecar.rs` — binding model (`ControlSelector`, `ControlMode`,
  `Taper`, `BindingTarget`, `SidecarConfig`) + the FaderDb taper (unity at
  ~3/4 travel, −inf detent; breakpoint table is an approximation of the S21
  law — calibrate there if A/B against the desk feels off).
- `src/console/sidecar_engine.rs` — device thread owning the midir
  input+output pair. Bytes → `HwEvent`s; `MotorMove` → bytes. 2 s port scans,
  unplug detection, auto-reconnect. Distinct from the trigger-output
  `midi_engine`.
- `src/console/sidecar_decode.rs` — pure decode: 14-bit CC pairing (30 ms
  window), relative encodings, pitch-bend deadband (±4 LSB, endpoints exempt).
- `src/console/sidecar_learn.rs` — debounced hardware capture + phase machine.
- `src/console/sidecar_service.rs` — tokio runtime: taper → console via the
  shared `send_parameter` (optimistic mirror update, 15 ms/binding floor),
  generation-keyed motor poll (25 ms), triple echo protection (touch gate,
  `sent_to_console`, `sent_to_motor`), console-wins sync sweeps.
- `src/ui/sidecar_tab.rs` — device card, learn wizard, binding editor.

Persistence: binding table in the show file (`ShowFile::sidecar`, v18);
MIDI port choice machine-bound (`AppPreferences::sidecar_midi`).

Notes:
- TotalGain (fader + CG sum, read-only) is rejected as a binding target and
  never reaches learn (dropped in `connection::process_message`).
- A hardware move during a timed recall registers as an operator override via
  the existing `automation_registry` echo path — no special wiring.
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
4. **Touch + recall** — hold fader 1 while recalling a cue with a timed fade
   on CH12: the fade releases CH12 (operator override), the motor stays under
   the hand; release → motor snaps to console truth.
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
