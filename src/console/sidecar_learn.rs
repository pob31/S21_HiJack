//! Learn flow for the fader sidecar.
//!
//! The operator presses Learn, wiggles a console parameter (captured
//! UI-side from `DaemonState::last_received`, exactly like the Macros
//! tab's "track latest OSC"), then moves a control on the sidecar
//! surface. This module owns the hardware half: a debounced
//! accumulator that watches the incoming MIDI stream and decides which
//! physical control the operator *meant* — brushing a fader in passing
//! must not bind it, and the control's mode (7-bit / 14-bit pair /
//! relative encoding / pitch bend) is auto-detected from how its
//! values behave. The guess is shown before Confirm and editable
//! after, so detection is best-effort, not load-bearing.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use crate::console::sidecar_decode::HwEvent;
use crate::model::sidecar::{BindingTarget, ControlMode, ControlSelector, RelativeMode};

/// Pitch bend: how many events and how much travel (of 16383) before a
/// fader counts as deliberately moved.
const PB_MIN_EVENTS: u32 = 3;
const PB_MIN_SPAN: u16 = 512;

/// Absolute CC: events and span (of 127) for a deliberate move.
const CC_MIN_EVENTS: u32 = 3;
const CC_MIN_SPAN: u8 = 8;

/// Relative CC: ticks required when the encoder only ever turned one
/// way. Ticks in *both* directions qualify with fewer (see below).
const REL_MIN_TICKS_ONE_WAY: u32 = 6;
const REL_MIN_TICKS_BOTH_WAYS: u32 = 3;

/// Where the learn flow stands. Owned by the Sidecar tab UI; the
/// hardware capture is delegated to [`LearnShared`].
#[derive(Clone, Debug, PartialEq, Default)]
pub enum LearnPhase {
    #[default]
    Idle,
    /// Waiting for the operator to wiggle a console parameter (or type
    /// a raw OSC target instead).
    ArmedConsole,
    /// Target captured — showing it, waiting for "next" to arm the
    /// hardware side.
    GotTarget { target: BindingTarget },
    /// Collecting MIDI to identify the hardware control.
    ArmedHardware { target: BindingTarget },
    /// Both halves known — waiting for Confirm / re-arm / Cancel.
    Ready {
        target: BindingTarget,
        control: ControlSelector,
        mode: ControlMode,
    },
}

/// Candidate key: one physical control as seen on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum CandKey {
    Cc { channel: u8, cc: u8 },
    PitchBend { channel: u8 },
}

#[derive(Debug)]
struct CandStats {
    events: u32,
    /// Observed value range. CC values live in 0..=127, pitch bend in
    /// 0..=16383 — spans are compared against per-kind thresholds.
    min: u16,
    max: u16,
    /// Tick counts under the two's-complement reading — used with the
    /// cluster shape to spot relative encoders.
    pos_ticks: u32,
    neg_ticks: u32,
    /// True while every observed value sits inside a relative-encoder
    /// cluster (small positives, around-64, high negatives). One value
    /// outside and the candidate is treated as absolute.
    all_in_rel_cluster: bool,
}

impl CandStats {
    fn new() -> Self {
        Self {
            events: 0,
            min: u16::MAX,
            max: 0,
            pos_ticks: 0,
            neg_ticks: 0,
            all_in_rel_cluster: true,
        }
    }

    fn span(&self) -> u16 {
        self.max.saturating_sub(self.min)
    }
}

/// Is this CC value plausible as a relative-encoder tick? (Small
/// positives, the around-64 band, or high wrap-around negatives.)
fn in_rel_cluster(v: u8) -> bool {
    matches!(v, 1..=8 | 56..=72 | 120..=127)
}

/// Debounced hardware-capture accumulator.
#[derive(Debug, Default)]
pub struct LearnAccumulator {
    candidates: HashMap<CandKey, CandStats>,
}

