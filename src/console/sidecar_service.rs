//! Fader sidecar service — binding-table runtime on the tokio side.
//!
//! Sits between the sidecar MIDI engine (device thread, dumb bytes)
//! and the console: decodes hardware events through each binding's
//! taper and writes the result to the desk (or a raw OSC target), and
//! mirrors console-state changes back onto motorized faders.
//!
//! ## Feedback-loop protection (the key correctness concern)
//! A motor fader creates a potential loop: console echo → motor move →
//! (surface echoes the move) → MIDI in → console send → …. Three
//! guards, mirroring `gang_engine`'s proven suppression shape:
//! 1. **Touch gate** — while an MCU touch note is held, motor pushes to
//!    that control are suppressed (the motor never fights the hand);
//!    on release the motor snaps to console truth once.
//! 2. **`sent_to_console`** — values we just sent (and optimistically
//!    mirrored) are consumed-on-match by the motor poll so our own
//!    write's generation bump doesn't bounce back to the motor.
//! 3. **`sent_to_motor`** — motor positions we just pushed are matched
//!    against inbound hardware events so surfaces that echo motor
//!    moves (most non-touch CC boards) don't loop back to the console.
//!
//! ## Console-wins sync
//! On console (re)connect, MIDI (re)connect, enable, and show load the
//! service sweeps console state out to every feedback-capable motor —
//! stale hardware positions never blast the console (requirement:
//! "the sidecar can be all messed up when connecting").
//!
//! ## Disable semantics
//! The master rocker only flips `SidecarConfig::enabled`; this service
//! then drops both directions while the MIDI connection and port scans
//! stay warm, so re-enable is instant and starts with a sync sweep.
//!
//! An operator moving a hardware fader mid-recall needs no special
//! wiring here: the console echoes our send, `process_message` runs
//! `automation_registry::maybe_override`, and the un-matched value
//! registers as an operator override — same as touching the desk.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{RwLock, mpsc, watch};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::console::cue_manager::CueManager;
use crate::console::send_util::send_parameter;
use crate::console::sidecar_decode::{DecodeState, HwEvent, decode, event_matches};
use crate::console::sidecar_learn::LearnShared;
use crate::model::parameter::{ParameterAddress, ParameterValue};
use crate::model::sidecar::{
    BindingTarget, ControlMode, ControlSelector, SidecarBinding, SidecarConfig, taper_to_norm,
    taper_to_value,
};
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::ipad_client::IpadSender;

/// Echo-suppression window (matches `gang_engine`).
const SUPPRESSION_WINDOW: Duration = Duration::from_millis(300);
/// Value tolerance for console-side echo matching (dB / units).
const FLOAT_TOLERANCE: f32 = 0.01;
/// Per-binding floor between console sends (~66 Hz per control; a
/// deliberate cap so a wide-open pitch-bend stream can't flood the
/// desk's ARM chip — snapshot recalls hit it far harder than this).
const CONSOLE_SEND_FLOOR: Duration = Duration::from_millis(15);
/// Motor poll / coalesce-flush cadence (~40 Hz — motors can't usefully
/// track faster).
const TICK: Duration = Duration::from_millis(25);
/// Hardware-echo tolerance in 14-bit steps, per mode: a 7-bit surface
/// echoes our 14-bit motor value quantized to the MSB (up to 127 off).
fn motor_echo_tolerance(mode: &ControlMode) -> u16 {
    match mode {
        ControlMode::Absolute7 => 128,
        _ => 16,
    }
}

/// UI → service commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SvcCmd {
    /// Push console state onto all feedback-capable motors ("console
    /// wins"). Sent on enable, MIDI reconnect, show load, and via the
    /// tab's "Sync surface now" button. Also clears decode state so
    /// edited bindings restart clean.
    SyncSurface,
}

