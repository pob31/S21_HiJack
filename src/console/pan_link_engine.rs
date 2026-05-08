//! Pan Link engine: propagates input-channel main pan changes to the
//! linked stereo aux send pans configured in `PanLinkBindings`.
//!
//! Triggered from the OSC inbound dispatch in `connection::process_message`
//! after the gang engine has had its turn. Skipped entirely while the
//! snapshot engine's dirty-suppression flag is active so memory recalls
//! (snapshots, cues, macros) are never modified by pan link.
//!
//! Pan-link bindings are per-input. When the moved input is a member of
//! a smart-gang group whose `linked_sections` include `FaderMutePan`,
//! the engine *fans out* the binding's aux writes to every sibling in
//! that gang — using each sibling's current main-pan value (which the
//! gang engine has already written) so Relative-mode gangs propagate
//! correctly. Operators only have to set a binding once on a gang
//! representative for every member's matching aux to follow.
//!
//! Stereo / mono filter: mix-output buses must be stereo to be
//! pan-linkable in Mode 2 / Mode 3 (iPad protocol active), since
//! that's where the bus mode is discoverable. In Mode 1 (GP OSC only)
//! the desk doesn't expose channel mode, so the auto-detection is
//! bypassed and every aux is allowed. The operator can override on a
//! per-aux basis via `PanLinkBindings.mono_overrides` — useful both
//! in Mode 1 (where there's no auto-detection) and in Mode 2/3 (to
//! disable pan-link for a specific stereo aux for any reason).
//!
//! Send-enable: the engine does NOT gate on `SendEnabled`. Writing
//! the aux send pan when the send is disabled keeps the aux pan
//! pre-staged so it's already in sync with the input main pan when
//! the operator next enables the send — otherwise the aux pan would
//! be whatever stale value it held while the send was off.

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::gang_manager::GangManager;
use crate::model::channel::ChannelId;
use crate::model::config::ChannelMode;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::pan_link::PanLinkBindings;
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterSection, ParameterValue};
use crate::model::state::ConsoleState;
use crate::osc::client::OscSender;
use crate::osc::encode;
use crate::osc::ipad_client::IpadSender;
use crate::osc::ipad_encode;

/// Engine that turns input-channel pan moves into aux send pan moves
/// for every active `(input, aux)` binding.
pub struct PanLinkEngine {
    state: Arc<RwLock<ConsoleState>>,
    sender: OscSender,
    ipad_sender: Option<IpadSender>,
    bindings: Arc<RwLock<PanLinkBindings>>,
    dirty_tracker: Arc<RwLock<DirtyTracker>>,
    gang_manager: Arc<RwLock<GangManager>>,
}

impl PanLinkEngine {
    pub fn new(
        state: Arc<RwLock<ConsoleState>>,
        sender: OscSender,
        bindings: Arc<RwLock<PanLinkBindings>>,
        dirty_tracker: Arc<RwLock<DirtyTracker>>,
        gang_manager: Arc<RwLock<GangManager>>,
    ) -> Self {
        Self {
            state,
            sender,
            ipad_sender: None,
            bindings,
            dirty_tracker,
            gang_manager,
        }
    }

    pub fn set_ipad_sender(&mut self, sender: Option<IpadSender>) {
        self.ipad_sender = sender;
    }

    /// Process an inbound parameter update. Only main pan on input
    /// channels is interesting; everything else is a no-op.
    pub async fn process_pan_update(&self, addr: &ParameterAddress, new_value: &ParameterValue) {
        let writes = self.compute_pan_writes(addr, new_value).await;
        if !writes.is_empty() {
            // info-level so it's visible without enabling debug, but
            // doesn't fire on every parameter — only when we actually
            // have aux pan writes to push.
            tracing::info!(
                source = %addr,
                writes = writes.len(),
                "PanLink: propagating to aux send pan(s)",
            );
        }
        for (target, value) in writes {
            self.send_to_console(&target, &value).await;
        }
    }

