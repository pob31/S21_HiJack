use std::collections::HashMap;
use std::time::Instant;

use tracing::{debug, info};
use uuid::Uuid;

use crate::model::monitor::{ClientEndpoint, MonitorClient};

/// Manages monitoring client profiles — CRUD, connection tracking, and timeout.
pub struct MonitorManager {
    pub clients: HashMap<Uuid, MonitorClient>,
}

impl MonitorManager {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
        }
    }

    pub fn add_client(&mut self, client: MonitorClient) {
        info!(name = %client.name, id = %client.id, "Added monitor client");
        self.clients.insert(client.id, client);
    }

    pub fn remove_client(&mut self, id: Uuid) -> bool {
        let removed = self.clients.remove(&id).is_some();
        if removed {
            info!(%id, "Removed monitor client");
        }
        removed
    }

    /// Mutate an existing client's name, permitted auxes, and visible inputs.
    /// Returns `true` if a client with that id existed and was updated.
    /// Connection-tracking fields (`endpoint`, `last_seen`) are preserved.
    pub fn update_client(
        &mut self,
        id: Uuid,
        name: String,
        permitted_auxes: Vec<u8>,
        visible_inputs: Vec<u8>,
        pin: Option<String>,
    ) -> bool {
        if let Some(client) = self.clients.get_mut(&id) {
            info!(%id, name = %name, "Updated monitor client");
            client.name = name;
            client.permitted_auxes = permitted_auxes;
            client.visible_inputs = visible_inputs;
            client.pin = pin;
            true
        } else {
            false
        }
    }

    pub fn find_by_name(&self, name: &str) -> Option<&MonitorClient> {
        self.clients
            .values()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    pub fn find_by_name_mut(&mut self, name: &str) -> Option<&mut MonitorClient> {
        self.clients
            .values_mut()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// Return all clients sorted by name (for UI display).
    pub fn sorted_clients(&self) -> Vec<&MonitorClient> {
        let mut clients: Vec<_> = self.clients.values().collect();
        clients.sort_by(|a, b| a.name.cmp(&b.name));
        clients
    }

    /// Update a client's last-seen timestamp and endpoint (called on each received message).
    pub fn update_last_seen(&mut self, name: &str, endpoint: ClientEndpoint) {
        if let Some(client) = self.find_by_name_mut(name) {
            client.last_seen = Some(Instant::now());
            client.endpoint = Some(endpoint);
            debug!(name, ?endpoint, "Monitor client heartbeat");
        }
    }

    /// Mark clients as disconnected if they haven't been seen within the timeout.
    /// Clears `endpoint` and `last_seen` for timed-out clients.
    pub fn mark_disconnected_clients(&mut self) -> Vec<String> {
        let mut disconnected = Vec::new();
        for client in self.clients.values_mut() {
            if client.endpoint.is_some() && !client.is_connected() {
                info!(name = %client.name, "Monitor client timed out");
                client.endpoint = None;
                client.last_seen = None;
                disconnected.push(client.name.clone());
            }
        }
        disconnected
    }

    /// Count of currently connected clients.
    pub fn connected_count(&self) -> usize {
        self.clients.values().filter(|c| c.is_connected()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::monitor::MonitorClient;
    use std::net::SocketAddr;

    fn make_client(name: &str) -> MonitorClient {
        MonitorClient::new(name.to_string(), vec![1, 2], vec![])
    }

    #[test]
    fn add_and_remove() {
        let mut mgr = MonitorManager::new();
        let client = make_client("Drummer");
        let id = client.id;
        mgr.add_client(client);

        assert!(mgr.clients.contains_key(&id));
        assert!(mgr.remove_client(id));
        assert!(!mgr.clients.contains_key(&id));
        // Removing again returns false
        assert!(!mgr.remove_client(id));
    }

    #[test]
    fn update_client_preserves_connection_state() {
        let mut mgr = MonitorManager::new();
        let mut client = make_client("Keys");
        client.last_seen = Some(Instant::now());
        let id = client.id;
        let original_seen = client.last_seen;
        mgr.add_client(client);

        // Update auxes + inputs + name
        let updated = mgr.update_client(id, "Keys 2".into(), vec![3, 4, 5], vec![10, 11], None);
        assert!(updated);

        let c = mgr.clients.get(&id).unwrap();
        assert_eq!(c.name, "Keys 2");
        assert_eq!(c.permitted_auxes, vec![3, 4, 5]);
        assert_eq!(c.visible_inputs, vec![10, 11]);
        // The connection-tracking fields must survive an edit so a connected
        // musician doesn't appear to disconnect mid-show when their profile
        // is tweaked.
        assert_eq!(c.last_seen, original_seen);

        // Unknown id is a no-op
        assert!(!mgr.update_client(Uuid::new_v4(), "x".into(), vec![1], vec![], None));
    }

    #[test]
    fn find_by_name_case_insensitive() {
        let mut mgr = MonitorManager::new();
        mgr.add_client(make_client("Keys"));

        assert!(mgr.find_by_name("keys").is_some());
        assert!(mgr.find_by_name("KEYS").is_some());
        assert!(mgr.find_by_name("nonexistent").is_none());
    }

    #[test]
    fn sorted_clients() {
        let mut mgr = MonitorManager::new();
        mgr.add_client(make_client("Zebra"));
        mgr.add_client(make_client("Alpha"));
        mgr.add_client(make_client("Middle"));

        let sorted = mgr.sorted_clients();
        assert_eq!(sorted[0].name, "Alpha");
        assert_eq!(sorted[1].name, "Middle");
        assert_eq!(sorted[2].name, "Zebra");
    }

    #[test]
    fn update_last_seen() {
        let mut mgr = MonitorManager::new();
        mgr.add_client(make_client("Drummer"));

        let addr: SocketAddr = "192.168.1.100:9000".parse().unwrap();
        mgr.update_last_seen("drummer", ClientEndpoint::Udp(addr));

        let client = mgr.find_by_name("Drummer").unwrap();
        assert_eq!(client.endpoint, Some(ClientEndpoint::Udp(addr)));
        assert!(client.last_seen.is_some());
        assert!(client.is_connected());
    }

    #[test]
    fn mark_disconnected_clients() {
        let mut mgr = MonitorManager::new();
        mgr.add_client(make_client("Active"));
        mgr.add_client(make_client("Stale"));

        let addr: SocketAddr = "192.168.1.100:9000".parse().unwrap();

        // Mark both as seen
        mgr.update_last_seen("Active", ClientEndpoint::Udp(addr));
        mgr.update_last_seen("Stale", ClientEndpoint::Udp(addr));

        // Force the stale client to have an old timestamp
        if let Some(client) = mgr.find_by_name_mut("Stale") {
            client.last_seen = Some(Instant::now() - std::time::Duration::from_secs(60));
        }

        let disconnected = mgr.mark_disconnected_clients();
        assert_eq!(disconnected.len(), 1);
        assert_eq!(disconnected[0], "Stale");

        // Stale should be cleared
        let stale = mgr.find_by_name("Stale").unwrap();
        assert!(stale.endpoint.is_none());
        assert!(!stale.is_connected());

        // Active should still be connected
        let active = mgr.find_by_name("Active").unwrap();
        assert!(active.is_connected());
    }

    #[test]
    fn connected_count() {
        let mut mgr = MonitorManager::new();
        mgr.add_client(make_client("A"));
        mgr.add_client(make_client("B"));

        assert_eq!(mgr.connected_count(), 0);

        let addr: SocketAddr = "192.168.1.100:9000".parse().unwrap();
        mgr.update_last_seen("A", ClientEndpoint::Udp(addr));
        assert_eq!(mgr.connected_count(), 1);

        mgr.update_last_seen("B", ClientEndpoint::Udp(addr));
        assert_eq!(mgr.connected_count(), 2);
    }
}
