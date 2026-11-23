use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalBindAddress {
    pub ip: IpAddr,
    pub interface: Option<String>,
}

fn is_bindable_local_ip(ip: IpAddr) -> bool {
    if ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    match ip {
        IpAddr::V4(_) => true,
        IpAddr::V6(v6) => {
            // Skip IPv6 link-local (fe80::/10) because SocketAddrV6 with scope_id=0 cannot bind.
            (v6.segments()[0] & 0xffc0) != 0xfe80
        }
    }
}

#[cfg(unix)]
fn collect_interface_ips() -> Vec<LocalBindAddress> {
    use std::ffi::CStr;

    let mut out = Vec::new();
    let mut addrs: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `getifaddrs` allocates a linked list of `ifaddrs` entries freed via `freeifaddrs` below.
    if unsafe { libc::getifaddrs(&mut addrs) } != 0 || addrs.is_null() {
        return out;
    }
    struct Guard(*mut libc::ifaddrs);
    impl Drop for Guard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` was allocated by a successful `getifaddrs` call.
                unsafe { libc::freeifaddrs(self.0) };
            }
        }
    }
    let _guard = Guard(addrs);
    let mut cur = addrs;
    while !cur.is_null() {
        // SAFETY: `cur` points to a valid `ifaddrs` node in the list returned by `getifaddrs`.
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        if ifa.ifa_addr.is_null() || (ifa.ifa_flags & (libc::IFF_UP as u32)) == 0 {
            continue;
        }
        // SAFETY: `ifa.ifa_addr` was checked non-null; inspect its `sa_family` before casting.
        let family = unsafe { (*ifa.ifa_addr).sa_family as i32 };
        let ip = if family == libc::AF_INET {
            // SAFETY: `sa_family == AF_INET` guarantees `ifa_addr` points to `sockaddr_in`.
            let sin = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
            IpAddr::V4(Ipv4Addr::from(sin.sin_addr.s_addr.to_ne_bytes()))
        } else if family == libc::AF_INET6 {
            // SAFETY: `sa_family == AF_INET6` guarantees `ifa_addr` points to `sockaddr_in6`.
            let sin6 = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in6) };
            IpAddr::V6(Ipv6Addr::from(sin6.sin6_addr.s6_addr))
        } else {
            continue;
        };
        if !is_bindable_local_ip(ip) {
            continue;
        }
        let iface = if ifa.ifa_name.is_null() {
            None
        } else {
            // SAFETY: `ifa_name` is a NUL-terminated interface name when non-null.
            let name = unsafe { CStr::from_ptr(ifa.ifa_name) }
                .to_string_lossy()
                .trim()
                .to_owned();
            (!name.is_empty()).then_some(name)
        };
        out.push(LocalBindAddress {
            ip,
            interface: iface,
        });
    }
    out
}

#[cfg(not(unix))]
fn collect_interface_ips() -> Vec<LocalBindAddress> {
    let mut out = Vec::new();
    if let Ok(sock) = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        && sock.connect("192.0.2.1:80").is_ok()
        && let Ok(local) = sock.local_addr()
    {
        let ip = local.ip();
        if is_bindable_local_ip(ip) && !ip.is_loopback() {
            out.push(LocalBindAddress {
                ip,
                interface: None,
            });
        }
    }
    out
}

/// Lists local bindable IP addresses for proxy listeners (excluding IPv6 link-local addresses).
pub fn list_local_bind_addresses() -> Vec<LocalBindAddress> {
    let raw = collect_interface_ips();
    let mut seen = BTreeSet::new();
    let mut non_loopback_v4 = Vec::new();
    let mut non_loopback_v6 = Vec::new();

    for entry in raw {
        if entry.ip.is_loopback() || !is_bindable_local_ip(entry.ip) || !seen.insert(entry.ip) {
            continue;
        }
        match entry.ip {
            IpAddr::V4(_) => non_loopback_v4.push(entry),
            IpAddr::V6(_) => non_loopback_v6.push(entry),
        }
    }

    non_loopback_v4.sort_by(|a, b| {
        a.interface
            .cmp(&b.interface)
            .then_with(|| a.ip.to_string().cmp(&b.ip.to_string()))
    });
    non_loopback_v6.sort_by(|a, b| {
        a.interface
            .cmp(&b.interface)
            .then_with(|| a.ip.to_string().cmp(&b.ip.to_string()))
    });

    let mut result = Vec::with_capacity(non_loopback_v4.len() + non_loopback_v6.len() + 3);
    result.extend(non_loopback_v4);
    result.extend(non_loopback_v6);

    for (ip, label) in [
        (IpAddr::V4(Ipv4Addr::LOCALHOST), "loopback"),
        (IpAddr::V6(Ipv6Addr::LOCALHOST), "loopback"),
        (IpAddr::V6(Ipv6Addr::UNSPECIFIED), "all IPv6"),
    ] {
        if seen.insert(ip) {
            result.push(LocalBindAddress {
                ip,
                interface: Some(label.to_owned()),
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn list_local_bind_addresses_includes_loopback_all_v6_is_unique_and_skips_link_local() {
        let entries = list_local_bind_addresses();
        let mut seen = HashSet::new();
        for entry in &entries {
            assert!(seen.insert(entry.ip), "duplicate IP emitted: {}", entry.ip);
            if let IpAddr::V6(v6) = entry.ip {
                assert_ne!(
                    v6.segments()[0] & 0xffc0,
                    0xfe80,
                    "IPv6 link-local address must be filtered: {v6}"
                );
            }
        }
        assert!(seen.contains(&IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(seen.contains(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(seen.contains(&IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
    }
}
