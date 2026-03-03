# S21 HiJack

Middleware daemon and GUI for **DiGiCo S21/S31** mixing consoles. Sits between the console and your workflow tools to add snapshot/cue management, macros, EQ palettes, smart ganging, personal monitoring, and QLab integration — features the console doesn't natively provide.

Communicates with the console over **GP OSC** (the documented open protocol) and optionally the **iPad protocol** (reverse-engineered) for parameters that GP OSC doesn't cover.

## Features

### Snapshots & Cue List
- Capture the live console state as a named snapshot, scoped to specific channels and parameter sections
- Organize snapshots into a numbered cue list with Go Next / Go Previous / Fire by number
- Scope templates — reusable channel/section selections for consistent captures
- Fade interpolation — continuous parameters (faders, gains, EQ) crossfade over a configurable time; discrete parameters (mute, solo) fire instantly at the start

### Macros
- Record a sequence of console parameter changes in learn mode
- Three step modes: **Fixed** (absolute value), **Relative** (delta from current), **Toggle** (flip on/off)
- Execute macros on demand or via OSC trigger
- Quick-trigger slots for frequently used macros

### EQ Palettes
- Capture the EQ section of a channel as a reusable palette
- Link palettes to snapshots — when a palette is updated, all linked snapshots inherit the new EQ values on recall
- Useful for maintaining consistent EQ across multiple scenes

### Smart Ganging
- Link channels together with selective parameter sections (e.g. gang faders but not EQ)
- Bidirectional relative propagation — move one fader and the others follow by the same delta
- Mixed channel types supported (e.g. inputs + auxes in the same gang); routing sections automatically restrict to same-type members
- Anti-feedback suppression prevents infinite loops from console echo-back
- Enable/disable gangs on the fly

### Personal Monitoring (Aux Sends)
- Define named monitor clients with permitted aux sends and visible input channels
- Performers connect via OSC from any device and can only adjust their own mix
- Permission validation — each client can only touch the auxes they're assigned to
- FOH changes automatically echo to connected clients

### QLab / External Trigger Integration
- OSC trigger listener accepts `/cue/go`, `/cue/previous`, `/cue/fire/{number}`, `/cue/current`, and `/macro/fire/{name}`
- Drive cue recall from QLab, companion apps, Stream Deck, or any OSC sender

### iPad Protocol Support (Modes 2 & 3)
- **Mode 1**: GP OSC only (default)
- **Mode 2**: Direct iPad protocol connection to the console — accesses parameters that GP OSC doesn't expose (phantom power, insert slots, stereo mode, etc.)
- **Mode 3**: iPad proxy — sits between a real iPad and the console, forwarding traffic while maintaining the state mirror

### Show File Persistence
- Save/load the entire show (snapshots, cue list, macros, palettes, monitor clients, gang groups) as a single JSON file
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

This opens the native desktop UI with tabs for Setup, Snapshots, Macros, Live, Gangs, and Monitor.

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
| `/cue/fire/{number}` | — | Fire a specific cue by number |
| `/cue/current` | — | Query current cue number (replies to sender) |
| `/macro/fire/{name}` | — | Execute a macro by name or ID |

## Monitor Client OSC Protocol

Performers connect to the monitor port with these messages:

| OSC Path | Args | Action |
|---|---|---|
| `/monitor/{name}/connect` | — | Register/reconnect a client |
| `/monitor/{name}/aux/{n}/send/{input}/level` | float | Set send level |
| `/monitor/{name}/aux/{n}/send/{input}/pan` | float | Set send pan |
| `/monitor/{name}/aux/{n}/send/{input}/on` | int (0/1) | Set send on/off |
| `/monitor/{name}/request_state` | — | Request full current state |

## Project Structure

```
src/
  main.rs              Entry point (CLI args, headless/UI modes)
  model/               Data model (channels, parameters, snapshots, macros, gangs, etc.)
  osc/                 OSC protocol (GP OSC + iPad encode/parse/client)
  console/             Business logic (connection, engines, managers)
  persistence/         Show file save/load
  ui/                  Native egui desktop UI
  bin/mock_console.rs  Mock console simulator
Documentation/
  PRD.md               Full product requirements document
```

## Running Tests

```bash
cargo test
```

208 tests covering the data model, OSC protocol parsing/encoding, engine logic, persistence backward compatibility, and UI parsing utilities.

## Target Platforms

- **Raspberry Pi** (Linux ARM) — primary deployment target, runs headless as a daemon
- **macOS / Windows** — development and GUI use
- Cross-compile for Pi with `cross build --release --target armv7-unknown-linux-gnueabihf` or build natively on the Pi

## License

See [LICENSE](LICENSE) for details.
