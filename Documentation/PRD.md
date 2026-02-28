# DiGiCo S Series Snapshot Manager — Product Requirements Document

## 1. Overview

### 1.1 Problem Statement

DiGiCo S series consoles (S21, S31) have a limited snapshot system compared to the higher-end SD and Quantum ranges. The built-in snapshot scope is set per channel and applies globally across the entire show — there is no way to vary which parameters are recalled on a per-cue basis. The SD/Quantum theatre option offers features like EQ palettes (linked EQ references across cues) and cast variants (per-performer parameter sets), but these are unavailable on the S series.

### 1.2 Solution

A middleware application ("the daemon") that sits alongside the console on the network, maintains a live mirror of console state via OSC, and provides a full-featured snapshot/cue system with per-cue scope control. The application integrates with QLab as a trigger source while maintaining all snapshot data and logic internally.

### 1.3 Target Consoles

- **DiGiCo S21**: 2 screens, 20 faders (10 per screen), 48 inputs / 16 mix channels (expandable to 60 inputs / 24 mix channels with paid upgrade)
- **DiGiCo S31**: 3 screens, 30 faders (10 per screen), same channel count options as S21

### 1.4 Technology Stack

| Component | Technology | Platform |
|---|---|---|
| Daemon + main UI | Rust | Raspberry Pi (Linux) or macOS |
| Personal monitoring app | Flutter/Dart | iOS + Android |
| Console communication | OSC over UDP | — |
| QLab integration | OSC listener | — |

---

## 2. Architecture

### 2.1 System Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                        NETWORK (UDP/OSC)                           │
│                                                                     │
│  ┌──────────┐    GP OSC     ┌──────────────────────────────────┐   │
│  │          │◄─────────────►│                                  │   │
│  │  DiGiCo  │               │         DAEMON (Rust)            │   │
│  │  S21/S31 │  iPad Proto   │                                  │   │
│  │          │◄─────────────►│  ┌────────────────────────────┐  │   │
│  └──────────┘               │  │ Layer 1: Network Core      │  │   │
│                             │  │  - GP OSC Client            │  │   │
│  ┌──────────┐  iPad Proto   │  │  - iPad Protocol Proxy      │  │   │
│  │   iPad   │◄─────────────►│  │  - Client Connection Mgr    │  │   │
│  │  (FOH)   │  (via proxy)  │  └────────────────────────────┘  │   │
│  └──────────┘               │  ┌────────────────────────────┐  │   │
│                             │  │ Layer 2: Console State      │  │   │
│  ┌──────────┐               │  │  - Live State Mirror        │  │   │
│  │  QLab    │───OSC────────►│  │  - State Diffing            │  │   │
│  │  (Mac)   │               │  │  - Parameter Ownership      │  │   │
│  └──────────┘               │  └────────────────────────────┘  │   │
│                             │  ┌────────────────────────────┐  │   │
│  ┌──────────┐               │  │ Layer 3: Snapshot Engine    │  │   │
│  │ Monitor  │               │  │  - Snapshot Store           │  │   │
│  │ Tablets  │───OSC────────►│  │  - Scope Definitions        │  │   │
│  │ (Flutter)│◄──────────────│  │  - Recall Engine            │  │   │
│  └──────────┘               │  │  - EQ Palette System        │  │   │
│                             │  └────────────────────────────┘  │   │
│                             │  ┌────────────────────────────┐  │   │
│                             │  │ Layer 4: Macro Engine       │  │   │
│                             │  │  - Macro Store              │  │   │
│                             │  │  - Learn / Record           │  │   │
│                             │  │  - Execution Engine         │  │   │
│                             │  └────────────────────────────┘  │   │
│                             │  ┌────────────────────────────┐  │   │
│                             │  │ Layer 5: Integration        │  │   │
│                             │  │  - QLab Trigger Listener    │  │   │
│                             │  │  - QLab Export              │  │   │
│                             │  └────────────────────────────┘  │   │
│                             │  ┌────────────────────────────┐  │   │
│                             │  │ Layer 6: UI                 │  │   │
│                             │  │  - Native touchscreen UI    │  │   │
│                             │  │  - Tabbed interface         │  │   │
│                             │  └────────────────────────────┘  │   │
│                             └──────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

### 2.2 Operating Modes

The daemon supports three operating modes for console communication. These can coexist.

**Mode 1: Direct GP OSC (default, always active)**
- Connects to the console on the general-purpose OSC port
- Reads state via `/console/resend` and ongoing parameter change messages
- Writes snapshot recalls and macro commands
- The iPad connects directly to the console — no proxy, no single point of failure
- Covers the majority of snapshot parameters

**Mode 2: Direct iPad Protocol (spoofed handshake)**
- Connects to the console's iPad remote port after performing the handshake
- Used for parameters only available via the iPad protocol (e.g., insert enables, graphic EQ)
- Mutually exclusive with a real iPad connection (console locks to one iPad IP)
- Used when the iPad is not needed, or as a quick burst to send specific commands

**Mode 3: Full iPad Proxy**
- The daemon registers its IP as the iPad remote on the console
- The real iPad connects to the daemon instead of the console
- The daemon forwards all traffic bidirectionally, capturing state data in transit
- Enables full iPad protocol state capture during programming/rehearsal
- Enables personal monitoring (multiple clients multiplexed through single iPad connection)
- Risk: if the daemon or its host machine goes down, the iPad loses its connection to the console until IPs are manually reconfigured on both console and iPad

### 2.3 Console Network Constraints

The console has two IP registration tables:

- **iPad remote**: multiple IPs can be registered, but only one can be **active** at a time
- **General-purpose OSC**: same — multiple registered, only one active

This means:
- The daemon always occupies the GP OSC slot (permanent, no conflict with iPad)
- The iPad occupies the iPad remote slot for normal operation
- For Mode 2 or Mode 3, the daemon's IP must be activated in the iPad remote slot (displacing the iPad)
- Switching between modes requires changing which IP is active on the console

---