impl LearnAccumulator {
    /// Feed one hardware event. Returns the detected control + mode as
    /// soon as a candidate crosses the significance bar; the first to
    /// cross wins. Notes are ignored — touch-sense and buttons are
    /// configured per-binding, not learnable as value controls.
    pub fn feed(&mut self, ev: &HwEvent, _now: Instant) -> Option<(ControlSelector, ControlMode)> {
        let (key, value) = match ev {
            HwEvent::Cc { channel, cc, value } => (
                CandKey::Cc {
                    channel: *channel,
                    cc: *cc,
                },
                u16::from(*value),
            ),
            HwEvent::PitchBend { channel, value } => {
                (CandKey::PitchBend { channel: *channel }, *value)
            }
            HwEvent::Note { .. } => return None,
        };

        let st = self.candidates.entry(key).or_insert_with(CandStats::new);
        st.events += 1;
        st.min = st.min.min(value);
        st.max = st.max.max(value);
        if let CandKey::Cc { .. } = key {
            let v = value as u8;
            if !in_rel_cluster(v) {
                st.all_in_rel_cluster = false;
            }
            let ticks = RelativeMode::TwosComplement.ticks(v);
            if ticks > 0 {
                st.pos_ticks += 1;
            } else if ticks < 0 {
                st.neg_ticks += 1;
            }
        }

        self.evaluate(key)
    }

    /// How many distinct controls have produced events so far — lets the
    /// UI hint "several controls moved; kept the first deliberate one".
    pub fn candidates_seen(&self) -> usize {
        self.candidates.len()
    }

    fn evaluate(&self, key: CandKey) -> Option<(ControlSelector, ControlMode)> {
        let st = self.candidates.get(&key)?;
        match key {
            CandKey::PitchBend { channel } => {
                (st.events >= PB_MIN_EVENTS && st.span() >= PB_MIN_SPAN).then_some((
                    ControlSelector::PitchBend { channel },
                    ControlMode::PitchBend14,
                ))
            }
            CandKey::Cc { channel, cc } => {
                // Relative first: a wrap-around encoder (1, 2, 127, …)
                // spans nearly the whole CC range and would otherwise
                // masquerade as a big absolute move.
                if st.all_in_rel_cluster {
                    let total = st.pos_ticks + st.neg_ticks;
                    let both = st.pos_ticks > 0 && st.neg_ticks > 0;
                    if (both && total >= REL_MIN_TICKS_BOTH_WAYS) || total >= REL_MIN_TICKS_ONE_WAY
                    {
                        let mode = self.guess_relative_mode(st);
                        return Some((ControlSelector::Cc { channel, cc }, mode));
                    }
                    return None;
                }
                if st.events >= CC_MIN_EVENTS && st.span() >= u16::from(CC_MIN_SPAN) {
                    // 14-bit pair detection: activity on cc±32 of the
                    // same channel marks an MSB/LSB pair. Whichever half
                    // crossed the bar first, the binding always selects
                    // the MSB.
                    let partner_up = CandKey::Cc {
                        channel,
                        cc: cc.wrapping_add(32),
                    };
                    let partner_down = CandKey::Cc {
                        channel,
                        cc: cc.wrapping_sub(32),
                    };
                    if cc < 96 && self.candidates.contains_key(&partner_up) {
                        return Some((
                            ControlSelector::Cc { channel, cc },
                            ControlMode::Absolute14 { lsb_cc: cc + 32 },
                        ));
                    }
                    if cc >= 32 && self.candidates.contains_key(&partner_down) {
                        return Some((
                            ControlSelector::Cc {
                                channel,
                                cc: cc - 32,
                            },
                            ControlMode::Absolute14 { lsb_cc: cc },
                        ));
                    }
                    return Some((ControlSelector::Cc { channel, cc }, ControlMode::Absolute7));
                }
                None
            }
        }
    }

    /// Best-effort relative-encoding guess from the observed value
    /// shape. Shown before Confirm and editable after — ambiguity
    /// (e.g. binary-offset vs sign-magnitude positives) is resolved by
    /// the operator, not here.
    fn guess_relative_mode(&self, st: &CandStats) -> ControlMode {
        let has_low = st.min <= 8 && st.min >= 1;
        let has_high = st.max >= 120;
        let around_64 = st.min >= 56 && st.max <= 72;
        if around_64 {
            ControlMode::Relative(RelativeMode::BinaryOffset)
        } else if has_low && st.max >= 65 && st.max <= 72 {
            ControlMode::Relative(RelativeMode::SignMagnitude)
        } else if has_low || has_high {
            ControlMode::Relative(RelativeMode::TwosComplement)
        } else {
            ControlMode::Relative(RelativeMode::TwosComplement)
        }
    }
}

