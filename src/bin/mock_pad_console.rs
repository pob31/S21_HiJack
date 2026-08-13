//! Mock DiGiCo SD / Quantum console — "Pad" (iPad) OSC protocol simulator.
//!
//! SD and Quantum desks have no S-series GP OSC dialect, so everything the app
//! needs has to arrive over the Pad surface: the bare TitleCase address tree
//! (`/Input_Channels/1/fader`), `/?` queries answered on the un-suffixed path,
//! and push feedback when a human moves a control on the desk. This binary
//! speaks exactly that dialect over UDP, so the Pad-only connection path can be
//! developed and demoed with no console in the room.
//!
//! What it simulates:
//! * **Handshake** — the nine `/Console/…/?` config queries, the four Pad-only
//!   count queries, `/Layout/Layout/Banks/?`, and the closing `/Meters/clear`.
//!   Replies use the exact paths and argument types `ipad_handshake` parses.
//! * **Enumeration** — every addressable parameter answers its `/?` query with
//!   a plausible value, so the paced one-in-flight pump never stalls.
//! * **Heartbeat** — the repeated `/Console/Name/?` keeps answering forever.
//! * **Write echo** — a real desk repeats a write back to the sender.
//! * **Desk-side moves** — `--wiggle` stands in for an operator pushing the
//!   Input 1 fader, so a gang can be watched propagating in the GUI.
//! * **Misbehaviour worth testing against** — query-burst dropping
//!   (`--drop-burst`), truncated OSC padding (`--truncate-padding`), and a desk
//!   that never echoes (`--no-echo`).
//!
//! It does NOT simulate meters, snapshot recall, or session changes; unknown
//! addresses are met with silence, exactly as a real desk meets them.
//!
//! Run with:
//! `cargo run --bin mock_pad_console -- --port 8000 --family quantum --inputs 96 --wiggle 3`

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use rosc::{OscMessage, OscPacket, OscType};
use tokio::net::UdpSocket;
use tokio::time;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

/// How long the desk pretends to be busy servicing one query under
/// `--drop-burst`. Anything that arrives inside this window is dropped.
const SERVICE_DELAY: Duration = Duration::from_millis(2);

/// dB values `--wiggle` walks the Input 1 fader through, one per tick.
const WIGGLE_STEPS: [f32; 6] = [-30.0, -20.0, -12.0, -6.0, -3.0, 0.0];

/// Wire names of the channel types this desk exposes, in enumeration order.
const CHANNEL_TYPES: [&str; 7] = [
    "Input_Channels",
    "Aux_Outputs",
    "Group_Outputs",
    "Matrix_Outputs",
    "Matrix_Inputs",
    "Control_Groups",
    "Graphic_EQ",
];

/// Mock DiGiCo SD/Quantum Console — Pad (iPad) OSC simulator
#[derive(Parser, Debug)]
#[command(name = "mock_pad_console")]
struct Args {
    /// UDP port to listen on (the app sends its queries here)
    #[arg(long, default_value_t = 8000)]
    port: u16,

    /// Console family the mock reports itself as: "quantum" or "sd"
    #[arg(long, default_value_t = String::from("quantum"), value_parser = ["quantum", "sd"])]
    family: String,

    /// Number of input channels
    #[arg(long, default_value_t = 96)]
    inputs: u16,

    /// Number of aux outputs
    #[arg(long, default_value_t = 16)]
    auxes: u16,

    /// Number of group outputs
    #[arg(long, default_value_t = 16)]
    groups: u16,

    /// Number of matrix outputs
    #[arg(long, default_value_t = 16)]
    matrices: u16,

    /// Number of matrix inputs
    #[arg(long, default_value_t = 16)]
    matrix_inputs: u16,

    /// Number of control groups
    #[arg(long, default_value_t = 16)]
    control_groups: u16,

    /// Number of graphic EQs
    #[arg(long, default_value_t = 16)]
    geqs: u16,

    /// Drop any query that arrives while another query is still unanswered.
    ///
    /// Real SD desks drop query bursts; a paced one-in-flight client is never
    /// affected. Note the handshake burst is not paced by anyone, so with this
    /// flag the app learns only the first config reply and falls back to its
    /// default channel counts — that is the cost this flag exists to show.
    #[arg(long, default_value_t = false)]
    drop_burst: bool,