/// Everything the service task needs. All handles are shared with the
/// app; the receivers are owned.
pub struct SidecarDeps {
    pub config: Arc<RwLock<SidecarConfig>>,
    pub state: Arc<RwLock<ConsoleState>>,
    /// Decoded hardware events from the sidecar MIDI engine.
    pub hw_rx: mpsc::UnboundedReceiver<HwEvent>,
    /// UI commands (sync requests).
    pub svc_rx: mpsc::UnboundedReceiver<SvcCmd>,
    /// Live console senders, rewired after each (re)connect from
    /// `pickup_pending_engines`; `None` while disconnected.
    pub senders: watch::Receiver<Option<(OscSender, Option<IpadSender>)>>,
    /// For resolving `RawOsc { target_id }` against the show's targets.
    pub cue_manager: Arc<RwLock<CueManager>>,
    /// Learn capture shared with the Sidecar tab.
    pub learn: Arc<std::sync::Mutex<LearnShared>>,
    /// Motor output — the app passes a closure onto
    /// `SidecarMidiEngine::motor_move`; tests pass a channel writer.
    pub motor: Arc<dyn Fn(ControlSelector, ControlMode, u16) + Send + Sync>,
}

/// Spawn the service on the ambient tokio runtime. Runs until the
/// hardware channel closes (i.e. the MIDI engine is dropped at app
/// shutdown). Callers outside a runtime context can `handle.spawn(run(deps))`.
pub fn spawn(deps: SidecarDeps) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run(deps))
}

#[derive(Default)]
struct Runtime {
    decode: HashMap<Uuid, DecodeState>,
    /// Console-echo suppression: what we sent, consumed on match.
    sent_to_console: HashMap<ParameterAddress, (f32, Instant)>,
    /// Hardware-echo suppression: what we pushed to motors.
    sent_to_motor: HashMap<ControlSelector, (u16, Instant)>,
    /// Touch selectors currently held down.
    touched: HashSet<ControlSelector>,
    /// Last value14 pushed per binding (dedup for the motor poll).
    last_motor_value: HashMap<Uuid, u16>,
    /// Per-binding console-send rate limiting + pending coalesced value.
    last_console_send: HashMap<Uuid, Instant>,
    pending_console: HashMap<Uuid, f32>,
    /// Console-state generation at the last motor poll.
    last_generation: u64,
}

pub async fn run(mut deps: SidecarDeps) {
    let mut rt = Runtime::default();
    let mut raw_osc_socket: Option<tokio::net::UdpSocket> = None;
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    info!("Sidecar service started");
    loop {
        tokio::select! {
            ev = deps.hw_rx.recv() => {
                let Some(ev) = ev else {
                    info!("Sidecar service stopping — hardware channel closed");
                    return;
                };
                handle_hw_event(&deps, &mut rt, &mut raw_osc_socket, &ev).await;
            }
            cmd = deps.svc_rx.recv() => {
                match cmd {
                    Some(SvcCmd::SyncSurface) => sync_surface(&deps, &mut rt).await,
                    None => {
                        // The UI half owns this sender for the app's
                        // lifetime — closure means shutdown.
                        info!("Sidecar service stopping — command channel closed");
                        return;
                    }
                }
            }
            changed = deps.senders.changed() => {
                if changed.is_err() {
                    // Watch sender dropped — app teardown. Exiting here
                    // also avoids a busy loop (a closed watch reports
                    // "changed" immediately, forever).
                    info!("Sidecar service stopping — sender watch closed");
                    return;
                }
                if deps.senders.borrow().is_some() {
                    // Console (re)connected — console wins.
                    sync_surface(&deps, &mut rt).await;
                }
            }
            _ = tick.tick() => {
                flush_pending_console(&deps, &mut rt).await;
                motor_poll(&deps, &mut rt).await;
            }
        }
    }
}

