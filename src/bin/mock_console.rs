//! Mock S21 console simulator for testing the daemon without real hardware.
//!
//! Simulates GP OSC responses based on the captured iPad handshake data
//! from a real S21 (serial S21-210385).
//!
//! Run with: cargo run --bin mock_console -- --port 8000

use std::net::SocketAddr;

use clap::Parser;
use rosc::{OscMessage, OscPacket, OscType};
use tokio::net::UdpSocket;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

/// Mock DiGiCo S21 Console — GP OSC Simulator
#[derive(Parser, Debug)]
#[command(name = "mock_console")]
struct Args {
    /// UDP port to listen on
    #[arg(long, default_value_t = 8000)]
    port: u16,

    /// Number of input channels
    #[arg(long, default_value_t = 48)]
    inputs: u8,

    /// Number of aux outputs
    #[arg(long, default_value_t = 8)]
    auxes: u8,

    /// Number of group outputs
    #[arg(long, default_value_t = 9)]
    groups: u8,

    /// Number of matrix outputs
    #[arg(long, default_value_t = 8)]
    matrices: u8,

    /// Number of control groups
    #[arg(long, default_value_t = 10)]
    control_groups: u8,
}

struct MockConsole {
    socket: UdpSocket,
    config: Args,
}

impl MockConsole {
    async fn new(args: Args) -> std::io::Result<Self> {
        let addr: SocketAddr = format!("0.0.0.0:{}", args.port).parse().unwrap();
        let socket = UdpSocket::bind(addr).await?;
        info!("Mock S21 console listening on port {}", args.port);
        info!(
            "Config: {} inputs, {} auxes, {} groups, {} matrices, {} CGs",
            args.inputs, args.auxes, args.groups, args.matrices, args.control_groups
        );
        Ok(Self {
            socket,
            config: args,
        })
    }

    async fn run(&self) -> std::io::Result<()> {
        let mut buf = vec![0u8; 65536];
        loop {
            let (size, src) = self.socket.recv_from(&mut buf).await?;
            match rosc::decoder::decode_udp(&buf[..size]) {
                Ok((_, packet)) => {
                    self.handle_packet(packet, src).await;
                }
                Err(e) => {
                    warn!("Failed to decode OSC from {src}: {e}");
                }
            }
        }
    }