## 3. Console Protocol Reference

### 3.1 General-Purpose OSC Protocol

#### 3.1.1 Channel Number Mapping

The GP protocol uses a unified channel number space:

| OSC Channel Range | Console Channel Type | Console Numbers |
|---|---|---|
| 1–60 | Input | 1–60 |
| 70–77 | Aux | 1–8 |
| 78–93 | Group | 1–16 |
| 120–127 | Matrix | 1–8 |
| 110–119 | Control Group | 1–10 |

Note: The actual channel counts depend on console configuration (48/16 base or 60/24 expanded) and the aux/group split. The aux range (70–77) and group range (78–93) shown are for the base 8-aux configuration.

#### 3.1.2 System Commands

| OSC Path | Value Type | Description |
|---|---|---|
| `/console/channel/counts` | INT | Query channel counts per type |
| `/console/ping` | None | Keepalive ping |
| `/console/pong` | None | Keepalive response |
| `/console/resend` | None | Request full state dump |
| `/digico/snapshots/fire` | INT | Fire snapshot by number |
| `/digico/snapshots/fire/next` | None | Fire next snapshot |
| `/digico/snapshots/fire/previous` | None | Fire previous snapshot |

#### 3.1.3 Channel Parameters

All paths follow the pattern `/channel/{channel}/...` where `{channel}` is the OSC channel number.

**Input Section (Input channels only unless noted)**

| OSC Path | Value Type | Range | Availability |
|---|---|---|---|
| `/channel/{ch}/name` | String | — | Input, Aux, Group, Matrix, CG |
| `/channel/{ch}/total/gain` | float | -20 to +60 | Input only |
| `/channel/{ch}/input/gain_tracking` | Boolean | true/false | Input only |
| `/channel/{ch}/input/trim` | float | -40 to +40 | Input, Aux, Group, Matrix |
| `/channel/{ch}/input/balance` | float | -1 to +1 | Input only |
| `/channel/{ch}/input/width` | float | -1 to +1 | Input only |
| `/channel/{ch}/input/delay/enabled` | Boolean | true/false | Input, Aux, Group, Matrix |
| `/channel/{ch}/input/delay/time` | float | 0 to 0.682 (seconds) | Input, Aux, Group, Matrix |
| `/channel/{ch}/input/digitube/enabled` | Boolean | true/false | Input, Aux, Group, Matrix |
| `/channel/{ch}/input/digitube/drive` | float | 0.1 to 50 | Input, Aux, Group, Matrix |
| `/channel/{ch}/input/digitube/bias` | INT | 0–6 | Input, Aux, Group, Matrix |
| `/channel/{ch}/input/polarity` | INT | 0/1/2/3 | Input, Aux, Group, Matrix |

**EQ Section (Input, Aux, Group, Matrix — NOT Control Groups)**

4 parametric bands per channel. `{band}` = 1–4 (1=Hi, 2=Hi-mid, 3=Lo-mid, 4=Low).

| OSC Path | Value Type | Range |
|---|---|---|
| `/channel/{ch}/eq/enabled` | Boolean | true/false |
| `/channel/{ch}/eq/highpass/enabled` | Boolean | true/false |
| `/channel/{ch}/eq/highpass/frequency` | float | 20–20000 Hz |
| `/channel/{ch}/eq/lowpass/enabled` | Boolean | true/false |
| `/channel/{ch}/eq/lowpass/frequency` | float | 20–20000 Hz |
| `/channel/{ch}/eq/{band}/frequency` | float | 20–20000 Hz |
| `/channel/{ch}/eq/{band}/gain` | float | -18 to +18 dB |
| `/channel/{ch}/eq/{band}/q` | float | 0.1–20 |
| `/channel/{ch}/eq/{band}/dyn/enabled` | Boolean | true/false |
| `/channel/{ch}/eq/{band}/dyn/threshold` | float | -60 to 0 dB |
| `/channel/{ch}/eq/{band}/dyn/ratio` | float | 1–10 |
| `/channel/{ch}/eq/{band}/dyn/attack` | float | 0.0005–0.1 s |
| `/channel/{ch}/eq/{band}/dyn/release` | float | 0.01–10 s |

**Dynamics 1 — Multiband Compressor (Input, Aux, Group, Matrix)**

3 bands with crossover. `{band}` = 1 (Hi), 2 (Mid), 3 (Low).

| OSC Path | Value Type | Range |
|---|---|---|
| `/channel/{ch}/dyn1/mode` | INT | 0/1 |
| `/channel/{ch}/dyn1/enabled` | Boolean | true/false |
| `/channel/{ch}/dyn1/{band}/threshold` | float | -60 to 0 dB |
| `/channel/{ch}/dyn1/{band}/knee` | INT | 0/1/2 |
| `/channel/{ch}/dyn1/{band}/ratio` | float | 1–50 |
| `/channel/{ch}/dyn1/{band}/attack` | float | 0.0005–0.1 s |
| `/channel/{ch}/dyn1/{band}/release` | float | 0.005–5 s |
| `/channel/{ch}/dyn1/{band}/gain` | float | 0 to +40 dB |
| `/channel/{ch}/dyn1/{band}/listen` | Boolean | true/false |
| `/channel/{ch}/dyn1/crossover_high` | float | 20–20000 Hz |
| `/channel/{ch}/dyn1/crossover_low` | float | 20–20000 Hz |

**Dynamics 2 — Gate/Expander (Input, Aux, Group, Matrix)**

| OSC Path | Value Type | Range |
|---|---|---|
| `/channel/{ch}/dyn2/enabled` | Boolean | true/false |
| `/channel/{ch}/dyn2/mode` | INT | 0/1/2 |
| `/channel/{ch}/dyn2/threshold` | float | -60 to 0 dB |
| `/channel/{ch}/dyn2/knee` | INT | 0/1/2 |
| `/channel/{ch}/dyn2/ratio` | float | 1–50 |
| `/channel/{ch}/dyn2/range` | float | -90 to 0 dB |
| `/channel/{ch}/dyn2/attack` | float | 0.0005–0.1 s |
| `/channel/{ch}/dyn2/hold` | float | 0.002–2 s |
| `/channel/{ch}/dyn2/release` | float | 0.005–5 s |
| `/channel/{ch}/dyn2/gain` | float | 0 to +40 dB |
| `/channel/{ch}/dyn2/highpass` | float | 20–20000 Hz |
| `/channel/{ch}/dyn2/lowpass` | float | 20–20000 Hz |
| `/channel/{ch}/dyn2/listen` | Boolean | true/false |

