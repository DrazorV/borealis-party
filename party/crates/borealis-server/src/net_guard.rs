//! Which addresses the server may download from, and which peers may say who the client is.
//!
//! Any account can give the server an address to download from: an index (`add_index`, and the indexes a bundle's
//! packs came from), or a 5etools folder (`stage_5etools_url`, `update_5etools_url`). Without a check, that makes the
//! server fetch addresses on its own network (the router, the hypervisor's page, other containers). [`check_url`]
//! refuses them before anything is saved or fetched, and [`Guarded`] checks every address a download asks for,
//! including the files an index lists.
//!
//! This is not complete on its own: upstream's HTTPS client resolves the name again and follows redirects, and a name
//! can resolve differently the second time. The firewall rule in `party/README.md` ("Security and proxies") blocks
//! the home network for the server's container, whatever the program does.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Mutex, PoisonError};

use borealis_lib::content_fetch_retry::{FetchError, CODE_REFUSED};
use borealis_lib::content_fivetools_remote::{FetchRequest, FetchResponse, RemoteFetcher};
use borealis_lib::content_index::{Fetcher, Tagged};

/// An address range, `10.0.0.0/8` or a single address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    net: IpAddr,
    bits: u8,
}

impl Cidr {
    /// `a.b.c.d`, `a.b.c.d/n`, an IPv6 address or `addr/n`.
    ///
    /// # Errors
    /// What is wrong with it.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (addr, bits) = match text.split_once('/') {
            Some((a, b)) => (a, Some(b)),
            None => (text, None),
        };
        let net: IpAddr = addr
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse()
            .map_err(|_| format!("`{text}` is not an address or a range like 172.20.20.0/24"))?;
        let net = canonical(net);
        let max = if net.is_ipv4() { 32 } else { 128 };
        let bits = match bits {
            None => max,
            Some(b) => b
                .parse::<u8>()
                .ok()
                .filter(|b| *b <= max)
                .ok_or_else(|| format!("`{text}`: the prefix length must be 0 to {max}"))?,
        };
        Ok(Self { net, bits })
    }

    /// The prefix length: 32 or 128 for one address.
    #[must_use]
    pub fn bits(&self) -> u8 {
        self.bits
    }

    /// Whether it is an IPv4 range (IPv4 written as IPv6 counts as IPv4).
    #[must_use]
    pub fn is_ipv4(&self) -> bool {
        self.net.is_ipv4()
    }

    /// Whether `ip` is in the range.
    #[must_use]
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.net, canonical(ip)) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => prefix_eq(&net.octets(), &ip.octets(), self.bits),
            (IpAddr::V6(net), IpAddr::V6(ip)) => prefix_eq(&net.octets(), &ip.octets(), self.bits),
            _ => false,
        }
    }
}

fn prefix_eq(a: &[u8], b: &[u8], bits: u8) -> bool {
    let full = usize::from(bits / 8);
    let rest = bits % 8;
    if a[..full] != b[..full] {
        return false;
    }
    rest == 0 || {
        let mask = 0xffu8 << (8 - rest);
        a[full] & mask == b[full] & mask
    }
}

/// An IPv4 address written as IPv6 (`::ffff:a.b.c.d`) is that IPv4 address.
#[must_use]
pub fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

/// Why the server must not download from `ip`, or `None` for an ordinary internet address.
#[must_use]
pub fn private_reason(ip: IpAddr) -> Option<&'static str> {
    match canonical(ip) {
        IpAddr::V4(v4) => v4_reason(v4),
        IpAddr::V6(v6) => v6_reason(v6),
    }
}

fn v4_reason(ip: Ipv4Addr) -> Option<&'static str> {
    let [a, b, c, _] = ip.octets();
    if ip.is_loopback() {
        Some("this machine")
    } else if ip.is_private() {
        Some("a home or office network")
    } else if a == 100 && (64..128).contains(&b) {
        Some("a carrier or Tailscale network")
    } else if ip.is_link_local() {
        Some("a link-local network")
    } else if a == 0
        || ip.is_broadcast()
        || a >= 240
        || ip.is_multicast()
        || (a, b, c) == (192, 0, 0)
        || ip.is_documentation()
        || (a == 198 && (18..20).contains(&b))
    {
        Some("a reserved range")
    } else {
        None
    }
}

