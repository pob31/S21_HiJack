use std::net::{Ipv4Addr, SocketAddr};
#[cfg(target_os = "macos")]
use std::os::fd::AsRawFd;

use tracing::{info, warn};

/// Target SO_RCVBUF/SO_SNDBUF for the high-volume Mode 3 proxy sockets.
/// The OS may clamp this (Linux: net.core.rmem_max / wmem_max); the actual size
/// achieved is logged so a clamp is visible.
pub const PROXY_SOCKET_BUFFER_BYTES: usize = 4 * 1024 * 1024; // 4 MB

/// A discovered network interface with its name and IPv4 address.
#[derive(Clone, Debug)]
pub struct NetInterface {
    pub name: String,
    pub ip: Ipv4Addr,
}

impl NetInterface {
    pub fn label(&self) -> String {
        format!("{} ({})", self.name, self.ip)
    }
}

/// Enumerate all active IPv4 network interfaces.
/// Filters out loopback (127.x.x.x) and link-local (169.254.x.x).
#[cfg(unix)]
pub fn list_interfaces() -> Vec<NetInterface> {
    let mut result = Vec::new();

    unsafe {
        let mut ifaddrs: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifaddrs) != 0 {
            return result;
        }

        let mut current = ifaddrs;
        while !current.is_null() {
            let ifa = &*current;
            if !ifa.ifa_addr.is_null() {
                let family = (*ifa.ifa_addr).sa_family as i32;
                if family == libc::AF_INET {
                    let addr = ifa.ifa_addr as *const libc::sockaddr_in;
                    let ip = Ipv4Addr::from(u32::from_be((*addr).sin_addr.s_addr));
                    let name = std::ffi::CStr::from_ptr(ifa.ifa_name)
                        .to_string_lossy()
                        .to_string();

                    // Skip loopback and link-local
                    if !ip.is_loopback() && !ip.is_link_local() {
                        result.push(NetInterface { name, ip });
                    }
                }
            }
            current = ifa.ifa_next;
        }
        libc::freeifaddrs(ifaddrs);
    }

    // Sort by interface name
    result.sort_by(|a, b| a.name.cmp(&b.name));
    result
}

/// Windows fallback: discover the primary outbound IPv4 by opening a UDP
/// socket and "connecting" it to a public address (no packets are sent).
/// This is a simplified implementation — it returns a single entry rather
/// than enumerating every adapter the way `getifaddrs` does on Unix, but it
/// is enough to keep the dev build running. If finer control is needed on
/// Windows, swap in `GetAdaptersAddresses` via `windows-sys`.
#[cfg(windows)]
pub fn list_interfaces() -> Vec<NetInterface> {
    let mut result = Vec::new();

    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        // 8.8.8.8 is just a routing hint — no traffic is sent.
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(local) = socket.local_addr() {
                if let std::net::IpAddr::V4(ip) = local.ip() {
                    if !ip.is_loopback() && !ip.is_link_local() {
                        result.push(NetInterface {
                            name: "primary".to_string(),
                            ip,
                        });
                    }
                }
            }
        }
    }

    result
}

/// Find the interface name for a given IP address.
pub fn interface_for_ip(ip: &str) -> Option<String> {
    list_interfaces()
        .into_iter()
        .find(|i| i.ip.to_string() == ip)
        .map(|i| i.name)
}

/// Create a tokio UDP socket bound to `bind_addr`, optionally pinned to a specific
/// network interface. The interface binding is set BEFORE bind() to ensure the very
/// first packet goes out the correct interface.
///
/// On macOS: uses `IP_BOUND_IF` with the interface index.
/// On Linux: uses `SO_BINDTODEVICE` with the interface name.
pub async fn create_bound_udp_socket(
    bind_addr: SocketAddr,
    interface_name: Option<&str>,
) -> std::io::Result<tokio::net::UdpSocket> {
    create_bound_udp_socket_inner(bind_addr, interface_name, None).await
}