    async fn handle_packet(&self, packet: OscPacket, src: SocketAddr) {
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

    async fn handle_message(&self, path: &str, args: &[OscType], src: SocketAddr) {
        debug!(path, %src, "Received");

        // Report parameter writes by the channel they land on under this
        // mock's bus split, so a daemon's bus numbering can be checked
        // against a non-default split (e.g. `--auxes 10 --groups 14`).
        if let Some(rest) = path.strip_prefix("/channel/")
            && let Some((n, param)) = rest.split_once('/')
            && let Ok(n) = n.parse::<u8>()
            && !args.is_empty()
        {
            let channel = self.channel_name(n);
            info!(%channel, param, ?args, "Write from {src}");
            return;
        }

        match path {
            "/console/channel/counts" => {
                info!("Discovery request from {src} — sending channel counts");
                self.send_channel_counts(src).await;
            }
            "/console/resend" => {
                info!("Resend request from {src} — dumping full state");
                self.send_full_state(src).await;
            }
            "/console/ping" => {
                debug!("Ping from {src} — sending pong");
                self.send_osc(src, "/console/pong", vec![]).await;
            }
            _ => {
                debug!(path, "Unhandled message from {src}");
            }
        }
    }

    /// Reply to a counts query in the positional form a real S21+ uses:
    /// `/console/channel/counts <inputs> <aux> <groups> <control_groups>
    /// <matrices> <master>`.
    async fn send_channel_counts(&self, dest: SocketAddr) {
        let c = &self.config;
        let counts = [c.inputs, c.auxes, c.groups, c.control_groups, c.matrices, 1];
        let args = counts.iter().map(|&n| OscType::Int(n.into())).collect();
        self.send_osc(dest, "/console/channel/counts", args).await;
    }

    /// First GP OSC number of the group buses. Buses 70–93 are one pool,
    /// auxes first, so the groups start right after the last aux.
    fn first_group_number(&self) -> i32 {
        70 + i32::from(self.config.auxes)
    }

    /// Name a GP OSC channel number under this mock's configuration.
    fn channel_name(&self, n: u8) -> String {
        let c = &self.config;
        let n16 = u16::from(n);
        let first_group = self.first_group_number() as u16;
        match n16 {
            1..=60 if n <= c.inputs => format!("Input {n}"),
            70..=93 if n16 < first_group => format!("Aux {}", n16 - 69),
            70..=93 if n16 < first_group + u16::from(c.groups) => {
                format!("Group {}", n16 - first_group + 1)
            }
            110..=119 if n16 - 109 <= u16::from(c.control_groups) => format!("CG {}", n16 - 109),
            120..=127 if n16 - 119 <= u16::from(c.matrices) => format!("Matrix {}", n16 - 119),
            _ => format!("unassigned GP channel {n}"),
        }
    }

    /// Dump full console state — fader, mute, solo, name for all channels.
    async fn send_full_state(&self, dest: SocketAddr) {
        let mut msg_count = 0u32;

        // Input channels: OSC 1–N
        for i in 1..=self.config.inputs {
            let ch = i as i32;
            self.send_channel_state(dest, ch, &format!("Input {i}"))
                .await;
            msg_count += 4;
        }

        // Aux outputs: OSC 70–(70+N-1)
        for i in 1..=self.config.auxes {
            let ch = 69 + i as i32;
            self.send_channel_state(dest, ch, &format!("Aux {i}")).await;
            msg_count += 4;
        }

        // Group outputs: straight after the auxes in the shared bus pool
        for i in 1..=self.config.groups {
            let ch = self.first_group_number() - 1 + i as i32;
            self.send_channel_state(dest, ch, &format!("Group {i}"))
                .await;
            msg_count += 4;
        }

        // Matrix outputs: OSC 120–(120+N-1)
        for i in 1..=self.config.matrices {
            let ch = 119 + i as i32;
            self.send_channel_state(dest, ch, &format!("Matrix {i}"))
                .await;
            msg_count += 4;
        }

        // Control groups: OSC 110–(110+N-1)
        for i in 1..=self.config.control_groups {
            let ch = 109 + i as i32;
            self.send_channel_state(dest, ch, &format!("CG {i}")).await;
            msg_count += 4;
        }

        info!(msg_count, "Full state dump complete");
    }

    /// Send fader, mute, solo, name for a single channel.
    async fn send_channel_state(&self, dest: SocketAddr, osc_ch: i32, name: &str) {
        let prefix = format!("/channel/{osc_ch}");

        // Fader at -150 dB (fully down)
        self.send_osc(
            dest,
            &format!("{prefix}/fader"),
            vec![OscType::Float(-150.0)],
        )
        .await;

        // Mute off
        self.send_osc(dest, &format!("{prefix}/mute"), vec![OscType::Int(0)])
            .await;

        // Solo off
        self.send_osc(dest, &format!("{prefix}/solo"), vec![OscType::Int(0)])
            .await;

        // Channel name
        self.send_osc(
            dest,
            &format!("{prefix}/name"),
            vec![OscType::String(name.to_string())],
        )
        .await;
    }

    /// Send a single OSC message to the given destination.
    async fn send_osc(&self, dest: SocketAddr, path: &str, args: Vec<OscType>) {
        let msg = OscMessage {
            addr: path.to_string(),
            args,
        };
        let packet = OscPacket::Message(msg);
        match rosc::encoder::encode(&packet) {
            Ok(buf) => {
                if let Err(e) = self.socket.send_to(&buf, dest).await {
                    error!("Failed to send to {dest}: {e}");
                }
            }
            Err(e) => {
                error!("Failed to encode OSC: {e}");
            }
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,mock_console=debug,sctk_adwaita=error")),
        )
        .init();

    let args = Args::parse();

    let console = match MockConsole::new(args).await {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to start mock console: {e}");
            std::process::exit(1);
        }
    };

    info!("Mock console running. Press Ctrl+C to stop.");
    if let Err(e) = console.run().await {
        error!("Mock console error: {e}");
    }
}
