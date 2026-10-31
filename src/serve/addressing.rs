//! Where a session is reached, and what it is called there.
//!
//! Both shapes, and every string question either of them asks: a path under this
//! listener's own origin, or a name one label under it. The awkward parts are
//! here rather than spread through the listener — that a credential has to be
//! spelled in base32 to fit a DNS label, that a spelling which is not the only
//! spelling would give one session several addresses, that a host nothing can
//! be named under has to be refused before a port is bound.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

use crate::{Error, Result};

/// The path segment every proxied Provider UI lives under.
///
/// It is not under the listener's own credential, and that is the point: the
/// URL a Provider UI is framed at must be one the Provider UI may know.
pub(super) const PROXY_SEGMENT: &str = "s";

/// What marks a hostname as one session's own origin.
///
/// A label rather than a whole name, so one wildcard record and one wildcard
/// certificate cover every session there will ever be. The prefix keeps session
/// names out of the way of anything else living under the same host.
pub(super) const ORIGIN_LABEL_PREFIX: &str = "s-";

/// The authority the browser asked for, lowercased.
///
/// The `Host` header is what a browser sends; the request target carries one
/// only when a proxy wrote it. Either is read, because which one arrives is not
/// this listener's decision.
pub(super) fn requested_authority<B>(request: &axum::http::Request<B>) -> Option<String> {
    let host = request
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| request.uri().authority().map(|value| value.to_string()))?;
    Some(host.to_ascii_lowercase())
}

/// The name part of an authority, without the port.
pub(super) fn hostname_of(authority: &str) -> &str {
    match authority.rsplit_once(':') {
        Some((host, port))
            if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            host
        }
        _ => authority,
    }
}

/// Where the page points a frame at one session.
///
/// Scheme-relative in the per-session shape, so the frame is fetched over
/// whatever the page itself was: a TLS terminator in front would otherwise turn
/// every frame into blocked mixed content.
pub(super) fn frame_address(secret: &str, shell_authority: Option<&str>) -> Option<String> {
    match shell_authority {
        Some(authority) => Some(format!(
            "//{ORIGIN_LABEL_PREFIX}{}.{authority}/",
            origin_label(secret)?
        )),
        None => Some(format!("/{PROXY_SEGMENT}/{secret}/")),
    }
}

/// The alphabet a session's own name is written in.
///
/// RFC 4648 base32, lowercased: letters and digits that are all legal in a DNS
/// label, in an encoding that is case-insensitive the way a hostname is. The
/// credential is re-encoded rather than shortened because hex would spell it in
/// 64 characters and a label may have 63.
const LABEL_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// The label that names the mount this credential opens.
pub(super) fn origin_label(secret: &str) -> Option<String> {
    let bytes = hex::decode(secret).ok()?;
    let mut label = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut buffer = 0_u16;
    let mut bits = 0_u32;
    for byte in bytes {
        buffer = (buffer << 8) | u16::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            label.push(char::from(
                LABEL_ALPHABET[usize::from((buffer >> bits) & 0x1f)],
            ));
        }
    }
    if bits > 0 {
        label.push(char::from(
            LABEL_ALPHABET[usize::from((buffer << (5 - bits)) & 0x1f)],
        ));
    }
    Some(label)
}

/// The credential a session's own name carries, in the form the mount table
/// keys on.
///
/// Every step can refuse: a character outside the alphabet, or padding bits
/// that are not zero. The second one matters — without it sixteen spellings of
/// one label would open the same mount, and a session would have sixteen
/// addresses instead of one.
pub(super) fn mount_secret_of(label: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(label.len() * 5 / 8);
    let mut buffer = 0_u16;
    let mut bits = 0_u32;
    for character in label.bytes() {
        let value = LABEL_ALPHABET
            .iter()
            .position(|candidate| *candidate == character)?;
        buffer = (buffer << 5) | u16::try_from(value).ok()?;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            bytes.push(u8::try_from((buffer >> bits) & 0xff).ok()?);
        }
    }
    if bits > 0 && buffer & ((1 << bits) - 1) != 0 {
        return None;
    }
    Some(hex::encode(bytes))
}

