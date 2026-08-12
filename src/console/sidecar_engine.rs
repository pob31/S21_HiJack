//! Fader sidecar MIDI engine — the device thread.
//!
//! Owns the `midir` input+output connection *pair* for the sidecar
//! surface (X-Touch-class MCU boards, CC fader boxes, encoder boxes).
//! Deliberately dumb: raw bytes in → typed [`HwEvent`]s out to the
//! sidecar service (tokio side), typed motor moves in → raw bytes out.
//! No OSC, no taper math, no binding lookup — that all lives in
//! `sidecar_service` where it's testable without hardware.
//!
//! Same shape as [`crate::console::midi_engine`] / the Stream Deck
//! thread: `midir`'s handles are `!Send`, so both connections live on
//! one dedicated `std::thread`, commands arrive over an `mpsc`, and
//! the UI reads cached port state via `try_read` each frame. The
//! thread re-enumerates ports every ~2 s and auto-reconnects when the
//! configured input reappears after an unplug.
//!
//! ## Windows note
//! WinMM invokes the midir input callback on a driver-owned thread —
//! the callback here does the absolute minimum (copy ≤3 bytes into a
//! channel); all parsing happens on the engine thread.

use std::sync::mpsc;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use tracing::{debug, info, warn};

use crate::console::sidecar_decode::HwEvent;
use crate::model::sidecar::{ControlMode, ControlSelector};
use crate::ui::UiEvent;

/// `midir` client name shown to the host MIDI system.
const CLIENT_NAME: &str = "S21_HiJack sidecar";

/// Cached state the UI reads each frame via `try_read()`.
#[derive(Default, Clone)]
pub struct SidecarMidiState {
    /// Input port names (re-enumerated every ~2 s).
    pub available_inputs: Vec<String>,
    /// Output port names (re-enumerated every ~2 s).
    pub available_outputs: Vec<String>,
    /// Connected input port, if any.
    pub connected_input: Option<String>,
    /// Connected output port (motor feedback), if any. Can be `None`
    /// while the input is up — feedback simply doesn't flow.
    pub connected_output: Option<String>,
    /// Last connect / send error, surfaced in the UI.
    pub last_error: Option<String>,
}

/// Commands into the engine thread.
enum SidecarCmd {
    /// Connect input (+ output) by name. `output: None` = auto: use the
    /// output port with the same name as the input.
    Connect {
        input: String,
        output: Option<String>,
    },
    /// Disconnect and forget the configured ports (no auto-reconnect).
    Disconnect,
    /// Push a motor / LED-ring position (14-bit, 0..=16383), re-encoded
    /// per the control's mode. Relative and note controls are ignored.
    MotorMove {
        control: ControlSelector,
        mode: ControlMode,
        value14: u16,
    },
    Shutdown,
}

/// Everything the engine thread receives on its single channel: public
/// commands and raw bytes from the midir input callback.
enum ThreadMsg {
    Cmd(SidecarCmd),
    /// First ≤3 bytes of an inbound MIDI message + its actual length.
    Raw([u8; 3], usize),
}

