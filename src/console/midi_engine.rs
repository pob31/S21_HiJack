//! MIDI output engine (v0.1.2).
//!
//! Owns a dedicated background thread that sends raw MIDI out a system port (or
//! a virtual port we create) via the `midir` crate. `midir`'s API is blocking
//! and its connection handle is not `Send`, so — exactly like the Stream Deck
//! device thread ([`crate::console::streamdeck_engine`]) — the connection lives
//! on its own `std::thread` and the rest of the app talks to it over an `mpsc`
//! channel.
//!
//! There is a single active MIDI output. MIDI cue triggers carry no target
//! reference: they are simply encoded to bytes and pushed onto the command
//! channel, so firing them from the async recall path never blocks tokio.
//!
//! While idle the thread re-enumerates available output ports every ~2 s so the
//! Setup UI's port combo stays fresh; the UI reads the cached state via
//! `try_read` each frame.
//!
//! ## Platform notes
//! - **macOS / Linux**: can create a *virtual* output port named `S21_HiJack`
//!   that other apps (Ableton, etc.) see as a MIDI source with zero setup.
//! - **Windows**: WinMM cannot create virtual ports — route through an existing
//!   port (e.g. loopMIDI). `create_virtual` returns an error there.

use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use midir::{MidiOutput, MidiOutputConnection};
use tracing::{debug, info, warn};

/// `midir` client name shown to the host MIDI system.
const CLIENT_NAME: &str = "S21_HiJack";

/// Suffix appended to the connected-port label when we created a virtual port,
/// so the UI can show "S21_HiJack (virtual)".
const VIRTUAL_SUFFIX: &str = " (virtual)";

/// Cached state the UI reads each frame via `try_read()`.
#[derive(Default, Clone)]
pub struct MidiEngineState {
    /// Names of available output ports (re-enumerated every ~2 s).
    pub available: Vec<String>,
    /// Label of the currently-connected port, if any (a port name, or
    /// `"<name> (virtual)"` for a virtual port we created).
    pub connected: Option<String>,
    /// Last connect / send error, surfaced in the UI.
    pub last_error: Option<String>,
}

/// Commands sent from the app to the MIDI thread.
enum MidiCmd {
    /// Connect to an existing output port by name.
    Connect(String),
    /// Create and connect to a virtual output port with the given name
    /// (macOS / Linux only).
    CreateVirtual(String),
    /// Disconnect the current port (if any).
    Disconnect,
    /// Send raw MIDI bytes out the connected port. Silently dropped (with a
    /// logged error) when not connected.
    Send(Vec<u8>),
    /// Tell the worker thread to exit cleanly.
    Shutdown,
}

/// Public handle. Cheap to clone via `Arc`. Spawns the MIDI thread on
/// construction and never blocks the caller.
pub struct MidiEngine {
    cmd_tx: mpsc::Sender<MidiCmd>,
    state: Arc<RwLock<MidiEngineState>>,
    /// Held so we can `take().join()` on Drop. Mutex keeps the engine
    /// `Send + Sync` for `Arc<MidiEngine>`.
    join: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl MidiEngine {
    /// Construct the engine, spawning the MIDI thread immediately. Starts
    /// disconnected and enumerates available ports every ~2 s.
    pub fn new() -> Arc<Self> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<MidiCmd>();
        let state = Arc::new(RwLock::new(MidiEngineState::default()));
        let state_thread = state.clone();
        let join = std::thread::Builder::new()
            .name("midi".into())
            .spawn(move || run_midi_thread(cmd_rx, state_thread))
            .expect("spawn midi worker thread");
        Arc::new(Self {
            cmd_tx,
            state,
            join: std::sync::Mutex::new(Some(join)),
        })
    }

    /// True iff the platform can create virtual MIDI output ports.
    pub fn supports_virtual_ports() -> bool {
        cfg!(unix)
    }

    /// Snapshot of available output port names (re-enumerated every ~2 s).
    pub fn available_ports(&self) -> Vec<String> {
        self.state
            .read()
            .map(|s| s.available.clone())
            .unwrap_or_default()
    }

    /// Label of the connected port, if any.
    pub fn connected_port(&self) -> Option<String> {
        self.state.read().ok().and_then(|s| s.connected.clone())
    }

    /// Whether a port is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connected_port().is_some()
    }

    /// Last error message, if any.
    pub fn last_error(&self) -> Option<String> {
        self.state.read().ok().and_then(|s| s.last_error.clone())
    }

    /// Request a connect to an existing output port by name.
    pub fn connect(&self, port_name: String) {
        let _ = self.cmd_tx.send(MidiCmd::Connect(port_name));
    }

    /// Request creation of a virtual output port (macOS / Linux only).
    pub fn create_virtual(&self, port_name: String) {
        let _ = self.cmd_tx.send(MidiCmd::CreateVirtual(port_name));
    }

    /// Disconnect the current port.
    pub fn disconnect(&self) {
        let _ = self.cmd_tx.send(MidiCmd::Disconnect);
    }

    /// Send raw MIDI bytes (non-blocking enqueue). Dropped if not connected.
    pub fn send(&self, bytes: Vec<u8>) {
        let _ = self.cmd_tx.send(MidiCmd::Send(bytes));
    }
}