fn v6_reason(ip: Ipv6Addr) -> Option<&'static str> {
    let s = ip.segments();
    let embedded =
        |hi: u16, lo: u16| v4_reason(Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo)));
    if ip.is_loopback() {
        Some("this machine")
    } else if ip.is_unspecified() || ip.is_multicast() {
        Some("a reserved range")
    } else if s[0] & 0xfe00 == 0xfc00 {
        Some("a home or office network")
    } else if s[0] & 0xffc0 == 0xfe80 || s[0] & 0xffc0 == 0xfec0 {
        Some("a link-local network")
    } else if s[0] == 0x2001 && s[1] == 0x0db8 {
        Some("a reserved range")
    } else if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        // NAT64 carries an IPv4 address in the last 32 bits.
        embedded(s[6], s[7])
    } else if s[..6] == [0; 6] {
        // The old IPv4-compatible form.
        Some("a reserved range")
    } else if s[0] == 0x2002 {
        // 6to4 carries one in bits 16-48.
        embedded(s[1], s[2])
    } else {
        None
    }
}

/// Names that only mean something on a local network, whatever they resolve to.
const LOCAL_SUFFIXES: [&str; 8] = [
    ".localhost",
    ".local",
    ".lan",
    ".home",
    ".internal",
    ".intranet",
    ".corp",
    ".home.arpa",
];

/// Query parameter names that carry a secret: an address with one would be saved, shown and shared with the content.
fn is_secret_param(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "signature",
        "credential",
        "apikey",
        "api_key",
        "auth",
    ]
    .iter()
    .any(|part| name.contains(part))
        || ["key", "sig", "code", "jwt", "session"].contains(&name.as_str())
}

fn normal_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Turns a name into its addresses. The real one asks the system; tests answer from a table, never the network.
pub trait Resolve: Send + Sync {
    /// The addresses `host` has.
    ///
    /// # Errors
    /// When the name can't be found.
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String>;
}

/// The system resolver.
pub struct SystemResolver;

impl Resolve for SystemResolver {
    #[cfg(not(test))]
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
        use std::net::ToSocketAddrs as _;
        (host, port)
            .to_socket_addrs()
            .map(|addrs| addrs.map(|a| a.ip()).collect())
            .map_err(|e| e.to_string())
    }

    /// Tests never touch the network: names under `.example`, `.invalid` or `example.com` are internet addresses,
    /// except the ones that say where they point.
    #[cfg(test)]
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
        let _ = port;
        let public = || Ok(vec![IpAddr::from([93, 184, 215, 14])]);
        match host {
            "lan.example.invalid" | "rebind.example.invalid" => {
                Ok(vec![IpAddr::from([172, 20, 20, 100])])
            }
            "mixed.example.invalid" => Ok(vec![
                IpAddr::from([93, 184, 215, 14]),
                IpAddr::from([10, 0, 0, 1]),
            ]),
            "v6lan.example.invalid" => {
                Ok(vec!["fd00::1".parse().map_err(|_| "bad test address")?])
            }
            h if h.ends_with(".example")
                || h.ends_with(".invalid")
                || h.ends_with("example.com") =>
            {
                public()
            }
            "raw.githubusercontent.com" | "github.com" | "api.github.com" => public(),
            _ => Err("not found".into()),
        }
    }
}

/// Whether the server may download from `url`, as the user is told when it may not.
///
/// # Errors
/// A sentence for the user.
pub fn check_url(url: &str) -> Result<(), String> {
    // The server's own name: downloading from itself goes out and back in through the proxy.
    check_url_with(url, &SystemResolver, &crate::config::network().own_hosts)
}

