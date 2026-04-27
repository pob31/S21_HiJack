//! Pan Link engine: propagates input-channel main pan changes to the
//! linked stereo aux send pans configured in `PanLinkBindings`.
//!
//! Triggered from the OSC inbound dispatch in `connection::process_message`
//! after the gang engine has had its turn. Skipped entirely while the
//! snapshot engine's dirty-suppression flag is active so memory recalls
//! (snapshots, cues, macros) are never modified by pan link.

use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::model::channel::ChannelId;
use crate::model::config::ChannelMode;
use crate::model::dirty_tracker::DirtyTracker;
use crate::model::pan_link::PanLinkBindings;
use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};
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
}

impl PanLinkEngine {
    pub fn new(
        state: Arc<RwLock<ConsoleState>>,
        sender: OscSender,
        bindings: Arc<RwLock<PanLinkBindings>>,
        dirty_tracker: Arc<RwLock<DirtyTracker>>,
    ) -> Self {
        Self {
            state,
            sender,
            ipad_sender: None,
            bindings,
            dirty_tracker,
        }
    }

    pub fn set_ipad_sender(&mut self, sender: Option<IpadSender>) {
        self.ipad_sender = sender;
    }

    /// Process an inbound parameter update. Only main pan on input
    /// channels is interesting; everything else is a no-op.
    pub async fn process_pan_update(&self, addr: &ParameterAddress, new_value: &ParameterValue) {
        // Only react to main pan on input channels.
        let ChannelId::Input(input_n) = addr.channel else {
            return;
        };
        if addr.parameter != ParameterPath::Pan {
            return;
        }

        // Recall guard: never propagate while a memory recall is in flight.
        if self.dirty_tracker.read().await.is_suppressed() {
            debug!(input = input_n, "PanLink: skipped during recall");
            return;
        }

        // Snapshot the active aux list for this input.
        let auxes: Vec<u8> = {
            let b = self.bindings.read().await;
            b.auxes_for(input_n)
        };
        if auxes.is_empty() {
            return;
        }

        // Filter to stereo aux buses that the input is currently sending to.
        let state = self.state.read().await;
        let mix_modes = state.config.mix_output_modes.clone();
        let mix_types = state.config.mix_output_types.clone();
        for aux in auxes {
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
            // Verify the input is currently sending to this aux. If the
            // SendEnabled state is unknown, default to allowing the push
            // — better to over-send than to silently drop.
            let send_enabled_addr = ParameterAddress {
                channel: ChannelId::Input(input_n),
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
                channel: ChannelId::Input(input_n),
                parameter: ParameterPath::SendPan(aux),
            };
            self.send_to_console(&target, new_value).await;
        }
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