    /// Compute the list of `SendPan` writes that should fire for a given
    /// pan change. Pure-async (reads only), so unit tests can assert on
    /// the targets without needing to capture OSC sends.
    ///
    /// The list always starts with the moved input's own bindings (if
    /// any), then appends the same auxes for every pan-shared gang
    /// sibling, using each sibling's *current* main-pan value rather
    /// than the moved input's. This matters for Relative-mode gangs
    /// where siblings shift by the same delta but end up at different
    /// absolute pans.
    async fn compute_pan_writes(
        &self,
        addr: &ParameterAddress,
        new_value: &ParameterValue,
    ) -> Vec<(ParameterAddress, ParameterValue)> {
        let ChannelId::Input(input_n) = addr.channel else {
            return Vec::new();
        };
        if addr.parameter != ParameterPath::Pan {
            return Vec::new();
        }

        // Recall guard: never propagate while a memory recall is in flight.
        if self.dirty_tracker.read().await.is_suppressed() {
            debug!(input = input_n, "PanLink: skipped during recall");
            return Vec::new();
        }

        // Snapshot the active aux list for the moved input plus the
        // operator's mono-override set. The override blocks pan-link
        // for marked auxes regardless of console-reported mode.
        let (auxes, mono_overrides) = {
            let b = self.bindings.read().await;
            (b.auxes_for(input_n), b.mono_overrides.clone())
        };
        if auxes.is_empty() {
            return Vec::new();
        }

        let state = self.state.read().await;
        let mix_modes = state.config.mix_output_modes.clone();
        let mix_types = state.config.mix_output_types.clone();

        // Build the (channel, pan_value) targets list. The moved input
        // is always first; pan-shared gang siblings follow with each
        // sibling's own main-pan from state. Dedup by channel number so
        // a sibling that appears in multiple matching gangs is only
        // written once.
        let mut targets: Vec<(u8, ParameterValue)> = vec![(input_n, new_value.clone())];
        let mut seen: HashSet<u8> = HashSet::new();
        seen.insert(input_n);
        {
            let gm = self.gang_manager.read().await;
            let gangs = gm.find_gangs_for_channel_and_section(
                &ChannelId::Input(input_n),
                &ParameterSection::FaderMutePan,
            );
            for gang in gangs {
                if gang.paused {
                    continue;
                }
                for sibling in gang.other_members(&ChannelId::Input(input_n)) {
                    let ChannelId::Input(sib_n) = sibling else {
                        continue;
                    };
                    if !seen.insert(*sib_n) {
                        continue;
                    }
                    let sib_pan = state
                        .get(&ParameterAddress {
                            channel: ChannelId::Input(*sib_n),
                            parameter: ParameterPath::Pan,
                        })
                        .cloned();
                    if let Some(p) = sib_pan {
                        // Defensive clamp: even though the gang
                        // engine now clamps when it writes pan, a
                        // stale state from before this fix landed
                        // could still hold an out-of-range value.
                        // Belt-and-braces — the writes we emit are
                        // always in [-1, +1].
                        let p = ParameterPath::Pan.clamp_value(p);
                        targets.push((*sib_n, p));
                    }
                    // If the sibling's main pan isn't in the state
                    // mirror yet, skip — we don't want to invent a
                    // value for an aux send that may not match the
                    // unknown main pan.
                }
            }
        }

        // In Mode 1 (GP OSC only, no iPad sender attached) the channel
        // mode of mix-output buses isn't reported, so we can't tell
        // mono from stereo — assume stereo and let the operator decide
        // by binding (or not) in the Pan Link tab. Mode 2 / Mode 3
        // (iPad protocol active) still enforces the stereo filter.
        let assume_all_stereo = self.ipad_sender.is_none();

        let mut writes: Vec<(ParameterAddress, ParameterValue)> = Vec::new();
        for (ch_n, pan_value) in targets {
            for &aux in &auxes {
                // Operator's manual mono-mark always wins.
                if mono_overrides.contains(&aux) {
                    continue;
                }
                let idx0 = aux.checked_sub(1).map(|i| i as usize);
                let is_aux_bus = idx0.and_then(|i| mix_types.get(i)).copied().unwrap_or(true);
                if !is_aux_bus {
                    continue;
                }
                let is_stereo = assume_all_stereo
                    || idx0
                        .and_then(|i| mix_modes.get(i))
                        .map(|m| *m == ChannelMode::Stereo)
                        .unwrap_or(false);
                if !is_stereo {
                    continue;
                }

                // No `SendEnabled` gate: if we only wrote when the send
                // was active, the bound aux's pan would be stale at
                // the moment the operator next enables that send. The
                // cost of always writing is negligible (one extra OSC
                // message per inactive aux per pan move), and it
                // keeps the bound auxes' pans in lockstep with the
                // input main pan at all times.

                let target = ParameterAddress {
                    channel: ChannelId::Input(ch_n),
                    parameter: ParameterPath::SendPan(aux),
                };
                writes.push((target, pan_value.clone()));
            }
        }
        writes
    }

