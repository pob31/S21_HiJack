use std::net::SocketAddr;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Connection timeout: a client is considered disconnected after this duration of silence.
const CONNECTION_TIMEOUT_SECS: u64 = 30;

/// A monitoring client profile: a musician who can control specific sends from a tablet.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MonitorClient {
    pub id: Uuid,
    /// Display name used in OSC paths (e.g., "Drummer", "Keys").
    pub name: String,
    /// Aux numbers this client may control (1-based).
    pub permitted_auxes: Vec<u8>,
    /// Input numbers visible to this client (1-based). Empty = all inputs.
    pub visible_inputs: Vec<u8>,

    /// Current UDP address of the connected client (runtime only).
    #[serde(skip)]
    pub connected_addr: Option<SocketAddr>,
    /// Timestamp of the last received message (runtime only).
    #[serde(skip)]
    pub last_seen: Option<Instant>,
}

impl MonitorClient {
    /// Create a new client profile with a generated UUID.
    pub fn new(name: String, permitted_auxes: Vec<u8>, visible_inputs: Vec<u8>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            permitted_auxes,
            visible_inputs,
            connected_addr: None,
            last_seen: None,
        }
    }

    /// Check whether this client is allowed to control a given send.
    /// Returns true if the aux is in `permitted_auxes` AND the input is
    /// in `visible_inputs` (or `visible_inputs` is empty, meaning all).
    pub fn is_permitted(&self, input_ch: u8, aux_ch: u8) -> bool {
        let aux_ok = self.permitted_auxes.contains(&aux_ch);
        let input_ok = self.visible_inputs.is_empty() || self.visible_inputs.contains(&input_ch);
        aux_ok && input_ok
    }

    /// Whether the client is currently connected (has been seen within the timeout window).
    pub fn is_connected(&self) -> bool {
        self.last_seen
            .map(|t| t.elapsed().as_secs() < CONNECTION_TIMEOUT_SECS)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation() {
        let client = MonitorClient::new("Drummer".into(), vec![1, 2], vec![1, 2, 3]);
        assert_eq!(client.name, "Drummer");
        assert_eq!(client.permitted_auxes, vec![1, 2]);
        assert_eq!(client.visible_inputs, vec![1, 2, 3]);
        assert!(client.connected_addr.is_none());
        assert!(client.last_seen.is_none());
        assert!(!client.is_connected());
    }

    #[test]
    fn is_permitted_checks_both_aux_and_input() {
        let client = MonitorClient::new("Keys".into(), vec![1, 3], vec![5, 10]);

        // Permitted: aux 1, input 5
        assert!(client.is_permitted(5, 1));
        // Permitted: aux 3, input 10
        assert!(client.is_permitted(10, 3));
        // Denied: aux 2 not permitted
        assert!(!client.is_permitted(5, 2));
        // Denied: input 6 not visible
        assert!(!client.is_permitted(6, 1));
        // Denied: both wrong
        assert!(!client.is_permitted(6, 2));
    }

    #[test]
    fn empty_visible_inputs_means_all() {
        let client = MonitorClient::new("FOH".into(), vec![1], vec![]);
        // Any input should be permitted when visible_inputs is empty
        assert!(client.is_permitted(1, 1));
        assert!(client.is_permitted(60, 1));
        // But aux still must match
        assert!(!client.is_permitted(1, 2));
    }

    #[test]
    fn serde_round_trip() {
        let original = MonitorClient::new("Bass".into(), vec![2, 4], vec![1, 2]);
        let json = serde_json::to_string(&original).unwrap();
        let loaded: MonitorClient = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.id, original.id);
        assert_eq!(loaded.name, "Bass");
        assert_eq!(loaded.permitted_auxes, vec![2, 4]);
        assert_eq!(loaded.visible_inputs, vec![1, 2]);
        // Skipped fields should be None after deserialization
        assert!(loaded.connected_addr.is_none());
        assert!(loaded.last_seen.is_none());
    }

    #[test]
    fn is_connected_respects_timeout() {
        let mut client = MonitorClient::new("Test".into(), vec![1], vec![]);

        // Not connected initially
        assert!(!client.is_connected());

        // Mark as seen now — should be connected
        client.last_seen = Some(Instant::now());
        assert!(client.is_connected());
    }
}
