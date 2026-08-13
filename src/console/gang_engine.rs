use std::collections::HashMap;
use std::mem;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;
use tracing::debug;

use crate::console::console_tx::ConsoleTx;
use crate::model::parameter::{
    FADER_GANG_FLOOR_DB, FADER_INF_DB, ParameterAddress, ParameterPath, ParameterSection,
    ParameterValue,
};
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::ipad_client::IpadSender;

use super::gang_manager::GangManager;
use crate::model::gang::{GangMode, GangPanMode};

/// Duration (ms) to suppress echo-back from the console.
const SUPPRESSION_WINDOW_MS: u64 = 300;

/// Float tolerance for suppression matching.
const FLOAT_TOLERANCE: f32 = 0.01;

/// Sections that should only propagate between members of the same channel type.
const ROUTING_SECTIONS: &[ParameterSection] = &[
    ParameterSection::Sends,
    ParameterSection::GroupRouting,
    ParameterSection::MatrixSends,
    ParameterSection::CgMembership,
];

/// Processes gang propagation: when a parameter changes on one gang member,
/// compute the appropriate value for other members and send to the console.
pub struct GangEngine {
    state: Arc<RwLock<ConsoleState>>,
    tx: ConsoleTx,
    /// Recently-sent ganged changes, keyed by address.
    /// Used to suppress feedback loops from console echo-back.
    suppression_set: HashMap<ParameterAddress, (ParameterValue, Instant)>,
    /// Unclamped "virtual" gang position per member, for continuous Float
    /// parameters that clamp at a bound (fader −inf snap, pan-family ±1). A
    /// relative gang move accumulates the delta here *without* clamping, so the
    /// offset survives a round trip to a bound; only the value SENT to the
    /// console is clamped. Seeded from the mirror and re-baselined whenever the
    /// mirror no longer matches our last clamped send (operator grabbed the
    /// member, or it moved while the gang was paused). Runtime-only.
    gang_virtual: HashMap<ParameterAddress, f32>,
}

impl GangEngine {
    pub fn new(state: Arc<RwLock<ConsoleState>>, sender: OscSender) -> Self {
        Self::from_tx(state, ConsoleTx::new(sender))
    }

    /// Build on an existing console write path — the route for consoles with
    /// no GP OSC link at all (SD/Quantum), where there is no `OscSender` to
    /// hand over. See [`ConsoleTx::pad_only`].
    pub fn from_tx(state: Arc<RwLock<ConsoleState>>, tx: ConsoleTx) -> Self {
        Self {
            state,
            tx,
            suppression_set: HashMap::new(),
            gang_virtual: HashMap::new(),
        }
    }

    pub fn set_ipad_sender(&mut self, sender: Option<IpadSender>) {
        self.tx.set_pad_sender(sender);
    }

    /// Attach the shared sent-value log (echo screening on the iPad link).
    pub fn set_sent_log(&mut self, log: crate::console::console_tx::SentLog) {
        self.tx.set_sent_log(log);
    }

    /// Point the write path at the connected console's profile (Pad wire quirks).
    pub fn set_profile(&mut self, profile: Arc<crate::model::family::ConsoleProfile>) {
        self.tx.set_profile(profile);
    }

    /// Check if this update should be suppressed (it's an echo-back from our own send).
    /// Also cleans expired entries.
    pub fn is_suppressed(&mut self, addr: &ParameterAddress, value: &ParameterValue) -> bool {
        let now = Instant::now();

        // Clean expired entries
        self.suppression_set.retain(|_, (_, ts)| {
            now.duration_since(*ts).as_millis() < SUPPRESSION_WINDOW_MS as u128
        });

        // Check for a match
        if let Some((suppressed_value, _)) = self.suppression_set.remove(addr) {
            if values_match(&suppressed_value, value) {
                return true;
            }
        }
        false
    }

    /// Main entry point: process a parameter change for gang propagation.
    ///
    /// Called from process_message_inner() after the state update.
    pub async fn process_gang_update(
        &mut self,
        addr: &ParameterAddress,
        new_value: &ParameterValue,
        old_value: Option<&ParameterValue>,
        manager: &GangManager,
    ) {
        // 1. Check if this is an echo-back from our own send
        if self.is_suppressed(addr, new_value) {
            debug!(%addr, "Gang: suppressed echo-back");
            return;
        }

        // Main channel pan is decoupled from the Fader/Mute/Pan section and
        // driven by each gang's own pan mode (Off / On / Reversed). Handle it
        // separately and return so it never double-propagates through the
        // section loop below (Pan's section() is still FaderMutePan).
        if addr.parameter == ParameterPath::Pan {
            self.process_pan_gang_update(addr, new_value, old_value, manager)
                .await;
            return;
        }

        // 2. Get the parameter's section
        let section = addr.parameter.section();

        // 3. Find matching gangs
        let gangs = manager.find_gangs_for_channel_and_section(&addr.channel, &section);
        if gangs.is_empty() {
            return;
        }

        let is_routing = ROUTING_SECTIONS.contains(&section);
        let is_continuous = addr.parameter.is_continuous();
        let is_fader = addr.parameter == ParameterPath::Fader;

        // 4. For each matching gang, propagate to other members
        for gang in gangs {
            // Skip paused gangs
            if gang.paused {
                debug!(gang = %gang.name, "Gang: skipped (paused)");
                continue;
            }

            // Fader dead-zone (Relative mode only): when the source moves
            // entirely below the gang floor it's inaudible at both ends, yet the
            // compressed bottom of the track turns a tiny physical nudge into a
            // large dB delta. Propagating that delta would rocket an audible
            // sibling up the track, so siblings hold. Done as a skip (NOT a zero
            // delta) — a `None` delta would fall through to the absolute-copy
            // fallback below and re-introduce the bug. Absolute mode is left
            // alone: it intentionally copies the raw value, dead zone or not.
            if is_fader && gang.mode == GangMode::Relative {
                if let (Some(ParameterValue::Float(o)), ParameterValue::Float(n)) =
                    (old_value, new_value)
                {
                    if *o <= FADER_GANG_FLOOR_DB && *n <= FADER_GANG_FLOOR_DB {
                        debug!(gang = %gang.name, "Gang fader: source swept inaudible dead zone — holding siblings");
                        continue;
                    }
                }
            }

            // Compute delta for relative mode (continuous params only). The
            // fader uses floored-space deltas so the inaudible bottom of the
            // track collapses to a single point (see `fader_gang_delta`).
            let delta = if is_continuous && gang.mode == GangMode::Relative {
                if is_fader {
                    old_value.and_then(|old| fader_gang_delta(old, new_value))
                } else {
                    old_value.and_then(|old| compute_delta(old, new_value))
                }
            } else {
                None
            };

            for target_channel in gang.other_members(&addr.channel) {
                // Routing section guard: only propagate between same channel type
                if is_routing
                    && mem::discriminant(&addr.channel) != mem::discriminant(target_channel)
                {
                    continue;
                }

                let target_addr = ParameterAddress {
                    channel: target_channel.clone(),
                    parameter: addr.parameter.clone(),
                };

                // Compute target value
                let target_value = if let Some(d) = delta {
                    // Relative mode: accumulate the delta on the sibling's
                    // unclamped virtual position so the offset survives a round
                    // trip to a bound (fader −inf, pan-family ±1); only the sent
                    // value is clamped. The fader's floored source delta and
                    // dead-zone guard are already applied to `d` above.
                    let current = self.state.read().await.get(&target_addr).cloned();
                    match current {
                        Some(ParameterValue::Float(mirror)) => {
                            ParameterValue::Float(self.next_gang_value(&target_addr, mirror, d))
                        }
                        // Non-float continuous (e.g. Int): raw delta, no bound.
                        Some(ref cv) => apply_delta(cv, d).unwrap_or_else(|| new_value.clone()),
                        None => new_value.clone(), // no current value, use absolute
                    }
                } else {
                    // Absolute mode or discrete: propagate exact value
                    new_value.clone()
                };

                self.dispatch_target(target_addr, target_value).await;
            }
        }
    }

