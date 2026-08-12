//! Pure MIDI → normalized-position decoding for the fader sidecar.
//!
//! The sidecar MIDI engine hands us typed [`HwEvent`]s (already parsed
//! from raw bytes but binding-agnostic); this module turns them into
//! normalized 0..=1 positions per binding. Everything here is pure and
//! `Instant`-parameterized so the state machines (14-bit MSB/LSB
//! pairing, relative accumulation, pitch-bend deadband) are
//! unit-testable without hardware or a console.

use std::time::{Duration, Instant};

use crate::model::sidecar::{ControlMode, ControlSelector, SidecarBinding};

/// A typed event out of the sidecar MIDI thread. Channels are 1-based
/// (1..=16), matching [`ControlSelector`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HwEvent {
    /// Control Change. `value` is 0..=127.
    Cc { channel: u8, cc: u8, value: u8 },
    /// Pitch bend, full range 0..=16383 (8192 = center).
    PitchBend { channel: u8, value: u16 },
    /// Note on/off. MCU surfaces report touch release as note-on with
    /// velocity 0 — the engine normalizes that to `on: false`.
    Note { channel: u8, note: u8, on: bool },
}

/// How long an MSB waits for its LSB before we treat the device as
/// 7-bit-only and emit the MSB alone.
const PAIR_WINDOW: Duration = Duration::from_millis(30);

/// Pitch-bend deadband in 14-bit steps. X-Touch fader ADCs jitter
/// ±1–2 LSB at rest; without this the surface streams phantom moves.
const PITCH_BEND_DEADBAND: f32 = 4.0 / 16383.0;

/// Per-binding runtime decode state.
#[derive(Debug, Default)]
pub struct DecodeState {
    /// MSB received, awaiting its LSB partner (Absolute14).
    pending_msb: Option<(u8, Instant)>,
    /// Last emitted normalized position. Relative modes accumulate on
    /// it; pitch bend deadbands against it. Seed it from console state
    /// (see [`DecodeState::seed`]) so a relative encoder's first tick
    /// nudges from reality rather than from zero.
    pub last_norm: Option<f32>,
}

impl DecodeState {
    /// Seed the accumulator with the target's current normalized value
    /// (used for relative modes; harmless for absolute ones).
    pub fn seed(&mut self, norm: f32) {
        if self.last_norm.is_none() {
            self.last_norm = Some(norm.clamp(0.0, 1.0));
        }
    }
}

/// Does this event address this selector? (Value-independent match —
/// used both by decode and by the touch-sense gate.)
pub fn event_matches(sel: &ControlSelector, ev: &HwEvent) -> bool {
    match (sel, ev) {
        (
            ControlSelector::Cc { channel, cc },
            HwEvent::Cc {
                channel: ec,
                cc: ecc,
                ..
            },
        ) => channel == ec && cc == ecc,
        (ControlSelector::PitchBend { channel }, HwEvent::PitchBend { channel: ec, .. }) => {
            channel == ec
        }
        (
            ControlSelector::Note { channel, note },
            HwEvent::Note {
                channel: ec,
                note: en,
                ..
            },
        ) => channel == ec && note == en,
        _ => false,
    }
}

