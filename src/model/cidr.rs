//! Tiny IPv4 CIDR parser + matcher.
//!
//! Closed-LAN deployments (see audit C2 / H5) want to limit the monitor
//! and trigger UDP listeners to packets from a known operator-controlled
//! subnet — typically the IEM or show-control VLAN. This avoids pulling
//! in an `ipnetwork` / `ipnet` dependency for what's a few dozen lines.
//!
//! IPv6 is intentionally not supported — closed VLANs in this domain are
//! IPv4 in practice. If that changes, switch to a real crate.
//!
//! # Format
//!
//! `<a>.<b>.<c>.<d>/<prefix>` where `0 <= prefix <= 32`. Bare addresses
//! (no `/prefix`) are accepted as `/32` (single host).

use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv4Cidr {
    network: u32,
    prefix: u8,
}

impl Ipv4Cidr {
    /// Whether `addr` falls inside this CIDR. IPv6 addresses always return
    /// false (we only match v4).
    pub fn contains(&self, addr: IpAddr) -> bool {
        match addr {
            IpAddr::V4(v4) => self.contains_v4(v4),
            IpAddr::V6(_) => false,
        }
    }

    fn contains_v4(&self, addr: Ipv4Addr) -> bool {
        let mask = mask_from_prefix(self.prefix);
        (u32::from(addr) & mask) == self.network
    }
}

impl FromStr for Ipv4Cidr {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (addr_str, prefix) = match s.split_once('/') {
            Some((a, p)) => {
                let prefix: u8 = p
                    .parse()
                    .map_err(|_| format!("invalid CIDR prefix in {s:?}"))?;
                if prefix > 32 {
                    return Err(format!("CIDR prefix > 32 in {s:?}"));
                }
                (a, prefix)
            }
            None => (s, 32u8),
        };
        let addr: Ipv4Addr = addr_str
            .parse()
            .map_err(|_| format!("invalid IPv4 address in {s:?}"))?;
        let mask = mask_from_prefix(prefix);
        Ok(Self {
            network: u32::from(addr) & mask,
            prefix,
        })
    }
}

fn mask_from_prefix(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

/// Whether `addr` is permitted by `allowlist`. An empty allowlist is treated
/// as "accept everything" — the safe default that preserves current daemon
/// behavior when no CIDRs are configured.
pub fn ip_allowed(addr: IpAddr, allowlist: &[Ipv4Cidr]) -> bool {
    if allowlist.is_empty() {
        return true;
    }
    allowlist.iter().any(|c| c.contains(addr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn parse(s: &str) -> Ipv4Cidr {
        s.parse().unwrap()
    }

    #[test]
    fn parse_basic() {
        let c = parse("192.168.1.0/24");
        assert_eq!(c.prefix, 24);
        assert!(c.contains(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5))));
        assert!(c.contains(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 255))));
        assert!(!c.contains(IpAddr::V4(Ipv4Addr::new(192, 168, 2, 1))));
    }

    #[test]
    fn parse_host_no_prefix_is_slash_32() {
        let c = parse("10.0.0.1");
        assert_eq!(c.prefix, 32);
        assert!(c.contains(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(!c.contains(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))));
    }

    #[test]
    fn parse_normalizes_network() {
        // 192.168.1.5/24 should match the same hosts as 192.168.1.0/24
        // because the host bits are zeroed during construction.
        let c = parse("192.168.1.5/24");
        assert!(c.contains(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 0))));
        assert!(c.contains(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 200))));
        assert!(!c.contains(IpAddr::V4(Ipv4Addr::new(192, 168, 2, 5))));
    }

    #[test]
    fn parse_slash_zero_matches_everything() {
        let c = parse("0.0.0.0/0");
        assert!(c.contains(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(c.contains(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
    }

    #[test]
    fn parse_slash_32_is_single_host() {
        let c = parse("10.0.0.1/32");
        assert!(c.contains(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(!c.contains(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))));
    }

    #[test]
    fn parse_invalid_returns_err() {
        assert!("not-an-ip".parse::<Ipv4Cidr>().is_err());
        assert!("192.168.1.1/33".parse::<Ipv4Cidr>().is_err());
        assert!("192.168.1.1/abc".parse::<Ipv4Cidr>().is_err());
        assert!("192.168.1/24".parse::<Ipv4Cidr>().is_err());
    }

    #[test]
    fn ipv6_never_matches() {
        let c = parse("0.0.0.0/0");
        let v6: IpAddr = "::1".parse().unwrap();
        assert!(!c.contains(v6));
    }

    #[test]
    fn ip_allowed_empty_allowlist_accepts_all() {
        let allowlist: Vec<Ipv4Cidr> = vec![];
        let v4: IpAddr = "8.8.8.8".parse().unwrap();
        assert!(ip_allowed(v4, &allowlist));
    }

    #[test]
    fn ip_allowed_filters_by_membership() {
        let allowlist = vec![parse("192.168.10.0/24"), parse("10.0.0.5/32")];
        let inside: IpAddr = "192.168.10.42".parse().unwrap();
        let outside: IpAddr = "192.168.11.1".parse().unwrap();
        let host: IpAddr = "10.0.0.5".parse().unwrap();
        let other_host: IpAddr = "10.0.0.6".parse().unwrap();
        assert!(ip_allowed(inside, &allowlist));
        assert!(!ip_allowed(outside, &allowlist));
        assert!(ip_allowed(host, &allowlist));
        assert!(!ip_allowed(other_host, &allowlist));
    }
}