/// Like `create_bound_udp_socket` but requests an enlarged SO_RCVBUF/SO_SNDBUF
/// (`buffer_bytes`). Used by the Mode 3 proxy, where the console's initial flood
/// can overflow the default OS receive buffer and silently drop datagrams
/// (matrix/bus route responses going missing on the first connection).
pub async fn create_bound_udp_socket_buffered(
    bind_addr: SocketAddr,
    interface_name: Option<&str>,
    buffer_bytes: usize,
) -> std::io::Result<tokio::net::UdpSocket> {
    create_bound_udp_socket_inner(bind_addr, interface_name, Some(buffer_bytes)).await
}

async fn create_bound_udp_socket_inner(
    bind_addr: SocketAddr,
    interface_name: Option<&str>,
    buffer_bytes: Option<usize>,
) -> std::io::Result<tokio::net::UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

    // Set interface binding BEFORE bind
    if let Some(iface) = interface_name {
        set_interface_binding(&socket, iface)?;
    }

    // Enlarge the kernel socket buffers BEFORE bind so they apply from the very
    // first datagram. The OS may clamp the request (Linux caps at
    // net.core.rmem_max / wmem_max and reports ~2x the request internally); we
    // read back the actual sizes and log them so an operator can see when a
    // clamp leaves them too small. set_* failures are non-fatal — the socket
    // still works at the default size, just more prone to drops under a flood.
    if let Some(target) = buffer_bytes {
        if let Err(e) = socket.set_recv_buffer_size(target) {
            warn!(target, "Failed to set SO_RCVBUF: {e}");
        }
        if let Err(e) = socket.set_send_buffer_size(target) {
            warn!(target, "Failed to set SO_SNDBUF: {e}");
        }
        let actual_rcv = socket.recv_buffer_size().unwrap_or(0);
        let actual_snd = socket.send_buffer_size().unwrap_or(0);
        info!(
            requested = target,
            actual_rcvbuf = actual_rcv,
            actual_sndbuf = actual_snd,
            "Proxy socket buffer sizes (raise net.core.rmem_max/wmem_max if actual << requested)"
        );
        if actual_rcv < target {
            warn!(
                requested = target,
                actual = actual_rcv,
                "SO_RCVBUF clamped by OS — raise sysctl net.core.rmem_max to reduce UDP drops under flood"
            );
        }
    }

    socket.set_reuse_address(true)?;
    socket.bind(&bind_addr.into())?;
    socket.set_nonblocking(true)?;

    let std_socket: std::net::UdpSocket = socket.into();
    let tokio_socket = tokio::net::UdpSocket::from_std(std_socket)?;

    if let Some(iface) = interface_name {
        info!(
            interface = iface,
            local_addr = %tokio_socket.local_addr()?,
            "Socket bound to interface {iface} (IP_BOUND_IF set before bind)"
        );
    }

    Ok(tokio_socket)
}

/// Set the platform-specific interface binding on a raw socket (before bind).
#[cfg(target_os = "macos")]
fn set_interface_binding(socket: &socket2::Socket, interface_name: &str) -> std::io::Result<()> {
    const IP_BOUND_IF: libc::c_int = 25;

    let ifname = std::ffi::CString::new(interface_name)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let ifindex = unsafe { libc::if_nametoindex(ifname.as_ptr()) };
    if ifindex == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Interface not found: {interface_name}"),
        ));
    }

    let ret = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_IP,
            IP_BOUND_IF,
            &ifindex as *const u32 as *const libc::c_void,
            std::mem::size_of::<u32>() as libc::socklen_t,
        )
    };

    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }

    info!(interface_name, ifindex, "IP_BOUND_IF set before bind");
    Ok(())
}

#[cfg(target_os = "linux")]
fn set_interface_binding(socket: &socket2::Socket, interface_name: &str) -> std::io::Result<()> {
    socket.bind_device(Some(interface_name.as_bytes()))?;
    info!(interface_name, "SO_BINDTODEVICE set before bind");
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn set_interface_binding(_socket: &socket2::Socket, interface_name: &str) -> std::io::Result<()> {
    tracing::warn!(
        interface_name,
        "Interface binding not supported on this platform"
    );
    Ok(())
}