    /// Truncate the trailing OSC padding on every reply (exercises the app's
    /// tolerant decoder, which re-pads unaligned datagrams)
    #[arg(long, default_value_t = false)]
    truncate_padding: bool,

    /// Do not echo writes back to the sender (a real desk does echo)
    #[arg(long, default_value_t = false)]
    no_echo: bool,

    /// Spontaneously move the Input 1 fader every N seconds (0 = off)
    #[arg(long, default_value_t = 0)]
    wiggle: u64,
}

struct MockPadConsole {
    socket: Arc<UdpSocket>,
    args: Args,
    /// Live parameter values, keyed by their bare Pad address.
    ///
    /// Seeded at startup with the leaves an operator notices first (name,
    /// fader, mute, solo, pan, aux sends); the deep processing parameters are
    /// filled in on first query, so a 96-input desk costs a few thousand
    /// entries at startup instead of tens of thousands.
    values: HashMap<String, OscType>,
    /// Where the last inbound datagram came from — replies and `--wiggle`
    /// pushes go here, never to a hardcoded port.
    last_peer: Option<SocketAddr>,
    /// `/Console/Name` payload: "{name} {serial}", split on the first space by
    /// the app's config parser.
    console_name: String,
    session_filename: String,
    parameter_replies: u64,
    name_queries: u64,
    wiggle_step: usize,
}

impl MockPadConsole {
    async fn new(args: Args) -> std::io::Result<Self> {
        let addr: SocketAddr = format!("0.0.0.0:{}", args.port).parse().unwrap();
        let socket = UdpSocket::bind(addr).await?;
        info!("Mock Pad console listening on port {}", args.port);

        // Only the reported identity depends on the family — the address tree,
        // the query convention and the value encodings are shared.
        let (console_name, session_filename) = if args.family == "sd" {
            ("SD12 SD12-100234", "MockShow_SD.ses")
        } else {
            ("Quantum338 Q338-000123", "MockShow_Quantum.ses")
        };
        info!(
            console = console_name,
            "Config: {} inputs, {} auxes, {} groups, {} matrices, {} matrix inputs, {} CGs, {} GEQs",
            args.inputs,
            args.auxes,
            args.groups,
            args.matrices,
            args.matrix_inputs,
            args.control_groups,
            args.geqs
        );
        info!(
            drop_burst = args.drop_burst,
            truncate_padding = args.truncate_padding,
            echo = !args.no_echo,
            wiggle_secs = args.wiggle,
            "Behaviour flags"
        );

        let mut console = Self {
            socket: Arc::new(socket),
            args,
            values: HashMap::new(),
            last_peer: None,
            console_name: console_name.to_string(),
            session_filename: session_filename.to_string(),
            parameter_replies: 0,
            name_queries: 0,
            wiggle_step: 0,
        };
        console.seed();
        Ok(console)
    }

    // ─── startup state ──────────────────────────────────────────────────

    /// Count, display label and wire numbering for a channel type.
    ///
    /// Control Groups are the one type numbered from zero on the wire, so
    /// `/Control_Groups/0` is the operator's CG 1.
    fn channel_type_info(&self, wire_type: &str) -> Option<(u16, &'static str, bool)> {
        let a = &self.args;
        match wire_type {
            "Input_Channels" => Some((a.inputs, "Ch", false)),
            "Aux_Outputs" => Some((a.auxes, "Aux", false)),
            "Group_Outputs" => Some((a.groups, "Grp", false)),
            "Matrix_Outputs" => Some((a.matrices, "Mtx", false)),
            "Matrix_Inputs" => Some((a.matrix_inputs, "MtxIn", false)),
            "Control_Groups" => Some((a.control_groups, "CG", true)),
            "Graphic_EQ" => Some((a.geqs, "GEQ", false)),
            _ => None,
        }
    }