/// Feed one hardware event to a binding. Returns the new normalized
/// position when the event completes a value; `None` while waiting for
/// an LSB, when the event doesn't address this binding's control, or
/// when a pitch-bend move is inside the jitter deadband.
pub fn decode(
    binding: &SidecarBinding,
    st: &mut DecodeState,
    ev: &HwEvent,
    now: Instant,
) -> Option<f32> {
    match binding.mode {
        ControlMode::Absolute7 => {
            if !event_matches(&binding.control, ev) {
                return None;
            }
            let HwEvent::Cc { value, .. } = ev else {
                return None;
            };
            let norm = f32::from(*value) / 127.0;
            st.last_norm = Some(norm);
            Some(norm)
        }
        ControlMode::Absolute14 { lsb_cc } => {
            // The selector's cc is the MSB; the LSB arrives on lsb_cc.
            let ControlSelector::Cc {
                channel,
                cc: msb_cc,
            } = binding.control
            else {
                return None;
            };
            let HwEvent::Cc {
                channel: ec,
                cc: ecc,
                value,
            } = ev
            else {
                return None;
            };
            if *ec != channel {
                return None;
            }
            if *ecc == msb_cc {
                // A second MSB (or one whose LSB never came) means the
                // device is effectively 7-bit: emit the *stale* MSB
                // alone before stashing the new one.
                let stale = st.pending_msb.take().map(|(msb, _)| f32::from(msb) / 127.0);
                st.pending_msb = Some((*value, now));
                if let Some(norm) = stale {
                    st.last_norm = Some(norm);
                    return Some(norm);
                }
                None
            } else if *ecc == lsb_cc {
                match st.pending_msb.take() {
                    Some((msb, at)) if now.duration_since(at) <= PAIR_WINDOW => {
                        let v14 = (u16::from(msb) << 7) | u16::from(*value & 0x7f);
                        let norm = f32::from(v14) / 16383.0;
                        st.last_norm = Some(norm);
                        Some(norm)
                    }
                    // LSB-first (or stale MSB): drop — we can't build a
                    // meaningful 14-bit value from an LSB alone.
                    _ => None,
                }
            } else {
                None
            }
        }
        ControlMode::Relative(mode) => {
            if !event_matches(&binding.control, ev) {
                return None;
            }
            let HwEvent::Cc { value, .. } = ev else {
                return None;
            };
            let ticks = mode.ticks(*value);
            if ticks == 0 {
                return None;
            }
            let base = st.last_norm.unwrap_or(0.0);
            let norm = (base + ticks as f32 * binding.relative_step).clamp(0.0, 1.0);
            st.last_norm = Some(norm);
            Some(norm)
        }
        ControlMode::PitchBend14 => {
            if !event_matches(&binding.control, ev) {
                return None;
            }
            let HwEvent::PitchBend { value, .. } = ev else {
                return None;
            };
            let norm = f32::from(*value) / 16383.0;
            // Deadband against ADC jitter — but never swallow the
            // endpoints, so "slam to bottom/top" always lands exactly.
            if let Some(prev) = st.last_norm
                && (norm - prev).abs() < PITCH_BEND_DEADBAND
                && norm != 0.0
                && norm != 1.0
            {
                return None;
            }
            st.last_norm = Some(norm);
            Some(norm)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;
    use crate::model::parameter::{ParameterAddress, ParameterPath};
    use crate::model::sidecar::{BindingTarget, RelativeMode, Taper};
    use uuid::Uuid;

    fn binding(control: ControlSelector, mode: ControlMode) -> SidecarBinding {
        SidecarBinding {
            id: Uuid::from_bytes([1; 16]),
            label: String::new(),
            control,
            mode,
            target: BindingTarget::ConsoleParameter {
                address: ParameterAddress {
                    channel: ChannelId::Input(1),
                    parameter: ParameterPath::Fader,
                },
            },
            taper: Taper::FaderDb { max_db: 10.0 },
            motor_feedback: true,
            touch: None,
            relative_step: 1.0 / 100.0,
            enabled: true,
        }
    }

    fn cc(channel: u8, cc_num: u8, value: u8) -> HwEvent {
        HwEvent::Cc {
            channel,
            cc: cc_num,
            value,
        }
    }

    #[test]
    fn absolute7_maps_full_range() {
        let b = binding(
            ControlSelector::Cc { channel: 1, cc: 7 },
            ControlMode::Absolute7,
        );
        let mut st = DecodeState::default();
        let t = Instant::now();
        assert_eq!(decode(&b, &mut st, &cc(1, 7, 0), t), Some(0.0));
        assert_eq!(decode(&b, &mut st, &cc(1, 7, 127), t), Some(1.0));
        // Wrong channel / cc: ignored.
        assert_eq!(decode(&b, &mut st, &cc(2, 7, 64), t), None);
        assert_eq!(decode(&b, &mut st, &cc(1, 8, 64), t), None);
    }

    #[test]
    fn absolute14_pairs_msb_then_lsb() {
        let b = binding(
            ControlSelector::Cc { channel: 1, cc: 16 },
            ControlMode::Absolute14 { lsb_cc: 48 },
        );
        let mut st = DecodeState::default();
        let t = Instant::now();
        assert_eq!(decode(&b, &mut st, &cc(1, 16, 0x40), t), None); // MSB stashed
        let norm = decode(&b, &mut st, &cc(1, 48, 0x01), t).unwrap();
        let expect = f32::from((0x40u16 << 7) | 0x01) / 16383.0;
        assert!((norm - expect).abs() < 1e-6);
    }

    #[test]
    fn absolute14_second_msb_flushes_stale_as_7bit() {
        let b = binding(
            ControlSelector::Cc { channel: 1, cc: 16 },
            ControlMode::Absolute14 { lsb_cc: 48 },
        );
        let mut st = DecodeState::default();
        let t = Instant::now();
        assert_eq!(decode(&b, &mut st, &cc(1, 16, 64), t), None);
        // MSB-only device: the next MSB emits the previous one at 7-bit.
        let norm = decode(&b, &mut st, &cc(1, 16, 70), t).unwrap();
        assert!((norm - 64.0 / 127.0).abs() < 1e-6);
    }

    #[test]
    fn absolute14_stale_msb_not_paired() {
        let b = binding(
            ControlSelector::Cc { channel: 1, cc: 16 },
            ControlMode::Absolute14 { lsb_cc: 48 },
        );
        let mut st = DecodeState::default();
        let t = Instant::now();
        assert_eq!(decode(&b, &mut st, &cc(1, 16, 64), t), None);
        // LSB arrives 50 ms later — outside the window, dropped.
        let late = t + Duration::from_millis(50);
        assert_eq!(decode(&b, &mut st, &cc(1, 48, 1), late), None);
    }

    #[test]
    fn absolute14_lsb_first_dropped() {
        let b = binding(
            ControlSelector::Cc { channel: 1, cc: 16 },
            ControlMode::Absolute14 { lsb_cc: 48 },
        );
        let mut st = DecodeState::default();
        assert_eq!(decode(&b, &mut st, &cc(1, 48, 5), Instant::now()), None);
    }

    #[test]
    fn relative_accumulates_and_clamps() {
        let b = binding(
            ControlSelector::Cc { channel: 1, cc: 60 },
            ControlMode::Relative(RelativeMode::TwosComplement),
        );
        let mut st = DecodeState::default();
        st.seed(0.5);
        let t = Instant::now();
        // +3 ticks at 1/100 per tick.
        let norm = decode(&b, &mut st, &cc(1, 60, 3), t).unwrap();
        assert!((norm - 0.53).abs() < 1e-6);
        // -1 tick (127 = -1 in two's complement).
        let norm = decode(&b, &mut st, &cc(1, 60, 127), t).unwrap();
        assert!((norm - 0.52).abs() < 1e-6);
        // Clamp at 1.0.
        st.last_norm = Some(0.999);
        let norm = decode(&b, &mut st, &cc(1, 60, 10), t).unwrap();
        assert_eq!(norm, 1.0);
        // Clamp at 0.0.
        st.last_norm = Some(0.001);
        let norm = decode(&b, &mut st, &cc(1, 60, 120), t).unwrap();
        assert_eq!(norm, 0.0);
    }

    #[test]
    fn relative_unseeded_starts_from_zero() {
        let b = binding(
            ControlSelector::Cc { channel: 1, cc: 60 },
            ControlMode::Relative(RelativeMode::BinaryOffset),
        );
        let mut st = DecodeState::default();
        // 66 = +2 ticks in binary offset.
        let norm = decode(&b, &mut st, &cc(1, 60, 66), Instant::now()).unwrap();
        assert!((norm - 0.02).abs() < 1e-6);
    }

    #[test]
    fn seed_only_applies_once() {
        let mut st = DecodeState::default();
        st.seed(0.7);
        st.seed(0.2); // ignored — position already known
        assert_eq!(st.last_norm, Some(0.7));
    }

    #[test]
    fn pitch_bend_deadband_swallows_jitter_but_not_moves() {
        let b = binding(
            ControlSelector::PitchBend { channel: 3 },
            ControlMode::PitchBend14,
        );
        let mut st = DecodeState::default();
        let t = Instant::now();
        let pb = |v: u16| HwEvent::PitchBend {
            channel: 3,
            value: v,
        };
        assert!(decode(&b, &mut st, &pb(8000), t).is_some());
        // ±2 LSB jitter at rest: swallowed.
        assert_eq!(decode(&b, &mut st, &pb(8002), t), None);
        assert_eq!(decode(&b, &mut st, &pb(7998), t), None);
        // A real move gets through.
        assert!(decode(&b, &mut st, &pb(8100), t).is_some());
        // Wrong channel ignored.
        assert_eq!(
            decode(
                &b,
                &mut st,
                &HwEvent::PitchBend {
                    channel: 4,
                    value: 0
                },
                t
            ),
            None
        );
    }

    #[test]
    fn pitch_bend_endpoints_always_land() {
        let b = binding(
            ControlSelector::PitchBend { channel: 1 },
            ControlMode::PitchBend14,
        );
        let mut st = DecodeState::default();
        let t = Instant::now();
        let pb = |v: u16| HwEvent::PitchBend {
            channel: 1,
            value: v,
        };
        // Sitting 1 LSB above bottom, then slamming to exactly 0 must
        // emit even though the delta is inside the deadband.
        assert!(decode(&b, &mut st, &pb(1), t).is_some());
        assert_eq!(decode(&b, &mut st, &pb(0), t), Some(0.0));
        // Same at the top.
        assert!(decode(&b, &mut st, &pb(16382), t).is_some());
        assert_eq!(decode(&b, &mut st, &pb(16383), t), Some(1.0));
    }

    #[test]
    fn event_matches_selectors() {
        let pb = ControlSelector::PitchBend { channel: 2 };
        assert!(event_matches(
            &pb,
            &HwEvent::PitchBend {
                channel: 2,
                value: 0
            }
        ));
        assert!(!event_matches(
            &pb,
            &HwEvent::PitchBend {
                channel: 3,
                value: 0
            }
        ));
        let note = ControlSelector::Note {
            channel: 1,
            note: 0x68,
        };
        assert!(event_matches(
            &note,
            &HwEvent::Note {
                channel: 1,
                note: 0x68,
                on: true
            }
        ));
        assert!(!event_matches(&note, &cc(1, 0x68, 1)));
    }
}
