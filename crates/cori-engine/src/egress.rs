// SPDX-License-Identifier: Apache-2.0
//! Where an outbound request may go. A builder (an app's `cori.toml`) or an org admin (the connector registry, the LLM
//! providers) chooses the URL, and the request leaves from inside the platform's network, next to its databases, its
//! Temporal and its cloud's metadata endpoint. So the host is resolved once, every address it resolves to is checked,
//! the request is pinned to the address checked (a second lookup cannot swap it), and redirects are not followed.
//!
//! Behind an outbound proxy (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, minus `NO_PROXY`, as reqwest reads them), the proxy
//! resolves the host and connects: a name that resolves here is still checked (an internal name stays refused), but one
//! this machine cannot resolve is the proxy's to resolve, and nothing is pinned. The proxy's own rules then decide what
//! that name may reach; internal names belong in `NO_PROXY`, so that they are checked and pinned here.
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// What an outbound request may reach. The server decides it per connector and per LLM provider, and writes it into
/// the run's input: `public` for what an app declares itself, `private` for what the org registry declares, `any` on a
/// laptop (`cori up`). Local runs (`cori run`) are not checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Egress {
    /// Public internet addresses only.
    #[default]
    Public,
    /// Public addresses and private networks (RFC 1918, CGNAT, IPv6 unique local), where a company's own systems live.
    /// Never this machine, a link-local address (cloud metadata endpoints) or a reserved range.
    Private,
    /// Anything, unchecked: the laptop modes.
    Any,
}

impl Egress {
    /// `public`, `private` or `any`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() { "public" => Some(Self::Public), "private" => Some(Self::Private), "any" => Some(Self::Any), _ => None }
    }

    pub fn as_str(self) -> &'static str { match self { Self::Public => "public", Self::Private => "private", Self::Any => "any" } }

    /// Whether a request may reach `ip` under this policy.
    pub fn allows(self, ip: IpAddr) -> bool {
        match self {
            Self::Any => true,
            Self::Public => class(ip) == Class::Public,
            Self::Private => matches!(class(ip), Class::Public | Class::Private),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Class { Public, Private, Forbidden }

fn class(ip: IpAddr) -> Class {
    let v6 = match ip { IpAddr::V4(v4) => return class_v4(v4), IpAddr::V6(v6) => v6 };
    // an IPv4 address written as IPv6 (mapped ::ffff:a.b.c.d, NAT64 64:ff9b::/96, 6to4 2002::/16) is that IPv4 address
    if let Some(v4) = v6.to_ipv4_mapped() { return class_v4(v4); }
    let s = v6.segments();
    let embedded = |hi: u16, lo: u16| Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8);
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6] == [0, 0, 0, 0] { return class_v4(embedded(s[6], s[7])); }
    if s[0] == 0x2002 { return class_v4(embedded(s[1], s[2])); }
    if v6.is_unspecified() || v6.is_loopback() || v6.is_multicast() { return Class::Forbidden; }
    match s[0] {
        x if x & 0xfe00 == 0xfc00 => Class::Private,     // fc00::/7, unique local
        x if x & 0xffc0 == 0xfe80 => Class::Forbidden,   // fe80::/10, link-local
        x if x & 0xffc0 == 0xfec0 => Class::Forbidden,   // fec0::/10, site-local (deprecated)
        0x2001 if s[1] == 0x0db8 || s[1] == 0 => Class::Forbidden,   // documentation, Teredo
        0x0100 if s[1..4] == [0, 0, 0] => Class::Forbidden,          // 100::/64, discard only
        0 => Class::Forbidden,                           // ::/16, IPv4-compatible addresses among them
        _ => Class::Public,
    }
}