    /// Seed the headline leaves for every channel the configured counts imply.
    fn seed(&mut self) {
        for wire_type in CHANNEL_TYPES {
            let Some((count, label, zero_based)) = self.channel_type_info(wire_type) else {
                continue;
            };
            for display in 1..=count {
                let wire = if zero_based { display - 1 } else { display };
                let base = format!("/{wire_type}/{wire}");

                // A graphic EQ is a bank of band gains, not a fader strip.
                if wire_type == "Graphic_EQ" {
                    self.values.insert(format!("{base}/geq_in"), fl(0.0));
                    continue;
                }

                self.values.insert(
                    format!("{base}/Channel_Input/name"),
                    OscType::String(format!("{label} {display}")),
                );
                // A spread of levels so the GUI looks like a real show file.
                let db = -3.0 - (f32::from(display % 10) * 1.5);
                self.values.insert(format!("{base}/fader"), fl(db));
                self.values.insert(format!("{base}/mute"), fl(0.0));
                self.values.insert(format!("{base}/solo"), fl(0.0));
                // Pan is 0..1 on the wire with 0.5 centre.
                self.values.insert(format!("{base}/Panner/pan"), fl(0.5));

                if wire_type == "Input_Channels" {
                    for send in 1..=self.args.auxes {
                        let s = format!("{base}/Aux_Send/{send}");
                        self.values.insert(format!("{s}/send_level"), fl(-20.0));
                        self.values.insert(format!("{s}/send_on"), fl(0.0));
                        self.values.insert(format!("{s}/send_pan"), fl(0.5));
                    }
                }
            }
        }
        info!(parameters = self.values.len(), "Parameter map seeded");
    }

    // ─── main loop ──────────────────────────────────────────────────────

    async fn run(&mut self) -> std::io::Result<()> {
        // Own handle to the socket so the receive future borrows nothing from
        // `self`, leaving the branch bodies free to mutate the parameter map.
        let socket = Arc::clone(&self.socket);
        let mut buf = vec![0u8; 65536];
        let mut wiggle = (self.args.wiggle > 0).then(|| {
            let period = Duration::from_secs(self.args.wiggle);
            time::interval_at(time::Instant::now() + period, period)
        });

        loop {
            tokio::select! {
                received = socket.recv_from(&mut buf) => {
                    let (size, src) = received?;
                    match rosc::decoder::decode_udp(&buf[..size]) {
                        Ok((_, packet)) => {
                            // Model the desk as busy for the length of one
                            // service window: anything already queued when the
                            // reply goes out arrived while this query was
                            // unanswered, and a real SD desk would have dropped
                            // it. Draining *before* replying keeps a correctly
                            // paced client — which only sends after seeing the
                            // reply — from ever being caught by it.
                            if self.args.drop_burst && packet_is_query(&packet) {
                                time::sleep(SERVICE_DELAY).await;
                                self.drain_burst(&mut buf).await;
                            }
                            self.handle_packet(packet, src).await;
                        }
                        Err(e) => {
                            warn!("Failed to decode OSC from {src}: {e}");
                        }
                    }
                }
                () = wiggle_tick(wiggle.as_mut()) => {
                    self.wiggle_fader().await;
                }
            }
        }
    }

