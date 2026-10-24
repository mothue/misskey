//! Certificate identities preserve the original SNI separately from the
//! comparison value. Invalid scoped literals must remain DNS identities.
use std::net::{IpAddr, Ipv6Addr};

// Bun internal/net/isIP accepts a zone only on strict IPv6 and only these
// ASCII zone characters. The certificate matches the zone-free address.
pub(crate) fn typed_ip(host: &str) -> Option<IpAddr> {
    if let Ok(ip) = host.parse() {
        return Some(ip);
    }
    let (address, zone) = host.split_once('%')?;
    if zone.is_empty()
        || !zone
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-.:".contains(&byte))
    {
        return None;
    }
    address.parse::<Ipv6Addr>().ok().map(IpAddr::V6)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pinned_bun_ipv6_zone_syntax_is_an_ip_identity() {
        for host in ["fe80::1%eth0", "fe80::1%1", "fe80::1%a-b.c:9"] {
            assert_eq!(typed_ip(host), Some("fe80::1".parse().unwrap()), "{host}");
        }
        for host in [
            "fe80::1%",
            "fe80::1%zone_unsupported",
            "fe80::1%bad!",
            "fe80::1%eth0%1",
            "127.0.0.1%eth0",
            "[::1]",
        ] {
            assert!(typed_ip(host).is_none(), "{host}");
        }
        assert_eq!(typed_ip("127.0.0.1"), Some("127.0.0.1".parse().unwrap()));
        assert_eq!(typed_ip("::1"), Some("::1".parse().unwrap()));
    }
}
