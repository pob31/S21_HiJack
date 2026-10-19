# DiGiCo SD range — iPad V2 OSC command set

The complete OSC command set that an SD8/9/11/12 console exposes to a
**DiGiCo Pad** External Control device loaded with the **V2** command set,
with each path checked against a running desk where possible.

Machine-readable version: `DiGiCo SD iPad V2 OSC Commandset.csv` (same rows).

## Sources

- **Command-set file** `IPADV2sd8-9-11-12.dosc`, shipped with the SD
  software (`C:\SD12\` on the dev machine). Header
  `DiGiCo OSC Commands 1502`. It holds 778 command records. Each one gives the
  section (strip type or console area), an internal **parameter ID + index**,
  the value type, min/max, display units, and the command name with spaces.
  It does **not** hold the OSC folder structure.
- **Live desk**: SD12 offline software v22.0.2242 with a 90-input session,
  queried on 2026-10-19. Paths come from the desk's own replies.

The parameter IDs are the same IDs used in the SD `.ses` session-file object
database. See `SESSION_FILES_FIELD_NOTES.md` §2.3: for example `0405[0]` is
EQ band 1 frequency in both places.

## Status column

| Status | Meaning |
|---|---|
| verified (strip dump) | The path came back in a full-strip dump from the desk. |
| verified (exact query) | `<path>/?` was answered with exactly that path. |
| verified (query) | A console-level `…/?` query was answered (reply shown). |
| from command set (not sent) | An action or setter from the command set. The path is the section prefix + the name with `_` for spaces, matching every verified console path. These were **not sent**, because they change the session (fire/update/delete snapshot, save…). |
| no reply to query | Sent, no reply from this desk/session. |
| not answered | In the command set, but the desk never answered it. |
| prefix unknown | Console section whose OSC prefix could not be established. |

`{n}` = strip number (1-based), `{k}` = send/bus number (1-based).
"Value seen" = what the test session returned for strip 1. Text values are
replaced by `<text>`.

## Connecting

1. On the desk: **Master Screen → Setup → External Control** → enable →
   **Add Device → DiGiCo Pad** → IP of the controlling machine. Set desk
   **Send 9000 / Receive 8000**, tick **Enable**, choose the V2 command set and
   press **Load**.
2. Send OSC/UDP to the desk on port **8000** and listen on **9000**. The desk
   sends from an ephemeral source port.

## Wire conventions (observed)

- **Query**: append `/?` to any path. The reply is the same path with the
  current value.
- **`/<Strip>/{n}/name/?` returns the whole strip** for inputs, auxes, groups
  and matrix outputs (205, 146, 208 and 143 messages, meters included).
  Control groups, GEQs and solos answer with the name only, so query their
  parameters one by one.
- **Unknown paths fall back to a strip dump.** Asking for a path the desk does
  not know, under a valid strip, returns that whole strip. A reply therefore
  only confirms a path when the reply path equals the requested path.
- `/Matrix_Inputs/{n}/…` (anything except `Matrix_Send`) falls back to the
  dump of **`/Matrix_Outputs/{n}`**.
- **Value types**:
  - Continuous parameters are float32 in engineering units: dB, Hz, seconds,
    ratio.
  - Booleans and enums are also sent as float (`0.0` / `1.0` / `2.0` …).
  - Names are OSC strings. Meters are blobs.
- **Pan and send pan travel as 0…1 (0.5 = centre)**, even though the command
  set declares −1…+1. The `.ses` file stores −1…+1, so file = wire × 2 − 1
  (wire 0.3454 ↔ file −0.3092).
- **Filter frequencies on the wire are the displayed values.** The `.ses` file
  stores them scaled: low cut displayed = file × 1.25, high cut displayed =
  file × 0.8 (confirmed on screen).
- **Push: yes.** The desk sends changes made on its own surface, unasked, to
  the Pad device (port 9000), using the same paths and values as query
  replies. Observed on the SD12 offline software (not yet on hardware):
  - fader and EQ gain stream about every 30 ms while being dragged
    (`/Input_Channels/1/fader`, `/Input_Channels/3/EQ/eq_gain_2`);
  - a channel mute sends one message per click (`/Input_Channels/10/mute`);
  - control-group mutes send `/Control_Groups/{n}/mute` and the member
    channels' `/Input_Channels/{n}/CGs_mute`.
  With nothing touched, the desk sends nothing, not even meters.
- **Mute semantics**: `…/mute` is the channel's own MUTE button.
  `…/CGs_mute` = 1 means the channel is muted *through a control group* (the
  strip's "CG mute" indicator). Both are checked against the screen.

## Reply shapes for list queries (observed)

| Reply path | Arguments |
|---|---|
| `/Console/Input_Channels` etc. (to `/Console/Channels/?`) | `count` per strip type: Input, Aux, Group, Talkback, Control Group, Matrix In, Matrix Out, Graphic EQ, Multis |
| `/Console/<Type>/modes` | one int per strip: 1 = mono, 2 = stereo |
| `/Console/Input_Channels/{n}/group_sends` | one int per group |
| `/Console/Matrix_Inputs/{n}/send_levels` · `send_ons` | one value per matrix output |
| `/Snapshots/name` | `index, number × 100, group, name` |
| `/Snapshots/notes` | `index, font size, colour (0xRRGGBB), ?, text` (text is CP1252) |
| `/Macros/name` | `index, name` |
| `/Presets/name` | `index, ?, ?, category, group, name` |
| `/Layout/Layout/Banks` | `bank name, side, ?, position, strip type, first strip` |

## Gaps

- Listed in the command set but **not answered** by v22.0.2242: `gate range`,
  `gate hold`, `gate-duck-comp`, `gate meter` (all strip types); `Solo`
  fader/mute/solo; Talkback name; strip type `0x0b` name.
- **Prefix unknown**: `Save Session` / `Save current Session` (section
  `0xc9`), `Snapshot Notes view` (`0xcc`), `clear` / `request` / `values`
  (`0xcf`, probably meter subscription).
- The older V1 set (`IPAD SD8-9-11-12.dosc`) is not covered here.

## Commands

### Input_Channels (163)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `/Input_Channels/{n}/Channel_Input/trim` | trim | `0101[0]` | float | -40 … 60 | dB | verified (strip dump) | -2.54 |
| `/Input_Channels/{n}/Channel_Input/phase` | phase | `0102[0]` | int | 0 … 3 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Channel_Input/main/alt_in` | main/alt in | `010e[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Channel_Input/stereo_mode` | stereo mode | `011a[0]` | int | 1 … 2 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/Channel_Input/pre_meter/left` | pre meter/left | `0120[0]` | meter |  |  | verified (strip dump) | <text> |
| `/Input_Channels/{n}/Channel_Input/pre_meter/right` | pre meter/right | `0124[0]` | meter |  |  | verified (strip dump) | <text> |
| `/Input_Channels/{n}/Channel_Input/post_meter/left` | post meter/left | `0130[0]` | meter |  |  | verified (strip dump) | [] |
| `/Input_Channels/{n}/Channel_Input/post_meter/right` | post meter/right | `0134[0]` | meter |  |  | verified (strip dump) | [] |
| `/Input_Channels/{n}/Channel_Input/phantom` | phantom | `0183[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Channel_Input/alt_phantom` | alt phantom | `0183[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Channel_Input/analog_gain` | analog gain | `0186[0]` | float | -20 … 60 | dB | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Channel_Input/alt_analog_gain` | alt analog gain | `0186[1]` | float | -20 … 60 | dB | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Channel_Input/input_pad` | input pad | `0189[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Channel_Input/alt_input_pad` | alt input pad | `0189[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Channel_Input/input_type` | input type | `01e0[0]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Channel_Input/name` | name | `01ff[0]` | string |  |  | verified (strip dump) | <text> |
| `/Input_Channels/{n}/Aux_Send/{k}/send_level` | send level | `0201[0]` | float | -150 … 10 | dB, % | verified (strip dump) | -150 |
| `/Input_Channels/{n}/Aux_Send/{k}/send_on` | send on | `0202[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Aux_Send/{k}/send_pan` | send pan | `0203[0]` | float | -1 … 1 | % | verified (strip dump) | 0.5 |
| `/Input_Channels/{n}/Group_Send/{k}/group` | group | `0309[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/EQ/eq_in` | eq in | `0402[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/EQ/eq_on_1` | eq on 1 | `0403[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/EQ/eq_on_2` | eq on 2 | `0403[1]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/EQ/eq_on_3` | eq on 3 | `0403[2]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/EQ/eq_on_4` | eq on 4 | `0403[3]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/EQ/eq_curve_1` | eq curve 1 | `0404[0]` | int | 1 … 4 |  | verified (strip dump) | 4 |
| `/Input_Channels/{n}/EQ/eq_curve_2` | eq curve 2 | `0404[1]` | int | 1 … 4 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/EQ/eq_curve_3` | eq curve 3 | `0404[2]` | int | 1 … 4 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/EQ/eq_curve_4` | eq curve 4 | `0404[3]` | int | 1 … 4 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/EQ/eq_freq_1` | eq freq 1 | `0405[0]` | float | 20 … 20000 | Hz | verified (strip dump) | 2768 |
| `/Input_Channels/{n}/EQ/eq_freq_2` | eq freq 2 | `0405[1]` | float | 20 … 20000 | Hz | verified (strip dump) | 2688 |
| `/Input_Channels/{n}/EQ/eq_freq_3` | eq freq 3 | `0405[2]` | float | 20 … 20000 | Hz | verified (strip dump) | 427 |
| `/Input_Channels/{n}/EQ/eq_freq_4` | eq freq 4 | `0405[3]` | float | 20 … 20000 | Hz | verified (strip dump) | 438.7 |
| `/Input_Channels/{n}/EQ/eq_Q_1` | eq Q 1 | `0406[0]` | float | 0.1 … 20 |  | verified (strip dump) | 0.5792 |
| `/Input_Channels/{n}/EQ/eq_Q_2` | eq Q 2 | `0406[1]` | float | 0.1 … 20 |  | verified (strip dump) | 0.71 |
| `/Input_Channels/{n}/EQ/eq_Q_3` | eq Q 3 | `0406[2]` | float | 0.1 … 20 |  | verified (strip dump) | 2.018 |
| `/Input_Channels/{n}/EQ/eq_Q_4` | eq Q 4 | `0406[3]` | float | 0.1 … 20 |  | verified (strip dump) | 0.7768 |
| `/Input_Channels/{n}/EQ/eq_gain_1` | eq gain 1 | `0407[0]` | float | -18 … 18 | dB | verified (strip dump) | -1.341 |
| `/Input_Channels/{n}/EQ/eq_gain_2` | eq gain 2 | `0407[1]` | float | -18 … 18 | dB | verified (strip dump) | 1.341 |
| `/Input_Channels/{n}/EQ/eq_gain_3` | eq gain 3 | `0407[2]` | float | -18 … 18 | dB | verified (strip dump) | 0.9176 |
| `/Input_Channels/{n}/EQ/eq_gain_4` | eq gain 4 | `0407[3]` | float | -18 … 18 | dB | verified (strip dump) | -7.129 |
| `/Input_Channels/{n}/EQ/eq_over-under_1` | eq over-under 1 | `040b[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/EQ/eq_over-under_2` | eq over-under 2 | `040b[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/EQ/eq_over-under_3` | eq over-under 3 | `040b[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/EQ/eq_over-under_4` | eq over-under 4 | `040b[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/GR_meter_1 or /Input_Channels/{n}/EQ/GR_meter_1` | GR meter 1 | `040c[0]` | meter |  |  | verified (strip dump) | <text> |
| `/Input_Channels/{n}/Dynamics/GR_meter_2 or /Input_Channels/{n}/EQ/GR_meter_2` | GR meter 2 | `040c[1]` | meter |  |  | verified (strip dump) | <text> |
| `/Input_Channels/{n}/Dynamics/GR_meter_3 or /Input_Channels/{n}/EQ/GR_meter_3` | GR meter 3 | `040c[2]` | meter |  |  | verified (strip dump) | <text> |
| `/Input_Channels/{n}/Dynamics/GR_meter_4 or /Input_Channels/{n}/EQ/GR_meter_4` | GR meter 4 | `040c[3]` | meter |  |  | verified (strip dump) | <text> |
| `/Input_Channels/{n}/EQ/eq_thresh_1` | eq thresh 1 | `040e[0]` | float | -60 … 0 | dB | verified (strip dump) | -36 |
| `/Input_Channels/{n}/EQ/eq_thresh_2` | eq thresh 2 | `040e[1]` | float | -60 … 0 | dB | verified (strip dump) | -36 |
| `/Input_Channels/{n}/EQ/eq_thresh_3` | eq thresh 3 | `040e[2]` | float | -60 … 0 | dB | verified (strip dump) | -36 |
| `/Input_Channels/{n}/EQ/eq_thresh_4` | eq thresh 4 | `040e[3]` | float | -60 … 0 | dB | verified (strip dump) | -36 |
| `/Input_Channels/{n}/EQ/eq_ratio_1` | eq ratio 1 | `0410[0]` | float | 1 … 10 | : 1 | verified (strip dump) | 2 |
| `/Input_Channels/{n}/EQ/eq_ratio_2` | eq ratio 2 | `0410[1]` | float | 1 … 10 | : 1 | verified (strip dump) | 2 |
| `/Input_Channels/{n}/EQ/eq_ratio_3` | eq ratio 3 | `0410[2]` | float | 1 … 10 | : 1 | verified (strip dump) | 2 |
| `/Input_Channels/{n}/EQ/eq_ratio_4` | eq ratio 4 | `0410[3]` | float | 1 … 10 | : 1 | verified (strip dump) | 2 |
| `/Input_Channels/{n}/EQ/eq_attack_1` | eq attack 1 | `0411[0]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.01 |
| `/Input_Channels/{n}/EQ/eq_attack_2` | eq attack 2 | `0411[1]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.015 |
| `/Input_Channels/{n}/EQ/eq_attack_3` | eq attack 3 | `0411[2]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.025 |
| `/Input_Channels/{n}/EQ/eq_attack_4` | eq attack 4 | `0411[3]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.05 |
| `/Input_Channels/{n}/EQ/eq_release_1` | eq release 1 | `0412[0]` | float | 0.01 … 10 | s | verified (strip dump) | 0.3 |
| `/Input_Channels/{n}/EQ/eq_release_2` | eq release 2 | `0412[1]` | float | 0.01 … 10 | s | verified (strip dump) | 0.35 |
| `/Input_Channels/{n}/EQ/eq_release_3` | eq release 3 | `0412[2]` | float | 0.01 … 10 | s | verified (strip dump) | 0.4 |
| `/Input_Channels/{n}/EQ/eq_release_4` | eq release 4 | `0412[3]` | float | 0.01 … 10 | s | verified (strip dump) | 0.6 |
| `/Input_Channels/{n}/EQ/eq_symm_Q_1` | eq symm Q 1 | `047c[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/EQ/eq_symm_Q_2` | eq symm Q 2 | `047c[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/EQ/eq_symm_Q_3` | eq symm Q 3 | `047c[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/EQ/eq_symm_Q_4` | eq symm Q 4 | `047c[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/EQ/dynamic_eq_on_1` | dynamic eq on 1 | `047d[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/EQ/dynamic_eq_on_2` | dynamic eq on 2 | `047d[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/EQ/dynamic_eq_on_3` | dynamic eq on 3 | `047d[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/EQ/dynamic_eq_on_4` | dynamic eq on 4 | `047d[3]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/Dynamics/comp_in` | comp in | `0502[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/Dynamics/gate_in` | gate in | `0502[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_band_in_1` | comp band in 1 | `0503[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/Dynamics/comp_band_in_2` | comp band in 2 | `0503[1]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/Dynamics/comp_band_in_3` | comp band in 3 | `0503[2]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/Dynamics/comp_LP_crossover` | comp LP crossover | `0505[0]` | float | 20 … 20000 | Hz | verified (strip dump) | 130 |
| `/Input_Channels/{n}/Dynamics/comp_HP_crossover` | comp HP crossover | `0505[1]` | float | 20 … 20000 | Hz | verified (strip dump) | 1000 |
| `/Input_Channels/{n}/Dynamics/comp_gain_1` | comp gain 1 | `0509[0]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_gain_2` | comp gain 2 | `0509[1]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_gain_3` | comp gain 3 | `0509[2]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_gain_4` | comp gain 4 | `0509[3]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `—` | gate range | `050a[3]` | float | 0 … 90 | dB | not answered |  |
| `—` | gate-duck-comp | `050b[3]` | int | 0 … 2 |  | not answered |  |
| `/Input_Channels/{n}/Dynamics/GR_meter_1 or /Input_Channels/{n}/EQ/GR_meter_1` | GR meter 1 | `050c[0]` | meter |  |  | verified (strip dump) | <text> |
| `/Input_Channels/{n}/Dynamics/GR_meter_3 or /Input_Channels/{n}/EQ/GR_meter_3` | GR meter 3 | `050c[1]` | meter |  |  | verified (strip dump) | <text> |
| `/Input_Channels/{n}/Dynamics/GR_meter_2 or /Input_Channels/{n}/EQ/GR_meter_2` | GR meter 2 | `050c[2]` | meter |  |  | verified (strip dump) | <text> |
| `/Input_Channels/{n}/Dynamics/GR_meter_4 or /Input_Channels/{n}/EQ/GR_meter_4` | GR meter 4 | `050c[3]` | meter |  |  | verified (strip dump) | <text> |
| `—` | gate meter | `050d[3]` | meter |  |  | not answered |  |
| `/Input_Channels/{n}/Dynamics/comp_thresh_1` | comp thresh 1 | `050e[0]` | float | -60 … 0 | dB | verified (strip dump) | -22.35 |
| `/Input_Channels/{n}/Dynamics/comp_thresh_2` | comp thresh 2 | `050e[1]` | float | -60 … 0 | dB | verified (strip dump) | -21.41 |
| `/Input_Channels/{n}/Dynamics/comp_thresh_3` | comp thresh 3 | `050e[2]` | float | -60 … 0 | dB | verified (strip dump) | -21.41 |
| `/Input_Channels/{n}/Dynamics/gate_thresh` | gate thresh | `050e[3]` | float | -60 … 0 | dB | verified (strip dump) | -20 |
| `/Input_Channels/{n}/Dynamics/comp_knee_1` | comp knee 1 | `050f[0]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_knee_2` | comp knee 2 | `050f[1]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_knee_3` | comp knee 3 | `050f[2]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_knee_4` | comp knee 4 | `050f[3]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_ratio_1` | comp ratio 1 | `0510[0]` | float | 1 … 50 | : 1 | verified (strip dump) | 5.324 |
| `/Input_Channels/{n}/Dynamics/comp_ratio_2` | comp ratio 2 | `0510[1]` | float | 1 … 50 | : 1 | verified (strip dump) | 2 |
| `/Input_Channels/{n}/Dynamics/comp_ratio_3` | comp ratio 3 | `0510[2]` | float | 1 … 50 | : 1 | verified (strip dump) | 2 |
| `/Input_Channels/{n}/Dynamics/comp_ratio_4` | comp ratio 4 | `0510[3]` | float | 1 … 50 | : 1 | verified (strip dump) | 3 |
| `/Input_Channels/{n}/Dynamics/comp_attack_1` | comp attack 1 | `0511[0]` | float | 2e-05 … 0.1 | s | verified (strip dump) | 0.01266 |
| `/Input_Channels/{n}/Dynamics/comp_attack_2` | comp attack 2 | `0511[1]` | float | 2e-05 … 0.1 | s | verified (strip dump) | 0.005 |
| `/Input_Channels/{n}/Dynamics/comp_attack_3` | comp attack 3 | `0511[2]` | float | 2e-05 … 0.1 | s | verified (strip dump) | 0.025 |
| `/Input_Channels/{n}/Dynamics/gate_attack` | gate attack | `0511[3]` | float | 5e-05 … 0.1 | s | verified (strip dump) | 0.002 |
| `/Input_Channels/{n}/Dynamics/comp_release_1` | comp release 1 | `0512[0]` | float | 0.001 … 10 | s | verified (strip dump) | 0.02413 |
| `/Input_Channels/{n}/Dynamics/comp_release_2` | comp release 2 | `0512[1]` | float | 0.001 … 10 | s | verified (strip dump) | 0.2 |
| `/Input_Channels/{n}/Dynamics/comp_release_3` | comp release 3 | `0512[2]` | float | 0.001 … 10 | s | verified (strip dump) | 0.4 |
| `/Input_Channels/{n}/Dynamics/gate_release` | gate release | `0512[3]` | float | 0.005 … 10 | s | verified (strip dump) | 0.14 |
| `—` | gate hold | `0513[3]` | float | 0.002 … 2 | s | not answered |  |
| `/Input_Channels/{n}/Dynamics/key_solo` | key solo | `0517[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_listen_1` | comp listen 1 | `0518[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_listen_2` | comp listen 2 | `0518[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_listen_3` | comp listen 3 | `0518[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/input_meter/left` | input meter/left | `0530[0]` | meter |  |  | verified (strip dump) | [] |
| `/Input_Channels/{n}/Dynamics/input_meter/right` | input meter/right | `0534[0]` | meter |  |  | verified (strip dump) | [] |
| `/Input_Channels/{n}/Dynamics/comp_all_gain` | comp all gain | `0538[0]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_all_thresh` | comp all thresh | `0539[0]` | float | -60 … 0 | dB | verified (strip dump) | -22.35 |
| `/Input_Channels/{n}/Dynamics/comp-multiband-desser` | comp-multiband-desser | `053a[0]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_auto-gain_1` | comp auto-gain 1 | `053b[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_auto-gain_2` | comp auto-gain 2 | `053b[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_auto-gain_3` | comp auto-gain 3 | `053b[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/comp_auto-gain_4` | comp auto-gain 4 | `053b[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Dynamics/desser_centre_freq` | desser centre freq | `057e[0]` | float | 0 … 255 |  | verified (strip dump) | 127 |
| `/Input_Channels/{n}/Dynamics/gate_centre_freq` | gate centre freq | `057e[3]` | float | 0 … 255 |  | verified (strip dump) | 127 |
| `/Input_Channels/{n}/Dynamics/desser_freq_width` | desser freq width | `057f[0]` | float | 0 … 255 |  | verified (strip dump) | 255 |
| `/Input_Channels/{n}/Dynamics/gate_freq_width` | gate freq width | `057f[3]` | float | 0 … 255 |  | verified (strip dump) | 255 |
| `/Input_Channels/{n}/Filters/hi_filter_in` | hi filter in | `0703[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/Filters/lo_filter_in` | lo filter in | `0703[1]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/Filters/hi_filter_freq` | hi filter freq | `0705[0]` | float | 20 … 20000 | Hz | verified (strip dump) | 2e+04 |
| `/Input_Channels/{n}/Filters/lo_filter_freq` | lo filter freq | `0705[1]` | float | 20 … 20000 | Hz | verified (strip dump) | 81.81 |
| `/Input_Channels/{n}/Insert/insert_A_in` | insert A in | `0802[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Insert/insert_B_in` | insert B in | `0802[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Insert/insert_A_phantom` | insert A phantom | `0883[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Insert/insert_B_phantom` | insert B phantom | `0883[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Insert/insert_A_analog_gain` | insert A analog gain | `0886[0]` | float | -20 … 60 | dB | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Insert/insert_B_analog_gain` | insert B analog gain | `0886[1]` | float | -20 … 60 | dB | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Insert/insert_A_input_pad` | insert A input pad | `0889[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Insert/insert_B_input_pad` | insert B input pad | `0889[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Channel_Delay/delay_on` | delay on | `0907[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/Channel_Delay/delay` | delay | `0908[0]` | float | 0 … 1.3 | s | verified (strip dump) | 0.02448 |
| `/Input_Channels/{n}/Channel_Delay/fine_delay` | fine delay | `0917[0]` | float | -127 … 127 | samples | verified (strip dump) | 0 |
| `/Input_Channels/{n}/CGs_level` | CGs level | `0c01[0]` | float | -150 … 10 | dB, % | verified (strip dump) | 0.694 |
| `/Input_Channels/{n}/solo` | solo | `0c03[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/CGs_mute` | CGs mute | `0c1c[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Input_Channels/{n}/fader` | fader | `0c21[0]` | float | -150 … 10 | dB, % | verified (strip dump) | -1.667 |
| `/Input_Channels/{n}/mute` | mute | `0c22[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/fader_meter/left` | fader meter/left | `0c30[0]` | meter |  |  | verified (strip dump) | [] |
| `/Input_Channels/{n}/fader_meter/right` | fader meter/right | `0c34[0]` | meter |  |  | verified (strip dump) | [] |
| `/Input_Channels/{n}/Panner/f-b` | f-b | `0d16[0]` | float | 0 … 1 | % | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Panner/LFE_level` | LFE level | `0d17[0]` | float | -18 … 18 | dB | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Panner/LFE_off-only-all` | LFE off-only-all | `0d18[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Input_Channels/{n}/Panner/pan` | pan | `0d23[0]` | float | -1 … 1 | % | verified (strip dump) | 0.3454 |
| `/Input_Channels/{n}/Output/meter/left` | meter/left | `0e40[0]` | meter |  |  | verified (strip dump) | [] |
| `/Input_Channels/{n}/Output/meter2` | meter2 | `0e41[0]` | meter |  |  | verified (strip dump) | [] |
| `/Input_Channels/{n}/Output/meter/centre` | meter/centre | `0e42[0]` | meter |  |  | verified (strip dump) | [] |
| `/Input_Channels/{n}/Output/meter4` | meter4 | `0e43[0]` | meter |  |  | verified (strip dump) | [] |
| `/Input_Channels/{n}/Output/meter/right` | meter/right | `0e44[0]` | meter |  |  | verified (strip dump) | [] |
| `/Input_Channels/{n}/Output/meter/SL` | meter/SL | `0e45[0]` | meter |  |  | verified (strip dump) | [] |
| `/Input_Channels/{n}/Output/meter/SR` | meter/SR | `0e46[0]` | meter |  |  | verified (strip dump) | [] |
| `/Input_Channels/{n}/Output/meter/LFE` | meter/LFE | `0e47[0]` | meter |  |  | verified (strip dump) | [] |

### Aux_Outputs (150)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `/Aux_Outputs/{n}/EQ/eq_in` | eq in | `0402[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Aux_Outputs/{n}/EQ/eq_on_1` | eq on 1 | `0403[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Aux_Outputs/{n}/EQ/eq_on_2` | eq on 2 | `0403[1]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Aux_Outputs/{n}/EQ/eq_on_3` | eq on 3 | `0403[2]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Aux_Outputs/{n}/EQ/eq_on_4` | eq on 4 | `0403[3]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Aux_Outputs/{n}/EQ/eq_curve_1` | eq curve 1 | `0404[0]` | int | 1 … 4 |  | verified (strip dump) | 1 |
| `/Aux_Outputs/{n}/EQ/eq_curve_2` | eq curve 2 | `0404[1]` | int | 1 … 4 |  | verified (strip dump) | 1 |
| `/Aux_Outputs/{n}/EQ/eq_curve_3` | eq curve 3 | `0404[2]` | int | 1 … 4 |  | verified (strip dump) | 1 |
| `/Aux_Outputs/{n}/EQ/eq_curve_4` | eq curve 4 | `0404[3]` | int | 1 … 4 |  | verified (strip dump) | 1 |
| `/Aux_Outputs/{n}/EQ/eq_freq_1` | eq freq 1 | `0405[0]` | float | 20 … 20000 | Hz | verified (strip dump) | 6300 |
| `/Aux_Outputs/{n}/EQ/eq_freq_2` | eq freq 2 | `0405[1]` | float | 20 … 20000 | Hz | verified (strip dump) | 4759 |
| `/Aux_Outputs/{n}/EQ/eq_freq_3` | eq freq 3 | `0405[2]` | float | 20 … 20000 | Hz | verified (strip dump) | 300 |
| `/Aux_Outputs/{n}/EQ/eq_freq_4` | eq freq 4 | `0405[3]` | float | 20 … 20000 | Hz | verified (strip dump) | 100 |
| `/Aux_Outputs/{n}/EQ/eq_Q_1` | eq Q 1 | `0406[0]` | float | 0.1 … 20 |  | verified (strip dump) | 0.71 |
| `/Aux_Outputs/{n}/EQ/eq_Q_2` | eq Q 2 | `0406[1]` | float | 0.1 … 20 |  | verified (strip dump) | 0.71 |
| `/Aux_Outputs/{n}/EQ/eq_Q_3` | eq Q 3 | `0406[2]` | float | 0.1 … 20 |  | verified (strip dump) | 0.71 |
| `/Aux_Outputs/{n}/EQ/eq_Q_4` | eq Q 4 | `0406[3]` | float | 0.1 … 20 |  | verified (strip dump) | 0.71 |
| `/Aux_Outputs/{n}/EQ/eq_gain_1` | eq gain 1 | `0407[0]` | float | -18 … 18 | dB | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/EQ/eq_gain_2` | eq gain 2 | `0407[1]` | float | -18 … 18 | dB | verified (strip dump) | -2.612 |
| `/Aux_Outputs/{n}/EQ/eq_gain_3` | eq gain 3 | `0407[2]` | float | -18 … 18 | dB | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/EQ/eq_gain_4` | eq gain 4 | `0407[3]` | float | -18 … 18 | dB | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/EQ/eq_over-under_1` | eq over-under 1 | `040b[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/EQ/eq_over-under_2` | eq over-under 2 | `040b[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/EQ/eq_over-under_3` | eq over-under 3 | `040b[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/EQ/eq_over-under_4` | eq over-under 4 | `040b[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/EQ/GR_meter_1 or /Aux_Outputs/{n}/Dynamics/GR_meter_1` | GR meter 1 | `040c[0]` | meter |  |  | verified (strip dump) | <text> |
| `/Aux_Outputs/{n}/EQ/GR_meter_2 or /Aux_Outputs/{n}/Dynamics/GR_meter_2` | GR meter 2 | `040c[1]` | meter |  |  | verified (strip dump) | <text> |
| `/Aux_Outputs/{n}/EQ/GR_meter_3 or /Aux_Outputs/{n}/Dynamics/GR_meter_3` | GR meter 3 | `040c[2]` | meter |  |  | verified (strip dump) | <text> |
| `/Aux_Outputs/{n}/EQ/GR_meter_4 or /Aux_Outputs/{n}/Dynamics/GR_meter_4` | GR meter 4 | `040c[3]` | meter |  |  | verified (strip dump) | <text> |
| `/Aux_Outputs/{n}/EQ/eq_thresh_1` | eq thresh 1 | `040e[0]` | float | -60 … 0 | dB | verified (strip dump) | -36 |
| `/Aux_Outputs/{n}/EQ/eq_thresh_2` | eq thresh 2 | `040e[1]` | float | -60 … 0 | dB | verified (strip dump) | -36 |
| `/Aux_Outputs/{n}/EQ/eq_thresh_3` | eq thresh 3 | `040e[2]` | float | -60 … 0 | dB | verified (strip dump) | -36 |
| `/Aux_Outputs/{n}/EQ/eq_thresh_4` | eq thresh 4 | `040e[3]` | float | -60 … 0 | dB | verified (strip dump) | -36 |
| `/Aux_Outputs/{n}/EQ/eq_ratio_1` | eq ratio 1 | `0410[0]` | float | 1 … 10 | : 1 | verified (strip dump) | 2 |
| `/Aux_Outputs/{n}/EQ/eq_ratio_2` | eq ratio 2 | `0410[1]` | float | 1 … 10 | : 1 | verified (strip dump) | 2 |
| `/Aux_Outputs/{n}/EQ/eq_ratio_3` | eq ratio 3 | `0410[2]` | float | 1 … 10 | : 1 | verified (strip dump) | 2 |
| `/Aux_Outputs/{n}/EQ/eq_ratio_4` | eq ratio 4 | `0410[3]` | float | 1 … 10 | : 1 | verified (strip dump) | 2 |
| `/Aux_Outputs/{n}/EQ/eq_attack_1` | eq attack 1 | `0411[0]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.01 |
| `/Aux_Outputs/{n}/EQ/eq_attack_2` | eq attack 2 | `0411[1]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.015 |
| `/Aux_Outputs/{n}/EQ/eq_attack_3` | eq attack 3 | `0411[2]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.025 |
| `/Aux_Outputs/{n}/EQ/eq_attack_4` | eq attack 4 | `0411[3]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.05 |
| `/Aux_Outputs/{n}/EQ/eq_release_1` | eq release 1 | `0412[0]` | float | 0.01 … 10 | s | verified (strip dump) | 0.3 |
| `/Aux_Outputs/{n}/EQ/eq_release_2` | eq release 2 | `0412[1]` | float | 0.01 … 10 | s | verified (strip dump) | 0.35 |
| `/Aux_Outputs/{n}/EQ/eq_release_3` | eq release 3 | `0412[2]` | float | 0.01 … 10 | s | verified (strip dump) | 0.4 |
| `/Aux_Outputs/{n}/EQ/eq_release_4` | eq release 4 | `0412[3]` | float | 0.01 … 10 | s | verified (strip dump) | 0.6 |
| `/Aux_Outputs/{n}/EQ/eq_symm_Q_1` | eq symm Q 1 | `047c[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/EQ/eq_symm_Q_2` | eq symm Q 2 | `047c[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/EQ/eq_symm_Q_3` | eq symm Q 3 | `047c[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/EQ/eq_symm_Q_4` | eq symm Q 4 | `047c[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/EQ/dynamic_eq_on_1` | dynamic eq on 1 | `047d[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/EQ/dynamic_eq_on_2` | dynamic eq on 2 | `047d[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/EQ/dynamic_eq_on_3` | dynamic eq on 3 | `047d[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/EQ/dynamic_eq_on_4` | dynamic eq on 4 | `047d[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_in` | comp in | `0502[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/gate_in` | gate in | `0502[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_band_in_1` | comp band in 1 | `0503[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Aux_Outputs/{n}/Dynamics/comp_band_in_2` | comp band in 2 | `0503[1]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Aux_Outputs/{n}/Dynamics/comp_band_in_3` | comp band in 3 | `0503[2]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Aux_Outputs/{n}/Dynamics/comp_LP_crossover` | comp LP crossover | `0505[0]` | float | 20 … 20000 | Hz | verified (strip dump) | 130 |
| `/Aux_Outputs/{n}/Dynamics/comp_HP_crossover` | comp HP crossover | `0505[1]` | float | 20 … 20000 | Hz | verified (strip dump) | 1000 |
| `/Aux_Outputs/{n}/Dynamics/comp_gain_1` | comp gain 1 | `0509[0]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_gain_2` | comp gain 2 | `0509[1]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_gain_3` | comp gain 3 | `0509[2]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_gain_4` | comp gain 4 | `0509[3]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `—` | gate range | `050a[3]` | float | 0 … 90 | dB | not answered |  |
| `—` | gate-duck-comp | `050b[3]` | int | 0 … 2 |  | not answered |  |
| `/Aux_Outputs/{n}/EQ/GR_meter_1 or /Aux_Outputs/{n}/Dynamics/GR_meter_1` | GR meter 1 | `050c[0]` | meter |  |  | verified (strip dump) | <text> |
| `/Aux_Outputs/{n}/EQ/GR_meter_3 or /Aux_Outputs/{n}/Dynamics/GR_meter_3` | GR meter 3 | `050c[1]` | meter |  |  | verified (strip dump) | <text> |
| `/Aux_Outputs/{n}/EQ/GR_meter_2 or /Aux_Outputs/{n}/Dynamics/GR_meter_2` | GR meter 2 | `050c[2]` | meter |  |  | verified (strip dump) | <text> |
| `/Aux_Outputs/{n}/EQ/GR_meter_4 or /Aux_Outputs/{n}/Dynamics/GR_meter_4` | GR meter 4 | `050c[3]` | meter |  |  | verified (strip dump) | <text> |
| `—` | gate meter | `050d[3]` | meter |  |  | not answered |  |
| `/Aux_Outputs/{n}/Dynamics/comp_thresh_1` | comp thresh 1 | `050e[0]` | float | -60 … 0 | dB | verified (strip dump) | -20 |
| `/Aux_Outputs/{n}/Dynamics/comp_thresh_2` | comp thresh 2 | `050e[1]` | float | -60 … 0 | dB | verified (strip dump) | -20 |
| `/Aux_Outputs/{n}/Dynamics/comp_thresh_3` | comp thresh 3 | `050e[2]` | float | -60 … 0 | dB | verified (strip dump) | -20 |
| `/Aux_Outputs/{n}/Dynamics/gate_thresh` | gate thresh | `050e[3]` | float | -60 … 0 | dB | verified (strip dump) | -20 |
| `/Aux_Outputs/{n}/Dynamics/comp_knee_1` | comp knee 1 | `050f[0]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_knee_2` | comp knee 2 | `050f[1]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_knee_3` | comp knee 3 | `050f[2]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_knee_4` | comp knee 4 | `050f[3]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_ratio_1` | comp ratio 1 | `0510[0]` | float | 1 … 50 | : 1 | verified (strip dump) | 3 |
| `/Aux_Outputs/{n}/Dynamics/comp_ratio_2` | comp ratio 2 | `0510[1]` | float | 1 … 50 | : 1 | verified (strip dump) | 2 |
| `/Aux_Outputs/{n}/Dynamics/comp_ratio_3` | comp ratio 3 | `0510[2]` | float | 1 … 50 | : 1 | verified (strip dump) | 2 |
| `/Aux_Outputs/{n}/Dynamics/comp_ratio_4` | comp ratio 4 | `0510[3]` | float | 1 … 50 | : 1 | verified (strip dump) | 3 |
| `/Aux_Outputs/{n}/Dynamics/comp_attack_1` | comp attack 1 | `0511[0]` | float | 2e-05 … 0.1 | s | verified (strip dump) | 0.01 |
| `/Aux_Outputs/{n}/Dynamics/comp_attack_2` | comp attack 2 | `0511[1]` | float | 2e-05 … 0.1 | s | verified (strip dump) | 0.005 |
| `/Aux_Outputs/{n}/Dynamics/comp_attack_3` | comp attack 3 | `0511[2]` | float | 2e-05 … 0.1 | s | verified (strip dump) | 0.025 |
| `/Aux_Outputs/{n}/Dynamics/gate_attack` | gate attack | `0511[3]` | float | 5e-05 … 0.1 | s | verified (strip dump) | 0.002 |
| `/Aux_Outputs/{n}/Dynamics/comp_release_1` | comp release 1 | `0512[0]` | float | 0.001 … 10 | s | verified (strip dump) | 0.5 |
| `/Aux_Outputs/{n}/Dynamics/comp_release_2` | comp release 2 | `0512[1]` | float | 0.001 … 10 | s | verified (strip dump) | 0.2 |
| `/Aux_Outputs/{n}/Dynamics/comp_release_3` | comp release 3 | `0512[2]` | float | 0.001 … 10 | s | verified (strip dump) | 0.4 |
| `/Aux_Outputs/{n}/Dynamics/gate_release` | gate release | `0512[3]` | float | 0.005 … 10 | s | verified (strip dump) | 0.14 |
| `—` | gate hold | `0513[3]` | float | 0.002 … 2 | s | not answered |  |
| `/Aux_Outputs/{n}/Dynamics/key_solo` | key solo | `0517[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_listen_1` | comp listen 1 | `0518[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_listen_2` | comp listen 2 | `0518[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_listen_3` | comp listen 3 | `0518[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/input_meter/left` | input meter/left | `0530[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Dynamics/input_meter/right` | input meter/right | `0534[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Dynamics/comp_all_gain` | comp all gain | `0538[0]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_all_thresh` | comp all thresh | `0539[0]` | float | -60 … 0 | dB | verified (strip dump) | -20 |
| `/Aux_Outputs/{n}/Dynamics/comp-multiband-desser` | comp-multiband-desser | `053a[0]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_auto-gain_1` | comp auto-gain 1 | `053b[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_auto-gain_2` | comp auto-gain 2 | `053b[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_auto-gain_3` | comp auto-gain 3 | `053b[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/comp_auto-gain_4` | comp auto-gain 4 | `053b[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Dynamics/desser_centre_freq` | desser centre freq | `057e[0]` | float | 0 … 255 |  | verified (strip dump) | 127 |
| `/Aux_Outputs/{n}/Dynamics/gate_centre_freq` | gate centre freq | `057e[3]` | float | 0 … 255 |  | verified (strip dump) | 127 |
| `/Aux_Outputs/{n}/Dynamics/desser_freq_width` | desser freq width | `057f[0]` | float | 0 … 255 |  | verified (strip dump) | 255 |
| `/Aux_Outputs/{n}/Dynamics/gate_freq_width` | gate freq width | `057f[3]` | float | 0 … 255 |  | verified (strip dump) | 255 |
| `/Aux_Outputs/{n}/Filters/hi_filter_in` | hi filter in | `0703[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Filters/lo_filter_in` | lo filter in | `0703[1]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Aux_Outputs/{n}/Filters/hi_filter_freq` | hi filter freq | `0705[0]` | float | 20 … 20000 | Hz | verified (strip dump) | 2e+04 |
| `/Aux_Outputs/{n}/Filters/lo_filter_freq` | lo filter freq | `0705[1]` | float | 20 … 20000 | Hz | verified (strip dump) | 91.17 |
| `/Aux_Outputs/{n}/Insert/insert_A_in` | insert A in | `0802[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Insert/insert_B_in` | insert B in | `0802[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Channel_Delay/delay_on` | delay on | `0907[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Channel_Delay/delay` | delay | `0908[0]` | float | 0 … 1.3 | s | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Channel_Delay/fine_delay` | fine delay | `0917[0]` | float | -127 … 127 | samples | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Buss_Trim/trim` | trim | `0b01[0]` | float | -40 … 60 | dB | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Buss_Trim/phase` | phase | `0b02[0]` | int | 0 … 3 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Buss_Trim/pre_meter` | pre meter | `0b20[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Buss_Trim/pre_meter/right` | pre meter/right | `0b24[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Buss_Trim/post_meter` | post meter | `0b30[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Buss_Trim/post_meter/right` | post meter/right | `0b34[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Buss_Trim/tube_meter/left` | tube meter/left | `0b58[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Buss_Trim/tube_meter/right` | tube meter/right | `0b5c[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Buss_Trim/name` | name | `0bff[0]` | string |  |  | verified (strip dump) | <text> |
| `/Aux_Outputs/{n}/CGs_level` | CGs level | `0c01[0]` | float | -150 … 10 | dB, % | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/hard_mute` | hard mute | `0c02[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/solo` | solo | `0c03[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/solo_1_or_2` | solo 1 or 2 | `0c0b[0]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/sends_to_rotaries` | sends to rotaries | `0c0d[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/sends_to_faders` | sends to faders | `0c10[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/auto_solo` | auto solo | `0c11[0]` | int | 0 … 3 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/CGs_mute` | CGs mute | `0c1c[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/fader` | fader | `0c21[0]` | float | -150 … 10 | dB, % | verified (strip dump) | 0.005 |
| `/Aux_Outputs/{n}/mute` | mute | `0c22[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/fader_meter/left` | fader meter/left | `0c30[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/fader_meter/right` | fader meter/right | `0c34[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/alternate_solo` | alternate solo | `0ce4[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Aux_Outputs/{n}/Output/dir_meter/left` | dir meter/left | `0e28[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Output/dir_meter/right` | dir meter/right | `0e2c[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Output/meter/left` | meter/left | `0e40[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Output/meter2` | meter2 | `0e41[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Output/meter/centre` | meter/centre | `0e42[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Output/meter4` | meter4 | `0e43[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Output/meter/right` | meter/right | `0e44[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Output/meter/SL` | meter/SL | `0e45[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Output/meter/SR` | meter/SR | `0e46[0]` | meter |  |  | verified (strip dump) | [] |
| `/Aux_Outputs/{n}/Output/meter/LFE` | meter/LFE | `0e47[0]` | meter |  |  | verified (strip dump) | [] |

### Group_Outputs (155)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `/Group_Outputs/{n}/Aux_Send/{k}/send_level` | send level | `0201[0]` | float | -150 … 10 | dB, % | verified (strip dump) | -150 |
| `/Group_Outputs/{n}/Aux_Send/{k}/send_on` | send on | `0202[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Aux_Send/{k}/send_pan` | send pan | `0203[0]` | float | -1 … 1 | % | verified (strip dump) | 0.5 |
| `/Group_Outputs/{n}/Aux_Send/{k}/send_pre-post` | send pre-post | `0204[0]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Group_Send/{k}/group` | group | `0309[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/EQ/eq_in` | eq in | `0402[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Group_Outputs/{n}/EQ/eq_on_1` | eq on 1 | `0403[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Group_Outputs/{n}/EQ/eq_on_2` | eq on 2 | `0403[1]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Group_Outputs/{n}/EQ/eq_on_3` | eq on 3 | `0403[2]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Group_Outputs/{n}/EQ/eq_on_4` | eq on 4 | `0403[3]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Group_Outputs/{n}/EQ/eq_curve_1` | eq curve 1 | `0404[0]` | int | 1 … 4 |  | verified (strip dump) | 1 |
| `/Group_Outputs/{n}/EQ/eq_curve_2` | eq curve 2 | `0404[1]` | int | 1 … 4 |  | verified (strip dump) | 1 |
| `/Group_Outputs/{n}/EQ/eq_curve_3` | eq curve 3 | `0404[2]` | int | 1 … 4 |  | verified (strip dump) | 1 |
| `/Group_Outputs/{n}/EQ/eq_curve_4` | eq curve 4 | `0404[3]` | int | 1 … 4 |  | verified (strip dump) | 1 |
| `/Group_Outputs/{n}/EQ/eq_freq_1` | eq freq 1 | `0405[0]` | float | 20 … 20000 | Hz | verified (strip dump) | 5024 |
| `/Group_Outputs/{n}/EQ/eq_freq_2` | eq freq 2 | `0405[1]` | float | 20 … 20000 | Hz | verified (strip dump) | 719.7 |
| `/Group_Outputs/{n}/EQ/eq_freq_3` | eq freq 3 | `0405[2]` | float | 20 … 20000 | Hz | verified (strip dump) | 91.17 |
| `/Group_Outputs/{n}/EQ/eq_freq_4` | eq freq 4 | `0405[3]` | float | 20 … 20000 | Hz | verified (strip dump) | 41.56 |
| `/Group_Outputs/{n}/EQ/eq_Q_1` | eq Q 1 | `0406[0]` | float | 0.1 … 20 |  | verified (strip dump) | 4.703 |
| `/Group_Outputs/{n}/EQ/eq_Q_2` | eq Q 2 | `0406[1]` | float | 0.1 … 20 |  | verified (strip dump) | 3.402 |
| `/Group_Outputs/{n}/EQ/eq_Q_3` | eq Q 3 | `0406[2]` | float | 0.1 … 20 |  | verified (strip dump) | 1.404 |
| `/Group_Outputs/{n}/EQ/eq_Q_4` | eq Q 4 | `0406[3]` | float | 0.1 … 20 |  | verified (strip dump) | 6.42 |
| `/Group_Outputs/{n}/EQ/eq_gain_1` | eq gain 1 | `0407[0]` | float | -18 … 18 | dB | verified (strip dump) | -2.047 |
| `/Group_Outputs/{n}/EQ/eq_gain_2` | eq gain 2 | `0407[1]` | float | -18 … 18 | dB | verified (strip dump) | -2.308 |
| `/Group_Outputs/{n}/EQ/eq_gain_3` | eq gain 3 | `0407[2]` | float | -18 … 18 | dB | verified (strip dump) | -2.894 |
| `/Group_Outputs/{n}/EQ/eq_gain_4` | eq gain 4 | `0407[3]` | float | -18 … 18 | dB | verified (strip dump) | -3.176 |
| `/Group_Outputs/{n}/EQ/eq_over-under_1` | eq over-under 1 | `040b[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/EQ/eq_over-under_2` | eq over-under 2 | `040b[1]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Group_Outputs/{n}/EQ/eq_over-under_3` | eq over-under 3 | `040b[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/EQ/eq_over-under_4` | eq over-under 4 | `040b[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/EQ/GR_meter_1 or /Group_Outputs/{n}/Dynamics/GR_meter_1` | GR meter 1 | `040c[0]` | meter |  |  | verified (strip dump) | <text> |
| `/Group_Outputs/{n}/EQ/GR_meter_2 or /Group_Outputs/{n}/Dynamics/GR_meter_2` | GR meter 2 | `040c[1]` | meter |  |  | verified (strip dump) | <text> |
| `/Group_Outputs/{n}/EQ/GR_meter_3 or /Group_Outputs/{n}/Dynamics/GR_meter_3` | GR meter 3 | `040c[2]` | meter |  |  | verified (strip dump) | <text> |
| `/Group_Outputs/{n}/EQ/GR_meter_4 or /Group_Outputs/{n}/Dynamics/GR_meter_4` | GR meter 4 | `040c[3]` | meter |  |  | verified (strip dump) | <text> |
| `/Group_Outputs/{n}/EQ/eq_thresh_1` | eq thresh 1 | `040e[0]` | float | -60 … 0 | dB | verified (strip dump) | -36 |
| `/Group_Outputs/{n}/EQ/eq_thresh_2` | eq thresh 2 | `040e[1]` | float | -60 … 0 | dB | verified (strip dump) | -33.59 |
| `/Group_Outputs/{n}/EQ/eq_thresh_3` | eq thresh 3 | `040e[2]` | float | -60 … 0 | dB | verified (strip dump) | -36 |
| `/Group_Outputs/{n}/EQ/eq_thresh_4` | eq thresh 4 | `040e[3]` | float | -60 … 0 | dB | verified (strip dump) | -36.31 |
| `/Group_Outputs/{n}/EQ/eq_ratio_1` | eq ratio 1 | `0410[0]` | float | 1 … 10 | : 1 | verified (strip dump) | 2 |
| `/Group_Outputs/{n}/EQ/eq_ratio_2` | eq ratio 2 | `0410[1]` | float | 1 … 10 | : 1 | verified (strip dump) | 2 |
| `/Group_Outputs/{n}/EQ/eq_ratio_3` | eq ratio 3 | `0410[2]` | float | 1 … 10 | : 1 | verified (strip dump) | 2 |
| `/Group_Outputs/{n}/EQ/eq_ratio_4` | eq ratio 4 | `0410[3]` | float | 1 … 10 | : 1 | verified (strip dump) | 2 |
| `/Group_Outputs/{n}/EQ/eq_attack_1` | eq attack 1 | `0411[0]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.01 |
| `/Group_Outputs/{n}/EQ/eq_attack_2` | eq attack 2 | `0411[1]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.015 |
| `/Group_Outputs/{n}/EQ/eq_attack_3` | eq attack 3 | `0411[2]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.025 |
| `/Group_Outputs/{n}/EQ/eq_attack_4` | eq attack 4 | `0411[3]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.05 |
| `/Group_Outputs/{n}/EQ/eq_release_1` | eq release 1 | `0412[0]` | float | 0.01 … 10 | s | verified (strip dump) | 0.3 |
| `/Group_Outputs/{n}/EQ/eq_release_2` | eq release 2 | `0412[1]` | float | 0.01 … 10 | s | verified (strip dump) | 0.35 |
| `/Group_Outputs/{n}/EQ/eq_release_3` | eq release 3 | `0412[2]` | float | 0.01 … 10 | s | verified (strip dump) | 0.4 |
| `/Group_Outputs/{n}/EQ/eq_release_4` | eq release 4 | `0412[3]` | float | 0.01 … 10 | s | verified (strip dump) | 0.7582 |
| `/Group_Outputs/{n}/EQ/eq_symm_Q_1` | eq symm Q 1 | `047c[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/EQ/eq_symm_Q_2` | eq symm Q 2 | `047c[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/EQ/eq_symm_Q_3` | eq symm Q 3 | `047c[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/EQ/eq_symm_Q_4` | eq symm Q 4 | `047c[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/EQ/dynamic_eq_on_1` | dynamic eq on 1 | `047d[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/EQ/dynamic_eq_on_2` | dynamic eq on 2 | `047d[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/EQ/dynamic_eq_on_3` | dynamic eq on 3 | `047d[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/EQ/dynamic_eq_on_4` | dynamic eq on 4 | `047d[3]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Group_Outputs/{n}/Dynamics/comp_in` | comp in | `0502[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/gate_in` | gate in | `0502[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_band_in_1` | comp band in 1 | `0503[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Group_Outputs/{n}/Dynamics/comp_band_in_2` | comp band in 2 | `0503[1]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Group_Outputs/{n}/Dynamics/comp_band_in_3` | comp band in 3 | `0503[2]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Group_Outputs/{n}/Dynamics/comp_LP_crossover` | comp LP crossover | `0505[0]` | float | 20 … 20000 | Hz | verified (strip dump) | 130 |
| `/Group_Outputs/{n}/Dynamics/comp_HP_crossover` | comp HP crossover | `0505[1]` | float | 20 … 20000 | Hz | verified (strip dump) | 1000 |
| `/Group_Outputs/{n}/Dynamics/comp_gain_1` | comp gain 1 | `0509[0]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_gain_2` | comp gain 2 | `0509[1]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_gain_3` | comp gain 3 | `0509[2]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_gain_4` | comp gain 4 | `0509[3]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `—` | gate range | `050a[3]` | float | 0 … 90 | dB | not answered |  |
| `—` | gate-duck-comp | `050b[3]` | int | 0 … 2 |  | not answered |  |
| `/Group_Outputs/{n}/EQ/GR_meter_1 or /Group_Outputs/{n}/Dynamics/GR_meter_1` | GR meter 1 | `050c[0]` | meter |  |  | verified (strip dump) | <text> |
| `/Group_Outputs/{n}/EQ/GR_meter_3 or /Group_Outputs/{n}/Dynamics/GR_meter_3` | GR meter 3 | `050c[1]` | meter |  |  | verified (strip dump) | <text> |
| `/Group_Outputs/{n}/EQ/GR_meter_2 or /Group_Outputs/{n}/Dynamics/GR_meter_2` | GR meter 2 | `050c[2]` | meter |  |  | verified (strip dump) | <text> |
| `/Group_Outputs/{n}/EQ/GR_meter_4 or /Group_Outputs/{n}/Dynamics/GR_meter_4` | GR meter 4 | `050c[3]` | meter |  |  | verified (strip dump) | <text> |
| `—` | gate meter | `050d[3]` | meter |  |  | not answered |  |
| `/Group_Outputs/{n}/Dynamics/comp_thresh_1` | comp thresh 1 | `050e[0]` | float | -60 … 0 | dB | verified (strip dump) | -0.7058 |
| `/Group_Outputs/{n}/Dynamics/comp_thresh_2` | comp thresh 2 | `050e[1]` | float | -60 … 0 | dB | verified (strip dump) | -20 |
| `/Group_Outputs/{n}/Dynamics/comp_thresh_3` | comp thresh 3 | `050e[2]` | float | -60 … 0 | dB | verified (strip dump) | -20 |
| `/Group_Outputs/{n}/Dynamics/gate_thresh` | gate thresh | `050e[3]` | float | -60 … 0 | dB | verified (strip dump) | -19.76 |
| `/Group_Outputs/{n}/Dynamics/comp_knee_1` | comp knee 1 | `050f[0]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_knee_2` | comp knee 2 | `050f[1]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_knee_3` | comp knee 3 | `050f[2]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_knee_4` | comp knee 4 | `050f[3]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_ratio_1` | comp ratio 1 | `0510[0]` | float | 1 … 50 | : 1 | verified (strip dump) | 49.24 |
| `/Group_Outputs/{n}/Dynamics/comp_ratio_2` | comp ratio 2 | `0510[1]` | float | 1 … 50 | : 1 | verified (strip dump) | 2 |
| `/Group_Outputs/{n}/Dynamics/comp_ratio_3` | comp ratio 3 | `0510[2]` | float | 1 … 50 | : 1 | verified (strip dump) | 2 |
| `/Group_Outputs/{n}/Dynamics/comp_ratio_4` | comp ratio 4 | `0510[3]` | float | 1 … 50 | : 1 | verified (strip dump) | 3 |
| `/Group_Outputs/{n}/Dynamics/comp_attack_1` | comp attack 1 | `0511[0]` | float | 2e-05 … 0.1 | s | verified (strip dump) | 0.009388 |
| `/Group_Outputs/{n}/Dynamics/comp_attack_2` | comp attack 2 | `0511[1]` | float | 2e-05 … 0.1 | s | verified (strip dump) | 0.005 |
| `/Group_Outputs/{n}/Dynamics/comp_attack_3` | comp attack 3 | `0511[2]` | float | 2e-05 … 0.1 | s | verified (strip dump) | 0.025 |
| `/Group_Outputs/{n}/Dynamics/gate_attack` | gate attack | `0511[3]` | float | 5e-05 … 0.1 | s | verified (strip dump) | 0.002 |
| `/Group_Outputs/{n}/Dynamics/comp_release_1` | comp release 1 | `0512[0]` | float | 0.001 … 10 | s | verified (strip dump) | 0.5662 |
| `/Group_Outputs/{n}/Dynamics/comp_release_2` | comp release 2 | `0512[1]` | float | 0.001 … 10 | s | verified (strip dump) | 0.2 |
| `/Group_Outputs/{n}/Dynamics/comp_release_3` | comp release 3 | `0512[2]` | float | 0.001 … 10 | s | verified (strip dump) | 0.4 |
| `/Group_Outputs/{n}/Dynamics/gate_release` | gate release | `0512[3]` | float | 0.005 … 10 | s | verified (strip dump) | 0.14 |
| `—` | gate hold | `0513[3]` | float | 0.002 … 2 | s | not answered |  |
| `/Group_Outputs/{n}/Dynamics/key_solo` | key solo | `0517[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_listen_1` | comp listen 1 | `0518[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_listen_2` | comp listen 2 | `0518[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_listen_3` | comp listen 3 | `0518[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/input_meter/left` | input meter/left | `0530[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Dynamics/input_meter/right` | input meter/right | `0534[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Dynamics/comp_all_gain` | comp all gain | `0538[0]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_all_thresh` | comp all thresh | `0539[0]` | float | -60 … 0 | dB | verified (strip dump) | -0.7058 |
| `/Group_Outputs/{n}/Dynamics/comp-multiband-desser` | comp-multiband-desser | `053a[0]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_auto-gain_1` | comp auto-gain 1 | `053b[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_auto-gain_2` | comp auto-gain 2 | `053b[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_auto-gain_3` | comp auto-gain 3 | `053b[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/comp_auto-gain_4` | comp auto-gain 4 | `053b[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Dynamics/desser_centre_freq` | desser centre freq | `057e[0]` | float | 0 … 255 |  | verified (strip dump) | 127 |
| `/Group_Outputs/{n}/Dynamics/gate_centre_freq` | gate centre freq | `057e[3]` | float | 0 … 255 |  | verified (strip dump) | 127 |
| `/Group_Outputs/{n}/Dynamics/desser_freq_width` | desser freq width | `057f[0]` | float | 0 … 255 |  | verified (strip dump) | 255 |
| `/Group_Outputs/{n}/Dynamics/gate_freq_width` | gate freq width | `057f[3]` | float | 0 … 255 |  | verified (strip dump) | 255 |
| `/Group_Outputs/{n}/Filters/hi_filter_in` | hi filter in | `0703[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Filters/lo_filter_in` | lo filter in | `0703[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Filters/hi_filter_freq` | hi filter freq | `0705[0]` | float | 20 … 20000 | Hz | verified (strip dump) | 2e+04 |
| `/Group_Outputs/{n}/Filters/lo_filter_freq` | lo filter freq | `0705[1]` | float | 20 … 20000 | Hz | verified (strip dump) | 20 |
| `/Group_Outputs/{n}/Insert/insert_A_in` | insert A in | `0802[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Insert/insert_B_in` | insert B in | `0802[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Channel_Delay/delay_on` | delay on | `0907[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Channel_Delay/delay` | delay | `0908[0]` | float | 0 … 1.3 | s | verified (strip dump) | 0.001999 |
| `/Group_Outputs/{n}/Channel_Delay/fine_delay` | fine delay | `0917[0]` | float | -127 … 127 | samples | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Buss_Trim/trim` | trim | `0b01[0]` | float | -40 … 60 | dB | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Buss_Trim/phase` | phase | `0b02[0]` | int | 0 … 3 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Buss_Trim/pre_meter` | pre meter | `0b20[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Buss_Trim/pre_meter/right` | pre meter/right | `0b24[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Buss_Trim/post_meter` | post meter | `0b30[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Buss_Trim/post_meter/right` | post meter/right | `0b34[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Buss_Trim/tube_meter/left` | tube meter/left | `0b58[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Buss_Trim/tube_meter/right` | tube meter/right | `0b5c[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Buss_Trim/name` | name | `0bff[0]` | string |  |  | verified (strip dump) | <text> |
| `/Group_Outputs/{n}/CGs_level` | CGs level | `0c01[0]` | float | -150 … 10 | dB, % | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/hard_mute` | hard mute | `0c02[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/solo` | solo | `0c03[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/solo_1_or_2` | solo 1 or 2 | `0c0b[0]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/sends_to_rotaries` | sends to rotaries | `0c0d[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/sends_to_faders` | sends to faders | `0c10[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/auto_solo` | auto solo | `0c11[0]` | int | 0 … 3 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/CGs_mute` | CGs mute | `0c1c[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/fader` | fader | `0c21[0]` | float | -150 … 10 | dB, % | verified (strip dump) | -2.639 |
| `/Group_Outputs/{n}/mute` | mute | `0c22[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/fader_meter/left` | fader meter/left | `0c30[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/fader_meter/right` | fader meter/right | `0c34[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/alternate_solo` | alternate solo | `0ce4[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Group_Outputs/{n}/Output/dir_meter/left` | dir meter/left | `0e28[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Output/dir_meter/right` | dir meter/right | `0e2c[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Output/meter/left` | meter/left | `0e40[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Output/meter2` | meter2 | `0e41[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Output/meter/centre` | meter/centre | `0e42[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Output/meter4` | meter4 | `0e43[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Output/meter/right` | meter/right | `0e44[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Output/meter/SL` | meter/SL | `0e45[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Output/meter/SR` | meter/SR | `0e46[0]` | meter |  |  | verified (strip dump) | [] |
| `/Group_Outputs/{n}/Output/meter/LFE` | meter/LFE | `0e47[0]` | meter |  |  | verified (strip dump) | [] |

### Matrix_Outputs (147)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `/Matrix_Outputs/{n}/EQ/eq_in` | eq in | `0402[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/EQ/eq_on_1` | eq on 1 | `0403[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/EQ/eq_on_2` | eq on 2 | `0403[1]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/EQ/eq_on_3` | eq on 3 | `0403[2]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/EQ/eq_on_4` | eq on 4 | `0403[3]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/EQ/eq_curve_1` | eq curve 1 | `0404[0]` | int | 1 … 4 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/EQ/eq_curve_2` | eq curve 2 | `0404[1]` | int | 1 … 4 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/EQ/eq_curve_3` | eq curve 3 | `0404[2]` | int | 1 … 4 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/EQ/eq_curve_4` | eq curve 4 | `0404[3]` | int | 1 … 4 |  | verified (strip dump) | 3 |
| `/Matrix_Outputs/{n}/EQ/eq_freq_1` | eq freq 1 | `0405[0]` | float | 20 … 20000 | Hz | verified (strip dump) | 2967 |
| `/Matrix_Outputs/{n}/EQ/eq_freq_2` | eq freq 2 | `0405[1]` | float | 20 … 20000 | Hz | verified (strip dump) | 278.3 |
| `/Matrix_Outputs/{n}/EQ/eq_freq_3` | eq freq 3 | `0405[2]` | float | 20 … 20000 | Hz | verified (strip dump) | 1445 |
| `/Matrix_Outputs/{n}/EQ/eq_freq_4` | eq freq 4 | `0405[3]` | float | 20 … 20000 | Hz | verified (strip dump) | 98.89 |
| `/Matrix_Outputs/{n}/EQ/eq_Q_1` | eq Q 1 | `0406[0]` | float | 0.1 … 20 |  | verified (strip dump) | 0.6114 |
| `/Matrix_Outputs/{n}/EQ/eq_Q_2` | eq Q 2 | `0406[1]` | float | 0.1 … 20 |  | verified (strip dump) | 1.486 |
| `/Matrix_Outputs/{n}/EQ/eq_Q_3` | eq Q 3 | `0406[2]` | float | 0.1 … 20 |  | verified (strip dump) | 1.004 |
| `/Matrix_Outputs/{n}/EQ/eq_Q_4` | eq Q 4 | `0406[3]` | float | 0.1 … 20 |  | verified (strip dump) | 0.4533 |
| `/Matrix_Outputs/{n}/EQ/eq_gain_1` | eq gain 1 | `0407[0]` | float | -18 … 18 | dB | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/EQ/eq_gain_2` | eq gain 2 | `0407[1]` | float | -18 … 18 | dB | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/EQ/eq_gain_3` | eq gain 3 | `0407[2]` | float | -18 … 18 | dB | verified (strip dump) | -3.063 |
| `/Matrix_Outputs/{n}/EQ/eq_gain_4` | eq gain 4 | `0407[3]` | float | -18 … 18 | dB | verified (strip dump) | -1.906 |
| `/Matrix_Outputs/{n}/EQ/eq_over-under_1` | eq over-under 1 | `040b[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/EQ/eq_over-under_2` | eq over-under 2 | `040b[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/EQ/eq_over-under_3` | eq over-under 3 | `040b[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/EQ/eq_over-under_4` | eq over-under 4 | `040b[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/EQ/GR_meter_1 or /Matrix_Outputs/{n}/Dynamics/GR_meter_1` | GR meter 1 | `040c[0]` | meter |  |  | verified (strip dump) | <text> |
| `/Matrix_Outputs/{n}/EQ/GR_meter_2 or /Matrix_Outputs/{n}/Dynamics/GR_meter_2` | GR meter 2 | `040c[1]` | meter |  |  | verified (strip dump) | <text> |
| `/Matrix_Outputs/{n}/EQ/GR_meter_3 or /Matrix_Outputs/{n}/Dynamics/GR_meter_3` | GR meter 3 | `040c[2]` | meter |  |  | verified (strip dump) | <text> |
| `/Matrix_Outputs/{n}/EQ/GR_meter_4 or /Matrix_Outputs/{n}/Dynamics/GR_meter_4` | GR meter 4 | `040c[3]` | meter |  |  | verified (strip dump) | <text> |
| `/Matrix_Outputs/{n}/EQ/eq_thresh_1` | eq thresh 1 | `040e[0]` | float | -60 … 0 | dB | verified (strip dump) | -36 |
| `/Matrix_Outputs/{n}/EQ/eq_thresh_2` | eq thresh 2 | `040e[1]` | float | -60 … 0 | dB | verified (strip dump) | -39.23 |
| `/Matrix_Outputs/{n}/EQ/eq_thresh_3` | eq thresh 3 | `040e[2]` | float | -60 … 0 | dB | verified (strip dump) | -35.29 |
| `/Matrix_Outputs/{n}/EQ/eq_thresh_4` | eq thresh 4 | `040e[3]` | float | -60 … 0 | dB | verified (strip dump) | -49.18 |
| `/Matrix_Outputs/{n}/EQ/eq_ratio_1` | eq ratio 1 | `0410[0]` | float | 1 … 10 | : 1 | verified (strip dump) | 2 |
| `/Matrix_Outputs/{n}/EQ/eq_ratio_2` | eq ratio 2 | `0410[1]` | float | 1 … 10 | : 1 | verified (strip dump) | 1.711 |
| `/Matrix_Outputs/{n}/EQ/eq_ratio_3` | eq ratio 3 | `0410[2]` | float | 1 … 10 | : 1 | verified (strip dump) | 2.423 |
| `/Matrix_Outputs/{n}/EQ/eq_ratio_4` | eq ratio 4 | `0410[3]` | float | 1 … 10 | : 1 | verified (strip dump) | 1.184 |
| `/Matrix_Outputs/{n}/EQ/eq_attack_1` | eq attack 1 | `0411[0]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.01 |
| `/Matrix_Outputs/{n}/EQ/eq_attack_2` | eq attack 2 | `0411[1]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.015 |
| `/Matrix_Outputs/{n}/EQ/eq_attack_3` | eq attack 3 | `0411[2]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.01159 |
| `/Matrix_Outputs/{n}/EQ/eq_attack_4` | eq attack 4 | `0411[3]` | float | 0.0005 … 0.1 | s | verified (strip dump) | 0.05 |
| `/Matrix_Outputs/{n}/EQ/eq_release_1` | eq release 1 | `0412[0]` | float | 0.01 … 10 | s | verified (strip dump) | 0.3 |
| `/Matrix_Outputs/{n}/EQ/eq_release_2` | eq release 2 | `0412[1]` | float | 0.01 … 10 | s | verified (strip dump) | 0.748 |
| `/Matrix_Outputs/{n}/EQ/eq_release_3` | eq release 3 | `0412[2]` | float | 0.01 … 10 | s | verified (strip dump) | 0.3754 |
| `/Matrix_Outputs/{n}/EQ/eq_release_4` | eq release 4 | `0412[3]` | float | 0.01 … 10 | s | verified (strip dump) | 0.9256 |
| `/Matrix_Outputs/{n}/EQ/eq_symm_Q_1` | eq symm Q 1 | `047c[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/EQ/eq_symm_Q_2` | eq symm Q 2 | `047c[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/EQ/eq_symm_Q_3` | eq symm Q 3 | `047c[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/EQ/eq_symm_Q_4` | eq symm Q 4 | `047c[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/EQ/dynamic_eq_on_1` | dynamic eq on 1 | `047d[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/EQ/dynamic_eq_on_2` | dynamic eq on 2 | `047d[1]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/EQ/dynamic_eq_on_3` | dynamic eq on 3 | `047d[2]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/EQ/dynamic_eq_on_4` | dynamic eq on 4 | `047d[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_in` | comp in | `0502[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/gate_in` | gate in | `0502[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_band_in_1` | comp band in 1 | `0503[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/Dynamics/comp_band_in_2` | comp band in 2 | `0503[1]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/Dynamics/comp_band_in_3` | comp band in 3 | `0503[2]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/Dynamics/comp_LP_crossover` | comp LP crossover | `0505[0]` | float | 20 … 20000 | Hz | verified (strip dump) | 130 |
| `/Matrix_Outputs/{n}/Dynamics/comp_HP_crossover` | comp HP crossover | `0505[1]` | float | 20 … 20000 | Hz | verified (strip dump) | 1000 |
| `/Matrix_Outputs/{n}/Dynamics/comp_gain_1` | comp gain 1 | `0509[0]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_gain_2` | comp gain 2 | `0509[1]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_gain_3` | comp gain 3 | `0509[2]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_gain_4` | comp gain 4 | `0509[3]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `—` | gate range | `050a[3]` | float | 0 … 90 | dB | not answered |  |
| `—` | gate-duck-comp | `050b[3]` | int | 0 … 2 |  | not answered |  |
| `/Matrix_Outputs/{n}/EQ/GR_meter_1 or /Matrix_Outputs/{n}/Dynamics/GR_meter_1` | GR meter 1 | `050c[0]` | meter |  |  | verified (strip dump) | <text> |
| `/Matrix_Outputs/{n}/EQ/GR_meter_3 or /Matrix_Outputs/{n}/Dynamics/GR_meter_3` | GR meter 3 | `050c[1]` | meter |  |  | verified (strip dump) | <text> |
| `/Matrix_Outputs/{n}/EQ/GR_meter_2 or /Matrix_Outputs/{n}/Dynamics/GR_meter_2` | GR meter 2 | `050c[2]` | meter |  |  | verified (strip dump) | <text> |
| `/Matrix_Outputs/{n}/EQ/GR_meter_4 or /Matrix_Outputs/{n}/Dynamics/GR_meter_4` | GR meter 4 | `050c[3]` | meter |  |  | verified (strip dump) | <text> |
| `—` | gate meter | `050d[3]` | meter |  |  | not answered |  |
| `/Matrix_Outputs/{n}/Dynamics/comp_thresh_1` | comp thresh 1 | `050e[0]` | float | -60 … 0 | dB | verified (strip dump) | -20 |
| `/Matrix_Outputs/{n}/Dynamics/comp_thresh_2` | comp thresh 2 | `050e[1]` | float | -60 … 0 | dB | verified (strip dump) | -20 |
| `/Matrix_Outputs/{n}/Dynamics/comp_thresh_3` | comp thresh 3 | `050e[2]` | float | -60 … 0 | dB | verified (strip dump) | -20 |
| `/Matrix_Outputs/{n}/Dynamics/gate_thresh` | gate thresh | `050e[3]` | float | -60 … 0 | dB | verified (strip dump) | -20.24 |
| `/Matrix_Outputs/{n}/Dynamics/comp_knee_1` | comp knee 1 | `050f[0]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_knee_2` | comp knee 2 | `050f[1]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_knee_3` | comp knee 3 | `050f[2]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_knee_4` | comp knee 4 | `050f[3]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_ratio_1` | comp ratio 1 | `0510[0]` | float | 1 … 50 | : 1 | verified (strip dump) | 3 |
| `/Matrix_Outputs/{n}/Dynamics/comp_ratio_2` | comp ratio 2 | `0510[1]` | float | 1 … 50 | : 1 | verified (strip dump) | 2 |
| `/Matrix_Outputs/{n}/Dynamics/comp_ratio_3` | comp ratio 3 | `0510[2]` | float | 1 … 50 | : 1 | verified (strip dump) | 2 |
| `/Matrix_Outputs/{n}/Dynamics/comp_ratio_4` | comp ratio 4 | `0510[3]` | float | 1 … 50 | : 1 | verified (strip dump) | 3 |
| `/Matrix_Outputs/{n}/Dynamics/comp_attack_1` | comp attack 1 | `0511[0]` | float | 2e-05 … 0.1 | s | verified (strip dump) | 0.01 |
| `/Matrix_Outputs/{n}/Dynamics/comp_attack_2` | comp attack 2 | `0511[1]` | float | 2e-05 … 0.1 | s | verified (strip dump) | 0.005 |
| `/Matrix_Outputs/{n}/Dynamics/comp_attack_3` | comp attack 3 | `0511[2]` | float | 2e-05 … 0.1 | s | verified (strip dump) | 0.025 |
| `/Matrix_Outputs/{n}/Dynamics/gate_attack` | gate attack | `0511[3]` | float | 5e-05 … 0.1 | s | verified (strip dump) | 0.002 |
| `/Matrix_Outputs/{n}/Dynamics/comp_release_1` | comp release 1 | `0512[0]` | float | 0.001 … 10 | s | verified (strip dump) | 0.5 |
| `/Matrix_Outputs/{n}/Dynamics/comp_release_2` | comp release 2 | `0512[1]` | float | 0.001 … 10 | s | verified (strip dump) | 0.2 |
| `/Matrix_Outputs/{n}/Dynamics/comp_release_3` | comp release 3 | `0512[2]` | float | 0.001 … 10 | s | verified (strip dump) | 0.4 |
| `/Matrix_Outputs/{n}/Dynamics/gate_release` | gate release | `0512[3]` | float | 0.005 … 10 | s | verified (strip dump) | 0.14 |
| `—` | gate hold | `0513[3]` | float | 0.002 … 2 | s | not answered |  |
| `/Matrix_Outputs/{n}/Dynamics/key_solo` | key solo | `0517[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_listen_1` | comp listen 1 | `0518[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_listen_2` | comp listen 2 | `0518[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_listen_3` | comp listen 3 | `0518[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/input_meter/left` | input meter/left | `0530[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Dynamics/input_meter/right` | input meter/right | `0534[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Dynamics/comp_all_gain` | comp all gain | `0538[0]` | float | 0 … 40 | dB | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_all_thresh` | comp all thresh | `0539[0]` | float | -60 … 0 | dB | verified (strip dump) | -20 |
| `/Matrix_Outputs/{n}/Dynamics/comp-multiband-desser` | comp-multiband-desser | `053a[0]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_auto-gain_1` | comp auto-gain 1 | `053b[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_auto-gain_2` | comp auto-gain 2 | `053b[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_auto-gain_3` | comp auto-gain 3 | `053b[2]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/comp_auto-gain_4` | comp auto-gain 4 | `053b[3]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Dynamics/desser_centre_freq` | desser centre freq | `057e[0]` | float | 0 … 255 |  | verified (strip dump) | 127 |
| `/Matrix_Outputs/{n}/Dynamics/gate_centre_freq` | gate centre freq | `057e[3]` | float | 0 … 255 |  | verified (strip dump) | 127 |
| `/Matrix_Outputs/{n}/Dynamics/desser_freq_width` | desser freq width | `057f[0]` | float | 0 … 255 |  | verified (strip dump) | 255 |
| `/Matrix_Outputs/{n}/Dynamics/gate_freq_width` | gate freq width | `057f[3]` | float | 0 … 255 |  | verified (strip dump) | 255 |
| `/Matrix_Outputs/{n}/Filters/hi_filter_in` | hi filter in | `0703[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Filters/lo_filter_in` | lo filter in | `0703[1]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/Filters/hi_filter_freq` | hi filter freq | `0705[0]` | float | 20 … 20000 | Hz | verified (strip dump) | 1.651e+04 |
| `/Matrix_Outputs/{n}/Filters/lo_filter_freq` | lo filter freq | `0705[1]` | float | 20 … 20000 | Hz | verified (strip dump) | 49.2 |
| `/Matrix_Outputs/{n}/Insert/insert_A_in` | insert A in | `0802[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/Insert/insert_B_in` | insert B in | `0802[1]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Channel_Delay/delay_on` | delay on | `0907[0]` | bool | 0 … 1 |  | verified (strip dump) | 1 |
| `/Matrix_Outputs/{n}/Channel_Delay/delay` | delay | `0908[0]` | float | 0 … 1.3 | s | verified (strip dump) | 0.001999 |
| `/Matrix_Outputs/{n}/Channel_Delay/fine_delay` | fine delay | `0917[0]` | float | -127 … 127 | samples | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Buss_Trim/trim` | trim | `0b01[0]` | float | -40 … 60 | dB | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Buss_Trim/phase` | phase | `0b02[0]` | int | 0 … 3 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Buss_Trim/pre_meter` | pre meter | `0b20[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Buss_Trim/pre_meter/right` | pre meter/right | `0b24[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Buss_Trim/post_meter` | post meter | `0b30[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Buss_Trim/post_meter/right` | post meter/right | `0b34[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Buss_Trim/tube_meter/left` | tube meter/left | `0b58[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Buss_Trim/tube_meter/right` | tube meter/right | `0b5c[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Buss_Trim/name` | name | `0bff[0]` | string |  |  | verified (strip dump) | <text> |
| `/Matrix_Outputs/{n}/CGs_level` | CGs level | `0c01[0]` | float | -150 … 10 | dB, % | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/hard_mute` | hard mute | `0c02[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/solo` | solo | `0c03[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/solo_1_or_2` | solo 1 or 2 | `0c0b[0]` | int | 0 … 2 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/CGs_mute` | CGs mute | `0c1c[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/fader` | fader | `0c21[0]` | float | -150 … 10 | dB, % | verified (strip dump) | -10 |
| `/Matrix_Outputs/{n}/mute` | mute | `0c22[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/fader_meter/left` | fader meter/left | `0c30[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/fader_meter/right` | fader meter/right | `0c34[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/alternate_solo` | alternate solo | `0ce4[0]` | bool | 0 … 1 |  | verified (strip dump) | 0 |
| `/Matrix_Outputs/{n}/Output/dir_meter/left` | dir meter/left | `0e28[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Output/dir_meter/right` | dir meter/right | `0e2c[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Output/meter/left` | meter/left | `0e40[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Output/meter2` | meter2 | `0e41[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Output/meter/centre` | meter/centre | `0e42[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Output/meter4` | meter4 | `0e43[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Output/meter/right` | meter/right | `0e44[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Output/meter/SL` | meter/SL | `0e45[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Output/meter/SR` | meter/SR | `0e46[0]` | meter |  |  | verified (strip dump) | [] |
| `/Matrix_Outputs/{n}/Output/meter/LFE` | meter/LFE | `0e47[0]` | meter |  |  | verified (strip dump) | [] |

### Matrix_Inputs (3)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `—` | name | `01ff[0]` | string |  |  | not answered |  |
| `/Matrix_Inputs/{n}/Matrix_Send/{k}/send_level` | send level | `0624[0]` | float | -150 … 10 | dB, % | verified (exact query) | 0 |
| `/Matrix_Inputs/{n}/Matrix_Send/{k}/send_on` | send on | `0625[0]` | bool | 0 … 1 |  | verified (exact query) | 1 |

### Control_Groups (7)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `/Control_Groups/{n}/solo` | solo | `1003[0]` | bool | 0 … 1 |  | verified (exact query) | 0 |
| `/Control_Groups/{n}/auto-mute` | auto-mute | `1016[0]` | bool | 0 … 1 |  | verified (exact query) | 0 |
| `/Control_Groups/{n}/mode` | mode | `1019[0]` | int | 0 … 2 |  | verified (exact query) | 1 |
| `/Control_Groups/{n}/aux_send` | aux send | `101e[0]` | float | -18 … 18 | dB | verified (exact query) | 0 |
| `/Control_Groups/{n}/fader` | fader | `1021[0]` | float | -150 … 10 | dB, % | verified (exact query) | 0.694 |
| `/Control_Groups/{n}/mute` | mute | `1022[0]` | bool | 0 … 1 |  | verified (exact query) | 1 |
| `/Control_Groups/{n}/name` | name | `10ff[0]` | string |  |  | verified (exact query) | <text> |

### Graphic_EQ (37)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `/Graphic_EQ/{n}/geq_in` | geq in | `0f02[0]` | bool | 0 … 1 |  | verified (exact query) | 1 |
| `/Graphic_EQ/{n}/geq_gain_1` | geq gain 1 | `0f07[0]` | float | -12 … 12 | dB | verified (exact query) | -12 |
| `/Graphic_EQ/{n}/geq_gain_2` | geq gain 2 | `0f07[1]` | float | -12 … 12 | dB | verified (exact query) | -12 |
| `/Graphic_EQ/{n}/geq_gain_3` | geq gain 3 | `0f07[2]` | float | -12 … 12 | dB | verified (exact query) | -1.43 |
| `/Graphic_EQ/{n}/geq_gain_4` | geq gain 4 | `0f07[3]` | float | -12 … 12 | dB | verified (exact query) | -0.9844 |
| `/Graphic_EQ/{n}/geq_gain_5` | geq gain 5 | `0f07[4]` | float | -12 … 12 | dB | verified (exact query) | -8.109 |
| `/Graphic_EQ/{n}/geq_gain_6` | geq gain 6 | `0f07[5]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_7` | geq gain 7 | `0f07[6]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_8` | geq gain 8 | `0f07[7]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_9` | geq gain 9 | `0f07[8]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_10` | geq gain 10 | `0f07[9]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_11` | geq gain 11 | `0f07[10]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_12` | geq gain 12 | `0f07[11]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_13` | geq gain 13 | `0f07[12]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_14` | geq gain 14 | `0f07[13]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_15` | geq gain 15 | `0f07[14]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_16` | geq gain 16 | `0f07[15]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_17` | geq gain 17 | `0f07[16]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_18` | geq gain 18 | `0f07[17]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_19` | geq gain 19 | `0f07[18]` | float | -12 … 12 | dB | verified (exact query) | -1.758 |
| `/Graphic_EQ/{n}/geq_gain_20` | geq gain 20 | `0f07[19]` | float | -12 … 12 | dB | verified (exact query) | -4.758 |
| `/Graphic_EQ/{n}/geq_gain_21` | geq gain 21 | `0f07[20]` | float | -12 … 12 | dB | verified (exact query) | -4.828 |
| `/Graphic_EQ/{n}/geq_gain_22` | geq gain 22 | `0f07[21]` | float | -12 … 12 | dB | verified (exact query) | -2.812 |
| `/Graphic_EQ/{n}/geq_gain_23` | geq gain 23 | `0f07[22]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_24` | geq gain 24 | `0f07[23]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_25` | geq gain 25 | `0f07[24]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_26` | geq gain 26 | `0f07[25]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_27` | geq gain 27 | `0f07[26]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_28` | geq gain 28 | `0f07[27]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_29` | geq gain 29 | `0f07[28]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_30` | geq gain 30 | `0f07[29]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_31` | geq gain 31 | `0f07[30]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/geq_gain_32` | geq gain 32 | `0f07[31]` | float | -12 … 12 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/input_meter` | input meter | `0f30[0]` | meter |  |  | verified (exact query) | [] |
| `/Graphic_EQ/{n}/geq__trim` | geq  trim | `0f3c[0]` | float | -18 … 18 | dB | verified (exact query) | 0 |
| `/Graphic_EQ/{n}/output_meter` | output meter | `0f40[0]` | meter |  |  | verified (exact query) | [] |
| `/Graphic_EQ/{n}/name` | name | `0fff[0]` | string |  |  | verified (exact query) | <text> |

### Solo (4)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `—` | solo | `1003[0]` | bool | 0 … 1 |  | not answered |  |
| `—` | fader | `1021[0]` | float | -150 … 10 | dB, % | not answered |  |
| `—` | mute | `1022[0]` | bool | 0 … 1 |  | not answered |  |
| `/Solo/{n}/name` | name | `10ff[0]` | string |  |  | verified (exact query) | <text> |

### strip type 0x0b (1)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `—` | name | `17ff[0]` | string |  |  | not answered |  |

### strip type 0x0c (1)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `—` | name | `01ff[0]` | string |  |  | not answered |  |

### Console (41)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `/Console/Session/Filename` | Session/Filename | `0000[0]` | string |  |  | from command set (not sent) |  |
| `/Console/Session/Filename/?` | Session/Filename/? | `0002[0]` | int |  |  | verified (query) | /Console/Session/Filename ["<text>"] |
| `/Console/Session/Name` | Session/Name | `0000[0]` | string |  |  | from command set (not sent) |  |
| `/Console/Session/Name/?` | Session/Name/? | `0002[0]` | int |  |  | verified (query) | /Console/Session/Name ["<text>"] |
| `/Console/Channels/?` | Channels/? | `0002[0]` | int |  |  | verified (query) | /Console/Input_Channels [90] |
| `/Console/Input_Channels` | Input Channels | `0001[0]` | int | 0 … 46280 |  | from command set (not sent) |  |
| `/Console/Aux_Outputs` | Aux Outputs | `0001[0]` | int | 0 … 30240 |  | from command set (not sent) |  |
| `/Console/Group_Outputs` | Group Outputs | `0001[0]` | int | 0 … 256 |  | from command set (not sent) |  |
| `/Console/Talkback_Outputs` | Talkback Outputs | `0001[0]` | int |  |  | from command set (not sent) |  |
| `/Console/Control_Groups` | Control Groups | `0001[0]` | bool | 0 … 1 |  | from command set (not sent) |  |
| `/Console/Matrix_Inputs` | Matrix Inputs | `0001[0]` | int |  |  | from command set (not sent) |  |
| `/Console/Matrix_Outputs` | Matrix Outputs | `0001[0]` | int | 0 … 36 |  | from command set (not sent) |  |
| `/Console/Graphic_EQ` | Graphic EQ | `0001[0]` | int | 0 … 36 |  | from command set (not sent) |  |
| `/Console/Multis` | Multis | `0001[0]` | int | 0 … 36 |  | from command set (not sent) |  |
| `/Console/Name` | Name | `0001[0]` | string |  |  | from command set (not sent) |  |
| `/Console/Name/?` | Name/? | `0002[0]` | int |  |  | verified (query) | /Console/Name ["<text>"] |
| `/Console/Session/!` | Session/! | `0003[0]` | int |  |  | from command set (not sent) |  |
| `/Console/Tubes/limit` | Tubes/limit | `0001[0]` | int | 0 … 255 |  | from command set (not sent) |  |
| `/Console/Tubes/limit/?` | Tubes/limit/? | `0002[0]` | int |  |  | verified (query) | /Console/Tubes/limit [255] |
| `/Console/DynamicEQs/limit` | DynamicEQs/limit | `0001[0]` | int | 0 … 255 |  | from command set (not sent) |  |
| `/Console/DynamicEQs/limit/?` | DynamicEQs/limit/? | `0002[0]` | int |  |  | verified (query) | /Console/DynamicEQs/limit [255] |
| `/Console/MultibandDynamics/limit` | MultibandDynamics/limit | `0001[0]` | int | 0 … 255 |  | from command set (not sent) |  |
| `/Console/MultibandDynamics/limit/?` | MultibandDynamics/limit/? | `0002[0]` | int |  |  | verified (query) | /Console/MultibandDynamics/limit [255] |
| `/Console/Input_Channels/modes` | Input Channels/modes | `0001[0]` | int | 1 … 2 |  | from command set (not sent) |  |
| `/Console/Input_Channels/modes/?` | Input Channels/modes/? | `0002[0]` | int |  |  | verified (query) | /Console/Input_Channels/modes [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,  |
| `/Console/Aux_Outputs/modes` | Aux Outputs/modes | `0001[0]` | int | 1 … 2 |  | from command set (not sent) |  |
| `/Console/Aux_Outputs/modes/?` | Aux Outputs/modes/? | `0002[0]` | int |  |  | verified (query) | /Console/Aux_Outputs/modes [1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2] |
| `/Console/Group_Outputs/modes` | Group Outputs/modes | `0001[0]` | int | 1 … 6 |  | from command set (not sent) |  |
| `/Console/Group_Outputs/modes/?` | Group Outputs/modes/? | `0002[0]` | int |  |  | verified (query) | /Console/Group_Outputs/modes [2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2 |
| `/Console/Monitoring/modes` | Monitoring/modes | `0001[0]` | int | 1 … 6 |  | from command set (not sent) |  |
| `/Console/Monitoring/modes/?` | Monitoring/modes/? | `0002[0]` | int |  |  | verified (query) | /Console/Monitoring/modes [2] |
| `/Console/Solo/modes` | Solo/modes | `0001[0]` | int | 1 … 6 |  | from command set (not sent) |  |
| `/Console/Solo/modes/?` | Solo/modes/? | `0002[0]` | int |  |  | verified (query) | /Console/Solo/modes [2, 2] |
| `/Console/Input_Channels/*/group_sends` | Input Channels/*/group_sends | `0001[0]` | int |  |  | from command set (not sent) |  |
| `/Console/Input_Channels/*/group_sends/?` | Input Channels/*/group_sends/? | `0002[0]` | int |  |  | no reply to query |  |
| `/Console/Group_Outputs/*/group_sends` | Group Outputs/*/group_sends | `0001[0]` | int |  |  | from command set (not sent) |  |
| `/Console/Group_Outputs/*/group_sends/?` | Group Outputs/*/group_sends/? | `0002[0]` | int |  |  | no reply to query |  |
| `/Console/Matrix_Inputs/*/send_levels` | Matrix Inputs/*/send_levels | `0001[0]` | int |  |  | from command set (not sent) |  |
| `/Console/Matrix_Inputs/*/send_levels/?` | Matrix Inputs/*/send_levels/? | `0002[0]` | int |  |  | no reply to query |  |
| `/Console/Matrix_Inputs/*/send_ons` | Matrix Inputs/*/send_ons | `0001[0]` | int |  |  | from command set (not sent) |  |
| `/Console/Matrix_Inputs/*/send_ons/?` | Matrix Inputs/*/send_ons/? | `0002[0]` | int |  |  | no reply to query |  |

### Snapshots (32)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `/Snapshots/Snapshots` | Snapshots | `0000[0]` | bool | 0 … 1 |  | from command set (not sent) |  |
| `/Snapshots/count` | count | `000b[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/count/?` | count/? | `000c[0]` | int |  |  | verified (query) | /Snapshots/count [40] |
| `/Snapshots/names/?` | names/? | `000f[0]` | int |  |  | verified (query) | /Snapshots/name [0, 100, 0, "<text>"] |
| `/Snapshots/name` | name | `0015[0]` | int | 0 … 9999 |  | from command set (not sent) |  |
| `/Snapshots/name/?` | name/? | `0016[0]` | int | 0 … 9999 |  | verified (query) | /Snapshots/name [0, 100, 0, "<text>"] |
| `/Snapshots/notes` | notes | `0033[0]` | int | 0 … 9999 |  | from command set (not sent) |  |
| `/Snapshots/notes/?` | notes/? | `0034[0]` | int | 0 … 9999 |  | verified (query) | /Snapshots/notes [0, 15, 16777215, 65793, "<text>"] |
| `/Snapshots/Surface_Snapshot` | Surface_Snapshot | `0038[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/Surface_Snapshot/?` | Surface_Snapshot/? | `0039[0]` | int |  |  | verified (query) | /Snapshots/Surface_Snapshot [4] |
| `/Snapshots/Current_Snapshot` | Current_Snapshot | `003d[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/Current_Snapshot/?` | Current_Snapshot/? | `003e[0]` | int |  |  | verified (query) | /Snapshots/Current_Snapshot [3] |
| `/Snapshots/Renumber_Snapshot` | Renumber Snapshot | `0097[0]` | int | 0 … 99999 |  | from command set (not sent) |  |
| `/Snapshots/Update_Snapshot_Group` | Update Snapshot Group | `00a1[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/Change_Surface_Snapshot` | Change Surface Snapshot | `00bb[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/Recall_Snapshot` | Recall Snapshot | `00c8[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/New_Snapshot` | New Snapshot | `00c9[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/Fire_Snapshot_number` | Fire Snapshot number | `00ca[0]` | int | 0 … 9999 |  | from command set (not sent) |  |
| `/Snapshots/Update_Current_Snapshot` | Update Current Snapshot | `00cb[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/Update_Snapshot` | Update Snapshot | `00cc[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/Rename_Snapshot` | Rename Snapshot | `00cd[0]` | string |  |  | from command set (not sent) |  |
| `/Snapshots/Delete_Snapshot` | Delete Snapshot | `00ce[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/Move_Snapshot` | Move Snapshot | `00cf[0]` | int | 0 … 9999 |  | from command set (not sent) |  |
| `/Snapshots/Insert_New_Snapshot` | Insert New Snapshot | `00d3[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/Update_Waves_only` | Update Waves only | `00d4[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/Fire_Selected_Snapshot` | Fire Selected Snapshot | `00d5[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/Fire_Prev_Snapshot` | Fire Prev Snapshot | `00e8[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/Fire_Next_Snapshot` | Fire Next Snapshot | `00e9[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/MIDI_fires_Snapshots` | MIDI fires Snapshots | `00ea[0]` | bool | 0 … 1 |  | from command set (not sent) |  |
| `/Snapshots/Fire_Snapshot_sends_MIDI` | Fire Snapshot sends MIDI | `00eb[0]` | bool | 0 … 1 |  | from command set (not sent) |  |
| `/Snapshots/Undo_Snapshot` | Undo Snapshot | `00fb[0]` | int |  |  | from command set (not sent) |  |
| `/Snapshots/End_Recall_Snapshot` | End Recall Snapshot | `00ff[0]` | int |  |  | from command set (not sent) |  |

### Macros (13)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `/Macros/count` | count | `000b[0]` | int |  |  | from command set (not sent) |  |
| `/Macros/count/?` | count/? | `000c[0]` | int |  |  | verified (query) | /Macros/count [22] |
| `/Macros/names/?` | names/? | `000f[0]` | int |  |  | verified (query) | /Macros/name [0, "<text>"] |
| `/Macros/name` | name | `0015[0]` | int | 0 … 9999 |  | from command set (not sent) |  |
| `/Macros/name/?` | name/? | `0016[0]` | int | 0 … 9999 |  | verified (query) | /Macros/name [0, "<text>"] |
| `/Macros/Recall_Macro` | Recall Macro | `00c8[0]` | int |  |  | from command set (not sent) |  |
| `/Macros/Buttons/count` | Buttons/count | `00d3[0]` | int |  |  | from command set (not sent) |  |
| `/Macros/Buttons/count/?` | Buttons/count/? | `00d4[0]` | int |  |  | verified (query) | /Macros/Buttons/count [0] |
| `/Macros/Buttons/?` | Buttons/? | `00d9[0]` | int |  |  | no reply to query |  |
| `/Macros/Buttons/!` | Buttons/! | `00da[0]` | int |  |  | from command set (not sent) |  |
| `/Macros/Buttons/state` | Buttons/state | `00dd[0]` | int | 0 … 255 |  | from command set (not sent) |  |
| `/Macros/Buttons/state/?` | Buttons/state/? | `00de[0]` | int | 0 … 255 |  | no reply to query |  |
| `/Macros/Buttons/press` | Buttons/press | `00fa[0]` | int | 0 … 255 |  | from command set (not sent) |  |

### Presets (13)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `/Presets/count` | count | `000b[0]` | int |  |  | from command set (not sent) |  |
| `/Presets/count/?` | count/? | `000c[0]` | int |  |  | verified (query) | /Presets/count [34] |
| `/Presets/names/?` | names/? | `000f[0]` | int |  |  | verified (query) | /Presets/name [0, 1, 2, "<text>", "<text>", "<text>"] |
| `/Presets/name` | name | `0015[0]` | int | 0 … 9999 |  | from command set (not sent) |  |
| `/Presets/name/?` | name/? | `0016[0]` | int | 0 … 9999 |  | verified (query) | /Presets/name [0, 1, 2, "<text>", "<text>", "<text>"] |
| `/Presets/Recall_Preset` | Recall Preset | `00c8[0]` | string |  |  | from command set (not sent) |  |
| `/Presets/New_Preset` | New Preset | `00c9[0]` | string |  |  | from command set (not sent) |  |
| `/Presets/Update_Preset` | Update Preset | `00cc[0]` | string |  |  | from command set (not sent) |  |
| `/Presets/Rename_Preset` | Rename Preset | `00ce[0]` | string |  |  | from command set (not sent) |  |
| `/Presets/Rename_Preset_Group` | Rename Preset Group | `00cf[0]` | string |  |  | from command set (not sent) |  |
| `/Presets/Delete_Preset` | Delete Preset | `00d1[0]` | int |  |  | from command set (not sent) |  |
| `/Presets/Lock_Preset` | Lock Preset | `00d2[0]` | bool | 0 … 1 |  | from command set (not sent) |  |
| `/Presets/Recall_Scope` | Recall Scope | `00d3[0]` | int | 0 … 2 |  | from command set (not sent) |  |

### Layout (5)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `/Layout/Layout/Banks` | Layout/Banks | `0001[0]` | int |  |  | from command set (not sent) |  |
| `/Layout/Layout/Banks/?` | Layout/Banks/? | `0002[0]` | int |  |  | verified (query) | /Layout/Layout/Banks ["<text>", "<text>", 0, 0, "<text>", 1, "<text>", |
| `/Layout/Layout/Banks/!` | Layout/Banks/! | `0003[0]` | int |  |  | from command set (not sent) |  |
| `/Layout/Fader_Banks` | Fader Banks | `0000[0]` | bool | 0 … 1 |  | from command set (not sent) |  |
| `/Layout/Channel_List` | Channel List | `0000[0]` | bool | 0 … 1 |  | from command set (not sent) |  |

### console 0xc9 (2)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `—` | Save Session | `0000[0]` | bool | 0 … 1 |  | prefix unknown |  |
| `—` | Save current Session | `0000[0]` | int |  |  | prefix unknown |  |

### console 0xcc (1)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `—` | Snapshot Notes view | `0000[0]` | bool | 0 … 1 |  | prefix unknown |  |

### console 0xcf (3)

| OSC path | Command | ID[idx] | Type | Range | Units | Status | Value seen |
|---|---|---|---|---|---|---|---|
| `—` | clear | `0000[0]` | int |  |  | prefix unknown |  |
| `—` | request | `0000[0]` | string |  |  | prefix unknown |  |
| `—` | values | `0000[0]` | int |  |  | prefix unknown |  |