    /// Discard every query already sitting in the socket buffer; apply any
    /// writes found there normally, since a write is not a query.
    async fn drain_burst(&mut self, buf: &mut [u8]) {
        let socket = Arc::clone(&self.socket);
        let mut dropped = 0u32;
        loop {
            let received = socket.try_recv_from(buf);
            match received {
                Ok((size, src)) => match rosc::decoder::decode_udp(&buf[..size]) {
                    Ok((_, packet)) => {
                        if packet_is_query(&packet) {
                            dropped += 1;
                        } else {
                            self.handle_packet(packet, src).await;
                        }
                    }
                    Err(e) => warn!("Failed to decode queued OSC from {src}: {e}"),
                },
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    warn!("Drain read failed: {e}");
                    break;
                }
            }
        }
        if dropped > 0 {
            warn!(
                dropped,
                "--drop-burst: dropped {dropped} queries that arrived while the desk was busy"
            );
        }
    }

    async fn handle_packet(&mut self, packet: OscPacket, src: SocketAddr) {
        match packet {
            OscPacket::Message(msg) => {
                self.handle_message(&msg.addr, &msg.args, src).await;
            }
            OscPacket::Bundle(bundle) => {
                for p in bundle.content {
                    Box::pin(self.handle_packet(p, src)).await;
                }
            }
        }
    }

    async fn handle_message(&mut self, path: &str, args: &[OscType], src: SocketAddr) {
        debug!(path, ?args, %src, "Received");
        self.last_peer = Some(src);

        if let Some(bare) = path.strip_suffix("/?") {
            self.handle_query(bare, src).await;
        } else if !args.is_empty() {
            self.handle_write(path, args, src).await;
        } else {
            // Arg-less and not a query: a control message, not a value.
            match path {
                "/Meters/clear" => info!("Meters cleared — the app finished its handshake"),
                _ => debug!(path, "Ignoring arg-less message"),
            }
        }
    }

    /// Answer a `/?` query: config and heartbeat first, then the parameter map.
    async fn handle_query(&mut self, bare: &str, src: SocketAddr) {
        if let Some(replies) = self.config_reply(bare) {
            for (path, args) in replies {
                self.send_osc(src, &path, args).await;
            }
            return;
        }

        if let Some(value) = self.value_for(bare) {
            self.send_osc(src, bare, vec![value]).await;
            self.parameter_replies += 1;
            if self.parameter_replies == 1 {
                info!(path = bare, "Enumeration: first parameter query answered");
            } else if self.parameter_replies.is_multiple_of(500) {
                info!(
                    replies = self.parameter_replies,
                    "Enumeration: {} parameter replies sent", self.parameter_replies
                );
            }
            return;
        }

        debug!(
            path = bare,
            "Unknown address — staying silent, as a desk does"
        );
    }

    /// Store a write and echo it back, which is what a real desk does.
    async fn handle_write(&mut self, path: &str, args: &[OscType], src: SocketAddr) {
        if let Some(first) = args.first() {
            self.values.insert(path.to_string(), first.clone());
        }
        if self.args.no_echo {
            debug!(path, "Write stored, echo suppressed (--no-echo)");
            return;
        }
        self.send_osc(src, path, args.to_vec()).await;
    }

    // ─── handshake / config ─────────────────────────────────────────────

    /// Replies for a config, layout or heartbeat query, or `None` if `bare` is
    /// not one. Also carries the lifecycle logging for those queries.
    fn config_reply(&mut self, bare: &str) -> Option<Vec<(String, Vec<OscType>)>> {
        let replies = self.config_reply_for(bare)?;
        if bare == "/Console/Name" {
            self.name_queries += 1;
            if self.name_queries == 1 {
                info!(console = %self.console_name, "Handshake: console identified");
            } else {
                info!(beat = self.name_queries - 1, "Heartbeat answered");
            }
        } else {
            info!(query = bare, "Handshake query answered");
        }
        Some(replies)
    }

    /// The exact reply paths and argument types `ipad_handshake` parses.
    ///
    /// The app's base query list never asks for the aux and group *counts* —
    /// the S-series desk volunteers them alongside the `modes` reply — so the
    /// mock volunteers them the same way, otherwise those two counts would
    /// stay at the app's defaults.
    fn config_reply_for(&self, bare: &str) -> Option<Vec<(String, Vec<OscType>)>> {
        let a = &self.args;
        let one = |path: &str, arg: OscType| vec![(path.to_string(), vec![arg])];
        let count = |path: &str, n: u16| one(path, OscType::Int(i32::from(n)));
        // Every bus mono: mode 1 = Mono, 2 = Stereo.
        let modes = |n: u16| vec![OscType::Int(1); usize::from(n)];

        let replies = match bare {
            "/Snapshots/Current_Snapshot" => one(bare, OscType::Int(1)),
            "/Console/Name" => one(bare, OscType::String(self.console_name.clone())),
            "/Console/Session/Filename" => {
                one(bare, OscType::String(self.session_filename.clone()))
            }
            // "Channels" and "Input_Channels" both mean the input count.
            "/Console/Channels" | "/Console/Input_Channels" => count(bare, a.inputs),
            "/Console/Aux_Outputs" => count(bare, a.auxes),
            "/Console/Group_Outputs" => count(bare, a.groups),
            "/Console/Matrix_Outputs" => count(bare, a.matrices),
            "/Console/Matrix_Inputs" => count(bare, a.matrix_inputs),
            "/Console/Control_Groups" => count(bare, a.control_groups),
            "/Console/Graphic_EQ" => count(bare, a.geqs),
            "/Console/Multis" => count(bare, 0),
            "/Console/Input_Channels/modes" => vec![(bare.to_string(), modes(a.inputs))],
            "/Console/Aux_Outputs/modes" => vec![
                ("/Console/Aux_Outputs".to_string(), vec![i(a.auxes)]),
                (bare.to_string(), modes(a.auxes)),
            ],
            "/Console/Group_Outputs/modes" => vec![
                ("/Console/Group_Outputs".to_string(), vec![i(a.groups)]),
                (bare.to_string(), modes(a.groups)),
            ],
            // The bus-type list: 1 = aux, 0 = group, one entry per bus.
            "/Console/Aux_Outputs/types" => {
                let mut types = vec![OscType::Int(1); usize::from(a.auxes)];
                types.extend(std::iter::repeat_n(OscType::Int(0), usize::from(a.groups)));
                vec![(bare.to_string(), types)]
            }
            "/Layout/Layout/Banks" => self.layout_banks(),
            _ => return None,
        };
        Some(replies)
    }

    /// Two surface banks, in the arg shape `try_parse_layout_bank` expects:
    /// side, bank number, label, two spare ints, then ten channel slots.
    fn layout_banks(&self) -> Vec<(String, Vec<OscType>)> {
        let inputs: Vec<(&str, u16)> = (1..=self.args.inputs.min(10))
            .map(|n| ("Input_Channels", n))
            .collect();
        let busses: Vec<(&str, u16)> = (1..=self.args.auxes.min(10))
            .map(|n| ("Aux_Outputs", n))
            .collect();
        vec![
            (
                "/Layout/Layout/Banks".to_string(),
                bank_args("Left", 1, "Inputs", &inputs),
            ),
            (
                "/Layout/Layout/Banks".to_string(),
                bank_args("Right", 1, "Busses", &busses),
            ),
        ]
    }

    // ─── parameter map ──────────────────────────────────────────────────

    /// The value for a bare parameter address, inventing a plausible default
    /// the first time a real channel's deeper leaf is asked for. `None` means
    /// the address is not on this desk at all.
    fn value_for(&mut self, bare: &str) -> Option<OscType> {
        if let Some(value) = self.values.get(bare) {
            return Some(value.clone());
        }
        let (label, leaf) = self.split_addressable(bare)?;
        let value = default_for_leaf(leaf, &label);
        self.values.insert(bare.to_string(), value.clone());
        Some(value)
    }

    /// Split `/Input_Channels/3/EQ/eq_gain_2` into its channel label ("Ch 3")
    /// and its leaf ("EQ/eq_gain_2"), rejecting channels this desk does not
    /// have.
    fn split_addressable<'a>(&self, path: &'a str) -> Option<(String, &'a str)> {
        let (wire_type, rest) = path.strip_prefix('/')?.split_once('/')?;
        let (number, leaf) = rest.split_once('/')?;
        let number: u16 = number.parse().ok()?;
        let (count, label, zero_based) = self.channel_type_info(wire_type)?;
        let display = if zero_based {
            number.checked_add(1)?
        } else {
            number
        };
        if display < 1 || display > count {
            return None;
        }
        Some((format!("{label} {display}"), leaf))
    }

    // ─── desk-side moves ────────────────────────────────────────────────

    /// Stand in for an operator pushing the Input 1 fader on the desk.
    async fn wiggle_fader(&mut self) {
        let Some(dest) = self.last_peer else {
            debug!("Wiggle skipped — nobody has talked to the desk yet");
            return;
        };
        if self.args.inputs == 0 {
            debug!("Wiggle skipped — this desk has no input channels");
            return;
        }
        self.wiggle_step = (self.wiggle_step + 1) % WIGGLE_STEPS.len();
        let db = WIGGLE_STEPS[self.wiggle_step];
        let path = "/Input_Channels/1/fader";
        self.values.insert(path.to_string(), fl(db));
        info!(db, %dest, "Desk fader moved on Input 1 (--wiggle)");
        self.send_osc(dest, path, vec![fl(db)]).await;
    }

    // ─── transmit ───────────────────────────────────────────────────────

    /// Send a single OSC message to the given destination.
    async fn send_osc(&self, dest: SocketAddr, path: &str, args: Vec<OscType>) {
        let msg = OscMessage {
            addr: path.to_string(),
            args,
        };
        debug!(path, args = ?msg.args, %dest, "Sent");
        match rosc::encoder::encode(&OscPacket::Message(msg)) {
            Ok(mut buf) => {
                if self.args.truncate_padding {
                    truncate_padding(&mut buf);
                }
                if let Err(e) = self.socket.send_to(&buf, dest).await {
                    error!("Failed to send to {dest}: {e}");
                }
            }
            Err(e) => {
                error!("Failed to encode OSC for {path}: {e}");
            }
        }
    }
}