/// One hardware event: learn capture, touch gating, decode, send.
async fn handle_hw_event(
    deps: &SidecarDeps,
    rt: &mut Runtime,
    raw_osc_socket: &mut Option<tokio::net::UdpSocket>,
    ev: &HwEvent,
) {
    // Learn swallows everything while armed.
    {
        let active = deps.learn.lock().map(|g| g.active).unwrap_or(false);
        if active {
            LearnShared::feed(&deps.learn, ev, Instant::now());
            return;
        }
    }

    let config = deps.config.read().await.clone();
    if !config.enabled {
        return;
    }

    // Touch-sense gate: note events matching a binding's touch selector
    // toggle the gate; on release, snap that motor to console truth so
    // the hand-off never leaves the fader parked on a stale position.
    if let HwEvent::Note { on, .. } = ev {
        let mut matched = false;
        for b in &config.bindings {
            if let Some(touch) = &b.touch
                && event_matches(touch, ev)
            {
                matched = true;
                if *on {
                    rt.touched.insert(*touch);
                } else {
                    rt.touched.remove(touch);
                    push_binding_to_motor(deps, rt, b).await;
                }
            }
        }
        if matched {
            return;
        }
    }

    let now = Instant::now();
    for b in &config.bindings {
        if !b.enabled {
            continue;
        }
        // Hardware echo of our own motor push? Consume and drop.
        if b.mode.is_absolute()
            && event_matches(&b.control, ev)
            && let Some((sent_v14, at)) = rt.sent_to_motor.get(&b.control).copied()
        {
            let ev_v14 = match ev {
                HwEvent::PitchBend { value, .. } => Some(*value),
                HwEvent::Cc { value, .. } if matches!(b.mode, ControlMode::Absolute7) => {
                    Some(u16::from(*value) << 7)
                }
                _ => None,
            };
            if let Some(v14) = ev_v14
                && now.duration_since(at) <= SUPPRESSION_WINDOW
                && v14.abs_diff(sent_v14) <= motor_echo_tolerance(&b.mode)
            {
                rt.sent_to_motor.remove(&b.control);
                continue;
            }
        }

        let st = rt.decode.entry(b.id).or_default();
        // Seed relative encoders from live console state so the first
        // tick nudges from reality.
        if matches!(b.mode, ControlMode::Relative(_))
            && st.last_norm.is_none()
            && let BindingTarget::ConsoleParameter { address } = &b.target
            && let Some(ParameterValue::Float(v)) = deps.state.read().await.get(address).cloned()
        {
            st.seed(taper_to_norm(&b.taper, v));
        }

        let Some(norm) = decode(b, st, ev, now) else {
            continue;
        };
        dispatch_value(deps, rt, raw_osc_socket, b, norm, now).await;
    }
}

/// Route a decoded normalized position to the binding's target,
/// applying the taper and the per-binding console-send floor.
async fn dispatch_value(
    deps: &SidecarDeps,
    rt: &mut Runtime,
    raw_osc_socket: &mut Option<tokio::net::UdpSocket>,
    b: &SidecarBinding,
    norm: f32,
    now: Instant,
) {
    match &b.target {
        BindingTarget::ConsoleParameter { .. } => {
            // Coalesce: latest-wins under the per-binding send floor;
            // the tick loop flushes what's pending.
            let due = rt
                .last_console_send
                .get(&b.id)
                .is_none_or(|t| now.duration_since(*t) >= CONSOLE_SEND_FLOOR);
            if due {
                rt.pending_console.remove(&b.id);
                rt.last_console_send.insert(b.id, now);
                send_console_value(deps, rt, b, norm).await;
            } else {
                rt.pending_console.insert(b.id, norm);
            }
        }
        BindingTarget::RawOsc { .. } => {
            send_raw_osc(deps, raw_osc_socket, b, norm).await;
        }
    }
}

/// Send one value to the console (with optimistic mirror update).
async fn send_console_value(deps: &SidecarDeps, rt: &mut Runtime, b: &SidecarBinding, norm: f32) {
    let BindingTarget::ConsoleParameter { address } = &b.target else {
        return;
    };
    let Some((sender, ipad)) = deps.senders.borrow().clone() else {
        // No console link — nothing sensible to do with the move.
        return;
    };
    let raw = taper_to_value(&b.taper, norm);
    let value = address.parameter.clamp_value(ParameterValue::Float(raw));
    let ParameterValue::Float(v) = value else {
        return;
    };

    rt.sent_to_console
        .insert(address.clone(), (v, Instant::now()));
    if send_parameter(&sender, &ipad, address, &ParameterValue::Float(v)).await {
        // Optimistic mirror update (fade-engine precedent): the UI
        // reflects the move immediately; the console's echo then
        // matches both this value and the suppression entry above.
        deps.state
            .write()
            .await
            .update(address.clone(), ParameterValue::Float(v));
    } else {
        debug!(%address, "Sidecar console send failed to encode");
    }
}

