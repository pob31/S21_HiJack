use rosc::OscType;

use crate::model::parameter::{ParameterAddress, ParameterPath, ParameterValue};

/// System commands that can be sent to the console.
pub enum SystemCommand {
    ChannelCounts,
    Resend,
    Ping,
    Pong,
    SnapshotFire(i32),
    SnapshotNext,
    SnapshotPrevious,
}

impl SystemCommand {
    /// Get the OSC path for this system command.
    pub fn path(&self) -> &str {
        match self {
            SystemCommand::ChannelCounts => "/console/channel/counts",
            SystemCommand::Resend => "/console/resend",
            SystemCommand::Ping => "/console/ping",
            SystemCommand::Pong => "/console/pong",
            SystemCommand::SnapshotFire(_) => "/digico/snapshots/fire",
            SystemCommand::SnapshotNext => "/digico/snapshots/fire/next",
            SystemCommand::SnapshotPrevious => "/digico/snapshots/fire/previous",
        }
    }

    /// Get the OSC arguments for this system command.
    pub fn args(&self) -> Vec<OscType> {
        match self {
            SystemCommand::SnapshotFire(n) => vec![OscType::Int(*n)],
            _ => vec![],
        }
    }
}

/// Encode a parameter address and value into a GP OSC path and args.
/// Returns None for iPad-only parameters.
pub fn encode_parameter(addr: &ParameterAddress, value: &ParameterValue) -> Option<(String, Vec<OscType>)> {
    let ch_num = addr.channel.to_gp_osc_number()?;
    let suffix = addr.parameter.to_gp_osc_suffix()?;
    let path = format!("/channel/{ch_num}/{suffix}");
    let args = vec![value_to_osc_type(&addr.parameter, value)];
    Some((path, args))
}

/// Convert a ParameterValue to the appropriate OscType based on the parameter.
fn value_to_osc_type(_parameter: &ParameterPath, value: &ParameterValue) -> OscType {
    match value {
        ParameterValue::Float(f) => OscType::Float(*f),
        ParameterValue::Int(i) => OscType::Int(*i),
        ParameterValue::Bool(b) => {
            // DiGiCo uses int 0/1 for booleans over OSC
            OscType::Int(if *b { 1 } else { 0 })
        }
        ParameterValue::String(s) => OscType::String(s.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::channel::ChannelId;

    #[test]
    fn encode_fader() {
        let addr = ParameterAddress {
            channel: ChannelId::Input(1),
            parameter: ParameterPath::Fader,
        };
        let (path, args) = encode_parameter(&addr, &ParameterValue::Float(-10.0)).unwrap();
        assert_eq!(path, "/channel/1/fader");
        assert_eq!(args, vec![OscType::Float(-10.0)]);
    }

    #[test]
    fn encode_aux_mute() {
        let addr = ParameterAddress {
            channel: ChannelId::Aux(1),
            parameter: ParameterPath::Mute,
        };
        let (path, args) = encode_parameter(&addr, &ParameterValue::Bool(true)).unwrap();
        assert_eq!(path, "/channel/70/mute");
        assert_eq!(args, vec![OscType::Int(1)]);
    }

    #[test]
    fn encode_ipad_only_returns_none() {
        let addr = ParameterAddress {
            channel: ChannelId::GraphicEq(1),
            parameter: ParameterPath::GeqBandGain(1),
        };
        assert!(encode_parameter(&addr, &ParameterValue::Float(3.0)).is_none());
    }

    #[test]
    fn system_command_paths() {
        assert_eq!(SystemCommand::Ping.path(), "/console/ping");
        assert_eq!(SystemCommand::Resend.path(), "/console/resend");
        assert_eq!(
            SystemCommand::ChannelCounts.path(),
            "/console/channel/counts"
        );
    }
}
