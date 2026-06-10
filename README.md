# S21 HiJack

Middleware daemon and GUI for **DiGiCo S21/S31** mixing consoles. Sits between the console and your workflow tools to add snapshot/cue management, macros, channel palettes, smart ganging, pan link, personal monitoring (native app **and** browser), and QLab integration — features the console doesn't natively provide.

Communicates with the console over **GP OSC** (the documented open protocol) and optionally the **iPad protocol** (reverse-engineered) for parameters that GP OSC doesn't cover.

## Features

### Snapshots & Cue List
- Capture the live console state as a named snapshot, scoped to specific channels and parameter sections
- Organize snapshots into a numbered cue list with Go Next / Go Previous / Fire by number
- Scope templates — reusable channel/section selections for consistent captures
- Fade interpolation — continuous parameters (faders, gains, EQ) crossfade over a configurable time; discrete parameters (mute, solo) fire instantly at the start
- One-level undo — every recall stashes the prior live values so an accidental fire can be rolled back
- Inter-message pacing — configurable microsecond delay between sends during large recalls to avoid flooding the console's ARM chip
- **Auto-save on recall** — optional: when advancing to a new snapshot, dirty parameters within the previous snapshot's scope template are automatically written back into it. Mid-show tweaks ride along without manual re-capture
- **Console memory link** — each app snapshot can optionally reference a console memory row. On recall, the daemon fires the console's native memory load via the iPad protocol, waits for the console echo flood to settle (~250 ms), then writes the app's captured parameters on top. State-aware dedup means re-recalling the same snapshot does not re-fire the console memory
- **Follow mode** — when the operator drives the console's own snapshot list, the daemon mirrors the change by recalling the first matching app snapshot in cue order. Self-fired echoes are suppressed so there is no feedback loop
- **Renumbering helper** — bulk-shift every snapshot's console memory reference after inserting or deleting a row on the console (rows are positional; inserts shift everything below)

### Offline Mode
- Top-bar toggle that suspends **all** OSC traffic in both directions: outbound sends are dropped before they hit the socket, inbound packets are dropped before parsing, the state mirror freezes, gangs and pan link stop propagating
- Lets the operator edit show data — snapshots, scopes, palettes, gangs, pan links, console memory refs — without touching the live desk
- A persistent banner across every tab makes the state unmistakable
- Defaults to OFF on every startup; not persisted

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
- **Live in-session ripple** — while mixing, the tweaks you make to a linked channel are absorbed into the palette in real time (tracking a continuous knob sweep, not just the first touch), so every snapshot sharing that palette picks them up on its next recall without re-capturing
- These live edits stay in a working overlay until you commit them: **Store changes** (this palette) / **Store all palette changes** (every palette) write them into the saved values, or **Revert changes** reloads the last-stored values to the desk and discards the overlay