    /// Propagate the main channel pan across gang members, governed by each
    /// gang's [`GangPanMode`] rather than the Fader/Mute/Pan section.
    async fn process_pan_gang_update(
        &mut self,
        addr: &ParameterAddress,
        new_value: &ParameterValue,
        old_value: Option<&ParameterValue>,
        manager: &GangManager,
    ) {
        for gang in manager.find_gangs_for_channel_pan(&addr.channel) {
            if gang.paused {
                debug!(gang = %gang.name, "Gang pan: skipped (paused)");
                continue;
            }

            match gang.effective_pan_mode() {
                GangPanMode::Off => continue, // filtered out by the lookup, but be explicit
                GangPanMode::On => {
                    // Same semantics as any continuous parameter: relative
                    // delta when the gang is in Relative mode, else absolute.
                    let delta = if gang.mode == GangMode::Relative {
                        old_value.and_then(|old| compute_delta(old, new_value))
                    } else {
                        None
                    };
                    for target_channel in gang.other_members(&addr.channel) {
                        let target_addr = ParameterAddress {
                            channel: target_channel.clone(),
                            parameter: ParameterPath::Pan,
                        };
                        let target_value = if let Some(d) = delta {
                            let current = self.state.read().await.get(&target_addr).cloned();
                            match current {
                                // Virtual position so a pan offset survives a
                                // sweep to the ±1 rail and back (see next_gang_value).
                                Some(ParameterValue::Float(mirror)) => ParameterValue::Float(
                                    self.next_gang_value(&target_addr, mirror, d),
                                ),
                                Some(ref cv) => {
                                    apply_delta(cv, d).unwrap_or_else(|| new_value.clone())
                                }
                                None => new_value.clone(),
                            }
                        } else {
                            new_value.clone()
                        };
                        self.dispatch_target(target_addr, target_value).await;
                    }
                }
                GangPanMode::Reversed => {
                    // Pairs only: mirror the source pan around centre onto the
                    // single partner. Guard defensively — the UI blocks
                    // Reversed on non-pairs, but a hand-edited show file
                    // could still reach here.
                    if gang.members.len() != 2 {
                        debug!(
                            gang = %gang.name,
                            members = gang.members.len(),
                            "Gang pan: Reversed needs exactly 2 members — skipping"
                        );
                        continue;
                    }
                    let mirrored = match new_value {
                        ParameterValue::Float(f) => ParameterValue::Float(-f),
                        other => other.clone(),
                    };
                    for target_channel in gang.other_members(&addr.channel) {
                        let target_addr = ParameterAddress {
                            channel: target_channel.clone(),
                            parameter: ParameterPath::Pan,
                        };
                        self.dispatch_target(target_addr, mirrored.clone()).await;
                    }
                }
            }
        }
    }

    /// Clamp a computed target value to the parameter's valid range, send it
    /// to the console, and on success record it for echo-back suppression and
    /// update the local state mirror.
    ///
    /// Clamping is critical for pan / send pan / balance / width: relative
    /// delta application can otherwise drift past ±1.0 (sibling pan = 0.8 +
    /// delta 0.3 → 1.1) and the console's behaviour outside that range is
    /// undefined, producing the "jumping" the operator sees on aux pans.
    /// Pass-through for parameters whose range we don't yet model (Fader, EQ
    /// band gain, etc.).
    /// Advance one Relative continuous Float sibling by `delta`, preserving its
    /// offset across the parameter's bound. `mirror` is the sibling's current
    /// (clamped) value from the state mirror; the returned value is what to send
    /// (already clamped). The unclamped position is accumulated in
    /// `gang_virtual` so a round trip to a bound restores the offset.
    ///
    /// Seeding rule: trust the stored virtual only while `clamp(virtual)` still
    /// matches the mirror (the mirror reflects our last clamped send). If the
    /// mirror has moved out-of-band — the operator grabbed this member, or it
    /// was hand-moved while the gang was paused — re-baseline from the mirror.
    fn next_gang_value(&mut self, target: &ParameterAddress, mirror: f32, delta: f32) -> f32 {
        let vcur = match self.gang_virtual.get(target).copied() {
            // Keep the driven-down offset: the stored virtual is un-floored, so a
            // sibling pushed below a bound by the gang restores its true position.
            Some(v) if (clamp_gang_send(&target.parameter, v) - mirror).abs() < FLOAT_TOLERANCE => {
                v
            }
            // Re-baseline from the mirror. For the fader, a *resting* position
            // below the gang floor collapses to the floor (the inaudible sub-floor
            // region is one point), matching the original behaviour.
            _ => gang_seed(&target.parameter, mirror),
        };
        let vnew = vcur + delta;
        self.gang_virtual.insert(target.clone(), vnew);
        clamp_gang_send(&target.parameter, vnew)
    }

