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
    /// Which relative-encoder bands have been seen: 1..=8, 56..=63,
    /// 65..=72, 120..=127. They decide the encoding (see
    /// [`LearnAccumulator::relative_mode`]).
    seen_low: bool,
    seen_below_64: bool,
    seen_above_64: bool,
    seen_high: bool,
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
            seen_low: false,
            seen_below_64: false,
            seen_above_64: false,
            seen_high: false,
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
            match v {
                1..=8 => st.seen_low = true,
                56..=63 => st.seen_below_64 = true,
                65..=72 => st.seen_above_64 = true,
                120..=127 => st.seen_high = true,
                _ => {}
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
                    if ((both && total >= REL_MIN_TICKS_BOTH_WAYS)
                        || total >= REL_MIN_TICKS_ONE_WAY)
                        && let Some(mode) = Self::relative_mode(st)
                    {
                        return Some((
                            ControlSelector::Cc { channel, cc },
                            ControlMode::Relative(mode),
                        ));
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

    /// The relative encoding the observed values prove, or `None` while they
    /// fit more than one. 120..=127 only occurs in two's complement, 56..=63
    /// only in binary offset, and 1..=8 with 65..=72 is sign-magnitude.
    /// Clicks one way are ambiguous (1..=8 is two's complement or
    /// sign-magnitude; 65..=72 binary offset or sign-magnitude), so learn
    /// waits for a click the other way. Guessing from one direction learned
    /// the MCU/D700's sign-magnitude encoders wrongly, reversed or reading
    /// the first click back as −63 (audit M21).
    fn relative_mode(st: &CandStats) -> Option<RelativeMode> {
        if st.seen_high {
            Some(RelativeMode::TwosComplement)
        } else if st.seen_below_64 {
            Some(RelativeMode::BinaryOffset)
        } else if st.seen_low && st.seen_above_64 {
            Some(RelativeMode::SignMagnitude)
        } else {
            None
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
    /// When the capture was armed, for [`LEARN_TIMEOUT`].
    armed_at: Option<Instant>,
    /// Set when the capture gave up; the UI takes it with
    /// [`LearnShared::take_timed_out`].
    timed_out: bool,
}

/// How long the hardware capture stays armed without detecting anything.
/// While armed it diverts every hardware event, so every bound control is
/// dead; an operator who leaves the Sidecar tab mid-learn used to leave them
/// dead until they came back and cancelled (audit M20).
pub const LEARN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

impl LearnShared {
    /// Arm the hardware capture (fresh accumulator, no stale result).
    pub fn arm(shared: &Mutex<Self>) {
        let mut g = shared.lock().unwrap();
        g.active = true;
        g.acc = LearnAccumulator::default();
        g.result = None;
        g.armed_at = Some(Instant::now());
        g.timed_out = false;
    }

    /// Disarm without keeping anything.
    pub fn disarm(shared: &Mutex<Self>) {
        let mut g = shared.lock().unwrap();
        g.active = false;
        g.acc = LearnAccumulator::default();
        g.result = None;
        g.armed_at = None;
        g.timed_out = false;
    }

    /// Service side: whether to divert `ev` into the capture. Past
    /// [`LEARN_TIMEOUT`] without a detection the capture disarms itself and
    /// lets events through again (audit M20).
    pub fn wants(shared: &Mutex<Self>, now: Instant) -> bool {
        let Ok(mut g) = shared.lock() else {
            return false;
        };
        if g.active
            && g.armed_at
                .is_some_and(|t| now.duration_since(t) >= LEARN_TIMEOUT)
        {
            g.active = false;
            g.acc = LearnAccumulator::default();
            g.timed_out = true;
        }
        g.active
    }

    /// Service side: feed one event while active. The first detection
    /// disarms the capture, so bound controls work again while the
    /// operator reviews it (audit M20); later movement can't steal it.
    pub fn feed(shared: &Mutex<Self>, ev: &HwEvent, now: Instant) {
        let mut g = shared.lock().unwrap();
        if !g.active || g.result.is_some() {
            return;
        }
        if let Some(found) = g.acc.feed(ev, now) {
            g.result = Some(found);
            g.active = false;
        }
    }

    /// UI side: whether the capture timed out since the last call.
    pub fn take_timed_out(shared: &Mutex<Self>) -> bool {
        std::mem::take(&mut shared.lock().unwrap().timed_out)
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

    /// Audit M21: clicks one way don't say which encoding it is, so learn
    /// waits for a click back rather than guessing.
    #[test]
    fn one_way_clicks_wait_for_a_click_back() {
        let mut acc = LearnAccumulator::default();
        // +1 ticks well past the one-way bar: two's complement or
        // sign-magnitude, can't tell yet.
        let evs: Vec<_> = (0..8).map(|_| cc(1, 61, 1)).collect();
        assert_eq!(feed_all(&mut acc, &evs), None);
        // 127 is one click back only in two's complement.
        let got = acc.feed(&cc(1, 61, 127), Instant::now());
        assert_eq!(
            got,
            Some((
                ControlSelector::Cc { channel: 1, cc: 61 },
                ControlMode::Relative(RelativeMode::TwosComplement)
            ))
        );
    }

    /// Audit M21: the MCU/D700 encoders (1, 2, then 65 = one click back)
    /// learn as sign-magnitude. They used to learn as two's complement, so
    /// the first click back read as −63.
    #[test]
    fn mackie_encoder_learns_as_sign_magnitude() {
        let mut acc = LearnAccumulator::default();
        let got = feed_all(&mut acc, &[cc(1, 16, 1), cc(1, 16, 2), cc(1, 16, 65)]);
        assert_eq!(
            got,
            Some((
                ControlSelector::Cc { channel: 1, cc: 16 },
                ControlMode::Relative(RelativeMode::SignMagnitude)
            ))
        );

        // Turned back first: 65, 66 alone could be binary offset going up.
        let mut acc = LearnAccumulator::default();
        let evs: Vec<_> = (0..8).map(|_| cc(1, 17, 65)).collect();
        assert_eq!(feed_all(&mut acc, &evs), None);
        let got = acc.feed(&cc(1, 17, 1), Instant::now());
        assert_eq!(
            got,
            Some((
                ControlSelector::Cc { channel: 1, cc: 17 },
                ControlMode::Relative(RelativeMode::SignMagnitude)
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

    /// Audit M20: the capture lets go once it has found the control, and
    /// by itself after LEARN_TIMEOUT, so bound controls don't stay dead.
    #[test]
    fn learn_shared_lets_go_on_detection_and_on_timeout() {
        let shared = Mutex::new(LearnShared::default());
        LearnShared::arm(&shared);
        let t = Instant::now();
        assert!(LearnShared::wants(&shared, t));
        for v in [4000u16, 5000, 6000, 9000] {
            LearnShared::feed(&shared, &pb(1, v), t);
        }
        assert!(
            !LearnShared::wants(&shared, t),
            "detected: events flow again"
        );
        assert!(LearnShared::take_result(&shared).is_some());

        LearnShared::arm(&shared);
        let t = Instant::now();
        assert!(LearnShared::wants(&shared, t + LEARN_TIMEOUT / 2));
        assert!(!LearnShared::take_timed_out(&shared));
        assert!(!LearnShared::wants(&shared, t + LEARN_TIMEOUT));
        assert!(LearnShared::take_timed_out(&shared));
        assert!(!LearnShared::take_timed_out(&shared), "taken once");
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