/// Shared learn state between the Sidecar tab (UI thread) and the
/// sidecar service (tokio). While `active`, the service diverts every
/// hardware event into the accumulator instead of the binding table
/// and parks the first detection in `result`; the UI polls it each
/// frame (the same poll-a-shared-slot idiom as `last_received`).
#[derive(Debug, Default)]
pub struct LearnShared {
    pub active: bool,
    pub acc: LearnAccumulator,
    pub result: Option<(ControlSelector, ControlMode)>,
}

impl LearnShared {
    /// Arm the hardware capture (fresh accumulator, no stale result).
    pub fn arm(shared: &Mutex<Self>) {
        let mut g = shared.lock().unwrap();
        g.active = true;
        g.acc = LearnAccumulator::default();
        g.result = None;
    }

    /// Disarm without keeping anything.
    pub fn disarm(shared: &Mutex<Self>) {
        let mut g = shared.lock().unwrap();
        g.active = false;
        g.acc = LearnAccumulator::default();
        g.result = None;
    }

    /// Service side: feed one event while active. Keeps the first
    /// detection (later movement doesn't steal the capture).
    pub fn feed(shared: &Mutex<Self>, ev: &HwEvent, now: Instant) {
        let mut g = shared.lock().unwrap();
        if !g.active || g.result.is_some() {
            return;
        }
        if let Some(found) = g.acc.feed(ev, now) {
            g.result = Some(found);
        }
    }