**Sends (Input channels only)**

`{send}` corresponds to the aux number. Send count depends on how many mix channels are configured as auxes.

| OSC Path | Value Type | Range |
|---|---|---|
| `/channel/{ch}/send/{send}/enabled` | Boolean | true/false |
| `/channel/{ch}/send/{send}/level` | float | -150 to +10 dB |
| `/channel/{ch}/send/{send}/pan` | float | -1 to +1 |

**Output Section (all channel types)**

| OSC Path | Value Type | Range | Availability |
|---|---|---|---|
| `/channel/{ch}/pan` | float | -1 to +1 | Input only |
| `/channel/{ch}/mute` | Boolean | true/false | All |
| `/channel/{ch}/solo` | Boolean | true/false | All |
| `/channel/{ch}/fader` | float | -150 to +10 dB | All |

### 3.2 iPad Remote Protocol

The iPad protocol uses named channel type prefixes instead of a unified number space.

#### 3.2.1 Address Format

```
/{ChannelType}/{number}/{parameter}
```

Channel types: `Input_Channels`, `Aux_Outputs`, `Group_Outputs`, `Matrix_Outputs`, `Control_Groups`, `Matrix_Inputs`, `Solo_Outputs`, `Graphic_EQ`

#### 3.2.2 Query Mechanism

Appending `/?` to any parameter path requests its current value from the console. The console responds asynchronously (responses arrive out of order).

```
iPad → Console:  /Input_Channels/1/fader/?
Console → iPad:  /Input_Channels/1/fader -150
```

#### 3.2.3 Handshake Sequence

On connection, the iPad queries the console in this order:

1. `/Snapshots/Current_Snapshot/?` — current snapshot number
2. `/Console/Name/?` — console name and serial
3. `/Console/Session/Filename/?` — current session file
4. `/Console/Channels/?` — triggers channel count responses
5. `/Console/Aux_Outputs/modes/?` — mono(1)/stereo(2) per mix output
6. `/Console/Aux_Outputs/types/?` — aux(1)/group(0) per mix output
7. `/Console/Input_Channels/modes/?` — mono(1)/stereo(2) per input
8. `/Console/Group_Outputs/modes/?` — mono(1)/stereo(2) per group
9. `/Console/Multis/?` — multi-channel count
10. `/Layout/Layout/Banks/?` — fader bank layout (which channels on which bank/screen)

The console responds with configuration data:

| Response Path | Value | Description |
|---|---|---|
| `/Console/Input_Channels` | INT | Number of input channels (48 or 60) |
| `/Console/Aux_Outputs` | INT | Number of mix outputs |
| `/Console/Group_Outputs` | INT | Number of group outputs |
| `/Console/Matrix_Outputs` | INT | Number of matrix outputs |
| `/Console/Matrix_Inputs` | INT | Number of matrix inputs |
| `/Console/Control_Groups` | INT | Number of control groups (10) |
| `/Console/Graphic_EQ` | INT | Number of graphic EQs (16) |
| `/Console/Talkback_Outputs` | INT | Number of talkback outputs |
| `/Console/Multis` | INT | Number of multi-channels |
| `/Console/Aux_Outputs/modes` | INT... | 1=mono, 2=stereo per output |
| `/Console/Aux_Outputs/types` | INT... | 1=aux, 0=group per output |
| `/Console/Input_Channels/modes` | INT... | 1=mono, 2=stereo per input |
| `/Console/Group_Outputs/modes` | INT... | 1=mono, 2=stereo per group |

After receiving configuration, the iPad queries all fader values, then names, then detailed per-channel parameters for the visible bank.

#### 3.2.4 iPad Protocol Parameters (beyond GP OSC)

The iPad protocol exposes additional parameters not available via GP OSC:

| Parameter | iPad Path Example | Description |
|---|---|---|
| Aux sends (level) | `/Input_Channels/1/Aux_Send/3/send_level` | Send level to aux |
| Aux sends (pan) | `/Input_Channels/1/Aux_Send/3/send_pan` | Send pan to aux |
| Aux sends (on) | `/Input_Channels/1/Aux_Send/3/send_on` | Send enable |
| Group sends (on) | `/Input_Channels/1/Group_Send/4/send_on` | Group routing on/off |
| Master bus (on) | `/Input_Channels/1/Group_Send/17/send_on` | Master bus assign |
| Matrix sends (level) | `/Matrix_Inputs/2/Matrix_Send/5/send_level` | Matrix send level |
| Matrix sends (on) | `/Matrix_Inputs/2/Matrix_Send/5/send_on` | Matrix send enable |
| Insert A | `/Input_Channels/1/Insert/insert_A_in` | Insert A enable (0/1) |
| Insert B | `/Input_Channels/1/Insert/insert_B_in` | Insert B enable (0/1) |
| Phantom power | `/Input_Channels/1/Channel_Input/phantom` | 48V phantom (0/1) |
| Phase/polarity | `/Input_Channels/1/Channel_Input/phase` | 0/1/2/3 |
| Analog gain | `/Input_Channels/1/Channel_Input/analog_gain` | -20 to +60 dB |
| Trim | `/Input_Channels/1/Channel_Input/trim` | -40 to +40 dB |
| Delay on | `/Input_Channels/1/Channel_Delay/delay_on` | 0/1 |
| Delay time | `/Input_Channels/1/Channel_Delay/delay` | 0–0.682 s |
| Stereo mode | `/Input_Channels/1/Channel_Input/stereo_mode` | mode int |
| Main/alt input | `/Input_Channels/1/Channel_Input/main/alt_in` | 0/1 |
| Panner | `/Input_Channels/1/Panner/pan` | 0–1 (note: 0.5 = center) |
| CG level membership | `/Input_Channels/1/CGs_level` | bitmask or int |
| CG mute membership | `/Input_Channels/1/CGs_mute` | bitmask or int |
| EQ flatten | `/Input_Channels/1/EQ/flatten` | trigger |
| EQ enable | `/Input_Channels/1/EQ/eq_in` | 0/1 |
| EQ band freq | `/Input_Channels/1/EQ/eq_freq_{band}` | 20–20000 Hz |
| EQ band gain | `/Input_Channels/1/EQ/eq_gain_{band}` | -18 to +18 dB |
| EQ band Q | `/Input_Channels/1/EQ/eq_Q_{band}` | 0.1–20 |
| EQ band curve | `/Input_Channels/1/EQ/eq_curve_{band}` | curve type int |
| Dynamic EQ on | `/Input_Channels/1/EQ/dynamic_eq_on_{band}` | 0/1 |
| Dynamic EQ thresh | `/Input_Channels/1/EQ/eq_thresh_{band}` | -60 to 0 dB |
| Dynamic EQ over/under | `/Input_Channels/1/EQ/eq_over-under_{band}` | 0/1 |
| Dynamic EQ ratio | `/Input_Channels/1/EQ/eq_ratio_{band}` | 1–10 |
| Dynamic EQ attack | `/Input_Channels/1/EQ/eq_attack_{band}` | 0.0005–0.1 s |
| Dynamic EQ release | `/Input_Channels/1/EQ/eq_release_{band}` | 0.01–10 s |
| Filter lo freq | `/Input_Channels/1/Filters/lo_filter_freq` | 20–20000 Hz |
| Filter lo enable | `/Input_Channels/1/Filters/lo_filter_in` | 0/1 |
| Filter hi freq | `/Input_Channels/1/Filters/hi_filter_freq` | 20–20000 Hz |
| Filter hi enable | `/Input_Channels/1/Filters/hi_filter_in` | 0/1 |
| Compressor enable | `/Input_Channels/1/Dynamics/comp_in` | 0/1 |
| Compressor multiband/deesser | `/Input_Channels/1/Dynamics/comp-multiband-desser` | 0/1 |
| Compressor threshold | `/Input_Channels/1/Dynamics/comp_thresh` | -60 to 0 dB |
| Compressor attack | `/Input_Channels/1/Dynamics/comp_attack` | 0.0005–0.1 s |
| Compressor release | `/Input_Channels/1/Dynamics/comp_release` | 0.005–5 s |
| Compressor ratio | `/Input_Channels/1/Dynamics/comp_ratio` | 1–50 |
| Compressor gain | `/Input_Channels/1/Dynamics/comp_gain` | 0–40 dB |
| Compressor knee | `/Input_Channels/1/Dynamics/comp_knee` | 0/1/2 |
| Comp multiband thresh | `/Input_Channels/1/Dynamics/comp_thresh_{band}` | -60 to 0 dB |
| Comp multiband attack | `/Input_Channels/1/Dynamics/comp_attack_{band}` | 0.0005–0.1 s |
| Comp multiband release | `/Input_Channels/1/Dynamics/comp_release_{band}` | 0.005–5 s |
| Comp multiband ratio | `/Input_Channels/1/Dynamics/comp_ratio_{band}` | 1–50 |
| Comp multiband gain | `/Input_Channels/1/Dynamics/comp_auto-gain_{band}` | 0–40 dB |
| Comp multiband knee | `/Input_Channels/1/Dynamics/comp_knee_{band}` | 0/1/2 |
| Comp multiband listen | `/Input_Channels/1/Dynamics/comp_listen_{band}` | 0/1 |
| Comp HP crossover | `/Input_Channels/1/Dynamics/comp_HP_crossover_{band}` | 20–20000 Hz |
| Comp LP crossover | `/Input_Channels/1/Dynamics/comp_LP_crossover_{band}` | 20–20000 Hz |
| Gate enable | `/Input_Channels/1/Dynamics/gate_in` | 0/1 |
| Gate threshold | `/Input_Channels/1/Dynamics/gate_thresh` | -60 to 0 dB |
| Gate attack | `/Input_Channels/1/Dynamics/gate_attack` | 0.005–0.1 s |
| Gate hold | `/Input_Channels/1/Dynamics/gate_hold` | 0.002–2 s |
| Gate release | `/Input_Channels/1/Dynamics/gate_release` | 0.005–5 s |
| Gate range | `/Input_Channels/1/Dynamics/gate_range` | -90 to 0 dB |
| Gate HP sidechain | `/Input_Channels/1/Dynamics/gate_hp` | 20–20000 Hz |
| Gate LP sidechain | `/Input_Channels/1/Dynamics/gate_lp` | 20–20000 Hz |
| Gate mode | `/Input_Channels/1/Dynamics/gate-duck-comp` | 0/1/2 |
| Key solo | `/Input_Channels/1/Dynamics/key_solo` | 0/1 |
| Graphic EQ band gain | `/Graphic_EQ/1/geq_gain {band}` | band 1–32, -12 to +12 dB |
| Graphic EQ enable | `/Graphic_EQ/1/geq_in` | 0/1 |
| Meters | `/Input_Channels/1/Channel_Input/post_meter {meter_id} {enable}` | meter subscription |
| Meter values | `/Meters/values {ch} {val} ...` | bulk meter data |

Note: The iPad protocol uses a pan range of 0–1 (0.5 = center) while the GP protocol uses -1 to +1 (0 = center). The daemon must handle this conversion.

#### 3.2.5 Layout Bank Data

The console sends bank layout information describing which channels are on which fader bank:

```
/Layout/Layout/Banks {Side} {BankNumber} {SideLabel} {Unknown1} {Unknown2} {ChType} {ChNum} ... (10 slots)
```