/// Send a raw OSC value to an external target (fire-and-forget UDP,
/// same pattern as the trigger dispatcher). The tapered value is
/// appended as a trailing Float after the fixed args.
async fn send_raw_osc(
    deps: &SidecarDeps,
    socket_slot: &mut Option<tokio::net::UdpSocket>,
    b: &SidecarBinding,
    norm: f32,
) {
    let BindingTarget::RawOsc {
        target_id,
        host,
        port,
        path,
        args,
    } = &b.target
    else {
        return;
    };

    // Resolve destination: named target first, inline host/port second.
    let dest = if let Some(id) = target_id {
        let mgr = deps.cue_manager.read().await;
        mgr.osc_targets.get(id).map(|t| (t.host.clone(), t.port))
    } else {
        None
    };
    let (host, port) = match dest {
        Some(hp) => hp,
        None => match (host, port) {
            (Some(h), Some(p)) => (h.clone(), *p),
            _ => {
                debug!(binding = %b.label, "Raw OSC binding has no destination");
                return;
            }
        },
    };

    let mut osc_args: Vec<rosc::OscType> = args.iter().map(|a| a.to_osc()).collect();
    osc_args.push(rosc::OscType::Float(taper_to_value(&b.taper, norm)));
    let packet = rosc::OscPacket::Message(rosc::OscMessage {
        addr: path.clone(),
        args: osc_args,
    });
    let Ok(buf) = rosc::encoder::encode(&packet) else {
        warn!(path = %path, "Raw OSC encode failed");
        return;
    };

    if socket_slot.is_none() {
        match tokio::net::UdpSocket::bind("0.0.0.0:0").await {
            Ok(s) => *socket_slot = Some(s),
            Err(e) => {
                warn!("Raw OSC socket bind failed: {e}");
                return;
            }
        }
    }
    if let Some(socket) = socket_slot
        && let Err(e) = socket.send_to(&buf, (host.as_str(), port)).await
    {
        debug!(host = %host, port, "Raw OSC send failed: {e}");
    }
}

/// Flush coalesced console values whose send floor has elapsed.
async fn flush_pending_console(deps: &SidecarDeps, rt: &mut Runtime) {
    if rt.pending_console.is_empty() {
        return;
    }
    let config = deps.config.read().await.clone();
    if !config.enabled {
        rt.pending_console.clear();
        return;
    }
    let now = Instant::now();
    let due: Vec<(Uuid, f32)> = rt
        .pending_console
        .iter()
        .filter(|(id, _)| {
            rt.last_console_send
                .get(id)
                .is_none_or(|t| now.duration_since(*t) >= CONSOLE_SEND_FLOOR)
        })
        .map(|(id, norm)| (*id, *norm))
        .collect();
    for (id, norm) in due {
        rt.pending_console.remove(&id);
        if let Some(b) = config.bindings.iter().find(|b| b.id == id) {
            rt.last_console_send.insert(id, now);
            send_console_value(deps, rt, b, norm).await;
        }
    }
}