/// A host every session can be given a name under.
///
/// Returns the name itself, without whatever port came with it: the port is the
/// page's business and every session shares it.
pub(super) fn session_origin_base(host: &str) -> Result<String> {
    let refuse = |reason: &'static str| Error::InvalidSessionOriginHost {
        value: host.to_owned(),
        reason,
    };
    let name = hostname_of(host.trim()).to_ascii_lowercase();
    if name.is_empty() {
        return Err(refuse("it names no host at all"));
    }
    if name.parse::<IpAddr>().is_ok() || name.starts_with('[') {
        return Err(refuse(
            "an address has no names under it, and no wildcard record can be written for one",
        ));
    }
    if !name
        .split('.')
        .all(|label| !label.is_empty() && label.len() <= 63 && is_host_label(label))
    {
        return Err(refuse(
            "a host is dot-separated labels of letters, digits and hyphens",
        ));
    }
    // 253 is the whole name a resolver will carry, and a session spends the
    // first 55 of them on `s-` and its own credential.
    if name.len() + ORIGIN_LABEL_PREFIX.len() + LABEL_LENGTH + 1 > 253 {
        return Err(refuse(
            "a session's own name is 55 characters longer, and that is past what a hostname may be",
        ));
    }
    Ok(name)
}

/// How long a credential is once it is spelled as a label: 32 bytes at five
/// bits a character.
pub(super) const LABEL_LENGTH: usize = 52;

fn is_host_label(label: &str) -> bool {
    label
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !label.starts_with('-')
        && !label.ends_with('-')
}

/// The authority to put in the URL when the caller did not name one.
///
/// A wildcard bind is reachable from elsewhere but cannot be typed into
/// anything, so it is the one case worth resolving to a real interface.
pub(super) fn advertised_authority(local_addr: SocketAddr) -> String {
    if local_addr.ip().is_unspecified()
        && let Some(lan) = primary_lan_ipv4()
    {
        return format!("{lan}:{}", local_addr.port());
    }
    local_addr.to_string()
}

/// This machine's address on the local network, as far as its routing table
/// knows.
///
/// Probes the IPv4 all-hosts multicast route rather than a unicast default
/// route: VPN and overlay interfaces commonly capture the latter without owning
/// the route that reaches another device on the physical network. Nothing is
/// sent — a connected UDP socket only fixes a source address.
fn primary_lan_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(224, 0, 0, 1), 9)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip)
            if !ip.is_loopback()
                && !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && !ip.is_link_local() =>
        {
            Some(ip)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_nothing_can_be_named_under_is_refused_before_binding() {
        assert_eq!(session_origin_base("localhost").unwrap(), "localhost");
        assert_eq!(session_origin_base("localhost:8787").unwrap(), "localhost");
        assert_eq!(
            session_origin_base("UI.Example.COM").unwrap(),
            "ui.example.com"
        );
        // An address has nothing under it: no wildcard record, no certificate.
        assert!(session_origin_base("127.0.0.1").is_err());
        assert!(session_origin_base("127.0.0.1:8787").is_err());
        assert!(session_origin_base("[::1]:8787").is_err());
        assert!(session_origin_base("").is_err());
        assert!(session_origin_base("under_score.example").is_err());
        assert!(session_origin_base("-leading.example").is_err());
        assert!(session_origin_base("http://localhost").is_err());
        // No room left for a session's own name.
        assert!(session_origin_base(&format!("{}.example", "a".repeat(200))).is_err());
    }

    #[test]
    fn an_authority_is_read_without_its_port() {
        assert_eq!(hostname_of("localhost:8787"), "localhost");
        assert_eq!(hostname_of("localhost"), "localhost");
        assert_eq!(hostname_of("[::1]"), "[::1]");
        assert_eq!(hostname_of("ui.example.com"), "ui.example.com");
    }

    #[test]
    fn a_name_that_is_not_exactly_one_sessions_opens_nothing() {
        assert_eq!(mount_secret_of("not base32: !"), None);
        // Padding bits that are not zero: a second spelling of one credential,
        // refused so that a session has one address rather than sixteen.
        let label = origin_label(&"ab".repeat(32)).unwrap();
        assert_eq!(
            mount_secret_of(&label).as_deref(),
            Some("ab".repeat(32).as_str())
        );
        let mut tampered = label.clone();
        tampered.pop();
        tampered.push('b');
        assert_eq!(mount_secret_of(&tampered), None);
    }
}
