use std::io;
use std::net::{IpAddr, Ipv4Addr, UdpSocket};

// Probe the IPv4 all-hosts multicast route. VPN and overlay interfaces commonly
// capture the default unicast route, but they do not normally own the local
// multicast route used to reach another device on the physical LAN.
const LAN_ROUTE_PROBE_IPV4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 1);

/// `host` as an authority: as given when it already names a port, with `port`
/// otherwise.
///
/// "Already names a port" cannot be read as "contains a colon", because every
/// IPv6 address does. A bracketed address names a port only after its `]`, and
/// a bare IPv6 address is bracketed so the port appended to it is not read as
/// one more group of the address.
#[cfg(feature = "session")]
pub(crate) fn authority_with_port(host: &str, port: u16) -> String {
    if let Some(bracketed) = host.strip_prefix('[') {
        let names_port = bracketed.split_once("]:").is_some_and(|(_, digits)| {
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
        });
        return if names_port {
            host.to_owned()
        } else {
            format!("{host}:{port}")
        };
    }
    match host.matches(':').count() {
        0 => format!("{host}:{port}"),
        1 => host.to_owned(),
        _ => format!("[{host}]:{port}"),
    }
}

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

    #[cfg(feature = "session")]
    #[test]
    fn an_advertised_host_keeps_the_bound_port_unless_it_names_one() {
        for (host, expected) in [
            ("phone.example", "phone.example:8787"),
            ("phone.example:9000", "phone.example:9000"),
            ("192.168.1.5", "192.168.1.5:8787"),
            ("[fd00::5]", "[fd00::5]:8787"),
            ("[fd00::5]:9000", "[fd00::5]:9000"),
            ("fd00::5", "[fd00::5]:8787"),
        ] {
            assert_eq!(super::authority_with_port(host, 8787), expected, "{host}");
        }
    }

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