    async fn dispatch_target(&mut self, target_addr: ParameterAddress, value: ParameterValue) {
        let target_value = target_addr.parameter.clamp_value(value);
        if self.send_to_console(&target_addr, &target_value).await {
            self.suppression_set
                .insert(target_addr.clone(), (target_value.clone(), Instant::now()));
            self.state.write().await.update(target_addr, target_value);
        }
    }

    /// Send a parameter change to the console via GP OSC (with iPad fallback).
    async fn send_to_console(&self, addr: &ParameterAddress, value: &ParameterValue) -> bool {
        self.tx.send_parameter_logged("Gang", addr, value).await
    }
}

/// Compute the delta between old and new values.
fn compute_delta(old: &ParameterValue, new: &ParameterValue) -> Option<f32> {
    match (old, new) {
        (ParameterValue::Float(a), ParameterValue::Float(b)) => Some(b - a),
        (ParameterValue::Int(a), ParameterValue::Int(b)) => Some((b - a) as f32),
        _ => None,
    }
}

/// Apply a delta to a current value.
fn apply_delta(current: &ParameterValue, delta: f32) -> Option<ParameterValue> {
    match current {
        ParameterValue::Float(f) => Some(ParameterValue::Float(f + delta)),
        ParameterValue::Int(i) => Some(ParameterValue::Int(i + delta.round() as i32)),
        _ => None,
    }
}

/// Gang-effective fader level: collapse the inaudible sub-floor region to a
/// single point so relative gang offsets are preserved in audible space, not in
/// the compressed bottom of the dB track.
fn gang_floor(db: f32) -> f32 {
    db.max(FADER_GANG_FLOOR_DB)
}

/// Delta between two fader values in floored space. A move entirely within the
/// sub-floor dead zone yields `0.0` (both ends floor to the same point); a move
/// above the floor matches the raw delta. `None` for non-float values — faders
/// are always floats, so this is purely defensive.
///
/// Scoped to the main `Fader`; send/CG levels share the taper but are
/// deliberately left on raw-delta semantics to keep this change small.
fn fader_gang_delta(old: &ParameterValue, new: &ParameterValue) -> Option<f32> {
    match (old, new) {
        (ParameterValue::Float(a), ParameterValue::Float(b)) => {
            Some(gang_floor(*b) - gang_floor(*a))
        }
        _ => None,
    }
}

/// Clamp a gang sibling's (unclamped) virtual position to what actually goes on
/// the wire, reproducing each parameter's existing bound exactly:
/// - **Fader**: snap to −inf below the gang floor (everything below reads fully
///   off); the top is left to the console clamp, as before.
/// - **Pan / SendPan / Balance / Width**: ±1 via [`ParameterPath::clamp_value`].
/// - **everything else**: pass-through (the console owns the range).
///
/// The virtual position itself is kept unclamped by the caller, so an offset
/// driven past a bound and back is restored — only this sent value is clamped.
fn clamp_gang_send(param: &ParameterPath, v: f32) -> f32 {
    if *param == ParameterPath::Fader && v <= FADER_GANG_FLOOR_DB {
        return FADER_INF_DB;
    }
    match param.clamp_value(ParameterValue::Float(v)) {
        ParameterValue::Float(c) => c,
        _ => v,
    }
}

/// Re-baseline seed for a sibling's virtual position from its current mirror
/// value. The fader collapses a *resting* position below the gang floor to the
/// floor — the inaudible sub-floor region is a single point, so two parked
/// faders rise together from the floor rather than preserving a meaningless
/// sub-floor offset. Other parameters seed from the raw mirror.
fn gang_seed(param: &ParameterPath, mirror: f32) -> f32 {
    if *param == ParameterPath::Fader {
        gang_floor(mirror)
    } else {
        mirror
    }
}