- Side: "Left" or "Right" (corresponding to screen position)
- 10 channel slots per bank side (matching 10 faders per screen)
- Channel slots are `{ChannelType} {Number}` pairs, or `0` for empty

---

## 4. Data Model

### 4.1 Console Configuration

Discovered at startup via GP OSC (`/console/channel/counts`) or iPad protocol handshake.

```rust
struct ConsoleConfig {
    console_name: String,
    console_serial: String,
    session_filename: Option<String>,

    input_channel_count: u8,       // 48 or 60
    mix_output_count: u8,          // 16 or 24
    group_output_count: u8,
    matrix_output_count: u8,       // 8
    matrix_input_count: u8,        // 10
    control_group_count: u8,       // 10
    graphic_eq_count: u8,          // 16
    talkback_output_count: u8,

    /// Per mix output: true = aux, false = group/bus
    mix_output_types: Vec<bool>,
    /// Per mix output: Mono or Stereo
    mix_output_modes: Vec<ChannelMode>,
    /// Per input: Mono or Stereo
    input_modes: Vec<ChannelMode>,
    /// Per group: Mono or Stereo
    group_modes: Vec<ChannelMode>,
}

enum ChannelMode {
    Mono,   // 1
    Stereo, // 2
}
```

### 4.2 Channel Identification

A unified way to refer to any channel across both protocols.

```rust
/// Logical channel identifier (protocol-agnostic)
enum ChannelId {
    Input(u8),        // 1-based, 1–60
    Aux(u8),          // 1-based, 1–n (where n = number of aux-type mix outputs)
    Group(u8),        // 1-based, 1–n
    Matrix(u8),       // 1-based, 1–8
    ControlGroup(u8), // 1-based, 1–10 (note: iPad protocol is 0-based for CG)
    GraphicEq(u8),    // 1-based, 1–16
    MatrixInput(u8),  // 1-based, 1–10
}

impl ChannelId {
    /// Convert to GP OSC channel number
    fn to_gp_osc_number(&self) -> Option<u8>;

    /// Convert to iPad protocol path prefix
    fn to_ipad_path_prefix(&self) -> String;

    /// Parse from GP OSC channel number
    fn from_gp_osc_number(n: u8) -> Option<Self>;

    /// Parse from iPad protocol path
    fn from_ipad_path(path: &str) -> Option<Self>;
}
```

### 4.3 Parameter Identification

```rust
/// A specific parameter on a specific channel
struct ParameterAddress {
    channel: ChannelId,
    parameter: ParameterPath,
}

/// Parameter within a channel, organized by section
enum ParameterPath {
    // Output
    Name,
    Fader,
    Mute,
    Solo,
    Pan,

    // Input section
    Gain,
    GainTracking,
    Trim,
    Balance,
    Width,
    Polarity,
    Phantom,          // iPad protocol only
    MainAltIn,        // iPad protocol only
    StereoMode,       // iPad protocol only

    // Delay
    DelayEnabled,
    DelayTime,

    // Digitube
    DigitubeEnabled,
    DigitubeDrive,
    DigitubeBias,

    // EQ
    EqEnabled,
    HighpassEnabled,
    HighpassFrequency,
    LowpassEnabled,
    LowpassFrequency,
    EqBandFrequency(u8),    // band 1–4
    EqBandGain(u8),
    EqBandQ(u8),
    EqBandCurve(u8),        // iPad protocol only
    EqBandDynEnabled(u8),
    EqBandDynThreshold(u8),
    EqBandDynRatio(u8),
    EqBandDynAttack(u8),
    EqBandDynRelease(u8),
    EqBandDynOverUnder(u8), // iPad protocol only

    // Dynamics 1 (compressor)
    Dyn1Enabled,
    Dyn1Mode,
    Dyn1MultibandDeesser,    // iPad protocol only
    Dyn1Threshold(u8),       // band 1–3
    Dyn1Knee(u8),
    Dyn1Ratio(u8),
    Dyn1Attack(u8),
    Dyn1Release(u8),
    Dyn1Gain(u8),
    Dyn1Listen(u8),
    Dyn1CrossoverHigh,
    Dyn1CrossoverLow,

    // Dynamics 2 (gate)
    Dyn2Enabled,
    Dyn2Mode,
    Dyn2Threshold,
    Dyn2Knee,
    Dyn2Ratio,
    Dyn2Range,
    Dyn2Attack,
    Dyn2Hold,
    Dyn2Release,
    Dyn2Gain,
    Dyn2Highpass,
    Dyn2Lowpass,
    Dyn2Listen,
    Dyn2KeySolo,            // iPad protocol only

    // Sends (input channels only)
    SendEnabled(u8),        // send/aux number
    SendLevel(u8),
    SendPan(u8),

    // Group routing (iPad protocol only)
    GroupSendOn(u8),        // group number
    MasterBusOn,

    // Inserts (iPad protocol only)
    InsertAEnabled,
    InsertBEnabled,

    // CG membership (iPad protocol only)
    CgLevel,
    CgMute,

    // Matrix sends (MatrixInput channels, iPad protocol only)
    MatrixSendLevel(u8),
    MatrixSendOn(u8),

    // Graphic EQ (GraphicEq channels only, iPad protocol only)
    GeqBandGain(u8),        // band 1–32
    GeqEnabled,
}

/// Typed parameter value
enum ParameterValue {
    Float(f32),
    Int(i32),
    Bool(bool),
    String(String),
}
```

### 4.4 Console State Mirror

```rust
/// Live mirror of the full console state
struct ConsoleState {
    config: ConsoleConfig,
    /// All parameter values indexed by address
    parameters: HashMap<ParameterAddress, ParameterValue>,
    /// Timestamp of last update per parameter
    last_updated: HashMap<ParameterAddress, Instant>,
}

impl ConsoleState {
    /// Apply a parameter change (from incoming OSC)
    fn update(&mut self, addr: ParameterAddress, value: ParameterValue);

    /// Get current value
    fn get(&self, addr: &ParameterAddress) -> Option<&ParameterValue>;

    /// Take a snapshot of selected parameters
    fn capture(&self, scope: &Scope) -> SnapshotData;

    /// Diff two states or a snapshot against live state
    fn diff(&self, snapshot: &SnapshotData) -> Vec<ParameterChange>;
}
```