/// Diff console state onto the motors (generation-keyed, like
/// `monitor_engine::poll_and_push_state_changes`).
async fn motor_poll(deps: &SidecarDeps, rt: &mut Runtime) {
    let config = deps.config.read().await.clone();
    if !config.enabled {
        return;
    }
    {
        let state = deps.state.read().await;
        if state.generation() == rt.last_generation {
            return;
        }
        rt.last_generation = state.generation();
    }

    for b in &config.bindings {
        if !b.enabled || !b.wants_motor_feedback() {
            continue;
        }
        let BindingTarget::ConsoleParameter { address } = &b.target else {
            continue;
        };
        let Some(ParameterValue::Float(v)) = deps.state.read().await.get(address).cloned() else {
            continue;
        };
        // Our own send (or its echo): consume the suppression entry
        // instead of bouncing it back to the motor.
        if let Some((sent, at)) = rt.sent_to_console.get(address).copied() {
            if at.elapsed() <= SUPPRESSION_WINDOW && (v - sent).abs() <= FLOAT_TOLERANCE {
                rt.sent_to_console.remove(address);
                continue;
            }
            if at.elapsed() > SUPPRESSION_WINDOW {
                rt.sent_to_console.remove(address);
            }
        }
        // Never fight the operator's hand.
        if let Some(touch) = &b.touch
            && rt.touched.contains(touch)
        {
            continue;
        }
        let v14 = (taper_to_norm(&b.taper, v) * 16383.0).round() as u16;
        if rt.last_motor_value.get(&b.id) == Some(&v14) {
            continue;
        }
        rt.last_motor_value.insert(b.id, v14);
        rt.sent_to_motor.insert(b.control, (v14, Instant::now()));
        (deps.motor)(b.control, b.mode, v14);
    }
}

/// Push one binding's console value to its motor unconditionally
/// (touch release hand-off).
async fn push_binding_to_motor(deps: &SidecarDeps, rt: &mut Runtime, b: &SidecarBinding) {
    if !b.wants_motor_feedback() {
        return;
    }
    let BindingTarget::ConsoleParameter { address } = &b.target else {
        return;
    };
    let Some(ParameterValue::Float(v)) = deps.state.read().await.get(address).cloned() else {
        return;
    };
    let v14 = (taper_to_norm(&b.taper, v) * 16383.0).round() as u16;
    rt.last_motor_value.insert(b.id, v14);
    rt.sent_to_motor.insert(b.control, (v14, Instant::now()));
    (deps.motor)(b.control, b.mode, v14);
}