/// [`check_url`] with a given resolver and own names.
///
/// # Errors
/// As [`check_url`].
pub fn check_url_with(url: &str, resolver: &dyn Resolve, own: &[String]) -> Result<(), String> {
    let parsed = url::Url::parse(url.trim())
        .map_err(|_| "That is not a web address. Use one that starts with https://".to_string())?;
    if parsed.scheme() != "https" {
        return Err("Index and file addresses must start with https://".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(
            "Addresses with a user name or password in them are not accepted: the address is saved and shared with \
             the content. Use a public address."
                .into(),
        );
    }
    if parsed.query_pairs().any(|(name, _)| is_secret_param(&name)) {
        return Err(
            "Addresses with an access key or token in them are not accepted: the address is saved and shared with \
             the content. Use a public address."
                .into(),
        );
    }
    let refused = |what: &str, why: &str| {
        Err(format!(
            "The server does not download from {what}: it is on {why}. Use an address on the internet."
        ))
    };
    let host = match parsed.host() {
        None => return Err("That address has no host name.".into()),
        Some(url::Host::Ipv4(ip)) => {
            return private_reason(ip.into()).map_or(Ok(()), |why| refused(&ip.to_string(), why))
        }
        Some(url::Host::Ipv6(ip)) => {
            return private_reason(ip.into()).map_or(Ok(()), |why| refused(&ip.to_string(), why))
        }
        Some(url::Host::Domain(name)) => normal_host(name),
    };
    if host == "localhost"
        || LOCAL_SUFFIXES.iter().any(|s| host.ends_with(s))
        || !host.contains('.')
    {
        return refused(&host, "a local network");
    }
    if own.contains(&host) {
        return Err(format!(
            "The server does not download from its own address ({host}). Import the file instead."
        ));
    }
    let port = parsed.port_or_known_default().unwrap_or(443);
    let addrs = resolver
        .resolve(&host, port)
        .map_err(|_| format!("The server could not find {host}. Check the address."))?;
    if addrs.is_empty() {
        return Err(format!(
            "The server could not find {host}. Check the address."
        ));
    }
    // Every address the name has must be on the internet: a client may pick any of them.
    match addrs.iter().find_map(|ip| private_reason(*ip)) {
        Some(why) => refused(&host, why),
        None => Ok(()),
    }
}

/// A downloader that checks every address before the one inside fetches it. Answers per host are kept, since one
/// 5etools download asks for hundreds of files from the same two hosts.
pub struct Guarded<F> {
    inner: F,
    seen: Mutex<HashMap<String, Result<(), String>>>,
}

impl<F> Guarded<F> {
    /// Check every address `inner` is asked for.
    pub fn new(inner: F) -> Self {
        Self {
            inner,
            seen: Mutex::default(),
        }
    }

    /// The downloader inside.
    #[cfg(test)]
    pub fn inner(&self) -> &F {
        &self.inner
    }

    fn check(&self, url: &str) -> Result<(), FetchError> {
        // The verdict depends on the host only, apart from userinfo and the query, which are checked every time.
        let host = url::Url::parse(url)
            .ok()
            .filter(|u| u.username().is_empty() && u.password().is_none() && u.query().is_none())
            .and_then(|u| u.host_str().map(|h| format!("{}:{:?}", h, u.port())));
        let verdict = match host {
            Some(key) => {
                let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
                seen.entry(key).or_insert_with(|| check_url(url)).clone()
            }
            None => check_url(url),
        };
        verdict.map_err(|m| {
            tracing::warn!("download refused: {m}");
            FetchError::new(CODE_REFUSED, m)
        })
    }
}

impl<F: Fetcher> Fetcher for Guarded<F> {
    fn get(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
        self.check(url).map_err(|e| e.wire())?;
        self.inner.get(url, max_bytes)
    }

    fn get_classified(&self, url: &str, max_bytes: usize) -> Result<Vec<u8>, FetchError> {
        self.check(url)?;
        self.inner.get_classified(url, max_bytes)
    }

    fn get_tagged(
        &self,
        url: &str,
        max_bytes: usize,
        etag: Option<&str>,
    ) -> Result<Tagged, FetchError> {
        self.check(url)?;
        self.inner.get_tagged(url, max_bytes, etag)
    }
}

impl<F: RemoteFetcher> RemoteFetcher for Guarded<F> {
    fn fetch(&self, request: &FetchRequest<'_>) -> Result<FetchResponse, String> {
        self.check(request.url).map_err(|e| e.wire())?;
        self.inner.fetch(request)
    }

    fn fetch_classified(&self, request: &FetchRequest<'_>) -> Result<FetchResponse, FetchError> {
        self.check(request.url)?;
        self.inner.fetch_classified(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(url: &str) -> Result<(), String> {
        check_url_with(url, &SystemResolver, &["dnd.example.com".to_string()])
    }

    #[test]
    fn internet_addresses_pass() {
        for ok in [
            "https://raw.githubusercontent.com/owner/repo/main/index.json",
            "https://content.example/packs/index.xml?v=3",
            "https://93.184.215.14/index.json",
            "https://[2606:2800:220:1:248:1893:25c8:1946]/x",
            "https://github.com/owner/repo/blob/main/x.index#top",
        ] {
            assert_eq!(check(ok), Ok(()), "{ok}");
        }
    }

    #[test]
    fn home_network_and_reserved_addresses_are_refused() {
        for no in [
            "https://127.0.0.1/",
            "https://127.1/",
            "https://0x7f000001/",
            "https://2130706433/",
            "https://10.1.2.3/",
            "https://172.20.20.100:8006/",
            "https://192.168.1.1/",
            "https://100.100.1.1/",
            "https://169.254.169.254/latest/meta-data",
            "https://0.0.0.0/",
            "https://[::1]/",
            "https://[::ffff:172.20.20.100]/",
            "https://[::ffff:7f00:1]/",
            "https://[fd12::1]/",
            "https://[fe80::1]/",
            "https://[64:ff9b::a00:1]/",
            "https://[2002:c0a8:101::1]/",
            "https://localhost/",
            "https://LOCALHOST./",
            "https://pve.localhost/",
            "https://nas.local/",
            "https://proxmox/",
            "https://router.home.arpa/",
            "https://lan.example.invalid/index.json",
            "https://mixed.example.invalid/index.json",
            "https://v6lan.example.invalid/index.json",
        ] {
            let err = check(no).expect_err(no);
            assert!(
                err.starts_with("The server does not download from"),
                "{no}: {err}"
            );
        }
    }

    #[test]
    fn its_own_address_secrets_and_other_schemes_are_refused() {
        let own = check("https://DND.example.com./api/server").unwrap_err();
        assert!(own.contains("its own address"), "{own}");
        let user = check("https://me:pw@content.example/index.json").unwrap_err();
        assert!(user.contains("user name or password"), "{user}");
        for secret in [
            "https://raw.githubusercontent.com/o/r/main/x.json?token=GHSAT0AAA",
            "https://content.example/x.json?access_token=1",
            "https://content.example/x.json?X-Amz-Signature=1",
            "https://content.example/x.json?key=1",
        ] {
            let err = check(secret).unwrap_err();
            assert!(err.contains("access key or token"), "{secret}: {err}");
        }
        assert!(check("http://content.example/index.json").is_err());
        assert!(check("file:///etc/passwd").is_err());
        assert!(check("not a url").is_err());
        let gone = check("https://nowhere.test-unknown-tld/index.json").unwrap_err();
        assert!(gone.contains("could not find"), "{gone}");
    }

    #[test]
    fn ranges_parse_and_match() {
        let lan = Cidr::parse("172.20.20.0/24").unwrap();
        assert!(lan.contains("172.20.20.100".parse().unwrap()));
        assert!(lan.contains("::ffff:172.20.20.1".parse().unwrap()));
        assert!(!lan.contains("172.20.21.1".parse().unwrap()));
        let one = Cidr::parse("172.20.20.100").unwrap();
        assert!(one.contains("172.20.20.100".parse().unwrap()));
        assert!(!one.contains("172.20.20.101".parse().unwrap()));
        let v6 = Cidr::parse("2001:db8::/33").unwrap();
        assert!(v6.contains("2001:db8:7fff::1".parse().unwrap()));
        assert!(!v6.contains("2001:db8:8000::1".parse().unwrap()));
        assert!(Cidr::parse("0.0.0.0/0")
            .unwrap()
            .contains("8.8.8.8".parse().unwrap()));
        for bad in ["172.20.20.0/33", "nas", "10.0.0.0/x", ""] {
            assert!(Cidr::parse(bad).is_err(), "{bad}");
        }
    }

    struct Counting(std::sync::atomic::AtomicUsize);

    impl Fetcher for Counting {
        fn get(&self, _url: &str, _max: usize) -> Result<Vec<u8>, String> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(b"ok".to_vec())
        }
    }

    #[test]
    fn a_guarded_fetcher_never_asks_for_a_refused_address() {
        let guarded = Guarded::new(Counting(0.into()));
        assert_eq!(
            guarded.get("https://content.example/a.xml", 10).unwrap(),
            b"ok"
        );
        let err = guarded
            .get_classified("https://10.0.0.7/pack.xml", 10)
            .unwrap_err();
        assert_eq!(err.code, CODE_REFUSED);
        assert!(guarded.get_tagged("https://localhost/x", 10, None).is_err());
        assert_eq!(guarded.inner.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
