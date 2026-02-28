use serde::{Deserialize, Serialize};
use std::fmt;

/// Operating mode for the daemon.
///
/// - Mode 1: GP OSC only (default, always active).
/// - Mode 2: GP OSC + direct iPad protocol (spoofed handshake).
/// - Mode 3: GP OSC + full iPad proxy (bidirectional forwarding).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperatingMode {
    Mode1,
    Mode2,
    Mode3,
}

impl Default for OperatingMode {
    fn default() -> Self {
        Self::Mode1
    }
}

impl OperatingMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Mode1 => "Mode 1: GP OSC",
            Self::Mode2 => "Mode 2: Direct iPad",
            Self::Mode3 => "Mode 3: iPad Proxy",
        }
    }

    /// Whether this mode requires an iPad protocol connection.
    pub fn uses_ipad_protocol(&self) -> bool {
        matches!(self, Self::Mode2 | Self::Mode3)
    }
}

impl fmt::Display for OperatingMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Parse operating mode from CLI string.
impl OperatingMode {
    pub fn from_cli(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "mode1" | "1" => Some(Self::Mode1),
            "mode2" | "2" => Some(Self::Mode2),
            "mode3" | "3" => Some(Self::Mode3),
            _ => None,
        }
    }
}