### 4.5 Scope Definition

Scopes define which parameters are included in a snapshot capture or recall. Scopes operate at the **section level** per channel, matching the approach used by DiGiCo SD/Quantum theatre snapshots.

```rust
/// Reusable scope template
struct ScopeTemplate {
    id: Uuid,
    name: String,
    /// Which channels and which sections per channel
    channel_scopes: Vec<ChannelScope>,
}

struct ChannelScope {
    channel: ChannelId,
    sections: HashSet<ParameterSection>,
}

/// Parameter sections for scope control
enum ParameterSection {
    Name,
    InputGain,       // gain, trim, balance, width, polarity, phantom
    Delay,           // delay on/off and time
    Digitube,        // digitube on/off, drive, bias
    Eq,              // all EQ parameters including dynamic EQ, filters
    Dyn1,            // compressor — all parameters
    Dyn2,            // gate — all parameters
    Sends,           // all aux send levels, pans, on/off
    GroupRouting,    // group send on/off, master bus (iPad protocol)
    Inserts,         // insert A/B enable (iPad protocol)
    FaderMutePan,    // fader, mute, pan
    CgMembership,   // control group level/mute membership (iPad protocol)
    GraphicEq,       // all GEQ bands (GraphicEq channels only)
    MatrixSends,     // matrix send levels and on/off (MatrixInput channels only)
}

impl ParameterSection {
    /// Return all ParameterPath variants that belong to this section
    fn parameters(&self) -> Vec<ParameterPath>;
}
```

### 4.6 Snapshot

```rust
struct Snapshot {
    id: Uuid,
    name: String,
    /// The scope used when this snapshot was captured
    scope: ScopeTemplate,
    /// The stored parameter values
    data: SnapshotData,
    /// Optional reference to an EQ palette instead of stored EQ values
    eq_palette_refs: HashMap<ChannelId, Uuid>,  // channel → palette_id
    /// Timestamp of creation
    created_at: DateTime<Utc>,
    /// Timestamp of last modification
    modified_at: DateTime<Utc>,
}

struct SnapshotData {
    /// Parameter values captured within scope
    values: HashMap<ParameterAddress, ParameterValue>,
}
```

### 4.7 EQ Palette

An EQ palette is a named, canonical set of EQ parameters for a channel. Multiple snapshots can reference the same palette. When the palette is modified, all referencing snapshots inherit the change on next recall.

```rust
struct EqPalette {
    id: Uuid,
    name: String,
    /// The channel this palette's EQ is for
    channel: ChannelId,
    /// The EQ parameter values (all EQ-section parameters for this channel)
    eq_values: HashMap<ParameterPath, ParameterValue>,
    /// Which snapshots reference this palette (back-references for ripple)
    referencing_snapshots: Vec<Uuid>,
    modified_at: DateTime<Utc>,
}
```

When recalling a snapshot, the recall engine checks if a channel's EQ section has a palette reference. If so, it uses the palette's values instead of the snapshot's stored EQ values.

When a palette is modified (either by editing it directly or by capturing new values from the console), the system can identify all snapshots referencing that palette via `referencing_snapshots`.

### 4.8 Cue List

```rust
struct CueList {
    id: Uuid,
    name: String,
    cues: Vec<Cue>,
}

struct Cue {
    id: Uuid,
    cue_number: f32,          // supports decimal cue numbers (e.g., 1, 1.5, 2)
    name: String,
    snapshot_id: Uuid,        // reference to the snapshot to recall
    /// Scope override — if set, overrides the snapshot's built-in scope for this cue
    scope_override: Option<ScopeTemplate>,
    /// Fade time in seconds for parameter transitions
    fade_time: f32,
    /// QLab cue identifier for trigger mapping
    qlab_cue_id: Option<String>,
    /// Notes for the operator
    notes: String,
}
```

### 4.9 Macro

```rust
struct Macro {
    id: Uuid,
    name: String,
    steps: Vec<MacroStep>,
    created_at: DateTime<Utc>,
    modified_at: DateTime<Utc>,
}

struct MacroStep {
    /// The parameter to control
    address: ParameterAddress,
    /// How this step behaves
    mode: MacroStepMode,
    /// Delay before this step executes (relative to previous step)
    delay_ms: u32,
}

enum MacroStepMode {
    /// Send the opposite of current live value
    Toggle,
    /// Always send this specific value
    Fixed(ParameterValue),
    /// Offset from current live value (only for float/int parameters)
    Relative(f32),
}

/// Recorded during macro learn mode
struct MacroRecording {
    steps: Vec<RecordedStep>,
}

struct RecordedStep {
    address: ParameterAddress,
    value: ParameterValue,
    /// Time elapsed since previous step
    elapsed_ms: u32,
}

impl MacroRecording {
    /// Convert to a Macro, with all steps defaulting to Fixed mode
    /// The user then edits individual steps to Toggle or Relative as needed
    fn to_macro(&self, name: String) -> Macro;
}
```

### 4.10 Client Profile (Personal Monitoring)

```rust
struct MonitorClient {
    id: Uuid,
    name: String,              // e.g., "Drummer", "Keys", "Vocalist"
    /// Which aux output(s) this client can control
    permitted_auxes: Vec<u8>,
    /// Which input channels' sends to those auxes are visible/controllable
    visible_inputs: Vec<u8>,   // empty = all inputs
    /// Connection state
    connected: bool,
    last_seen: Option<Instant>,
}
```

---

## 5. Feature Specifications

### 5.1 Console State Mirror