impl Drop for MidiEngine {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(MidiCmd::Shutdown);
        if let Ok(mut guard) = self.join.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }
    }
}

// ─── Worker thread ───────────────────────────────────────────────────

fn run_midi_thread(cmd_rx: mpsc::Receiver<MidiCmd>, state: Arc<RwLock<MidiEngineState>>) {
    let mut conn: Option<MidiOutputConnection> = None;
    let enum_interval = Duration::from_secs(2);
    let mut last_enum = Instant::now()
        .checked_sub(Duration::from_secs(60))
        .unwrap_or_else(Instant::now);

    loop {
        // ── Drain queued commands without blocking ──
        loop {
            match cmd_rx.try_recv() {
                Ok(MidiCmd::Shutdown) => return,
                Ok(MidiCmd::Connect(name)) => {
                    conn = None; // drop any existing connection first
                    match connect_named(&name) {
                        Ok(c) => {
                            info!(port = %name, "MIDI output connected");
                            conn = Some(c);
                            if let Ok(mut s) = state.write() {
                                s.connected = Some(name.clone());
                                s.last_error = None;
                            }
                        }
                        Err(e) => {
                            warn!(port = %name, "MIDI connect failed: {e}");
                            if let Ok(mut s) = state.write() {
                                s.connected = None;
                                s.last_error = Some(e);
                            }
                        }
                    }
                }
                Ok(MidiCmd::CreateVirtual(name)) => {
                    conn = None;
                    match create_virtual_conn(&name) {
                        Ok(c) => {
                            info!(port = %name, "Virtual MIDI output created");
                            conn = Some(c);
                            if let Ok(mut s) = state.write() {
                                s.connected = Some(format!("{name}{VIRTUAL_SUFFIX}"));
                                s.last_error = None;
                            }
                        }
                        Err(e) => {
                            warn!(port = %name, "Virtual MIDI create failed: {e}");
                            if let Ok(mut s) = state.write() {
                                s.connected = None;
                                s.last_error = Some(e);
                            }
                        }
                    }
                }
                Ok(MidiCmd::Disconnect) => {
                    if conn.take().is_some() {
                        info!("MIDI output disconnected");
                    }
                    if let Ok(mut s) = state.write() {
                        s.connected = None;
                    }
                }
                Ok(MidiCmd::Send(bytes)) => {
                    if let Some(c) = conn.as_mut() {
                        if let Err(e) = c.send(&bytes) {
                            warn!("MIDI send failed: {e}");
                            if let Ok(mut s) = state.write() {
                                s.last_error = Some(format!("send failed: {e}"));
                            }
                        }
                    } else {
                        debug!("MIDI send dropped — no port connected");
                        if let Ok(mut s) = state.write() {
                            s.last_error = Some("send dropped — no MIDI port connected".into());
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }

        // ── Periodically re-enumerate available output ports ──
        if last_enum.elapsed() >= enum_interval {
            last_enum = Instant::now();
            let ports = enumerate_ports();
            if let Ok(mut s) = state.write() {
                s.available = ports;
            }
        }

        // Output-only: nothing to poll, so just idle briefly between scans
        // and command checks.
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// List available output port names (best-effort; empty on init failure).
fn enumerate_ports() -> Vec<String> {
    let out = match MidiOutput::new(&format!("{CLIENT_NAME} scan")) {
        Ok(o) => o,
        Err(e) => {
            debug!("MIDI enumerate: init failed: {e}");
            return Vec::new();
        }
    };
    out.ports()
        .iter()
        .filter_map(|p| out.port_name(p).ok())
        .collect()
}

/// Connect to an existing output port by name.
fn connect_named(name: &str) -> Result<MidiOutputConnection, String> {
    let out = MidiOutput::new(CLIENT_NAME).map_err(|e| format!("MIDI init failed: {e}"))?;
    let ports = out.ports();
    let port = ports
        .iter()
        .find(|p| out.port_name(p).map(|n| n == name).unwrap_or(false))
        .cloned()
        .ok_or_else(|| format!("MIDI port '{name}' not found"))?;
    out.connect(&port, name)
        .map_err(|e| format!("connect failed: {e}"))
}

/// Create + connect a virtual output port (macOS / Linux only).
#[cfg(unix)]
fn create_virtual_conn(name: &str) -> Result<MidiOutputConnection, String> {
    use midir::os::unix::VirtualOutput;
    let out = MidiOutput::new(CLIENT_NAME).map_err(|e| format!("MIDI init failed: {e}"))?;
    out.create_virtual(name)
        .map_err(|e| format!("virtual port create failed: {e}"))
}

#[cfg(not(unix))]
fn create_virtual_conn(_name: &str) -> Result<MidiOutputConnection, String> {
    Err("Virtual MIDI ports are not supported on this platform; route through an existing port (e.g. loopMIDI)".into())
}