fn class_v4(ip: Ipv4Addr) -> Class {
    let [a, b, c, _] = ip.octets();
    // RFC 1918, and the carrier-grade NAT range 100.64.0.0/10 (where some clusters put their pods)
    if ip.is_private() || (a == 100 && (b & 0xc0) == 64) { return Class::Private; }
    let reserved = a == 0                                   // "this network"
        || ip.is_loopback() || ip.is_link_local()           // this machine; 169.254.169.254, the cloud metadata endpoint
        || ip.is_multicast() || a >= 240                    // multicast; reserved, broadcast
        || (a == 192 && b == 0 && (c == 0 || c == 2))       // protocol assignments, documentation
        || (a == 192 && b == 88 && c == 99)                 // 6to4 relay
        || (a == 198 && (b & 0xfe) == 18)                   // benchmarking
        || (a == 198 && b == 51 && c == 100) || (a == 203 && b == 0 && c == 113);   // documentation
    if reserved { Class::Forbidden } else { Class::Public }
}

/// Why `ip` is refused under `egress`, for the step's error.
fn refusal(egress: Egress, ip: IpAddr) -> &'static str {
    match (egress, class(ip)) {
        (Egress::Public, Class::Private) => "a private network, which only connectors of the org registry may reach (and only when the server allows it)",
        _ => "this machine, a link-local address (a cloud's metadata endpoint) or a reserved range, which no hosted request reaches",
    }
}

/// Checks where `url` goes under `egress`: http or https, and every address its host resolves to allowed. Returns the
/// host and the address the request must be pinned to; none for an IP literal (checked as written), under `Any`, or
/// through a proxy (which connects).
pub async fn destination(url: &reqwest::Url, egress: Egress) -> Result<Option<(String, SocketAddr)>> {
    destination_via(url, egress, proxied(url, |k| std::env::var(k).ok())).await
}

async fn destination_via(url: &reqwest::Url, egress: Egress, proxied: bool) -> Result<Option<(String, SocketAddr)>> {
    if !matches!(url.scheme(), "http" | "https") { return Err(anyhow!("InvalidInputError: {url}: only http and https requests leave a hosted run")); }
    if egress == Egress::Any { return Ok(None); }
    let host = url.host_str().ok_or_else(|| anyhow!("InvalidInputError: {url} has no host"))?;
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        if !egress.allows(ip) { return Err(anyhow!("InvalidInputError: {url} is refused: {ip} is {}", refusal(egress, ip))); }
        return Ok(None);
    }
    let port = url.port_or_known_default().ok_or_else(|| anyhow!("InvalidInputError: {url} has no port"))?;
    let addrs: Vec<SocketAddr> = match tokio::net::lookup_host((bare, port)).await {
        Ok(a) => a.collect(),
        // the proxy resolves it: this machine may know no public name
        Err(_) if proxied => return Ok(None),
        Err(e) => return Err(anyhow!("Transient: resolving {bare}: {e}")),
    };
    if let Some(bad) = addrs.iter().find(|a| !egress.allows(a.ip())) {
        return Err(anyhow!("InvalidInputError: {url} is refused: {bare} resolves to {}, {}", bad.ip(), refusal(egress, bad.ip())));
    }
    if proxied { return Ok(None); }
    let first = addrs.first().ok_or_else(|| anyhow!("Transient: {bare} resolves to no address"))?;
    Ok(Some((bare.to_string(), *first)))
}

/// Whether requests to `url` go through a proxy: one named for its scheme (`HTTPS_PROXY`, `HTTP_PROXY`, else `ALL_PROXY`;
/// lower case too) and `NO_PROXY` not naming its host. `env` reads a variable.
pub fn proxied(url: &reqwest::Url, env: impl Fn(&str) -> Option<String>) -> bool {
    let var = |k: &str| env(k).or_else(|| env(&k.to_ascii_lowercase())).filter(|v| !v.trim().is_empty());
    let proxy = match url.scheme() { "https" => var("HTTPS_PROXY"), "http" => var("HTTP_PROXY"), _ => None }.or_else(|| var("ALL_PROXY"));
    let Some(host) = url.host_str() else { return false };
    proxy.is_some() && !no_proxy(&var("NO_PROXY").unwrap_or_default(), host)
}