**Startup sequence:**
1. Connect to console on GP OSC port
2. Send `/console/channel/counts` to discover configuration
3. Send `/console/resend` to request full state dump
4. Process all incoming parameter messages to build state mirror
5. Begin keepalive cycle (`/console/ping` / `/console/pong`)

**Ongoing operation:**
- Process all incoming OSC messages and update the state mirror
- Track timestamp of each parameter update
- Notify the snapshot engine and macro engine of state changes (for toggle logic)

**If iPad proxy is active (Mode 3):**
- Additionally capture all iPad protocol messages flowing through the proxy
- Translate iPad protocol addresses to the internal ParameterAddress format
- Handle pan value conversion (iPad 0–1 ↔ GP -1 to +1)

### 5.2 Snapshot Capture

**Workflow:**
1. User defines or selects a scope (which channels and sections)
2. User triggers "capture"
3. The daemon reads the current state mirror for all parameters within scope
4. A new Snapshot is created with the captured data
5. User names the snapshot and optionally assigns it to a cue

**Scope can be:**
- A saved ScopeTemplate (reusable across cues)
- An ad-hoc scope defined at capture time
- Modified per-cue via scope_override on the Cue

### 5.3 Snapshot Recall

**Workflow:**
1. A recall is triggered (by QLab cue, manual go button, or macro)
2. The recall engine looks up the Cue's snapshot and effective scope
3. For each channel/section in scope:
   a. Check if the channel's EQ has a palette reference → use palette values
   b. Otherwise use snapshot's stored values
4. Diff the recall values against current live state
5. For each changed parameter, send the stored value to the console via GP OSC
6. If fade_time > 0, interpolate float values over the fade duration

**Write path:** GP OSC for all standard parameters. iPad protocol (via Mode 2 spoofed handshake or Mode 3 proxy) for extended parameters if needed.

### 5.4 EQ Palette System

**Creating a palette:**
1. User selects a channel and captures its current EQ state
2. A new EqPalette is created with those values
3. User names it (e.g., "Lead Vocal EQ", "Tenor Sax EQ")

**Linking a palette to a snapshot:**
1. User edits a snapshot's channel EQ
2. Instead of storing values directly, user links to an existing palette
3. The snapshot's `eq_palette_refs` maps that channel to the palette ID

**Ripple behavior:**
1. User modifies a palette (either by editing directly or by capturing new values)
2. The system looks up `referencing_snapshots`
3. On next recall of any referencing snapshot, the updated palette values are used
4. Optionally: the system could notify the user which cues will be affected

### 5.5 Macro System

**Creating a macro manually:**
1. User creates a new macro
2. Adds steps by selecting channel, parameter, and mode (toggle/fixed/relative)
3. Sets timing between steps

**Macro learn mode:**
1. User presses "Learn" button
2. The daemon begins recording all parameter changes it receives from the console
3. Each change is logged with its timestamp relative to the start of recording
4. User presses "Stop"
5. The recording is converted to a Macro with all steps in Fixed mode
6. User edits individual steps to change mode to Toggle or Relative as needed
7. User can delete unwanted steps, adjust timing, reorder

**Macro execution:**
1. Triggered by UI button, QLab cue, or other trigger
2. Steps execute in sequence with configured delays
3. For Toggle mode: look up current live value from state mirror, send opposite
4. For Fixed mode: send the configured value
5. For Relative mode: look up current live value, apply offset, send result

### 5.6 QLab Integration

**QLab as trigger source:**
- The daemon listens on a configurable OSC port for QLab triggers
- QLab sends network cues to the daemon's IP and port
- Recommended OSC format: `/cue/fire {cue_number}` and `/macro/fire {macro_name_or_id}`
- The daemon maps incoming triggers to cue recalls or macro executions

**QLab export (optional):**
- The daemon can export snapshot recall data as QLab network cues
- Each cue becomes a QLab network cue that sends the OSC commands directly to the console
- This is a backup/alternative for users who want QLab to hold the data
- The primary workflow keeps data in the daemon with QLab as trigger-only

### 5.7 Personal Monitoring

**Setup:**
1. Admin creates MonitorClient profiles on the daemon
2. Each profile specifies which aux(es) the client can control and optionally which input channels are visible
3. The daemon exposes an OSC server for monitoring clients

**Client connection:**
1. Flutter app connects to daemon's OSC monitoring port
2. App identifies itself (client ID or name)
3. Daemon sends the current state of permitted sends
4. App displays faders for each input channel's send to the permitted aux(es)

**Operation:**
1. Musician adjusts a fader on their tablet
2. App sends OSC to daemon: e.g., `/monitor/{client_id}/send/{input}/{aux}/level {value}`
3. Daemon validates the command is within the client's permitted scope
4. Daemon forwards the command to the console via GP OSC or iPad protocol
5. Console state change flows back through the daemon to update all relevant clients

**Multi-client through proxy (Mode 3):**
- When the iPad proxy is active, the daemon can translate personal monitoring commands into iPad protocol messages
- This gives access to the richer iPad protocol parameter set for sends
- The FOH iPad operator continues using their iPad normally through the proxy

### 5.8 Main UI (Native, Tabbed)

The main UI runs on the daemon's host device (Raspberry Pi touchscreen or Mac).

**Setup Tab:**
- Console IP and port configuration (GP OSC and iPad remote)
- Operating mode selection (Mode 1/2/3)
- Connection status indicators
- Channel count and configuration display (from discovery)
- Personal monitoring client management

**Snapshots Tab:**
- Cue list view (ordered list of cues with number, name, snapshot, scope)
- Create/edit/delete snapshots
- Scope editor: select channels and sections to include
- EQ palette manager: create, edit, link palettes
- Scope template manager: save and reuse scope definitions

**Macros Tab:**
- Macro list
- Learn button (start/stop recording)
- Macro editor: steps with mode selection, timing, reorder/delete
- Manual trigger buttons for each macro

**Monitor Tab:**
- Live console state overview (channel names, fader levels, mute states)
- Connection health for all clients (console, iPad, monitoring tablets)
- Metering display if available