    /// UI side: take the detection once available.
    pub fn take_result(shared: &Mutex<Self>) -> Option<(ControlSelector, ControlMode)> {
        shared.lock().unwrap().result.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cc(channel: u8, cc_num: u8, value: u8) -> HwEvent {
        HwEvent::Cc {
            channel,
            cc: cc_num,
            value,
        }
    }

    fn pb(channel: u8, value: u16) -> HwEvent {
        HwEvent::PitchBend { channel, value }
    }

    fn feed_all(
        acc: &mut LearnAccumulator,
        evs: &[HwEvent],
    ) -> Option<(ControlSelector, ControlMode)> {
        let t = Instant::now();
        let mut out = None;
        for ev in evs {
            if out.is_none() {
                out = acc.feed(ev, t);
            }
        }
        out
    }

    #[test]
    fn brushing_a_fader_never_binds() {
        let mut acc = LearnAccumulator::default();
        // Two pitch-bend events with a small span: not deliberate.
        assert_eq!(feed_all(&mut acc, &[pb(1, 8000), pb(1, 8100)]), None);
        // Two CC events, tiny span.
        let mut acc = LearnAccumulator::default();
        assert_eq!(feed_all(&mut acc, &[cc(1, 7, 60), cc(1, 7, 62)]), None);
    }

    #[test]
    fn sustained_pitch_bend_move_binds() {
        let mut acc = LearnAccumulator::default();
        let got = feed_all(&mut acc, &[pb(3, 4000), pb(3, 5000), pb(3, 6000)]);
        assert_eq!(
            got,
            Some((
                ControlSelector::PitchBend { channel: 3 },
                ControlMode::PitchBend14
            ))
        );
    }

    #[test]
    fn absolute_cc_sweep_binds_7bit() {
        let mut acc = LearnAccumulator::default();
        let got = feed_all(&mut acc, &[cc(1, 7, 20), cc(1, 7, 30), cc(1, 7, 45)]);
        assert_eq!(
            got,
            Some((
                ControlSelector::Cc { channel: 1, cc: 7 },
                ControlMode::Absolute7
            ))
        );
    }

    #[test]
    fn interleaved_pair_binds_14bit_msb() {
        let mut acc = LearnAccumulator::default();
        // MSB on 16, LSB on 48, interleaved like a real 14-bit fader.
        let got = feed_all(
            &mut acc,
            &[
                cc(1, 16, 20),
                cc(1, 48, 90),
                cc(1, 16, 30),
                cc(1, 48, 15),
                cc(1, 16, 45),
            ],
        );
        assert_eq!(
            got,
            Some((
                ControlSelector::Cc { channel: 1, cc: 16 },
                ControlMode::Absolute14 { lsb_cc: 48 }
            ))
        );
    }

    #[test]
    fn lsb_crossing_first_still_selects_msb() {
        let mut acc = LearnAccumulator::default();
        // The LSB (cc 48) moves wildly and crosses the significance bar
        // before the MSB has 3 events — the binding must still name the
        // MSB (cc 16) as the selector.
        let got = feed_all(
            &mut acc,
            &[cc(1, 16, 20), cc(1, 48, 10), cc(1, 48, 90), cc(1, 48, 40)],
        );
        assert_eq!(
            got,
            Some((
                ControlSelector::Cc { channel: 1, cc: 16 },
                ControlMode::Absolute14 { lsb_cc: 48 }
            ))
        );
    }

    #[test]
    fn twos_complement_encoder_detected_not_absolute() {
        // 1, 2, 127 spans almost the whole CC range — a naive absolute
        // check would bind it as a fader. Both directions seen → relative.
        let mut acc = LearnAccumulator::default();
        let got = feed_all(&mut acc, &[cc(1, 60, 1), cc(1, 60, 2), cc(1, 60, 127)]);
        assert_eq!(
            got,
            Some((
                ControlSelector::Cc { channel: 1, cc: 60 },
                ControlMode::Relative(RelativeMode::TwosComplement)
            ))
        );
    }

    #[test]
    fn one_way_encoder_needs_more_ticks() {
        let mut acc = LearnAccumulator::default();
        // Five +1 ticks: not yet.
        let evs: Vec<_> = (0..5).map(|_| cc(1, 61, 1)).collect();
        assert_eq!(feed_all(&mut acc, &evs), None);
        // The sixth crosses the one-way bar.
        let got = acc.feed(&cc(1, 61, 1), Instant::now());
        assert_eq!(
            got,
            Some((
                ControlSelector::Cc { channel: 1, cc: 61 },
                ControlMode::Relative(RelativeMode::TwosComplement)
            ))
        );
    }

    #[test]
    fn binary_offset_encoder_guessed() {
        let mut acc = LearnAccumulator::default();
        let got = feed_all(&mut acc, &[cc(1, 62, 65), cc(1, 62, 63), cc(1, 62, 66)]);
        assert_eq!(
            got,
            Some((
                ControlSelector::Cc { channel: 1, cc: 62 },
                ControlMode::Relative(RelativeMode::BinaryOffset)
            ))
        );
    }

    #[test]
    fn biggest_mover_wins_first_cross() {
        let mut acc = LearnAccumulator::default();
        // A stray CC blip on cc 1, then a deliberate fader sweep on PB.
        let got = feed_all(
            &mut acc,
            &[
                cc(1, 1, 64),
                pb(2, 2000),
                pb(2, 4000),
                cc(1, 1, 65),
                pb(2, 6000),
            ],
        );
        assert_eq!(
            got,
            Some((
                ControlSelector::PitchBend { channel: 2 },
                ControlMode::PitchBend14
            ))
        );
        assert_eq!(acc.candidates_seen(), 2);
    }

    #[test]
    fn notes_are_ignored() {
        let mut acc = LearnAccumulator::default();
        let evs: Vec<_> = (0..10)
            .map(|_| HwEvent::Note {
                channel: 1,
                note: 0x68,
                on: true,
            })
            .collect();
        assert_eq!(feed_all(&mut acc, &evs), None);
    }

    #[test]
    fn learn_shared_captures_once() {
        let shared = Mutex::new(LearnShared::default());
        LearnShared::arm(&shared);
        let t = Instant::now();
        for v in [4000u16, 5000, 6000, 9000] {
            LearnShared::feed(&shared, &pb(1, v), t);
        }
        // Movement on another control after capture doesn't steal it.
        LearnShared::feed(&shared, &pb(2, 0), t);
        LearnShared::feed(&shared, &pb(2, 8000), t);
        LearnShared::feed(&shared, &pb(2, 16000), t);
        let got = LearnShared::take_result(&shared);
        assert_eq!(
            got,
            Some((
                ControlSelector::PitchBend { channel: 1 },
                ControlMode::PitchBend14
            ))
        );
        // Taken once; second take is empty.
        assert_eq!(LearnShared::take_result(&shared), None);
    }

    #[test]
    fn learn_shared_inactive_ignores_events() {
        let shared = Mutex::new(LearnShared::default());
        let t = Instant::now();
        LearnShared::feed(&shared, &pb(1, 0), t);
        LearnShared::feed(&shared, &pb(1, 8000), t);
        LearnShared::feed(&shared, &pb(1, 16000), t);
        assert_eq!(LearnShared::take_result(&shared), None);
    }

    #[test]
    fn learn_phase_default_is_idle() {
        assert_eq!(LearnPhase::default(), LearnPhase::Idle);
    }
}