/// Public handle. Cheap to clone via `Arc`. Spawns the engine thread on
/// construction and never blocks the caller.
pub struct SidecarMidiEngine {
    tx: mpsc::Sender<ThreadMsg>,
    state: Arc<RwLock<SidecarMidiState>>,
    join: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl SidecarMidiEngine {
    /// Construct the engine, spawning the device thread immediately.
    /// `hw_tx` delivers decoded events to the sidecar service;
    /// `ui_tx` carries connect/disconnect/error notifications.
    pub fn new(
        hw_tx: tokio::sync::mpsc::UnboundedSender<HwEvent>,
        ui_tx: mpsc::Sender<UiEvent>,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<ThreadMsg>();
        let state = Arc::new(RwLock::new(SidecarMidiState::default()));
        let state_thread = state.clone();
        let cb_tx = tx.clone();
        let join = std::thread::Builder::new()
            .name("sidecar-midi".into())
            .spawn(move || run_sidecar_thread(rx, cb_tx, state_thread, hw_tx, ui_tx))
            .expect("spawn sidecar midi thread");
        Arc::new(Self {
            tx,
            state,
            join: std::sync::Mutex::new(Some(join)),
        })
    }

    /// Snapshot of the cached port state (never blocks meaningfully).
    pub fn snapshot(&self) -> SidecarMidiState {
        self.state.read().map(|s| s.clone()).unwrap_or_default()
    }

    /// Whether the input side is currently connected.
    pub fn is_connected(&self) -> bool {
        self.state
            .read()
            .map(|s| s.connected_input.is_some())
            .unwrap_or(false)
    }

    /// Request a connect. `output: None` auto-matches the input name.
    pub fn connect(&self, input: String, output: Option<String>) {
        let _ = self
            .tx
            .send(ThreadMsg::Cmd(SidecarCmd::Connect { input, output }));
    }

    /// Disconnect and stop auto-reconnecting.
    pub fn disconnect(&self) {
        let _ = self.tx.send(ThreadMsg::Cmd(SidecarCmd::Disconnect));
    }

    /// Push a motor position (non-blocking enqueue).
    pub fn motor_move(&self, control: ControlSelector, mode: ControlMode, value14: u16) {
        let _ = self.tx.send(ThreadMsg::Cmd(SidecarCmd::MotorMove {
            control,
            mode,
            value14,
        }));
    }
}

impl Drop for SidecarMidiEngine {
    fn drop(&mut self) {
        let _ = self.tx.send(ThreadMsg::Cmd(SidecarCmd::Shutdown));
        if let Ok(mut guard) = self.join.lock()
            && let Some(handle) = guard.take()
        {
            let _ = handle.join();
        }
    }
}

// ─── Engine thread ───────────────────────────────────────────────────

struct Connections {
    input: MidiInputConnection<()>,
    input_name: String,
    output: Option<MidiOutputConnection>,
    output_name: Option<String>,
}

fn run_sidecar_thread(
    rx: mpsc::Receiver<ThreadMsg>,
    cb_tx: mpsc::Sender<ThreadMsg>,
    state: Arc<RwLock<SidecarMidiState>>,
    hw_tx: tokio::sync::mpsc::UnboundedSender<HwEvent>,
    ui_tx: mpsc::Sender<UiEvent>,
) {
    let mut conns: Option<Connections> = None;
    // Ports the operator asked for — kept across unplugs so the scan
    // loop can reconnect when the device reappears.
    let mut desired: Option<(String, Option<String>)> = None;
    // Running-status memory: some WinMM drivers hand us data-only
    // fragments; the last status byte fills them in.
    let mut last_status: Option<u8> = None;

    let enum_interval = Duration::from_secs(2);
    let mut last_enum = Instant::now()
        .checked_sub(Duration::from_secs(60))
        .unwrap_or_else(Instant::now);

    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(ThreadMsg::Cmd(SidecarCmd::Shutdown)) => return,
            Ok(ThreadMsg::Cmd(SidecarCmd::Connect { input, output })) => {
                conns = None; // drop existing connections first
                desired = Some((input.clone(), output.clone()));
                match open_connections(&input, output.as_deref(), &cb_tx) {
                    Ok(c) => {
                        info!(input = %c.input_name, output = ?c.output_name, "Sidecar MIDI connected");
                        set_connected(&state, Some(&c));
                        let _ = ui_tx.send(UiEvent::SidecarMidiConnected {
                            input: c.input_name.clone(),
                            output: c.output_name.clone(),
                        });
                        conns = Some(c);
                    }
                    Err(e) => {
                        warn!(port = %input, "Sidecar MIDI connect failed: {e}");
                        set_connected(&state, None);
                        if let Ok(mut s) = state.write() {
                            s.last_error = Some(e.clone());
                        }
                        let _ = ui_tx.send(UiEvent::SidecarError { message: e });
                    }
                }
            }
            Ok(ThreadMsg::Cmd(SidecarCmd::Disconnect)) => {
                desired = None;
                if conns.take().is_some() {
                    info!("Sidecar MIDI disconnected");
                    let _ = ui_tx.send(UiEvent::SidecarMidiDisconnected);
                }
                set_connected(&state, None);
            }
            Ok(ThreadMsg::Cmd(SidecarCmd::MotorMove {
                control,
                mode,
                value14,
            })) => {
                if let Some(c) = conns.as_mut()
                    && let Some(out) = c.output.as_mut()
                    && let Some(bytes) = encode_motor(&control, &mode, value14)
                {
                    if let Err(e) = out.send(&bytes) {
                        warn!("Sidecar motor send failed: {e}");
                        if let Ok(mut s) = state.write() {
                            s.last_error = Some(format!("motor send failed: {e}"));
                        }
                    }
                }
            }
            Ok(ThreadMsg::Raw(bytes, len)) => {
                if let Some(ev) = parse_midi(&bytes[..len.min(3)], &mut last_status) {
                    let _ = hw_tx.send(ev);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }

        // ── Periodic port scan: refresh combos, detect unplug, reconnect ──
        if last_enum.elapsed() >= enum_interval {
            last_enum = Instant::now();
            let (inputs, outputs) = enumerate_ports();

            // Unplug detection: the connected input vanished from the
            // scan. WinMM connections to unplugged devices go silent
            // rather than erroring, so this is the reliable signal.
            if let Some(c) = &conns
                && !inputs.iter().any(|n| *n == c.input_name)
            {
                warn!(port = %c.input_name, "Sidecar MIDI input vanished — disconnecting");
                conns = None;
                set_connected(&state, None);
                let _ = ui_tx.send(UiEvent::SidecarMidiDisconnected);
            }

            // Auto-reconnect: configured but not connected, and the
            // desired input is (back) in the list.
            if conns.is_none()
                && let Some((input, output)) = &desired
                && inputs.iter().any(|n| n == input)
                && let Ok(c) = open_connections(input, output.as_deref(), &cb_tx)
            {
                info!(input = %c.input_name, "Sidecar MIDI reconnected");
                set_connected(&state, Some(&c));
                let _ = ui_tx.send(UiEvent::SidecarMidiConnected {
                    input: c.input_name.clone(),
                    output: c.output_name.clone(),
                });
                conns = Some(c);
            }

            if let Ok(mut s) = state.write() {
                s.available_inputs = inputs;
                s.available_outputs = outputs;
            }
        }
    }
}

fn set_connected(state: &Arc<RwLock<SidecarMidiState>>, conns: Option<&Connections>) {
    if let Ok(mut s) = state.write() {
        match conns {
            Some(c) => {
                s.connected_input = Some(c.input_name.clone());
                s.connected_output = c.output_name.clone();
                s.last_error = None;
            }
            None => {
                s.connected_input = None;
                s.connected_output = None;
            }
        }
    }
}

/// List available (input, output) port names.
fn enumerate_ports() -> (Vec<String>, Vec<String>) {
    let inputs = match MidiInput::new(&format!("{CLIENT_NAME} scan-in")) {
        Ok(inp) => inp
            .ports()
            .iter()
            .filter_map(|p| inp.port_name(p).ok())
            .collect(),
        Err(e) => {
            debug!("Sidecar MIDI enumerate inputs failed: {e}");
            Vec::new()
        }
    };
    let outputs = match MidiOutput::new(&format!("{CLIENT_NAME} scan-out")) {
        Ok(out) => out
            .ports()
            .iter()
            .filter_map(|p| out.port_name(p).ok())
            .collect(),
        Err(e) => {
            debug!("Sidecar MIDI enumerate outputs failed: {e}");
            Vec::new()
        }
    };
    (inputs, outputs)
}

/// Open the input connection (callback → channel) and, best-effort, the
/// paired output. A missing output is not an error — motor feedback is
/// simply unavailable.
fn open_connections(
    input_name: &str,
    output_name: Option<&str>,
    cb_tx: &mpsc::Sender<ThreadMsg>,
) -> Result<Connections, String> {
    let inp = MidiInput::new(CLIENT_NAME).map_err(|e| format!("MIDI init failed: {e}"))?;
    let ports = inp.ports();
    let port = ports
        .iter()
        .find(|p| inp.port_name(p).map(|n| n == input_name).unwrap_or(false))
        .cloned()
        .ok_or_else(|| format!("MIDI input '{input_name}' not found"))?;

    let tx = cb_tx.clone();
    let input = inp
        .connect(
            &port,
            input_name,
            move |_ts, bytes, _| {
                // WinMM driver thread: copy ≤3 bytes and get out.
                let mut buf = [0u8; 3];
                let len = bytes.len().min(3);
                buf[..len].copy_from_slice(&bytes[..len]);
                let _ = tx.send(ThreadMsg::Raw(buf, len));
            },
            (),
        )
        .map_err(|e| format!("input connect failed: {e}"))?;

    // Output: explicit name, or auto-match the input's name.
    let wanted_out = output_name.unwrap_or(input_name);
    let (output, resolved_out) = match open_output(wanted_out) {
        Ok(c) => (Some(c), Some(wanted_out.to_string())),
        Err(e) => {
            debug!(port = %wanted_out, "Sidecar MIDI output unavailable: {e}");
            (None, None)
        }
    };

    Ok(Connections {
        input,
        input_name: input_name.to_string(),
        output,
        output_name: resolved_out,
    })
}

fn open_output(name: &str) -> Result<MidiOutputConnection, String> {
    let out = MidiOutput::new(CLIENT_NAME).map_err(|e| format!("MIDI init failed: {e}"))?;
    let ports = out.ports();
    let port = ports
        .iter()
        .find(|p| out.port_name(p).map(|n| n == name).unwrap_or(false))
        .cloned()
        .ok_or_else(|| format!("MIDI output '{name}' not found"))?;
    out.connect(&port, name)
        .map_err(|e| format!("output connect failed: {e}"))
}

/// Parse one inbound MIDI message into a typed event. Handles running
/// status (data-only fragments reuse the previous status byte).
/// Channels are converted to 1-based to match [`ControlSelector`].
fn parse_midi(bytes: &[u8], last_status: &mut Option<u8>) -> Option<HwEvent> {
    if bytes.is_empty() {
        return None;
    }
    let (status, data) = if bytes[0] >= 0x80 {
        // System messages (0xF0+) reset nothing we care about; skip.
        if bytes[0] >= 0xF0 {
            return None;
        }
        *last_status = Some(bytes[0]);
        (bytes[0], &bytes[1..])
    } else {
        ((*last_status)?, bytes)
    };

    let channel = (status & 0x0F) + 1;
    match status & 0xF0 {
        0xB0 if data.len() >= 2 => Some(HwEvent::Cc {
            channel,
            cc: data[0] & 0x7F,
            value: data[1] & 0x7F,
        }),
        0xE0 if data.len() >= 2 => Some(HwEvent::PitchBend {
            channel,
            value: u16::from(data[0] & 0x7F) | (u16::from(data[1] & 0x7F) << 7),
        }),
        // Note-on with velocity 0 is a release (X-Touch touch notes).
        0x90 if data.len() >= 2 => Some(HwEvent::Note {
            channel,
            note: data[0] & 0x7F,
            on: data[1] & 0x7F != 0,
        }),
        0x80 if data.len() >= 2 => Some(HwEvent::Note {
            channel,
            note: data[0] & 0x7F,
            on: false,
        }),
        _ => None,
    }
}

/// Re-encode a 14-bit position for the wire per the control's mode.
/// Relative encoders and note selectors have no position to drive.
fn encode_motor(control: &ControlSelector, mode: &ControlMode, value14: u16) -> Option<Vec<u8>> {
    let v = value14.min(16383);
    match (control, mode) {
        (ControlSelector::PitchBend { channel }, ControlMode::PitchBend14) => {
            let status = 0xE0 | (channel.saturating_sub(1) & 0x0F);
            Some(vec![status, (v & 0x7F) as u8, (v >> 7) as u8])
        }
        (ControlSelector::Cc { channel, cc }, ControlMode::Absolute14 { lsb_cc }) => {
            let status = 0xB0 | (channel.saturating_sub(1) & 0x0F);
            // MSB first, then LSB — the standard 14-bit CC order.
            Some(vec![
                status,
                *cc & 0x7F,
                (v >> 7) as u8,
                status,
                *lsb_cc & 0x7F,
                (v & 0x7F) as u8,
            ])
        }
        (ControlSelector::Cc { channel, cc }, ControlMode::Absolute7) => {
            let status = 0xB0 | (channel.saturating_sub(1) & 0x0F);
            Some(vec![status, *cc & 0x7F, (v >> 7) as u8])
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cc_pitchbend_and_notes() {
        let mut ls = None;
        assert_eq!(
            parse_midi(&[0xB0, 16, 100], &mut ls),
            Some(HwEvent::Cc {
                channel: 1,
                cc: 16,
                value: 100
            })
        );
        assert_eq!(
            parse_midi(&[0xE2, 0x01, 0x40], &mut ls),
            Some(HwEvent::PitchBend {
                channel: 3,
                value: (0x40 << 7) | 0x01
            })
        );
        assert_eq!(
            parse_midi(&[0x90, 0x68, 0x7F], &mut ls),
            Some(HwEvent::Note {
                channel: 1,
                note: 0x68,
                on: true
            })
        );
        // Note-on velocity 0 = release.
        assert_eq!(
            parse_midi(&[0x90, 0x68, 0x00], &mut ls),
            Some(HwEvent::Note {
                channel: 1,
                note: 0x68,
                on: false
            })
        );
        assert_eq!(
            parse_midi(&[0x85, 0x30, 0x40], &mut ls),
            Some(HwEvent::Note {
                channel: 6,
                note: 0x30,
                on: false
            })
        );
    }

    #[test]
    fn parse_running_status_reuses_last_status() {
        let mut ls = None;
        // Full message primes the status…
        assert!(parse_midi(&[0xB0, 16, 10], &mut ls).is_some());
        // …then a data-only fragment reuses it.
        assert_eq!(
            parse_midi(&[17, 20], &mut ls),
            Some(HwEvent::Cc {
                channel: 1,
                cc: 17,
                value: 20
            })
        );
        // Data-only with no prior status: dropped.
        let mut fresh = None;
        assert_eq!(parse_midi(&[17, 20], &mut fresh), None);
    }

    #[test]
    fn parse_ignores_system_and_short_messages() {
        let mut ls = None;
        assert_eq!(parse_midi(&[0xF8], &mut ls), None); // clock
        assert_eq!(parse_midi(&[0xB0, 16], &mut ls), None); // truncated
        assert_eq!(parse_midi(&[], &mut ls), None);
        // A system message must not clobber running status.
        assert!(parse_midi(&[0xB0, 16, 10], &mut ls).is_some());
        assert_eq!(parse_midi(&[0xF8], &mut ls), None);
        assert!(parse_midi(&[17, 20], &mut ls).is_some());
    }

    #[test]
    fn encode_motor_pitch_bend() {
        let bytes = encode_motor(
            &ControlSelector::PitchBend { channel: 3 },
            &ControlMode::PitchBend14,
            0x1234,
        )
        .unwrap();
        assert_eq!(bytes, vec![0xE2, 0x34, 0x24]);
    }

    #[test]
    fn encode_motor_14bit_cc_msb_then_lsb() {
        let bytes = encode_motor(
            &ControlSelector::Cc { channel: 1, cc: 16 },
            &ControlMode::Absolute14 { lsb_cc: 48 },
            16383,
        )
        .unwrap();
        assert_eq!(bytes, vec![0xB0, 16, 0x7F, 0xB0, 48, 0x7F]);
    }

    #[test]
    fn encode_motor_7bit_cc() {
        let bytes = encode_motor(
            &ControlSelector::Cc { channel: 2, cc: 7 },
            &ControlMode::Absolute7,
            8192,
        )
        .unwrap();
        assert_eq!(bytes, vec![0xB1, 7, 64]);
    }

    #[test]
    fn encode_motor_none_for_relative_and_notes() {
        use crate::model::sidecar::RelativeMode;
        assert!(
            encode_motor(
                &ControlSelector::Cc { channel: 1, cc: 60 },
                &ControlMode::Relative(RelativeMode::TwosComplement),
                100,
            )
            .is_none()
        );
        assert!(
            encode_motor(
                &ControlSelector::Note {
                    channel: 1,
                    note: 0x68
                },
                &ControlMode::Absolute7,
                100,
            )
            .is_none()
        );
    }
}
