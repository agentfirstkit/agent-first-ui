use std::io;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};

// Probe the IPv4 all-hosts multicast route. VPN and overlay interfaces commonly
// capture the default unicast route, but they do not normally own the local
// multicast route used to reach another device on the physical LAN.
const LAN_ROUTE_PROBE_IPV4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 1);

/// Pick the address another device on the same physical LAN can reach this
/// machine on.
///
/// [`crate::UiDeliveryPlan::deliver`]'s `link` branch calls this to advertise
/// a `link` session, but the function itself carries no session or delivery
/// knowledge — it is exactly the same probe a Provider that needs a
/// LAN-reachable address for a reason of its own (a second listener the same
/// process binds, say, independent of any session) would otherwise have to
/// copy. Public, and available without the `session` feature, so it does not
/// have to be copied.
pub fn primary_lan_ipv4() -> io::Result<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.connect((LAN_ROUTE_PROBE_IPV4, 9))?;
    let address = socket.local_addr()?;
    match address.ip() {
        IpAddr::V4(ip) if usable_lan_ipv4(ip) => Ok(ip),
        _ => Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "the local multicast route has no usable IPv4 source address",
        )),
    }
}

fn usable_lan_ipv4(ip: Ipv4Addr) -> bool {
    !ip.is_loopback()
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && !ip.is_link_local()
}

#[cfg(test)]
mod tests {
    use super::{LAN_ROUTE_PROBE_IPV4, usable_lan_ipv4};
    use std::net::Ipv4Addr;

    #[test]
    fn lan_route_probe_uses_link_local_multicast_instead_of_a_unicast_default_route() {
        assert!(LAN_ROUTE_PROBE_IPV4.is_multicast());
        assert_eq!(LAN_ROUTE_PROBE_IPV4.octets()[0], 224);
    }

    #[test]
    fn lan_address_rejects_non_advertisable_ipv4_classes() {
        for ip in [
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::LOCALHOST,
            Ipv4Addr::BROADCAST,
            Ipv4Addr::new(169, 254, 1, 2),
            Ipv4Addr::new(224, 0, 0, 1),
        ] {
            assert!(!usable_lan_ipv4(ip), "{ip}");
        }
        for ip in [
            Ipv4Addr::new(192, 168, 5, 142),
            Ipv4Addr::new(10, 0, 0, 7),
            Ipv4Addr::new(100, 81, 178, 243),
        ] {
            assert!(usable_lan_ipv4(ip), "{ip}");
        }
    }
}