**Live Tab (performance view):**
- Current cue number and name
- Next cue number and name
- GO button (fires next cue)
- Previous button
- Macro quick-trigger buttons (configurable selection of frequently used macros)
- Connection status indicators
- Minimal, high-contrast, glanceable design

---

## 6. OSC API — Daemon External Interface

The daemon exposes the following OSC endpoints for external systems (QLab, monitoring clients, other tools).

### 6.1 Cue Control

| OSC Path | Arguments | Description |
|---|---|---|
| `/cue/fire` | INT (cue_number) | Fire a specific cue by number |
| `/cue/go` | — | Fire next cue |
| `/cue/previous` | — | Go to previous cue |
| `/cue/current` | — | Query current cue (responds with cue number) |

### 6.2 Macro Control

| OSC Path | Arguments | Description |
|---|---|---|
| `/macro/fire` | STRING (macro_name) or INT (macro_id) | Execute a macro |

### 6.3 Personal Monitoring

| OSC Path | Arguments | Description |
|---|---|---|
| `/monitor/{client_id}/connect` | — | Client connection request |
| `/monitor/{client_id}/send/{input_ch}/{aux_ch}/level` | FLOAT | Set send level |
| `/monitor/{client_id}/send/{input_ch}/{aux_ch}/pan` | FLOAT | Set send pan |
| `/monitor/{client_id}/send/{input_ch}/{aux_ch}/on` | BOOL | Set send on/off |
| `/monitor/{client_id}/state` | — | Request current state for this client |

### 6.4 Status

| OSC Path | Arguments | Description |
|---|---|---|
| `/status/console` | — | Query console connection state |
| `/status/clients` | — | Query connected client count |

---

## 7. Persistence

### 7.1 Storage Format

All persistent data is stored as files on disk (not a database), for simplicity and portability.

- **Show file**: A single JSON or MessagePack file containing the complete show state:
  - Console configuration snapshot
  - All scope templates
  - All snapshots with data
  - All EQ palettes with references
  - Complete cue list
  - All macros
  - Monitor client profiles

- **Auto-save**: The daemon periodically writes the current show state to disk (e.g., every 30 seconds or on every modification)

- **Backup**: Timestamped copies on manual save

### 7.2 Import/Export

- Export show file for backup or transfer between machines
- Import show file to restore or load on a different daemon instance

---

## 8. Implementation Priorities

### Phase 1: Foundation
1. GP OSC client — connect, ping/pong, send/receive messages
2. Console configuration discovery (`/console/channel/counts`, `/console/resend`)
3. Console state mirror — process incoming parameters, build live state
4. Basic persistence — save/load show file

### Phase 2: Core Snapshot System
5. Scope template definition and storage
6. Snapshot capture from live state
7. Snapshot recall — diff and write to console via GP OSC
8. Cue list with ordering and navigation
9. QLab trigger listener — fire cues on incoming OSC

### Phase 3: Main UI
10. Native UI framework setup (egui or slint)
11. Setup tab — connection configuration and status
12. Snapshots tab — cue list, capture, scope editor
13. Live tab — current/next cue, GO button

### Phase 4: Macros
14. Macro definition and manual execution
15. Macro learn mode (record from incoming OSC)
16. Macro editor (toggle/fixed/relative modes, timing)
17. Macros tab in UI
18. QLab trigger for macros

### Phase 5: EQ Palettes
19. EQ palette creation and editing
20. Palette linking to snapshots
21. Ripple propagation on palette modification
22. Palette management in UI

### Phase 6: iPad Protocol Proxy
23. iPad protocol parser (addresses, values, handshake)
24. Proxy bridge — bidirectional forwarding with state capture
25. Protocol translation (iPad ↔ internal ParameterAddress)
26. Mode 2 — spoofed handshake for direct iPad protocol writes

### Phase 7: Personal Monitoring
27. Monitor client profiles and permission model
28. Daemon OSC server for monitoring clients
29. Command validation and forwarding
30. Flutter monitoring app (iOS + Android)

### Phase 8: Advanced Features
31. Fade time / parameter interpolation on recall
32. Snapshot scope override per cue
33. Cast variants (future)

---

## 9. Rust Crate Dependencies (Recommended)

| Crate | Purpose |
|---|---|
| `tokio` | Async runtime for networking |
| `rosc` | OSC message parsing and construction |
| `serde` / `serde_json` | Serialization for persistence |
| `uuid` | Unique identifiers for entities |
| `chrono` | Timestamps |
| `egui` or `slint` | Native UI framework |
| `rusqlite` | Optional, if file-based JSON proves insufficient |
| `tracing` | Structured logging |
| `clap` | CLI argument parsing |

---

## 10. Open Questions

1. **GP OSC channel number mapping with non-standard aux/group splits**: The documented ranges (70–77 for aux, 78–93 for group) may shift when the aux/group split is non-standard. Need to verify on hardware.

2. **`/console/resend` completeness**: Does this command return all parameters for all channels, or only a subset? Need to verify.

3. **Write confirmation**: Does the console acknowledge parameter writes, or is it fire-and-forget? This affects reliability of recall operations.

4. **iPad protocol write safety**: When injecting commands via Mode 2 (spoofed handshake), are there any commands that could put the console in an unexpected state?

5. **Fade interpolation timing**: What is the minimum reliable interval for sending parameter updates during a fade? Too fast may overwhelm the console's OSC input buffer.

6. **Concurrent writes**: If the FOH engineer changes a parameter on the console surface while a snapshot recall is in progress, which wins? The console surface should always have priority, but the daemon may overwrite with the next recall message.

7. **EQ band numbering**: Verify that band numbering in both protocols is consistent (1=Hi, 2=Hi-mid, 3=Lo-mid, 4=Low for EQ; 1=Hi, 2=Mid, 3=Low for multiband compressor).

8. **Control Group numbering offset**: The iPad protocol uses 0-based indexing for Control Groups while the GP protocol uses 110–119. Confirm the mapping (CG 0 on iPad = CG 1 on console = OSC channel 110).