/// Console-wins sweep: push mirror values to every feedback-capable
/// motor, bypassing dedup, and reset decode state so edited bindings
/// restart clean. Parameters the mirror has never seen are skipped —
/// we don't guess.
async fn sync_surface(deps: &SidecarDeps, rt: &mut Runtime) {
    let config = deps.config.read().await.clone();
    rt.decode.clear();
    rt.pending_console.clear();
    if !config.enabled {
        return;
    }
    let mut pushed = 0usize;
    for b in &config.bindings {
        if !b.enabled || !b.wants_motor_feedback() {
            continue;
        }
        let BindingTarget::ConsoleParameter { address } = &b.target else {
            continue;
        };
        let Some(ParameterValue::Float(v)) = deps.state.read().await.get(address).cloned() else {
            continue;
        };
        let v14 = (taper_to_norm(&b.taper, v) * 16383.0).round() as u16;
        rt.last_motor_value.insert(b.id, v14);
        rt.sent_to_motor.insert(b.control, (v14, Instant::now()));
        (deps.motor)(b.control, b.mode, v14);
        pushed += 1;
    }
    // Track the generation we synced at so the next poll only diffs
    // newer changes.
    rt.last_generation = deps.state.read().await.generation();
    info!(pushed, "Sidecar surface synced from console state");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::parameter::ParameterPath;
    use crate::model::sidecar::Taper;
    use crate::osc::client::OscClient;
    use std::net::SocketAddr;
    use std::sync::Mutex as StdMutex;
    use tokio::net::UdpSocket;

    fn fader_addr(n: u8) -> ParameterAddress {
        ParameterAddress {
            channel: ChannelId::Input(n),
            parameter: ParameterPath::Fader,
        }
    }

    fn pb_binding(midi_channel: u8, input: u8) -> SidecarBinding {
        SidecarBinding {
            id: Uuid::from_bytes([midi_channel; 16]),
            label: String::new(),
            control: ControlSelector::PitchBend {
                channel: midi_channel,
            },
            mode: ControlMode::PitchBend14,
            target: BindingTarget::ConsoleParameter {
                address: fader_addr(input),
            },
            taper: Taper::FaderDb { max_db: 10.0 },
            motor_feedback: true,
            touch: crate::model::sidecar::mcu_default_touch_note(midi_channel),
            relative_step: 1.0 / 300.0,
            enabled: true,
        }
    }

    struct Harness {
        hw_tx: mpsc::UnboundedSender<HwEvent>,
        svc_tx: mpsc::UnboundedSender<SvcCmd>,
        senders_tx: watch::Sender<Option<(OscSender, Option<IpadSender>)>>,
        state: Arc<RwLock<ConsoleState>>,
        motor_log: Arc<StdMutex<Vec<(ControlSelector, u16)>>>,
        /// Socket standing in for the console.
        console_sock: UdpSocket,
        _task: tokio::task::JoinHandle<()>,
    }

    async fn harness(config: SidecarConfig) -> Harness {
        let console_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let console_addr = console_sock.local_addr().unwrap();

        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let client = OscClient::new(local, console_addr, None).await.unwrap();
        let (sender, _rx) = client.into_parts();

        let (hw_tx, hw_rx) = mpsc::unbounded_channel();
        let (svc_tx, svc_rx) = mpsc::unbounded_channel();
        let (senders_tx, senders_rx) = watch::channel(Some((sender, None)));

        let config = Arc::new(RwLock::new(config));
        let state = Arc::new(RwLock::new(ConsoleState::new(
            crate::model::config::ConsoleConfig::default(),
        )));
        let motor_log: Arc<StdMutex<Vec<(ControlSelector, u16)>>> =
            Arc::new(StdMutex::new(Vec::new()));
        let motor_log_clone = motor_log.clone();

        let deps = SidecarDeps {
            config: config.clone(),
            state: state.clone(),
            hw_rx,
            svc_rx,
            senders: senders_rx,
            cue_manager: Arc::new(RwLock::new(CueManager::new(
                crate::model::snapshot::CueList::default(),
            ))),
            learn: Arc::new(StdMutex::new(LearnShared::default())),
            motor: Arc::new(move |c, _m, v| {
                motor_log_clone.lock().unwrap().push((c, v));
            }),
        };
        let task = spawn(deps);
        Harness {
            hw_tx,
            svc_tx,
            senders_tx,
            state,
            motor_log,
            console_sock,
            _task: task,
        }
    }

    async fn recv_osc(sock: &UdpSocket) -> Option<(String, Vec<rosc::OscType>)> {
        let mut buf = [0u8; 1024];
        let got = tokio::time::timeout(Duration::from_millis(500), sock.recv_from(&mut buf))
            .await
            .ok()?;
        let (n, _) = got.ok()?;
        let (_, packet) = rosc::decoder::decode_udp(&buf[..n]).ok()?;
        match packet {
            rosc::OscPacket::Message(m) => Some((m.addr, m.args)),
            _ => None,
        }
    }

    #[tokio::test]
    async fn fader_move_reaches_console_and_mirror() {
        let cfg = SidecarConfig {
            enabled: true,
            bindings: vec![pb_binding(1, 12)],
        };
        let h = harness(cfg).await;

        // A deliberate fader move on PB channel 1 → console CH12 fader.
        h.hw_tx
            .send(HwEvent::PitchBend {
                channel: 1,
                value: 12287, // 75% travel = unity (0 dB)
            })
            .unwrap();

        let (path, args) = recv_osc(&h.console_sock).await.expect("console datagram");
        assert_eq!(path, "/channel/12/fader");
        let rosc::OscType::Float(db) = args[0] else {
            panic!("expected float arg");
        };
        assert!(db.abs() < 0.1, "75% travel should be ~unity, got {db}");

        // Optimistic mirror update landed.
        tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if let Some(ParameterValue::Float(v)) =
                    h.state.read().await.get(&fader_addr(12)).cloned()
                    && v.abs() < 0.1
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("mirror updated");
    }

    #[tokio::test]
    async fn disabled_sidecar_drops_hardware_moves() {
        let cfg = SidecarConfig {
            enabled: false,
            bindings: vec![pb_binding(1, 12)],
        };
        let h = harness(cfg).await;
        h.hw_tx
            .send(HwEvent::PitchBend {
                channel: 1,
                value: 16383,
            })
            .unwrap();
        assert!(
            recv_osc(&h.console_sock).await.is_none(),
            "disabled sidecar must not reach the console"
        );
    }

    #[tokio::test]
    async fn console_change_pushes_motor_but_own_echo_does_not() {
        let cfg = SidecarConfig {
            enabled: true,
            bindings: vec![pb_binding(1, 12)],
        };
        let h = harness(cfg).await;

        // External console change (operator on the desk): motor follows.
        h.state
            .write()
            .await
            .update(fader_addr(12), ParameterValue::Float(0.0));
        tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if !h.motor_log.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("motor pushed on console change");
        let (control, v14) = h.motor_log.lock().unwrap()[0];
        assert_eq!(control, ControlSelector::PitchBend { channel: 1 });
        // Unity ≈ 75% travel.
        assert!((f32::from(v14) / 16383.0 - 0.75).abs() < 0.01);

        // Now a hardware move: its own optimistic mirror write must NOT
        // bounce back to the motor.
        h.motor_log.lock().unwrap().clear();
        h.hw_tx
            .send(HwEvent::PitchBend {
                channel: 1,
                value: 8000,
            })
            .unwrap();
        let _ = recv_osc(&h.console_sock).await; // console got the move
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(
            h.motor_log.lock().unwrap().is_empty(),
            "own echo must not move the motor"
        );
    }

    #[tokio::test]
    async fn touch_gate_holds_motor_and_release_snaps_to_truth() {
        let cfg = SidecarConfig {
            enabled: true,
            bindings: vec![pb_binding(1, 12)],
        };
        let h = harness(cfg).await;
        let touch = crate::model::sidecar::mcu_default_touch_note(1).unwrap();
        let ControlSelector::Note { channel, note } = touch else {
            panic!()
        };

        // Touch down.
        h.hw_tx
            .send(HwEvent::Note {
                channel,
                note,
                on: true,
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;

        // Console change while touched: motor must NOT move.
        h.state
            .write()
            .await
            .update(fader_addr(12), ParameterValue::Float(-20.0));
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(
            h.motor_log.lock().unwrap().is_empty(),
            "motor must hold while touched"
        );

        // Release: motor snaps to console truth.
        h.hw_tx
            .send(HwEvent::Note {
                channel,
                note,
                on: false,
            })
            .unwrap();
        tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if !h.motor_log.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("motor snapped on release");
    }

    #[tokio::test]
    async fn sync_surface_pushes_all_feedback_motors() {
        let cfg = SidecarConfig {
            enabled: true,
            bindings: vec![pb_binding(1, 12), pb_binding(2, 13)],
        };
        let h = harness(cfg).await;
        h.state
            .write()
            .await
            .update(fader_addr(12), ParameterValue::Float(0.0));
        h.state
            .write()
            .await
            .update(fader_addr(13), ParameterValue::Float(-60.0));

        h.svc_tx.send(SvcCmd::SyncSurface).unwrap();
        tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if h.motor_log.lock().unwrap().len() >= 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("both motors synced");
    }

    #[tokio::test]
    async fn console_reconnect_triggers_sync() {
        let cfg = SidecarConfig {
            enabled: true,
            bindings: vec![pb_binding(1, 12)],
        };
        let h = harness(cfg).await;
        h.state
            .write()
            .await
            .update(fader_addr(12), ParameterValue::Float(-10.0));
        // Wait out the motor poll that follows the state change, then
        // clear — we only want the reconnect-driven sync.
        tokio::time::sleep(Duration::from_millis(80)).await;
        h.motor_log.lock().unwrap().clear();

        // Simulate a reconnect: senders drop to None, then back.
        h.senders_tx.send(None).unwrap();
        let console_addr = h.console_sock.local_addr().unwrap();
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let client = OscClient::new(local, console_addr, None).await.unwrap();
        let (sender, _rx) = client.into_parts();
        h.senders_tx.send(Some((sender, None))).unwrap();

        tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                if !h.motor_log.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("reconnect pushed console state to motors");
    }

    #[tokio::test]
    async fn learn_swallows_hardware_events() {
        let cfg = SidecarConfig {
            enabled: true,
            bindings: vec![pb_binding(1, 12)],
        };
        let h = harness(cfg).await;
        // Arm learn via the shared slot — grab it from the harness by
        // rebuilding: instead, drive a fresh harness with learn armed.
        // (Simplest: send enough travel to also cross the learn bar.)
        // Here we reach through the service's shared learn handle.
        // Rebuild harness with an armed learn:
        drop(h);

        let console_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let console_addr = console_sock.local_addr().unwrap();
        let local: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let client = OscClient::new(local, console_addr, None).await.unwrap();
        let (sender, _rx) = client.into_parts();
        let (hw_tx, hw_rx) = mpsc::unbounded_channel();
        let (_svc_tx, svc_rx) = mpsc::unbounded_channel();
        let (_senders_tx, senders_rx) = watch::channel(Some((sender, None)));
        let learn = Arc::new(StdMutex::new(LearnShared::default()));
        LearnShared::arm(&learn);
        let deps = SidecarDeps {
            config: Arc::new(RwLock::new(SidecarConfig {
                enabled: true,
                bindings: vec![pb_binding(1, 12)],
            })),
            state: Arc::new(RwLock::new(ConsoleState::new(
                crate::model::config::ConsoleConfig::default(),
            ))),
            hw_rx,
            svc_rx,
            senders: senders_rx,
            cue_manager: Arc::new(RwLock::new(CueManager::new(
                crate::model::snapshot::CueList::default(),
            ))),
            learn: learn.clone(),
            motor: Arc::new(|_, _, _| {}),
        };
        let _task = spawn(deps);

        for v in [2000u16, 5000, 9000] {
            hw_tx
                .send(HwEvent::PitchBend {
                    channel: 1,
                    value: v,
                })
                .unwrap();
        }
        // The console must stay silent (learn diverted the events)…
        let mut buf = [0u8; 64];
        let got =
            tokio::time::timeout(Duration::from_millis(300), console_sock.recv_from(&mut buf))
                .await;
        assert!(
            got.is_err(),
            "learn must divert events away from the console"
        );
        // …and the capture landed.
        let result = LearnShared::take_result(&learn);
        assert_eq!(
            result,
            Some((
                ControlSelector::PitchBend { channel: 1 },
                ControlMode::PitchBend14
            ))
        );
    }

    #[tokio::test]
    async fn raw_osc_binding_sends_scaled_float() {
        let listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let l_addr = listener.local_addr().unwrap();
        let cfg = SidecarConfig {
            enabled: true,
            bindings: vec![SidecarBinding {
                id: Uuid::from_bytes([7; 16]),
                label: "ext".into(),
                control: ControlSelector::Cc { channel: 1, cc: 20 },
                mode: ControlMode::Absolute7,
                target: BindingTarget::RawOsc {
                    target_id: None,
                    host: Some(l_addr.ip().to_string()),
                    port: Some(l_addr.port()),
                    path: "/x/dim".into(),
                    args: vec![crate::model::cue_trigger::OscArg::Int(4)],
                },
                taper: Taper::Linear { min: 0.0, max: 1.0 },
                motor_feedback: false,
                touch: None,
                relative_step: 1.0 / 300.0,
                enabled: true,
            }],
        };
        let h = harness(cfg).await;
        h.hw_tx
            .send(HwEvent::Cc {
                channel: 1,
                cc: 20,
                value: 127,
            })
            .unwrap();
        let (path, args) = recv_osc(&listener).await.expect("raw OSC datagram");
        assert_eq!(path, "/x/dim");
        assert_eq!(args[0], rosc::OscType::Int(4));
        assert_eq!(args[1], rosc::OscType::Float(1.0));
    }
}