/// Whether `NO_PROXY` (comma-separated: `*`, domains, `.domains`, IP addresses and CIDR ranges) names `host`.
fn no_proxy(list: &str, host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    let ip = host.parse::<IpAddr>().ok();
    list.split(',').map(|e| e.trim().to_ascii_lowercase()).filter(|e| !e.is_empty()).any(|e| {
        if e == "*" { return true; }
        match ip {
            Some(ip) => e.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>().map(|x| x == ip).unwrap_or(false) || in_cidr(&e, ip),
            None => { let d = e.trim_start_matches("*.").trim_start_matches('.'); host == d || host.ends_with(&format!(".{d}")) }
        }
    })
}

/// Whether `ip` is in the range `cidr` (`10.0.0.0/8`, `fd00::/8`).
fn in_cidr(cidr: &str, ip: IpAddr) -> bool {
    let Some((net, bits)) = cidr.split_once('/') else { return false };
    let (Ok(net), Ok(bits)) = (net.parse::<IpAddr>(), bits.parse::<u32>()) else { return false };
    match (net, ip) {
        (IpAddr::V4(n), IpAddr::V4(i)) if bits <= 32 => { let m = if bits == 0 { 0 } else { u32::MAX << (32 - bits) }; u32::from(n) & m == u32::from(i) & m }
        (IpAddr::V6(n), IpAddr::V6(i)) if bits <= 128 => { let m = if bits == 0 { 0 } else { u128::MAX << (128 - bits) }; u128::from(n) & m == u128::from(i) & m }
        _ => false,
    }
}

