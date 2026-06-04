use std::collections::HashMap;
use std::mem;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::model::parameter::{
    FADER_GANG_FLOOR_DB, FADER_INF_DB, ParameterAddress, ParameterPath, ParameterSection,
    ParameterValue,
};
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::encode;
use crate::osc::ipad_client::IpadSender;
use crate::osc::ipad_encode;

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
    sender: OscSender,
    ipad_sender: Option<IpadSender>,
    /// Recently-sent ganged changes, keyed by address.
    /// Used to suppress feedback loops from console echo-back.
    suppression_set: HashMap<ParameterAddress, (ParameterValue, Instant)>,
}

impl GangEngine {
    pub fn new(state: Arc<RwLock<ConsoleState>>, sender: OscSender) -> Self {
        Self {
            state,
            sender,
            ipad_sender: None,
            suppression_set: HashMap::new(),
        }
    }

    pub fn set_ipad_sender(&mut self, sender: Option<IpadSender>) {
        self.ipad_sender = sender;
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
                    // Relative mode: apply delta to target's current value
                    let current = self.state.read().await.get(&target_addr).cloned();
                    match current {
                        Some(ref cv) => {
                            let applied = if is_fader {
                                apply_fader_gang_delta(cv, d)
                            } else {
                                apply_delta(cv, d)
                            };
                            applied.unwrap_or_else(|| new_value.clone()) // fallback to absolute
                        }
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
        // Try GP OSC first
        match encode::encode_parameter(addr, value) {
            Some((path, args)) => {
                if let Err(e) = self.sender.send(&path, args).await {
                    warn!(%addr, "Gang: failed to send to console: {e}");
                    return false;
                }
                true
            }
            None => {
                // Try iPad protocol fallback
                if let Some(ref ipad) = self.ipad_sender {
                    match ipad_encode::encode_ipad_parameter(addr, value) {
                        Some((path, args)) => {
                            if let Err(e) = ipad.send(&path, args).await {
                                warn!(%addr, "Gang: iPad send failed: {e}");
                                return false;
                            }
                            true
                        }
                        None => {
                            warn!(%addr, "Gang: cannot encode parameter for either protocol");
                            false
                        }
                    }
                } else {
                    warn!(%addr, "Gang: no sender available for iPad-only parameter");
                    false
                }
            }
        }
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

/// Apply a floored fader delta to a sibling's current value, snapping to −inf
/// when the result lands at or below the floor (everything below floor reads as
/// fully off). `None` for non-float values.
fn apply_fader_gang_delta(current: &ParameterValue, delta: f32) -> Option<ParameterValue> {
    match current {
        ParameterValue::Float(f) => {
            let t = gang_floor(*f) + delta;
            Some(ParameterValue::Float(if t <= FADER_GANG_FLOOR_DB {
                FADER_INF_DB
            } else {
                t
            }))
        }
        _ => None,
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
    fn apply_fader_gang_delta_normal_above_floor() {
        assert_eq!(
            apply_fader_gang_delta(&ParameterValue::Float(-30.0), 10.0),
            Some(ParameterValue::Float(-20.0))
        );
        // A sub-floor sibling floors to −60 before the delta is applied.
        assert_eq!(
            apply_fader_gang_delta(&ParameterValue::Float(-100.0), 10.0),
            Some(ParameterValue::Float(-50.0))
        );
    }

    #[test]
    fn apply_fader_gang_delta_snaps_to_inf_at_or_below_floor() {
        // Result below the floor snaps to −inf.
        assert_eq!(
            apply_fader_gang_delta(&ParameterValue::Float(-50.0), -30.0),
            Some(ParameterValue::Float(FADER_INF_DB))
        );
        // Boundary: a result of exactly the floor also snaps (`<=`).
        assert_eq!(
            apply_fader_gang_delta(&ParameterValue::Float(-50.0), -10.0),
            Some(ParameterValue::Float(FADER_INF_DB))
        );
    }

    #[test]
    fn apply_fader_gang_delta_non_float_none() {
        assert_eq!(
            apply_fader_gang_delta(&ParameterValue::Bool(true), 1.0),
            None
        );
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
        members: &[(u8, f32)],
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

    async fn fader_db(engine: &GangEngine, ch: u8) -> Option<f32> {
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
