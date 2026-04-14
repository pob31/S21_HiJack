# S21 HiJack

Middleware daemon and GUI for **DiGiCo S21/S31** mixing consoles. Sits between the console and your workflow tools to add snapshot/cue management, macros, channel palettes, smart ganging, pan link, personal monitoring, and QLab integration — features the console doesn't natively provide.

Communicates with the console over **GP OSC** (the documented open protocol) and optionally the **iPad protocol** (reverse-engineered) for parameters that GP OSC doesn't cover.

## Features

### Snapshots & Cue List
- Capture the live console state as a named snapshot, scoped to specific channels and parameter sections
- Organize snapshots into a numbered cue list with Go Next / Go Previous / Fire by number
- Scope templates — reusable channel/section selections for consistent captures
- Fade interpolation — continuous parameters (faders, gains, EQ) crossfade over a configurable time; discrete parameters (mute, solo) fire instantly at the start
- One-level undo — every recall stashes the prior live values so an accidental fire can be rolled back
- Inter-message pacing — configurable microsecond delay between sends during large recalls to avoid flooding the console's ARM chip

### Scope Editor & Recall Safes
- Per-cell parameter scope editor with channel × parameter matrix, grouped by signal-flow section
- "Select modified" + auto-preselect — the dirty tracker watches every parameter that changes since the last recall so you can snapshot exactly what you've been touching
- Visual recall scope and per-channel recall safes that mirror the console's surface (visualization aid; the console doesn't expose these over OSC)
- Both round-trip in the show file

### Macros
- Record a sequence of console parameter changes in learn mode
- Three step modes: **Fixed** (absolute value), **Relative** (delta from current), **Toggle** (flip on/off)
- Execute macros on demand or via OSC trigger
- Quick-trigger slots for frequently used macros

### Channel Palettes (EQ / Compressor / Gate)
- Capture the EQ, Dynamics 1 (compressor), or Dynamics 2 (gate) section of a channel as a reusable palette
- Link palettes to snapshots — when a palette is updated, all linked snapshots inherit the new values on recall
- Useful for maintaining consistent processing across multiple scenes without re-saving every snapshot

### Smart Ganging
- Link channels together with selective parameter sections (e.g. gang faders but not EQ)
- Two propagation modes: **Relative** (apply the delta to each member) and **Absolute** (snap members to the source value)
- Mixed channel types supported (e.g. inputs + auxes in the same gang); routing sections automatically restrict to same-type members
- Anti-feedback suppression prevents infinite loops from console echo-back
- Enable/disable gangs on the fly, or pause individual gangs without breaking them for mid-show edits

### Pan Link
- Bind an input channel's main pan to selected stereo aux send pans — useful when an effects send needs to follow the channel pan automatically
- Per-input configuration; any number of stereo auxes can be linked to the same input
- Absolute, unidirectional propagation: moving the input pan snaps every linked aux send pan to the same value; moving an aux send pan manually acts as a temporary override and does not propagate back
- Adapts to the discovered console shape — only stereo aux buses are linkable, and the UI greys out mono auxes
- Skipped during snapshot/cue/macro recalls so memory recalls are never altered
- Bindings are persisted in the show file

### Personal Monitoring (Aux Sends)
- Define named monitor clients with permitted aux sends and visible input channels
- Performers connect via OSC from any device and can only adjust their own mix
- Permission validation — each client can only touch the auxes they're assigned to
- FOH changes automatically echo to connected clients
- A companion **Flutter monitor app** (`monitor_app/`) ships alongside for Android/iOS performers — see its own README for build instructions