### Smart Ganging
- Link channels together with selective parameter sections (e.g. gang faders but not EQ)
- Two propagation modes: **Relative** (apply the delta to each member) and **Absolute** (snap members to the source value)
- Mixed channel types supported (e.g. inputs + auxes in the same gang); routing sections automatically restrict to same-type members
- **Independent pan toggle** — pan ganging is set separately from fader/mute, with three states: **Off**, **On** (follows the gang's relative/absolute mode), and **Reversed** (mirror — moving one member pushes the other the opposite way, for symmetric pairs)
- **Inaudible-floor aware faders** — gang fader moves collapse the dead bottom of the fader track: nudging a parked (−∞) fader doesn't drag its siblings, and pulling a member below audibility snaps it fully off rather than spreading a meaningless delta
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
- Permission validation — each client can only touch the auxes they're assigned to and the inputs the profile makes visible (re-checked server-side on every command)
- FOH changes automatically echo to connected clients, **across transports** — a move on a phone shows up on the desk app and on other performers' devices, and vice-versa
- **Two ways for performers to connect:**
  - **Browser — no install:** open `http://<daemon-ip>:8080` on a phone or tablet, pick your profile (and PIN, if the operator set one), and get a touch fader surface for your permitted sends — wide relative faders, pan, on/off, and an aux-master strip. The desktop UI's Monitor tab shows the per-interface URLs and a **per-profile QR code** that deep-links the name (and PIN), so a musician just scans it to connect. Served over HTTP + WebSocket with the web UI embedded in the binary — nothing extra to deploy (no Node, no separate web server)
  - **Native app:** a companion **Flutter monitor app** (`monitor_app/`) for Android/iOS performers over OSC/UDP — see its own README for build instructions
- **Optional per-profile PIN** hardens the browser login; the native UDP path stays name-only
- **LAN use only** (plain HTTP). Optional source-IP CIDR allowlists gate both the UDP (`--monitor-allow-cidr`) and web (`--web-allow-cidr`) servers

### QLab / External Trigger Integration
- **Inbound (trigger listener)**: OSC trigger listener accepts `/cue/go`, `/cue/previous`, `/cue/fire`, `/cue/current`, `/macro/fire`, `/snapshot/recall`, and `/snapshot/recall_full`. Drive cue recall from QLab, companion apps, Stream Deck, or any OSC sender — see the [OSC Trigger Commands](#osc-trigger-commands) table for argument types.
- **Outbound (QLab cue export)**: build QLab cue lists from app snapshots — emits `/new` + `/cue/selected/*` sequences that QLab consumes to create network cues (one per snapshot or per parameter), deterministically nested under a parent group cue. The exporter is **reply-driven**: it queries each created cue's `uniqueID`, waits for QLab's reply, then issues `/move` to place children in the group — so nesting no longer depends on QLab's insertion cursor. Each child cue can target either the app or the console via a configurable **QLab network patch** (one patch for the `/snapshot/recall` trigger cues pointed back at the daemon, another for the per-parameter GP OSC cues pointed at the console). Useful for building a QLab show that drives the console mix without re-keying snapshot names.

### iPad Protocol Support (Modes 2 & 3)
- **Mode 1**: GP OSC only (default)
- **Mode 2**: Direct iPad protocol connection to the console — accesses parameters that GP OSC doesn't expose (phantom power, insert slots, stereo mode, etc.)
- **Mode 3**: iPad proxy — sits between a real iPad and the console, forwarding traffic while maintaining the state mirror

### Diagnostics
- **OSC Log tab**: live tail of every inbound and outbound OSC message, useful for protocol debugging and reverse-engineering — GP OSC and the DiGiCo iPad protocol are colour-coded separately, with independent per-direction filter toggles for each (incl. iPad → Console and Console → iPad)
- **Inspector tab**: searchable view of the full state mirror — every (channel, parameter) → value the daemon currently knows about

### Help-Bubble Localization
- Every interactive control has a hover help bubble (tooltip). The main UI stays **English**; only the help bubbles localize, with English as the always-present per-key fallback
- Translations are plain JSON files (one per language) loaded at runtime from `assets/locales/` (shipped), a `locales/` folder beside the executable, or a per-machine `locales/` folder next to `preferences.json` — drop a file in to add or edit a language, **no rebuild**
- Bundled: **English** (reference) and **French** (proofread), plus machine-translated **German, Spanish, Italian, Portuguese, Chinese, Japanese, Korean** (awaiting native proofreading). Choose one in **Setup → Advanced… → Help bubbles** (also offered to performers in the browser monitor client)
- `s21_hijack --dump-help-template` prints the full English key list to seed a new translation; see [docs/install.md](docs/install.md) for the workflow
- Per-language **Noto Sans CJK** subsets (SC / JP / KR) are embedded so Chinese, Japanese and Korean each render with their region's glyph shapes; the app swaps the active subset when you change the help language. Each subset is tiny (~40–90 KB — only the glyphs that locale uses). Regenerate them with `python scripts/subset_cjk.py` (needs `fonttools`) if the zh/ja/ko text changes

### Show File Persistence
- Save/load the entire show (snapshots, cue list, macros, palettes, monitor clients, gang groups, pan links, recall safes, console memory refs, workflow toggles, console layout) as a single JSON file
- Console variant (S21 / S21+) and channel layout are restored on load, so offline editing reflects the saved console shape until you reconnect
- Backward-compatible — older show files load cleanly with sensible defaults

## Getting Started

### Prerequisites

- **Rust toolchain** 1.88 or newer (the `rust-version` MSRV in [Cargo.toml](Cargo.toml); edition 2024). Install via [rustup](https://rustup.rs/):
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
| `--monitor-port` | `8025` | Native (UDP/OSC) monitor server port (0 = disabled) |
| `--monitor-allow-cidr` | — | Repeatable: source-IP CIDR allowed to reach the monitor server (empty = accept all) |
| `--web-port` | `8080` | Browser (HTTP/WebSocket) monitor server port (0 = disabled) |
| `--web-allow-cidr` | — | Repeatable: source-IP CIDR allowed to reach the web monitor server (empty = accept all) |
| `--trigger-allow-cidr` | — | Repeatable: source-IP CIDR allowed to reach the trigger listener (empty = accept all) |
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

The **native** app connects to the UDP monitor port (`--monitor-port`) with these OSC messages; `{name}` is the client's configured monitor profile name. (Browser clients instead speak a JSON protocol over the WebSocket at `/ws` on the web port — a 1:1 mapping of the same commands, units identical; see [src/web/protocol.rs](src/web/protocol.rs).)

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
  web/                 Browser monitor server — axum HTTP + WebSocket, JSON
                       protocol (protocol.rs), embedded static UI (assets.rs)
  bin/mock_console.rs  Mock console simulator for testing without hardware
web/
  dist/index.html      Browser monitor touch UI (HTML/CSS/JS) — embedded into
                       the binary at build time via rust-embed (no Node/build step)
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

560+ tests covering the data model, OSC protocol parsing/encoding, engine logic (incl. gang fader-floor and pan-mode propagation, palette ripple, QLab reply handling), persistence backward compatibility, the web monitor protocol + UDP fan-out (cross-transport echo), and UI parsing utilities. Continuous integration runs `cargo check`, `cargo test`, `cargo fmt --check`, `cargo clippy`, and `cargo audit` on a Linux/macOS/Windows matrix on every push and PR — see [.github/workflows/ci.yml](.github/workflows/ci.yml).

## Stream Deck integration (Linux setup)

The Macros tab can drive an Elgato Stream Deck (Original / Mini / XL /
MK.2 / Plus / Pedal) — each physical button maps to a sequence of
macros, the LCD shows the next-to-fire macro's name, and pressing the
button fires it and advances to the next step.

On Linux, USB HID devices are root-only by default. Install the
shipped udev rules to grant non-root access (one-time setup):

```bash
sudo cp assets/udev/40-elgato-streamdeck.rules /etc/udev/rules.d/
sudo udevadm control --reload && sudo udevadm trigger
# Then unplug and replug the Stream Deck.
```

macOS and Windows work out of the box — no extra setup needed.

## Target Platforms

- **Raspberry Pi** (Linux ARM) — primary deployment target, runs headless as a daemon
- **macOS / Windows / Linux desktop** — development and GUI use
- Each release ships prebuilt Linux tarballs for **x86_64** and **aarch64/arm64** (Pi 3/4/5 and other 64-bit-OS SBCs) — download, extract, and run `./install-linux.sh`. For 32-bit Pi OS, cross-compile with `cross build --release --target armv7-unknown-linux-gnueabihf` or build natively on the Pi.
- Linux desktop integration (icon, .desktop entry) is provided by `install-linux.sh`

## License

Dual-licensed under either of:

- **MIT License** — see [LICENSE-MIT](LICENSE-MIT)
- **Apache License, Version 2.0** — see [LICENSE-APACHE](LICENSE-APACHE)

at your option. This is the standard Rust-ecosystem dual-license — pick whichever fits your downstream needs.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work shall be dual-licensed as above, without any additional terms or conditions.
