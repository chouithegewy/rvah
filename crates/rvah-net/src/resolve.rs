use std::net::IpAddr;

use crate::NetError;

/// Host-name resolution used by [`crate::PermissionedClient`] to vet and pin
/// addresses before the transport connects (SDD §9: DNS rebinding).
pub trait Resolve: Send + Sync {
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, NetError>;
}

/// The system resolver, via `std::net::ToSocketAddrs`.
#[derive(Debug, Default)]
pub struct SystemResolver;

impl Resolve for SystemResolver {
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, NetError> {
        use std::net::ToSocketAddrs;
        let addrs: Vec<IpAddr> = (host, 0)
            .to_socket_addrs()
            .map_err(|e| NetError::ResolveFailed(format!("{host}: {e}")))?
            .map(|sa| sa.ip())
            .collect();
        if addrs.is_empty() {
            return Err(NetError::ResolveFailed(host.to_string()));
        }
        Ok(addrs)
    }
}

/// True for addresses an internet host name has no business resolving to:
/// loopback, RFC-1918, link-local, CGNAT, documentation, unspecified,
/// broadcast, multicast, IPv6 unique-local, and IPv4-mapped disguises.
pub fn is_non_global(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                // Carrier-grade NAT, 100.64.0.0/10.
                || (octets[0] == 100 && (octets[1] & 0xc0) == 64)
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_non_global(IpAddr::V4(mapped));
            }
            let head = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Unique-local fc00::/7 and link-local fe80::/10.
                || (head & 0xfe00) == 0xfc00
                || (head & 0xffc0) == 0xfe80
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_resolver_maps_localhost_to_loopback() {
        let addrs = SystemResolver.resolve("localhost").unwrap();
        assert!(!addrs.is_empty());
        assert!(addrs.iter().all(|a| a.is_loopback()));
    }

    #[test]
    fn global_addresses_pass_the_policy() {
        for good in ["93.184.216.34", "2606:2800:220:1:248:1893:25c8:1946"] {
            assert!(!is_non_global(good.parse().unwrap()), "{good}");
        }
    }
}