### QLab / External Trigger Integration
- OSC trigger listener accepts `/cue/go`, `/cue/previous`, `/cue/fire`, `/cue/current`, `/macro/fire`, `/snapshot/recall`, and `/snapshot/recall_full`
- Drive cue recall from QLab, companion apps, Stream Deck, or any OSC sender — see the [OSC Trigger Commands](#osc-trigger-commands) table for argument types

### iPad Protocol Support (Modes 2 & 3)
- **Mode 1**: GP OSC only (default)
- **Mode 2**: Direct iPad protocol connection to the console — accesses parameters that GP OSC doesn't expose (phantom power, insert slots, stereo mode, etc.)
- **Mode 3**: iPad proxy — sits between a real iPad and the console, forwarding traffic while maintaining the state mirror

### Diagnostics
- **OSC Log tab**: live tail of every inbound and outbound OSC message with filtering, useful for protocol debugging and reverse-engineering
- **Inspector tab**: searchable view of the full state mirror — every (channel, parameter) → value the daemon currently knows about

### Show File Persistence
- Save/load the entire show (snapshots, cue list, macros, palettes, monitor clients, gang groups, pan links, recall safes, console layout) as a single JSON file
- Console variant (S21 / S21+) and channel layout are restored on load, so offline editing reflects the saved console shape until you reconnect
- Backward-compatible — older show files load cleanly with sensible defaults

## Getting Started

### Prerequisites

- **Rust toolchain** (1.75+ recommended). Install via [rustup](https://rustup.rs/):
  ```
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```
- A **DiGiCo S21 or S31** console on the same network, or use the included mock console for testing

### Build

```bash
git clone https://github.com/pob31/S21_HiJack.git
cd S21_HiJack
cargo build --release
```

The binary is at `target/release/s21_hijack` (Linux/macOS) or `target/release/s21_hijack.exe` (Windows).

### Run (GUI mode)

```bash
cargo run --release -- --console-ip 192.168.1.1
```

This opens the native desktop UI with tabs for Setup, Snapshots, Macros, Live, Gangs, Pan Link, Monitor, OSC Log, and Inspector.

### Run (headless daemon)

```bash
cargo run --release -- --headless --console-ip 192.168.1.1
```

Runs without a GUI — useful for Raspberry Pi or server deployments. All features are available via the OSC trigger interface and show files.

### Testing without hardware

A mock console simulator is included:

```bash
# Terminal 1 — start the mock console
cargo run --bin mock_console -- --port 8000

# Terminal 2 — connect to it
cargo run --release -- --console-ip 127.0.0.1 --console-port 8000
```

## CLI Arguments

| Argument | Default | Description |
|---|---|---|
| `--console-ip` | `192.168.1.1` | Console IP address |
| `--console-port` | `8000` | Console GP OSC port |
| `--local-port` | `8001` | Local UDP port to bind |
| `--trigger-port` | `53001` | QLab/OSC trigger listener port |
| `--mode` | `mode1` | Operating mode: `mode1`, `mode2`, `mode3` |
| `--ipad-send-port` | — | Console's iPad protocol port (send target) |
| `--ipad-receive-port` | — | Local port to receive iPad protocol messages |
| `--ipad-port` | `0` | Legacy: single port for both send/receive |
| `--ipad-ip` | — | iPad device IP (for Mode 3 proxy) |
| `--monitor-port` | `0` | Personal monitor server port (0 = disabled) |
| `--headless` | `false` | Run without the GUI |

## OSC Trigger Commands

Send these to the trigger port (default 53001) from QLab, Stream Deck, or any OSC source:

| OSC Path | Args | Action |
|---|---|---|
| `/cue/go` | — | Advance to next cue and recall |
| `/cue/previous` | — | Go back to previous cue and recall |
| `/cue/fire` | int or float | Fire a specific cue by number |
| `/cue/current` | — | Query current cue number (replies to sender on `/cue/current`) |
| `/macro/fire` | string or int | Execute a macro by name or ID |
| `/snapshot/recall` | string | Recall a snapshot by name or UUID, honouring its scope |
| `/snapshot/recall_full` | string | Recall a snapshot by name or UUID, ignoring its scope (full state) |

## Monitor Client OSC Protocol

Performers connect to the monitor port with these messages. `{name}` is the client's configured monitor profile name.

| OSC Path | Args | Action |
|---|---|---|
| `/monitor/discover` | — | Broadcast discovery — server replies with its address |
| `/monitor/{name}/connect` | — | Register/reconnect a client |
| `/monitor/{name}/state` | — | Request full current state for this client |
| `/monitor/{name}/send/{input}/{aux}/level` | float | Set send level (input → aux) |
| `/monitor/{name}/send/{input}/{aux}/pan` | float | Set send pan (input → aux) |
| `/monitor/{name}/send/{input}/{aux}/on` | bool / int (0/1) | Set send on/off |
| `/monitor/{name}/aux/{n}/fader` | float | Set the aux master fader |
| `/monitor/{name}/aux/{n}/mute` | bool / int (0/1) | Mute/unmute the aux master |
| `/status/console` | — | Query console connection status (replies to sender) |
| `/status/clients` | — | Query connected client count (replies to sender) |

Permission validation: each client can only touch the auxes assigned to its profile and the inputs the profile makes visible.

## Project Structure

```
src/
  main.rs              Entry point (CLI args, headless/UI modes)
  model/               Data model — channels, parameters, snapshots, macros,
                       gangs, pan_link, palettes, recall_scope, dirty_tracker,
                       config (incl. PlusMode), monitor profiles
  osc/                 OSC protocol — GP OSC + iPad encode/parse/client,
                       trigger listener, monitor server
  console/             Business logic — connection manager, snapshot/macro/
                       monitor/gang/pan_link engines, fade controller,
                       palette manager, iPad handshake & proxy
  persistence/         Show file save/load (versioned, backward-compatible)
  ui/                  Native egui desktop UI (one file per tab + scope editor)
  bin/mock_console.rs  Mock console simulator for testing without hardware
Documentation/
  PRD.md                                       Full product requirements document
  GP_OSC_help.jpeg                             GP OSC protocol reference image
  iPad_commands.png, iPad_handshake.txt        Reverse-engineered iPad protocol notes
  DiGiCo S OSC Commandset_*.csv                Console parameter address tables
```

## Running Tests

```bash
cargo test
```

330 tests covering the data model, OSC protocol parsing/encoding, engine logic, persistence backward compatibility, and UI parsing utilities.

## Target Platforms

- **Raspberry Pi** (Linux ARM) — primary deployment target, runs headless as a daemon
- **macOS / Windows / Linux desktop** — development and GUI use
- Cross-compile for Pi with `cross build --release --target armv7-unknown-linux-gnueabihf` or build natively on the Pi
- Linux desktop integration (icon, .desktop entry) is provided by `install-linux.sh`

## License

See [LICENSE](LICENSE) for details.
