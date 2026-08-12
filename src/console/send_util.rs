//! Shared console write path.
//!
//! Sending a parameter to the desk is a two-protocol affair: GP OSC covers
//! the documented command set, and anything GP OSC can't express (analog
//! gain, phantom, GEQ, …) falls back to the reverse-engineered iPad
//! protocol. Engines should route writes through here rather than
//! re-implementing the fallback.

use crate::model::parameter::{ParameterAddress, ParameterValue};
use crate::osc::client::OscSender;
use crate::osc::encode;
use crate::osc::ipad_client::IpadSender;
use crate::osc::ipad_encode;

/// Send a parameter via GP OSC, falling back to the iPad protocol.
///
/// Returns `true` if a message was handed to a sender successfully.
pub async fn send_parameter(
    sender: &OscSender,
    ipad_sender: &Option<IpadSender>,
    addr: &ParameterAddress,
    value: &ParameterValue,
) -> bool {
    match encode::encode_parameter(addr, value) {
        Some((path, args)) => sender.send(&path, args).await.is_ok(),
        None => {
            if let Some(ipad) = ipad_sender {
                match ipad_encode::encode_ipad_parameter(addr, value) {
                    Some((path, args)) => ipad.send(&path, args).await.is_ok(),
                    None => false,
                }
            } else {
                false
            }
        }
    }
}