/// A client for requests to `url` under `egress`: its destination checked (`destination`), the client pinned to the
/// address checked and following no redirect. `Any` gets a plain client.
pub async fn client_for(url: &reqwest::Url, egress: Egress, timeout: Option<std::time::Duration>) -> Result<reqwest::Client> {
    let mut b = crate::http::builder();
    if let Some(t) = timeout { b = b.timeout(t); }
    if egress != Egress::Any {
        b = b.redirect(reqwest::redirect::Policy::none());
        if let Some((host, addr)) = destination(url, egress).await? { b = b.resolve(&host, addr); }
    }
    b.build().map_err(|e| anyhow!("RuntimeUnavailableError: http client: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(egress: Egress, s: &str) -> bool { egress.allows(s.parse().unwrap()) }

    #[test]
    fn public_reaches_the_internet_only() {
        for ok in ["93.184.216.34", "1.1.1.1", "2606:4700::1111", "::ffff:93.184.216.34"] { assert!(allowed(Egress::Public, ok), "{ok}"); }
        for refused in ["10.1.2.3", "172.16.0.1", "192.168.0.10", "100.64.0.1", "fd00::1", "127.0.0.1", "169.254.169.254", "0.0.0.0", "0.1.2.3",
            "224.0.0.1", "255.255.255.255", "240.0.0.1", "192.0.2.1", "198.18.0.1", "::1", "::", "fe80::1", "fec0::1", "ff02::1", "2001:db8::1",
            "::ffff:127.0.0.1", "::ffff:10.0.0.1", "::ffff:169.254.169.254", "64:ff9b::a9fe:a9fe", "2002:a9fe:a9fe::1", "::7f00:1"] {
            assert!(!allowed(Egress::Public, refused), "{refused}");
        }
    }

    #[test]
    fn private_adds_private_networks_never_this_machine() {
        for ok in ["10.1.2.3", "172.16.0.1", "192.168.0.10", "100.64.0.1", "fd00::1", "::ffff:10.0.0.1", "93.184.216.34"] { assert!(allowed(Egress::Private, ok), "{ok}"); }
        for refused in ["127.0.0.1", "169.254.169.254", "0.0.0.0", "::1", "fe80::1", "::ffff:127.0.0.1", "64:ff9b::7f00:1"] { assert!(!allowed(Egress::Private, refused), "{refused}"); }
        assert!(allowed(Egress::Any, "127.0.0.1") && allowed(Egress::Any, "169.254.169.254"));
    }

    #[tokio::test]
    async fn a_url_is_checked_where_it_resolves() {
        let url = |s: &str| reqwest::Url::parse(s).unwrap();
        assert!(destination(&url("http://127.0.0.1:8123/x"), Egress::Private).await.is_err());
        assert!(destination(&url("http://localhost/x"), Egress::Private).await.is_err(), "localhost resolves to loopback");
        assert!(destination(&url("http://[::1]/x"), Egress::Public).await.is_err());
        assert!(destination(&url("http://[fd00::1]/x"), Egress::Public).await.is_err());
        assert!(destination(&url("file:///etc/passwd"), Egress::Any).await.is_err(), "http and https only, whatever the policy");
        assert!(destination(&url("https://10.0.0.7/hook"), Egress::Private).await.unwrap().is_none(), "an IP literal is checked as written");
        assert!(destination(&url("https://10.0.0.7/hook"), Egress::Public).await.is_err());
        assert!(destination(&url("http://127.0.0.1/x"), Egress::Any).await.unwrap().is_none());
        assert_eq!(Egress::parse(" Private "), Some(Egress::Private));
        assert_eq!(Egress::parse("all"), None);
    }

    #[test]
    fn a_proxy_is_used_unless_no_proxy_names_the_host() {
        let url = |s: &str| reqwest::Url::parse(s).unwrap();
        let env = |vars: &'static [(&'static str, &'static str)]| move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        assert!(!proxied(&url("https://api.example.com/x"), env(&[])));
        assert!(proxied(&url("https://api.example.com/x"), env(&[("HTTPS_PROXY", "http://proxy.corp:3128")])));
        assert!(proxied(&url("https://api.example.com/x"), env(&[("https_proxy", "http://proxy.corp:3128")])), "lower case too");
        assert!(!proxied(&url("http://api.example.com/x"), env(&[("HTTPS_PROXY", "http://proxy.corp:3128")])), "http has its own");
        assert!(proxied(&url("http://api.example.com/x"), env(&[("ALL_PROXY", "http://proxy.corp:3128")])));
        let corp = env(&[("HTTPS_PROXY", "http://proxy.corp:3128"), ("NO_PROXY", "localhost, .acme.internal,10.0.0.0/8 ,fd00::/8,192.168.1.7")]);
        assert!(!proxied(&url("https://vault.acme.internal/v1"), &corp));
        assert!(!proxied(&url("https://acme.internal/"), &corp), "the domain itself");
        assert!(proxied(&url("https://notacme.internal/"), &corp), "a suffix match is on labels");
        assert!(!proxied(&url("https://10.20.30.40/"), &corp) && !proxied(&url("https://192.168.1.7/"), &corp) && !proxied(&url("https://[fd12::1]/"), &corp));
        assert!(proxied(&url("https://192.168.1.8/"), &corp) && proxied(&url("https://11.0.0.1/"), &corp));
        assert!(!proxied(&url("https://anything.example/"), env(&[("HTTPS_PROXY", "http://p:1"), ("NO_PROXY", "*")])));
    }

    #[tokio::test]
    async fn through_a_proxy_a_name_this_machine_cannot_resolve_is_the_proxys() {
        let url = |s: &str| reqwest::Url::parse(s).unwrap();
        let unknown = url("https://cori-egress-test.invalid/x");
        assert!(destination_via(&unknown, Egress::Public, false).await.is_err(), "direct: refused, it resolves to nothing");
        assert!(destination_via(&unknown, Egress::Public, true).await.unwrap().is_none(), "proxied: the proxy resolves it");
        // a name that resolves here is checked all the same, and an IP literal as written
        assert!(destination_via(&url("http://localhost/x"), Egress::Private, true).await.is_err());
        assert!(destination_via(&url("https://10.0.0.7/x"), Egress::Public, true).await.is_err());
    }
}