// ─── free helpers ───────────────────────────────────────────────────────

/// Shorthand for the float encoding this dialect uses for levels and toggles.
fn fl(v: f32) -> OscType {
    OscType::Float(v)
}

/// Shorthand for a count argument.
fn i(v: u16) -> OscType {
    OscType::Int(i32::from(v))
}

/// Whether a packet (or any message inside a bundle) is a `/?` query.
fn packet_is_query(packet: &OscPacket) -> bool {
    match packet {
        OscPacket::Message(msg) => msg.addr.ends_with("/?"),
        OscPacket::Bundle(bundle) => bundle.content.iter().any(packet_is_query),
    }
}

/// Drop up to three trailing alignment NULs, the way DiGiCo desks do.
///
/// Capped at three on purpose: a conformant encoder pads to the next 4-byte
/// boundary, so a receiver that re-pads an unaligned datagram restores exactly
/// what was removed. Cutting a fourth byte would leave the packet genuinely
/// short rather than merely unaligned.
fn truncate_padding(buf: &mut Vec<u8>) {
    for _ in 0..3 {
        if buf.last() == Some(&0) {
            buf.pop();
        } else {
            break;
        }
    }
}

/// Layout bank args: side, bank number, label, two spare ints, ten slots.
/// An empty slot is a bare `0`, a filled one a channel-type/number pair.
fn bank_args(side: &str, number: i32, label: &str, slots: &[(&str, u16)]) -> Vec<OscType> {
    let mut args = vec![
        OscType::String(side.to_string()),
        OscType::Int(number),
        OscType::String(label.to_string()),
        OscType::Int(0),
        OscType::Int(0),
    ];
    let mut slots = slots.iter();
    for _ in 0..10 {
        match slots.next() {
            Some((wire_type, n)) => {
                args.push(OscType::String((*wire_type).to_string()));
                args.push(i(*n));
            }
            None => args.push(OscType::Int(0)),
        }
    }
    args
}