/// Check if two parameter values match (with float tolerance for suppression).
fn values_match(a: &ParameterValue, b: &ParameterValue) -> bool {
    match (a, b) {
        (ParameterValue::Float(fa), ParameterValue::Float(fb)) => (fa - fb).abs() < FLOAT_TOLERANCE,
        (ParameterValue::Int(ia), ParameterValue::Int(ib)) => ia == ib,
        (ParameterValue::Bool(ba), ParameterValue::Bool(bb)) => ba == bb,
        (ParameterValue::String(sa), ParameterValue::String(sb)) => sa == sb,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::net::SocketAddr;

    use crate::model::channel::ChannelId;
    use crate::model::config::ConsoleConfig;
    use crate::model::gang::GangGroup;
    use crate::model::parameter::{ParameterPath, ParameterSection};

    // ---- Pure function tests ----

    #[test]
    fn compute_delta_float() {
        assert_eq!(
            compute_delta(&ParameterValue::Float(0.5), &ParameterValue::Float(1.0),),
            Some(0.5)
        );
    }

    #[test]
    fn compute_delta_int() {
        assert_eq!(
            compute_delta(&ParameterValue::Int(3), &ParameterValue::Int(7),),
            Some(4.0)
        );
    }

    #[test]
    fn compute_delta_mismatched_types() {
        assert_eq!(
            compute_delta(&ParameterValue::Float(1.0), &ParameterValue::Int(2),),
            None,
        );
    }

    #[test]
    fn apply_delta_float() {
        assert_eq!(
            apply_delta(&ParameterValue::Float(0.5), 0.3),
            Some(ParameterValue::Float(0.8)),
        );
    }

    #[test]
    fn apply_delta_int() {
        assert_eq!(
            apply_delta(&ParameterValue::Int(5), 2.7),
            Some(ParameterValue::Int(8)),
        );
    }

    #[test]
    fn apply_delta_bool_returns_none() {
        assert_eq!(apply_delta(&ParameterValue::Bool(true), 1.0), None);
    }

    // ---- Fader floor (gang dead-zone) pure-function tests ----

    #[test]
    fn gang_floor_clamps_below_threshold() {
        assert_eq!(gang_floor(-150.0), FADER_GANG_FLOOR_DB);
        assert_eq!(gang_floor(-60.0), FADER_GANG_FLOOR_DB);
        assert_eq!(gang_floor(-59.9), -59.9);
        assert_eq!(gang_floor(0.0), 0.0);
    }

    #[test]
    fn fader_gang_delta_zero_when_both_subfloor() {
        // A move that stays entirely within the inaudible dead zone produces
        // no gang delta — both ends floor to the same point.
        assert_eq!(
            fader_gang_delta(
                &ParameterValue::Float(-150.0),
                &ParameterValue::Float(-90.0)
            ),
            Some(0.0)
        );
    }

    #[test]
    fn fader_gang_delta_crossing_and_above_floor() {
        // Rising out of the dead zone, and a fully-above-floor move, both
        // measure from the floor / raw value respectively.
        assert_eq!(
            fader_gang_delta(
                &ParameterValue::Float(-150.0),
                &ParameterValue::Float(-50.0)
            ),
            Some(10.0)
        );
        assert_eq!(
            fader_gang_delta(&ParameterValue::Float(-80.0), &ParameterValue::Float(-50.0)),
            Some(10.0)
        );
        // Above the floor it matches the raw delta.
        assert_eq!(
            fader_gang_delta(&ParameterValue::Float(-20.0), &ParameterValue::Float(-10.0)),
            Some(10.0)
        );
    }

    #[test]
    fn fader_gang_delta_downward_clamped_at_floor() {
        // −30 → −150 clamps to a −30 dB drop (to the floor), not −120.
        assert_eq!(
            fader_gang_delta(
                &ParameterValue::Float(-30.0),
                &ParameterValue::Float(-150.0)
            ),
            Some(-30.0)
        );
    }

    #[test]
    fn fader_gang_delta_non_float_none() {
        assert_eq!(
            fader_gang_delta(&ParameterValue::Int(1), &ParameterValue::Int(2)),
            None
        );
    }

    #[test]
    fn clamp_gang_send_fader_snaps_below_floor_passes_above() {
        // At/below the gang floor the SENT value snaps to −inf...
        assert_eq!(clamp_gang_send(&ParameterPath::Fader, -60.0), FADER_INF_DB);
        assert_eq!(clamp_gang_send(&ParameterPath::Fader, -66.0), FADER_INF_DB);
        // ...above the floor it passes through unchanged — the caller's virtual
        // position is what restores the offset on the way back up.
        assert_eq!(clamp_gang_send(&ParameterPath::Fader, -6.0), -6.0);
        assert_eq!(clamp_gang_send(&ParameterPath::Fader, 0.0), 0.0);
    }

    #[test]
    fn clamp_gang_send_pan_family_clamps_to_unit() {
        for p in [
            ParameterPath::Pan,
            ParameterPath::Balance,
            ParameterPath::Width,
        ] {
            assert_eq!(clamp_gang_send(&p, 1.3), 1.0);
            assert_eq!(clamp_gang_send(&p, -1.3), -1.0);
            assert_eq!(clamp_gang_send(&p, 0.4), 0.4);
        }
    }

    #[test]
    fn gang_seed_floors_resting_subfloor_fader_only() {
        // A fader resting below the floor seeds from the floor (sub-floor is one
        // point); above the floor it seeds raw.
        assert_eq!(
            gang_seed(&ParameterPath::Fader, -100.0),
            FADER_GANG_FLOOR_DB
        );
        assert_eq!(gang_seed(&ParameterPath::Fader, -6.0), -6.0);
        // Pan seeds raw — no floor concept.
        assert_eq!(gang_seed(&ParameterPath::Pan, 0.8), 0.8);
    }

    #[tokio::test]
    async fn next_gang_value_fader_offset_survives_inf_round_trip() {
        // Direct unit test of the virtual accumulation: sibling at −6, floored
        // master delta −60 then +60. Sent snaps to −inf, but the offset returns.
        let mut engine = make_engine();
        let t = fader(ChannelId::Input(4));
        assert_eq!(engine.next_gang_value(&t, -6.0, -60.0), FADER_INF_DB);
        // Mirror is now −150 (the clamped send); the virtual remembers −66.
        assert!((engine.next_gang_value(&t, FADER_INF_DB, 60.0) - (-6.0)).abs() < 1e-3);
    }

    #[test]
    fn values_match_floats_within_tolerance() {
        assert!(values_match(
            &ParameterValue::Float(1.0),
            &ParameterValue::Float(1.005),
        ));
        assert!(!values_match(
            &ParameterValue::Float(1.0),
            &ParameterValue::Float(1.02),
        ));
    }

    #[test]
    fn values_match_different_types() {
        assert!(!values_match(
            &ParameterValue::Float(1.0),
            &ParameterValue::Int(1),
        ));
    }

    // ---- Suppression set tests ----

    fn make_engine() -> GangEngine {
        let config = ConsoleConfig::default();
        let state = Arc::new(RwLock::new(ConsoleState::new(config)));
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let std_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        std_socket.set_nonblocking(true).unwrap();
        let socket = std::sync::Arc::new(tokio::net::UdpSocket::from_std(std_socket).unwrap());
        let sender = OscSender::new(socket, addr);
        GangEngine::new(state, sender)
    }

    #[tokio::test]
    async fn suppression_insert_and_check() {
        let mut engine = make_engine();
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        let value = ParameterValue::Float(-5.0);

        // Insert into suppression set
        engine
            .suppression_set
            .insert(addr.clone(), (value.clone(), Instant::now()));

        // Should be suppressed
        assert!(engine.is_suppressed(&addr, &value));
        // Should no longer be suppressed (consumed)
        assert!(!engine.is_suppressed(&addr, &value));
    }

    #[tokio::test]
    async fn suppression_value_mismatch() {
        let mut engine = make_engine();
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };

        engine
            .suppression_set
            .insert(addr.clone(), (ParameterValue::Float(-5.0), Instant::now()));

        // Different value — not suppressed
        assert!(!engine.is_suppressed(&addr, &ParameterValue::Float(0.0)));
    }

    // ---- Integration tests (process_gang_update) ----

    #[tokio::test]
    async fn process_gang_update_no_gang_match() {
        let mut engine = make_engine();
        let manager = GangManager::new();
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };

        // No gangs → no-op, should not panic
        engine
            .process_gang_update(
                &addr,
                &ParameterValue::Float(-5.0),
                Some(&ParameterValue::Float(-10.0)),
                &manager,
            )
            .await;
    }

    #[tokio::test]
    async fn process_gang_update_section_filtered() {
        let mut engine = make_engine();
        let mut manager = GangManager::new();

        // Gang links FaderMutePan only
        manager.add_group(GangGroup::new(
            "Drums".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        ));

        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::EqBandGain(1), // EQ section — not linked
        };

        // Should not propagate (section not in gang)
        engine
            .process_gang_update(
                &addr,
                &ParameterValue::Float(3.0),
                Some(&ParameterValue::Float(0.0)),
                &manager,
            )
            .await;

        // Input(2) should not have been updated
        let state = engine.state.read().await;
        assert!(
            state
                .get(&ParameterAddress {
                    channel: ChannelId::Input(2),
                    parameter: ParameterPath::EqBandGain(1),
                })
                .is_none()
        );
    }

    #[tokio::test]
    async fn process_gang_update_routing_type_guard() {
        let mut engine = make_engine();
        let mut manager = GangManager::new();

        // Mixed-type gang linking Sends section
        manager.add_group(GangGroup::new(
            "Mixed".into(),
            vec![ChannelId::Input(1), ChannelId::Aux(1)],
            HashSet::from([ParameterSection::Sends, ParameterSection::FaderMutePan]),
        ));

        // Set up state for Input(1) and Aux(1) faders
        {
            let mut state = engine.state.write().await;
            state.update(
                ParameterAddress {
                    channel: ChannelId::Input(1),
                    parameter: ParameterPath::Fader,
                },
                ParameterValue::Float(-10.0),
            );
            state.update(
                ParameterAddress {
                    channel: ChannelId::Aux(1),
                    parameter: ParameterPath::Fader,
                },
                ParameterValue::Float(-20.0),
            );
        }

        // FaderMutePan change on Input(1) should propagate to Aux(1)
        // (FaderMutePan is not a routing section)
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        engine
            .process_gang_update(
                &addr,
                &ParameterValue::Float(-5.0),
                Some(&ParameterValue::Float(-10.0)),
                &manager,
            )
            .await;

        // Aux(1) fader should have been updated with delta +5: -20 + 5 = -15
        // (but send_to_console fails in test, so check suppression_set instead)
        // In test env, send_to_console will fail (no real console), but the logic
        // up to the send attempt is validated by the section_filtered test above.
        // We can verify via suppression_set that an attempt was made.
        // Actually, send_to_console binds to 127.0.0.1:0 which may succeed to send.
        // Let's just verify the state wasn't updated (send likely fails).

        // Instead test the routing guard: Sends change on Input(1) should NOT
        // propagate to Aux(1) because they're different channel types
        let send_addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::SendLevel(1),
        };
        engine
            .process_gang_update(
                &send_addr,
                &ParameterValue::Float(-5.0),
                Some(&ParameterValue::Float(-10.0)),
                &manager,
            )
            .await;

        // Aux(1) should NOT have SendLevel updated (routing guard blocks it)
        let state = engine.state.read().await;
        assert!(
            state
                .get(&ParameterAddress {
                    channel: ChannelId::Aux(1),
                    parameter: ParameterPath::SendLevel(1),
                })
                .is_none()
        );
    }

    #[tokio::test]
    async fn process_gang_update_suppressed_echo() {
        let mut engine = make_engine();
        let mut manager = GangManager::new();

        manager.add_group(GangGroup::new(
            "Drums".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        ));

        let addr = ParameterAddress {
            channel: ChannelId::Input(2),
            parameter: ParameterPath::Fader,
        };

        // Simulate: we sent a ganged change to Input(2), it's in suppression set
        engine
            .suppression_set
            .insert(addr.clone(), (ParameterValue::Float(-5.0), Instant::now()));

        // Now the "echo-back" arrives from the console
        engine
            .process_gang_update(
                &addr,
                &ParameterValue::Float(-5.0),
                Some(&ParameterValue::Float(-10.0)),
                &manager,
            )
            .await;

        // Should have been suppressed — Input(1) should NOT be updated
        let state = engine.state.read().await;
        assert!(
            state
                .get(&ParameterAddress {
                    channel: ChannelId::Input(1),
                    parameter: ParameterPath::Fader,
                })
                .is_none()
        );
    }

    #[tokio::test]
    async fn relative_pan_delta_is_clamped_to_one() {
        // Relative-mode gang shares FaderMutePan. Sibling B sits at
        // pan 0.8. A nudge from 0.0 → +0.3 (delta +0.3) would push
        // B's pan to 1.1 without the clamp. We assert the engine
        // stores 1.0 in state instead.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        let mut group = GangGroup::new(
            "Vox".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        group.mode = GangMode::Relative;
        manager.add_group(group);

        // Pre-seed sibling B's pan to 0.8.
        engine.state.write().await.update(
            ParameterAddress {
                channel: ChannelId::Input(2),
                parameter: ParameterPath::Pan,
            },
            ParameterValue::Float(0.8),
        );

        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Pan,
        };
        engine
            .process_gang_update(
                &addr,
                &ParameterValue::Float(0.3),
                Some(&ParameterValue::Float(0.0)),
                &manager,
            )
            .await;

        // Sibling B's state should have been clamped to +1.0 (not
        // overshooting to +1.1).
        let state = engine.state.read().await;
        let v = state.get(&ParameterAddress {
            channel: ChannelId::Input(2),
            parameter: ParameterPath::Pan,
        });
        assert!(
            matches!(v, Some(ParameterValue::Float(f)) if (*f - 1.0).abs() < 1e-3),
            "Input(2) Pan should be clamped to 1.0, got {v:?}",
        );
    }

    #[tokio::test]
    async fn absolute_pan_propagation_is_also_clamped() {
        // Defensive: if the source channel somehow already holds an
        // out-of-range value (from a stale state, a buggy upstream,
        // or a malformed inbound packet), Absolute-mode propagation
        // shouldn't propagate the bad value. Verify by sending an
        // explicitly out-of-range new_value through Absolute mode.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        let group = GangGroup::new(
            "Vox".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        manager.add_group(group); // default mode is Absolute

        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Pan,
        };
        engine
            .process_gang_update(
                &addr,
                &ParameterValue::Float(1.7),
                Some(&ParameterValue::Float(0.0)),
                &manager,
            )
            .await;

        let state = engine.state.read().await;
        let v = state.get(&ParameterAddress {
            channel: ChannelId::Input(2),
            parameter: ParameterPath::Pan,
        });
        assert!(
            matches!(v, Some(ParameterValue::Float(f)) if (*f - 1.0).abs() < 1e-3),
            "Input(2) Pan should be clamped to 1.0 in Absolute mode too, got {v:?}",
        );
    }

    fn pan(channel: ChannelId) -> ParameterAddress {
        ParameterAddress {
            channel,
            parameter: ParameterPath::Pan,
        }
    }

    #[tokio::test]
    async fn reversed_pan_mirrors_pair() {
        // A 2-member gang with Reversed pan mirrors the source around centre
        // onto its partner, in either direction.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        let mut group = GangGroup::new(
            "Wide".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        group.pan_mode = Some(GangPanMode::Reversed);
        manager.add_group(group);

        // Move In1 right (+0.4) → In2 mirrors left (-0.4).
        engine
            .process_gang_update(
                &pan(ChannelId::Input(1)),
                &ParameterValue::Float(0.4),
                Some(&ParameterValue::Float(0.0)),
                &manager,
            )
            .await;
        {
            let state = engine.state.read().await;
            let v = state.get(&pan(ChannelId::Input(2)));
            assert!(
                matches!(v, Some(ParameterValue::Float(f)) if (*f + 0.4).abs() < 1e-3),
                "Input(2) Pan should mirror to -0.4, got {v:?}",
            );
        }

        // And the reverse: move In2 to -0.6 → In1 mirrors to +0.6.
        engine
            .process_gang_update(
                &pan(ChannelId::Input(2)),
                &ParameterValue::Float(-0.6),
                Some(&ParameterValue::Float(-0.4)),
                &manager,
            )
            .await;
        let state = engine.state.read().await;
        let v = state.get(&pan(ChannelId::Input(1)));
        assert!(
            matches!(v, Some(ParameterValue::Float(f)) if (*f - 0.6).abs() < 1e-3),
            "Input(1) Pan should mirror to +0.6, got {v:?}",
        );
    }

    #[tokio::test]
    async fn reversed_pan_clamps() {
        // Mirroring an out-of-range source still clamps the partner to ±1.0.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        let mut group = GangGroup::new(
            "Wide".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        group.pan_mode = Some(GangPanMode::Reversed);
        manager.add_group(group);

        engine
            .process_gang_update(
                &pan(ChannelId::Input(1)),
                &ParameterValue::Float(1.7),
                Some(&ParameterValue::Float(0.0)),
                &manager,
            )
            .await;
        let state = engine.state.read().await;
        let v = state.get(&pan(ChannelId::Input(2)));
        assert!(
            matches!(v, Some(ParameterValue::Float(f)) if (*f + 1.0).abs() < 1e-3),
            "Input(2) Pan should mirror-clamp to -1.0, got {v:?}",
        );
    }

    #[tokio::test]
    async fn pan_off_does_not_propagate_but_fader_still_does() {
        // Pan Off on a FaderMutePan gang: a pan change is not propagated, but
        // a fader change still gangs (fader/mute stay in the section).
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        let mut group = GangGroup::new(
            "Drums".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        group.pan_mode = Some(GangPanMode::Off);
        group.mode = GangMode::Absolute;
        manager.add_group(group);

        // Pan change → no propagation.
        engine
            .process_gang_update(
                &pan(ChannelId::Input(1)),
                &ParameterValue::Float(0.5),
                Some(&ParameterValue::Float(0.0)),
                &manager,
            )
            .await;
        assert!(
            engine
                .state
                .read()
                .await
                .get(&pan(ChannelId::Input(2)))
                .is_none(),
            "Input(2) Pan should be untouched when pan mode is Off",
        );

        // Fader change → still propagates (Absolute copy).
        engine
            .process_gang_update(
                &ParameterAddress {
                    channel: ChannelId::Input(1),
                    parameter: ParameterPath::Fader,
                },
                &ParameterValue::Float(-5.0),
                Some(&ParameterValue::Float(-10.0)),
                &manager,
            )
            .await;
        let state = engine.state.read().await;
        let v = state.get(&ParameterAddress {
            channel: ChannelId::Input(2),
            parameter: ParameterPath::Fader,
        });
        assert!(
            matches!(v, Some(ParameterValue::Float(f)) if (*f + 5.0).abs() < 1e-3),
            "Input(2) Fader should still gang when pan mode is Off, got {v:?}",
        );
    }

    #[tokio::test]
    async fn reversed_pan_ignored_for_non_pair() {
        // Reversed is pairs-only: a 3-member gang skips pan propagation.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        let mut group = GangGroup::new(
            "Trio".into(),
            vec![
                ChannelId::Input(1),
                ChannelId::Input(2),
                ChannelId::Input(3),
            ],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        group.pan_mode = Some(GangPanMode::Reversed);
        manager.add_group(group);

        engine
            .process_gang_update(
                &pan(ChannelId::Input(1)),
                &ParameterValue::Float(0.4),
                Some(&ParameterValue::Float(0.0)),
                &manager,
            )
            .await;
        let state = engine.state.read().await;
        assert!(state.get(&pan(ChannelId::Input(2))).is_none());
        assert!(state.get(&pan(ChannelId::Input(3))).is_none());
    }

    // ---- Fader floor (gang dead-zone) integration tests ----

    fn fader(channel: ChannelId) -> ParameterAddress {
        ParameterAddress {
            channel,
            parameter: ParameterPath::Fader,
        }
    }

    /// Build a Relative-mode gang linking Fader/Mute/Pan over the given inputs
    /// and pre-seed each sibling's fader state.
    async fn relative_fader_gang(
        engine: &GangEngine,
        manager: &mut GangManager,
        members: &[(u16, f32)],
    ) {
        let mut group = GangGroup::new(
            "Faders".into(),
            members.iter().map(|(n, _)| ChannelId::Input(*n)).collect(),
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        group.mode = GangMode::Relative;
        manager.add_group(group);
        let mut state = engine.state.write().await;
        for (n, db) in members {
            state.update(fader(ChannelId::Input(*n)), ParameterValue::Float(*db));
        }
    }

    async fn fader_db(engine: &GangEngine, ch: u16) -> Option<f32> {
        match engine.state.read().await.get(&fader(ChannelId::Input(ch))) {
            Some(ParameterValue::Float(f)) => Some(*f),
            _ => None,
        }
    }

    #[tokio::test]
    async fn fader_gang_deadzone_does_not_move_sibling() {
        // The core regression: source nudged within the inaudible dead zone
        // (−150 → −90) must NOT touch an audible-ish sibling parked at −60.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        relative_fader_gang(&engine, &mut manager, &[(1, -150.0), (2, -60.0)]).await;

        engine
            .process_gang_update(
                &fader(ChannelId::Input(1)),
                &ParameterValue::Float(-90.0),
                Some(&ParameterValue::Float(-150.0)),
                &manager,
            )
            .await;

        assert_eq!(fader_db(&engine, 2).await, Some(-60.0));
        // The dead-zone skip never touches the sibling's virtual position either.
        assert!(
            !engine
                .gang_virtual
                .contains_key(&fader(ChannelId::Input(2)))
        );
    }

    #[tokio::test]
    async fn fader_gang_lockstep_from_inf() {
        // Raising the parked fader above the floor brings the sibling up with it.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        relative_fader_gang(&engine, &mut manager, &[(1, -150.0), (2, -60.0)]).await;

        engine
            .process_gang_update(
                &fader(ChannelId::Input(1)),
                &ParameterValue::Float(-50.0),
                Some(&ParameterValue::Float(-150.0)),
                &manager,
            )
            .await;

        assert_eq!(fader_db(&engine, 2).await, Some(-50.0));
    }

    #[tokio::test]
    async fn fader_gang_both_subfloor_rise() {
        // Two sub-floor faders both rise to the same audible level.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        relative_fader_gang(&engine, &mut manager, &[(1, -80.0), (2, -100.0)]).await;

        engine
            .process_gang_update(
                &fader(ChannelId::Input(1)),
                &ParameterValue::Float(-50.0),
                Some(&ParameterValue::Float(-80.0)),
                &manager,
            )
            .await;

        assert_eq!(fader_db(&engine, 2).await, Some(-50.0));
    }

    #[tokio::test]
    async fn fader_gang_above_floor_normal() {
        // Wholly above the floor, behaviour is plain relative — unchanged.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        relative_fader_gang(&engine, &mut manager, &[(1, -20.0), (2, -30.0)]).await;

        engine
            .process_gang_update(
                &fader(ChannelId::Input(1)),
                &ParameterValue::Float(-10.0),
                Some(&ParameterValue::Float(-20.0)),
                &manager,
            )
            .await;

        assert_eq!(fader_db(&engine, 2).await, Some(-20.0));
    }

    #[tokio::test]
    async fn fader_gang_lowering_snaps_sibling_to_inf() {
        // Pulling the source down into the dead zone drives the sibling fully off.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        relative_fader_gang(&engine, &mut manager, &[(1, -30.0), (2, -50.0)]).await;

        engine
            .process_gang_update(
                &fader(ChannelId::Input(1)),
                &ParameterValue::Float(-150.0),
                Some(&ParameterValue::Float(-30.0)),
                &manager,
            )
            .await;

        assert_eq!(fader_db(&engine, 2).await, Some(FADER_INF_DB));
    }

    #[tokio::test]
    async fn fader_gang_continuous_drag_accumulates() {
        // A continuous sweep up from −inf: the sub-floor steps no-op, then the
        // sibling tracks the above-floor portion. Sibling starts at −55.
        // Net move above floor: (−58 − (−60)) + (−40 − (−58)) = 2 + 18 = 20.
        // So −55 + 20 = −35.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        relative_fader_gang(&engine, &mut manager, &[(1, -150.0), (2, -55.0)]).await;

        let steps = [
            (-150.0, -120.0),
            (-120.0, -90.0),
            (-90.0, -62.0),
            (-62.0, -58.0),
            (-58.0, -40.0),
        ];
        for (old, new) in steps {
            engine
                .process_gang_update(
                    &fader(ChannelId::Input(1)),
                    &ParameterValue::Float(new),
                    Some(&ParameterValue::Float(old)),
                    &manager,
                )
                .await;
        }

        let v = fader_db(&engine, 2).await.unwrap();
        assert!((v - (-35.0)).abs() < 1e-3, "expected −35, got {v}");
    }

    async fn pan_db(engine: &GangEngine, ch: u16) -> Option<f32> {
        match engine.state.read().await.get(&pan(ChannelId::Input(ch))) {
            Some(ParameterValue::Float(f)) => Some(*f),
            _ => None,
        }
    }

    // ---- Offset preservation across bounds (the headline fix) ----

    #[tokio::test]
    async fn fader_gang_offset_restored_after_inf_round_trip() {
        // f3 = 0, f4 = −6 (a −6 dB offset). Pull f3 to −inf and back: f4 reads
        // fully off while f3 is off, then RESTORES to −6 — not 0.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        relative_fader_gang(&engine, &mut manager, &[(3, 0.0), (4, -6.0)]).await;

        engine
            .process_gang_update(
                &fader(ChannelId::Input(3)),
                &ParameterValue::Float(-150.0),
                Some(&ParameterValue::Float(0.0)),
                &manager,
            )
            .await;
        assert_eq!(fader_db(&engine, 4).await, Some(FADER_INF_DB));

        engine
            .process_gang_update(
                &fader(ChannelId::Input(3)),
                &ParameterValue::Float(0.0),
                Some(&ParameterValue::Float(-150.0)),
                &manager,
            )
            .await;
        let v = fader_db(&engine, 4).await.unwrap();
        assert!((v - (-6.0)).abs() < 1e-3, "expected −6 restored, got {v}");
    }

    #[tokio::test]
    async fn fader_gang_offset_restored_multistep() {
        // 0 → −30 → −150 → 0 still restores the −6 offset at the end.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        relative_fader_gang(&engine, &mut manager, &[(3, 0.0), (4, -6.0)]).await;

        for (old, new) in [(0.0, -30.0), (-30.0, -150.0), (-150.0, 0.0)] {
            engine
                .process_gang_update(
                    &fader(ChannelId::Input(3)),
                    &ParameterValue::Float(new),
                    Some(&ParameterValue::Float(old)),
                    &manager,
                )
                .await;
        }
        let v = fader_db(&engine, 4).await.unwrap();
        assert!((v - (-6.0)).abs() < 1e-3, "expected −6 restored, got {v}");
    }

    #[tokio::test]
    async fn pan_gang_offset_restored_after_rail_round_trip() {
        // Sibling pan 0.5; master swept to the +1 rail and back must restore the
        // 0.5 offset (the ±1 clamp would otherwise crush it, like the fader −inf).
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        let mut group = GangGroup::new(
            "Vox".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        group.mode = GangMode::Relative;
        manager.add_group(group);
        engine
            .state
            .write()
            .await
            .update(pan(ChannelId::Input(2)), ParameterValue::Float(0.5));

        // Master pan 0.0 → +0.8: sibling 0.5 → 1.3 clamped to the +1 rail.
        engine
            .process_gang_update(
                &pan(ChannelId::Input(1)),
                &ParameterValue::Float(0.8),
                Some(&ParameterValue::Float(0.0)),
                &manager,
            )
            .await;
        let v = pan_db(&engine, 2).await.unwrap();
        assert!(
            (v - 1.0).abs() < 1e-3,
            "sibling pan should pin at the rail, got {v}"
        );

        // Master pan +0.8 → 0.0: sibling restores to 0.5, not 0.2.
        engine
            .process_gang_update(
                &pan(ChannelId::Input(1)),
                &ParameterValue::Float(0.0),
                Some(&ParameterValue::Float(0.8)),
                &manager,
            )
            .await;
        let v = pan_db(&engine, 2).await.unwrap();
        assert!(
            (v - 0.5).abs() < 1e-3,
            "sibling pan offset should restore to 0.5, got {v}"
        );
    }

    #[tokio::test]
    async fn fader_gang_direct_grab_rebaselines_offset() {
        // With the master at −inf (sibling off), the operator grabs the sibling
        // and sets it to −20. A later master move must track from the hand-set
        // −20 (mirror wins — clamp(stale virtual) ≠ mirror), not the old −66.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        relative_fader_gang(&engine, &mut manager, &[(3, 0.0), (4, -6.0)]).await;

        engine
            .process_gang_update(
                &fader(ChannelId::Input(3)),
                &ParameterValue::Float(-150.0),
                Some(&ParameterValue::Float(0.0)),
                &manager,
            )
            .await;
        assert_eq!(fader_db(&engine, 4).await, Some(FADER_INF_DB));

        // Operator re-grabs the sibling directly (mirror set, as the inbound
        // handler would before propagation).
        engine
            .state
            .write()
            .await
            .update(fader(ChannelId::Input(4)), ParameterValue::Float(-20.0));

        // Master −150 → −50 (floored delta +10). Sibling re-baselines from −20.
        engine
            .process_gang_update(
                &fader(ChannelId::Input(3)),
                &ParameterValue::Float(-50.0),
                Some(&ParameterValue::Float(-150.0)),
                &manager,
            )
            .await;
        let v = fader_db(&engine, 4).await.unwrap();
        assert!(
            (v - (-10.0)).abs() < 1e-3,
            "expected re-baseline −20 + 10 = −10, got {v}"
        );
    }

    #[tokio::test]
    async fn fader_gang_three_members_independent() {
        // Each sibling is floored against its own current value.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        relative_fader_gang(
            &engine,
            &mut manager,
            &[(1, -80.0), (2, -100.0), (3, -20.0)],
        )
        .await;

        engine
            .process_gang_update(
                &fader(ChannelId::Input(1)),
                &ParameterValue::Float(-50.0),
                Some(&ParameterValue::Float(-80.0)),
                &manager,
            )
            .await;

        // Δg = g(−50) − g(−80) = −50 − (−60) = +10.
        assert_eq!(fader_db(&engine, 2).await, Some(-50.0)); // g(−100)+10 = −50
        assert_eq!(fader_db(&engine, 3).await, Some(-10.0)); // −20 + 10 = −10
    }

    #[tokio::test]
    async fn fader_gang_absolute_mode_copies_raw() {
        // Absolute mode is untouched: it copies the raw source value, even from
        // deep in the dead zone — no flooring.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        let mut group = GangGroup::new(
            "Abs".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        group.mode = GangMode::Absolute;
        manager.add_group(group);
        engine
            .state
            .write()
            .await
            .update(fader(ChannelId::Input(2)), ParameterValue::Float(-60.0));

        engine
            .process_gang_update(
                &fader(ChannelId::Input(1)),
                &ParameterValue::Float(-90.0),
                Some(&ParameterValue::Float(-150.0)),
                &manager,
            )
            .await;

        assert_eq!(fader_db(&engine, 2).await, Some(-90.0));
    }

    #[tokio::test]
    async fn send_level_gang_is_not_floored() {
        // Scoping guard: the floor is Fader-only. A Relative Sends gang keeps
        // raw-delta semantics, so a sub-floor source move still propagates.
        let mut engine = make_engine();
        let mut manager = GangManager::new();
        let mut group = GangGroup::new(
            "Sends".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::Sends]),
        );
        group.mode = GangMode::Relative;
        manager.add_group(group);
        let send = |ch| ParameterAddress {
            channel: ChannelId::Input(ch),
            parameter: ParameterPath::SendLevel(1),
        };
        engine
            .state
            .write()
            .await
            .update(send(2), ParameterValue::Float(-60.0));

        engine
            .process_gang_update(
                &send(1),
                &ParameterValue::Float(-90.0),
                Some(&ParameterValue::Float(-150.0)),
                &manager,
            )
            .await;

        // Raw delta +60 applied: −60 + 60 = 0 (NOT floored to a no-op).
        let v = match engine.state.read().await.get(&send(2)) {
            Some(ParameterValue::Float(f)) => Some(*f),
            _ => None,
        };
        assert_eq!(v, Some(0.0));
    }
}
