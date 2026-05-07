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

        // Snapshot the active aux list for the moved input.
        let auxes: Vec<u8> = {
            let b = self.bindings.read().await;
            b.auxes_for(input_n)
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
                        targets.push((*sib_n, p));
                    }
                    // If the sibling's main pan isn't in the state
                    // mirror yet, skip — we don't want to invent a
                    // value for an aux send that may not match the
                    // unknown main pan.
                }
            }
        }

        let mut writes: Vec<(ParameterAddress, ParameterValue)> = Vec::new();
        for (ch_n, pan_value) in targets {
            for &aux in &auxes {
                let idx0 = aux.checked_sub(1).map(|i| i as usize);
                let is_aux_bus = idx0.and_then(|i| mix_types.get(i)).copied().unwrap_or(true);
                if !is_aux_bus {
                    continue;
                }
                let is_stereo = idx0
                    .and_then(|i| mix_modes.get(i))
                    .map(|m| *m == ChannelMode::Stereo)
                    .unwrap_or(false);
                if !is_stereo {
                    continue;
                }
                // Verify this channel is currently sending to this aux.
                // If the SendEnabled state is unknown, default to
                // allowing the push — better to over-send than to
                // silently drop.
                let send_enabled_addr = ParameterAddress {
                    channel: ChannelId::Input(ch_n),
                    parameter: ParameterPath::SendEnabled(aux),
                };
                let sending = match state.get(&send_enabled_addr) {
                    Some(ParameterValue::Bool(b)) => *b,
                    Some(ParameterValue::Int(i)) => *i != 0,
                    Some(ParameterValue::Float(f)) => *f != 0.0,
                    _ => true,
                };
                if !sending {
                    continue;
                }

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
    async fn skips_when_aux_is_mono() {
        let engine = make_engine();
        // Aux 5 → mono.
        engine.state.write().await.config.mix_output_modes[4] = ChannelMode::Mono;
        engine.bindings.write().await.set_active(1, 5, true);

        let writes = engine
            .compute_pan_writes(&pan_addr(1), &ParameterValue::Float(0.5))
            .await;
        assert!(writes.is_empty());
    }

    #[tokio::test]
    async fn skips_when_send_disabled() {
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
        assert!(writes.is_empty());
    }
}