/// A plausible value for a leaf nobody has set yet, so enumeration never
/// stalls on a parameter this mock did not think to seed.
fn default_for_leaf(leaf: &str, label: &str) -> OscType {
    match leaf {
        "Channel_Input/name" => OscType::String(label.to_string()),
        "fader" => fl(-10.0),
        "Panner/pan" => fl(0.5),
        "Channel_Input/analog_gain" => fl(30.0),
        "Filters/lo_filter_freq" => fl(60.0),
        "Filters/hi_filter_freq" => fl(16000.0),
        // CG membership arrives as a bitmask, not a level.
        "CGs_level" | "CGs_mute" => OscType::Int(0),
        _ if leaf.ends_with("send_pan") => fl(0.5),
        _ if leaf.ends_with("send_level") => fl(-20.0),
        _ if leaf.contains("eq_Q") => fl(1.0),
        _ if leaf.contains("freq") => fl(1000.0),
        _ if leaf.contains("thresh") => fl(-20.0),
        _ if leaf.contains("ratio") => fl(2.0),
        // Everything else — toggles, gains, times — reads as 0.
        _ => fl(0.0),
    }
}

/// Tick the wiggle timer, or wait forever when `--wiggle` is off.
async fn wiggle_tick(interval: Option<&mut time::Interval>) {
    match interval {
        Some(timer) => {
            timer.tick().await;
        }
        None => std::future::pending().await,
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,mock_pad_console=debug")),
        )
        .init();

    let args = Args::parse();

    let mut console = match MockPadConsole::new(args).await {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to start mock Pad console: {e}");
            std::process::exit(1);
        }
    };

    info!("Mock Pad console running. Press Ctrl+C to stop.");
    if let Err(e) = console.run().await {
        error!("Mock Pad console error: {e}");
    }
}