    async fn send_to_console(&self, addr: &ParameterAddress, value: &ParameterValue) {
        match encode::encode_parameter(addr, value) {
            Some((path, args)) => {
                if let Err(e) = self.sender.send(&path, args).await {
                    warn!(%addr, "PanLink: failed to send: {e}");
                }
            }
            None => {
                if let Some(ref ipad) = self.ipad_sender {
                    match ipad_encode::encode_ipad_parameter(addr, value) {
                        Some((path, args)) => {
                            if let Err(e) = ipad.send(&path, args).await {
                                warn!(%addr, "PanLink: iPad send failed: {e}");
                            }
                        }
                        None => warn!(%addr, "PanLink: cannot encode for either protocol"),
                    }
                } else {
                    warn!(%addr, "PanLink: no sender available for iPad-only parameter");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::ConsoleConfig;
    use crate::model::gang::GangGroup;
    use std::net::SocketAddr;

    fn make_engine() -> PanLinkEngine {
        let mut config = ConsoleConfig::default();
        config.input_channel_count = 8;
        config.aux_output_count = 8;
        config.mix_output_types = vec![true; 8];
        config.mix_output_modes = vec![ChannelMode::Stereo; 8];
        let state = Arc::new(RwLock::new(ConsoleState::new(config)));
        let bindings = Arc::new(RwLock::new(PanLinkBindings::default()));
        let dirty = Arc::new(RwLock::new(DirtyTracker::new()));
        let gang_mgr = Arc::new(RwLock::new(GangManager::new()));

        let addr: SocketAddr = "127.0.0.1:9001".parse().unwrap();
        let std_socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        std_socket.set_nonblocking(true).unwrap();
        let socket = std::sync::Arc::new(tokio::net::UdpSocket::from_std(std_socket).unwrap());
        let sender = OscSender::new(socket, addr);

        PanLinkEngine::new(state, sender, bindings, dirty, gang_mgr)
    }

    fn pan_addr(ch: u8) -> ParameterAddress {
        ParameterAddress {
            channel: ChannelId::Input(ch),
            parameter: ParameterPath::Pan,
        }
    }

    fn send_pan_addr(ch: u8, aux: u8) -> ParameterAddress {
        ParameterAddress {
            channel: ChannelId::Input(ch),
            parameter: ParameterPath::SendPan(aux),
        }
    }

    #[tokio::test]
    async fn no_writes_when_no_bindings() {
        let engine = make_engine();
        let writes = engine
            .compute_pan_writes(&pan_addr(1), &ParameterValue::Float(0.5))
            .await;
        assert!(writes.is_empty());
    }

    #[tokio::test]
    async fn writes_only_for_bound_input_when_no_gang() {
        let engine = make_engine();
        engine.bindings.write().await.set_active(1, 5, true);

        let writes = engine
            .compute_pan_writes(&pan_addr(1), &ParameterValue::Float(0.5))
            .await;
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, send_pan_addr(1, 5));
    }

    #[tokio::test]
    async fn fans_out_to_pan_shared_gang_siblings() {
        let engine = make_engine();
        engine.bindings.write().await.set_active(1, 5, true);

        let group = GangGroup::new(
            "Vox".into(),
            vec![
                ChannelId::Input(1),
                ChannelId::Input(2),
                ChannelId::Input(3),
            ],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        engine.gang_manager.write().await.add_group(group);

        // Seed each input's main pan in state — simulates the gang
        // engine having just propagated. Sibling 2 is at +0.5 (matches
        // A in absolute mode); sibling 3 is at -0.2 (would match A in
        // a hypothetical relative-mode shift).
        {
            let mut s = engine.state.write().await;
            s.update(pan_addr(1), ParameterValue::Float(0.5));
            s.update(pan_addr(2), ParameterValue::Float(0.5));
            s.update(pan_addr(3), ParameterValue::Float(-0.2));
        }

        let writes = engine
            .compute_pan_writes(&pan_addr(1), &ParameterValue::Float(0.5))
            .await;

        // Three writes — one per ganged input — all targeting aux 5.
        assert_eq!(writes.len(), 3);
        let by_channel: std::collections::HashMap<ChannelId, &ParameterValue> =
            writes.iter().map(|(a, v)| (a.channel.clone(), v)).collect();
        assert!(matches!(
            by_channel.get(&ChannelId::Input(1)),
            Some(ParameterValue::Float(v)) if (*v - 0.5).abs() < f32::EPSILON
        ));
        assert!(matches!(
            by_channel.get(&ChannelId::Input(2)),
            Some(ParameterValue::Float(v)) if (*v - 0.5).abs() < f32::EPSILON
        ));
        // Sibling 3's value comes from state, NOT from new_value — this
        // is what makes Relative-mode gangs work correctly.
        assert!(matches!(
            by_channel.get(&ChannelId::Input(3)),
            Some(ParameterValue::Float(v)) if (*v - (-0.2)).abs() < f32::EPSILON
        ));
    }

    #[tokio::test]
    async fn does_not_fan_out_when_gang_paused() {
        let engine = make_engine();
        engine.bindings.write().await.set_active(1, 5, true);

        let mut group = GangGroup::new(
            "Vox".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        group.paused = true;
        engine.gang_manager.write().await.add_group(group);
        engine
            .state
            .write()
            .await
            .update(pan_addr(2), ParameterValue::Float(0.5));

        let writes = engine
            .compute_pan_writes(&pan_addr(1), &ParameterValue::Float(0.5))
            .await;
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, send_pan_addr(1, 5));
    }

    #[tokio::test]
    async fn does_not_fan_out_when_fader_mute_pan_not_linked() {
        let engine = make_engine();
        engine.bindings.write().await.set_active(1, 5, true);

        // Gang exists and is enabled, but only links Eq — pan is NOT
        // shared, so the fan-out should not fire.
        let group = GangGroup::new(
            "Vox".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::Eq]),
        );
        engine.gang_manager.write().await.add_group(group);
        engine
            .state
            .write()
            .await
            .update(pan_addr(2), ParameterValue::Float(0.5));

        let writes = engine
            .compute_pan_writes(&pan_addr(1), &ParameterValue::Float(0.5))
            .await;
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, send_pan_addr(1, 5));
    }

    #[tokio::test]
    async fn dedupes_sibling_in_multiple_gangs() {
        let engine = make_engine();
        engine.bindings.write().await.set_active(1, 5, true);

        // Two overlapping gangs: both contain inputs 1 and 2 and link
        // FaderMutePan. Sibling 2 should only be written once.
        let g1 = GangGroup::new(
            "Vox".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        let g2 = GangGroup::new(
            "Bgv".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        {
            let mut gm = engine.gang_manager.write().await;
            gm.add_group(g1);
            gm.add_group(g2);
        }
        engine
            .state
            .write()
            .await
            .update(pan_addr(2), ParameterValue::Float(0.5));

        let writes = engine
            .compute_pan_writes(&pan_addr(1), &ParameterValue::Float(0.5))
            .await;
        // Two writes only: input 1 and input 2.
        assert_eq!(writes.len(), 2);
    }

    #[tokio::test]
    async fn skips_during_recall_suppression() {
        let engine = make_engine();
        engine.bindings.write().await.set_active(1, 5, true);
        engine.dirty_tracker.write().await.begin_suppression();

        let writes = engine
            .compute_pan_writes(&pan_addr(1), &ParameterValue::Float(0.5))
            .await;
        assert!(writes.is_empty());
    }

    #[tokio::test]
    async fn skips_when_aux_is_mono_in_ipad_mode() {
        // Mode 2 / Mode 3 (iPad protocol active) honours the stereo
        // filter — mono auxes are skipped.
        let mut engine = make_engine();
        attach_dummy_ipad_sender(&mut engine).await;
        engine.state.write().await.config.mix_output_modes[4] = ChannelMode::Mono;
        engine.bindings.write().await.set_active(1, 5, true);

        let writes = engine
            .compute_pan_writes(&pan_addr(1), &ParameterValue::Float(0.5))
            .await;
        assert!(writes.is_empty());
    }

    #[tokio::test]
    async fn allows_mono_aux_in_gp_osc_mode() {
        // Mode 1 (no iPad sender attached) skips the stereo filter so
        // pan-link can be set up against any aux — GP OSC doesn't
        // expose channel mode and we'd otherwise refuse every aux.
        let engine = make_engine();
        engine.state.write().await.config.mix_output_modes[4] = ChannelMode::Mono;
        engine.bindings.write().await.set_active(1, 5, true);

        let writes = engine
            .compute_pan_writes(&pan_addr(1), &ParameterValue::Float(0.5))
            .await;
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, send_pan_addr(1, 5));
    }

    /// Spin up an iPad sender pointing at a dummy local address and
    /// attach it to the engine so the stereo filter activates.
    async fn attach_dummy_ipad_sender(engine: &mut PanLinkEngine) {
        let ipad_client = crate::osc::ipad_client::IpadClient::new(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:1".parse().unwrap(),
            None,
        )
        .await
        .unwrap();
        let (ipad_sender, _ipad_rx) = ipad_client.into_parts();
        engine.set_ipad_sender(Some(ipad_sender));
    }

    #[tokio::test]
    async fn writes_even_when_send_disabled() {
        // Pre-staging the aux pan while the send is off keeps the
        // bound aux in sync for when the operator next enables the
        // send. The engine deliberately does NOT gate on SendEnabled.
        let engine = make_engine();
        engine.bindings.write().await.set_active(1, 5, true);
        engine.state.write().await.update(
            ParameterAddress {
                channel: ChannelId::Input(1),
                parameter: ParameterPath::SendEnabled(5),
            },
            ParameterValue::Bool(false),
        );

        let writes = engine
            .compute_pan_writes(&pan_addr(1), &ParameterValue::Float(0.5))
            .await;
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, send_pan_addr(1, 5));
    }

    #[tokio::test]
    async fn mono_override_blocks_link() {
        // Even in GP-OSC mode (no iPad sender attached) where the
        // stereo filter is bypassed, a manual mono-override blocks
        // pan-link for that aux.
        let engine = make_engine();
        engine.bindings.write().await.set_active(1, 5, true);
        engine.bindings.write().await.toggle_mono_override(5);

        let writes = engine
            .compute_pan_writes(&pan_addr(1), &ParameterValue::Float(0.5))
            .await;
        assert!(writes.is_empty());
    }

    #[tokio::test]
    async fn mono_override_blocks_in_ipad_mode_even_for_stereo_aux() {
        // Mode 2 / Mode 3: aux is reported as stereo by the desk,
        // but the operator marked it mono via the override. Override
        // wins.
        let mut engine = make_engine();
        attach_dummy_ipad_sender(&mut engine).await;
        // Aux 5 → stereo per console.
        engine.state.write().await.config.mix_output_modes[4] = ChannelMode::Stereo;
        engine.bindings.write().await.set_active(1, 5, true);
        engine.bindings.write().await.toggle_mono_override(5);

        let writes = engine
            .compute_pan_writes(&pan_addr(1), &ParameterValue::Float(0.5))
            .await;
        assert!(writes.is_empty());
    }

    #[tokio::test]
    async fn out_of_range_sibling_state_is_clamped_in_writes() {
        // Defensive: state somehow holds an out-of-range pan for a
        // gang sibling (e.g. from before the gang clamp landed, or
        // from a malformed inbound). Pan-link's fan-out should still
        // emit a write inside [-1, +1] for that sibling.
        let engine = make_engine();
        engine.bindings.write().await.set_active(1, 5, true);
        let group = GangGroup::new(
            "Vox".into(),
            vec![ChannelId::Input(1), ChannelId::Input(2)],
            HashSet::from([ParameterSection::FaderMutePan]),
        );
        engine.gang_manager.write().await.add_group(group);
        // Pre-seed input 2's pan to an out-of-range +1.3.
        engine
            .state
            .write()
            .await
            .update(pan_addr(2), ParameterValue::Float(1.3));

        let writes = engine
            .compute_pan_writes(&pan_addr(1), &ParameterValue::Float(0.5))
            .await;
        // Two writes: input 1 (the source) and input 2 (the sibling).
        assert_eq!(writes.len(), 2);
        let by_channel: std::collections::HashMap<ChannelId, &ParameterValue> =
            writes.iter().map(|(a, v)| (a.channel.clone(), v)).collect();
        assert!(matches!(
            by_channel.get(&ChannelId::Input(2)),
            Some(ParameterValue::Float(f)) if (*f - 1.0).abs() < 1e-3
        ));
    }
}
