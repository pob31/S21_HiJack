use std::net::{Ipv4Addr, SocketAddr};
use std::os::fd::AsRawFd;

use tracing::info;

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
    use socket2::{Domain, Protocol, Socket, Type};

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

    // Set interface binding BEFORE bind
    if let Some(iface) = interface_name {
        set_interface_binding(&socket, iface)?;
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
    tracing::warn!(interface_name, "Interface binding not supported on this platform");
    Ok(())
}
