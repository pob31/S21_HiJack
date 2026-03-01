use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Instant;

use tracing::{info, debug};
use uuid::Uuid;

use crate::model::monitor::MonitorClient;

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

    /// Update a client's last-seen timestamp and address (called on each received message).
    pub fn update_last_seen(&mut self, name: &str, addr: SocketAddr) {
        if let Some(client) = self.find_by_name_mut(name) {
            client.last_seen = Some(Instant::now());
            client.connected_addr = Some(addr);
            debug!(name, %addr, "Monitor client heartbeat");
        }
    }

    /// Mark clients as disconnected if they haven't been seen within the timeout.
    /// Clears `connected_addr` and `last_seen` for timed-out clients.
    pub fn mark_disconnected_clients(&mut self) -> Vec<String> {
        let mut disconnected = Vec::new();
        for client in self.clients.values_mut() {
            if client.connected_addr.is_some() && !client.is_connected() {
                info!(name = %client.name, "Monitor client timed out");
                client.connected_addr = None;
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
        mgr.update_last_seen("drummer", addr);

        let client = mgr.find_by_name("Drummer").unwrap();
        assert_eq!(client.connected_addr, Some(addr));
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
        mgr.update_last_seen("Active", addr);
        mgr.update_last_seen("Stale", addr);

        // Force the stale client to have an old timestamp
        if let Some(client) = mgr.find_by_name_mut("Stale") {
            client.last_seen = Some(Instant::now() - std::time::Duration::from_secs(60));
        }

        let disconnected = mgr.mark_disconnected_clients();
        assert_eq!(disconnected.len(), 1);
        assert_eq!(disconnected[0], "Stale");

        // Stale should be cleared
        let stale = mgr.find_by_name("Stale").unwrap();
        assert!(stale.connected_addr.is_none());
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
        mgr.update_last_seen("A", addr);
        assert_eq!(mgr.connected_count(), 1);

        mgr.update_last_seen("B", addr);
        assert_eq!(mgr.connected_count(), 2);
    }
}
