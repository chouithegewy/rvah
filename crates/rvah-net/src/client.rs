use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use rvah_addon_sdk::{HttpRequest, HttpResponse};
use url::Url;

use crate::resolve::is_non_global;
use crate::{Allowlist, Resolve};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NetError {
    #[error("domain not in allowlist: {0}")]
    DomainDenied(String),
    #[error("scheme not permitted: {0}")]
    SchemeDenied(String),
    #[error("request quota exceeded")]
    QuotaExceeded,
    #[error("transport error: {0}")]
    Transport(String),
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("host {host} resolves to disallowed address {addr}")]
    AddressDenied { host: String, addr: IpAddr },
    #[error("resolution failed: {0}")]
    ResolveFailed(String),
}

/// Pluggable HTTP execution. The real transport is [`UreqTransport`];
/// tests use [`crate::MockTransport`].
pub trait Transport: Send + Sync {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError>;

    /// Execute connecting only to `addrs` (already resolved and vetted by the
    /// caller), so the name cannot re-resolve between check and connect.
    /// Transports without address control fall back to plain `execute`.
    fn execute_pinned(
        &self,
        req: &HttpRequest,
        _addrs: &[SocketAddr],
    ) -> Result<HttpResponse, NetError> {
        self.execute(req)
    }
}

/// Real transport backed by `ureq`.
pub struct UreqTransport {
    agent: ureq::Agent,
    pins: PinnedAddrs,
}

/// ureq resolver that serves pinned addresses installed by `execute_pinned`
/// and falls back to the system resolver for unpinned hosts.
#[derive(Clone, Default)]
struct PinnedAddrs(Arc<Mutex<HashMap<String, Vec<SocketAddr>>>>);

impl ureq::Resolver for PinnedAddrs {
    fn resolve(&self, netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
        let (host, port) = netloc.rsplit_once(':').unwrap_or((netloc, "443"));
        let host = host
            .trim_matches(|c| c == '[' || c == ']')
            .to_ascii_lowercase();
        let port: u16 = port.parse().unwrap_or(443);
        if let Some(addrs) = self.0.lock().unwrap().get(&host) {
            return Ok(addrs
                .iter()
                .map(|sa| SocketAddr::new(sa.ip(), port))
                .collect());
        }
        use std::net::ToSocketAddrs;
        netloc.to_socket_addrs().map(|it| it.collect())
    }
}

impl Default for UreqTransport {
    fn default() -> Self {
        let pins = PinnedAddrs::default();
        UreqTransport {
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(30))
                // The transport never follows redirects: PermissionedClient
                // re-checks the allowlist on every hop (SDD §9).
                .redirects(0)
                .resolver(pins.clone())
                .build(),
            pins,
        }
    }
}

impl Transport for UreqTransport {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        let mut r = self.agent.request(&req.method, &req.url);
        for (k, v) in &req.headers {
            r = r.set(k, v);
        }
        let result = match &req.body {
            Some(body) => r.send_bytes(body),
            None => r.call(),
        };
        let resp = match result {
            Ok(resp) => resp,
            // Non-2xx statuses are still responses for the add-on to inspect.
            Err(ureq::Error::Status(_, resp)) => resp,
            Err(e) => return Err(NetError::Transport(e.to_string())),
        };
        let status = resp.status();
        let headers = resp
            .headers_names()
            .into_iter()
            .filter_map(|name| resp.header(&name).map(|v| (name.clone(), v.to_string())))
            .collect();
        let mut body = Vec::new();
        use std::io::Read;
        resp.into_reader()
            .take(32 * 1024 * 1024)
            .read_to_end(&mut body)
            .map_err(|e| NetError::Transport(e.to_string()))?;
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    fn execute_pinned(
        &self,
        req: &HttpRequest,
        addrs: &[SocketAddr],
    ) -> Result<HttpResponse, NetError> {
        let url = Url::parse(&req.url).map_err(|e| NetError::InvalidUrl(e.to_string()))?;
        let host = url
            .host_str()
            .ok_or_else(|| NetError::InvalidUrl("missing host".into()))?
            .to_ascii_lowercase();
        self.pins
            .0
            .lock()
            .unwrap()
            .insert(host.clone(), addrs.to_vec());
        let result = self.execute(req);
        self.pins.0.lock().unwrap().remove(&host);
        result
    }
}

/// The client handed (indirectly) to add-ons: allowlist + scheme + quota
/// checks run before the transport ever sees the request.
pub struct PermissionedClient {
    transport: Box<dyn Transport>,
    allowlist: Allowlist,
    max_requests: u32,
    used: AtomicU32,
    resolver: Option<Box<dyn Resolve>>,
}

impl PermissionedClient {
    pub fn new(transport: Box<dyn Transport>, allowlist: Allowlist, max_requests: u32) -> Self {
        PermissionedClient {
            transport,
            allowlist,
            max_requests,
            used: AtomicU32::new(0),
            resolver: None,
        }
    }

    /// Enable DNS pinning (SDD §9): every hop's host is resolved here, the
    /// addresses are vetted against the non-global policy, and the transport
    /// connects only to what was checked.
    pub fn with_resolver(mut self, resolver: Box<dyn Resolve>) -> Self {
        self.set_resolver(resolver);
        self
    }

    /// In-place form of [`PermissionedClient::with_resolver`].
    pub fn set_resolver(&mut self, resolver: Box<dyn Resolve>) {
        self.resolver = Some(resolver);
    }

    /// Resolve and vet the addresses for one hop. An allowlisted IP literal
    /// is an explicit user grant and is pinned as-is, without policy.
    fn pin_addrs(&self, url: &Url, host: &str) -> Result<Option<Vec<SocketAddr>>, NetError> {
        let Some(resolver) = &self.resolver else {
            return Ok(None);
        };
        let port = url.port_or_known_default().unwrap_or(443);
        if let Ok(literal) = host.parse::<IpAddr>() {
            return Ok(Some(vec![SocketAddr::new(literal, port)]));
        }
        let addrs = resolver.resolve(host)?;
        if addrs.is_empty() {
            return Err(NetError::ResolveFailed(host.to_string()));
        }
        if let Some(bad) = addrs.iter().copied().find(|ip| is_non_global(*ip)) {
            return Err(NetError::AddressDenied {
                host: host.to_string(),
                addr: bad,
            });
        }
        Ok(Some(
            addrs
                .into_iter()
                .map(|ip| SocketAddr::new(ip, port))
                .collect(),
        ))
    }

    /// Fetch with policy enforced on *every* hop: redirects are followed here
    /// (the transport never follows them itself), so a 30x can never smuggle
    /// a request to a disallowed host, and each hop consumes request quota.
    pub fn fetch(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        const MAX_REDIRECTS: u32 = 5;
        let mut current_url = req.url.clone();
        let mut method = req.method.clone();
        let mut body = req.body.clone();
        for _hop in 0..=MAX_REDIRECTS {
            let url = Url::parse(&current_url).map_err(|e| NetError::InvalidUrl(e.to_string()))?;
            match url.scheme() {
                "http" | "https" => {}
                other => return Err(NetError::SchemeDenied(other.to_string())),
            }
            let host = url
                .host_str()
                .ok_or_else(|| NetError::InvalidUrl("missing host".into()))?;
            if !self.allowlist.permits(host) {
                return Err(NetError::DomainDenied(host.to_string()));
            }
            let pinned = self.pin_addrs(&url, host)?;
            let used = self.used.fetch_add(1, Ordering::SeqCst);
            if used >= self.max_requests {
                return Err(NetError::QuotaExceeded);
            }

            let hop_req = HttpRequest {
                method: method.clone(),
                url: current_url.clone(),
                headers: req.headers.clone(),
                body: body.clone(),
            };
            let resp = match &pinned {
                Some(addrs) => self.transport.execute_pinned(&hop_req, addrs),
                None => self.transport.execute(&hop_req),
            }?;
            if !matches!(resp.status, 301 | 302 | 303 | 307 | 308) {
                return Ok(resp);
            }
            let Some(location) = resp
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("location"))
                .map(|(_, v)| v.clone())
            else {
                return Ok(resp); // 30x without Location: hand it to the caller.
            };
            current_url = url
                .join(&location)
                .map_err(|e| NetError::InvalidUrl(format!("redirect target: {e}")))?
                .to_string();
            if resp.status == 303 {
                method = "GET".into();
                body = None;
            }
        }
        Err(NetError::TooManyRedirects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockResolver, MockTransport};

    fn client(allow: &[&str], max: u32) -> (PermissionedClient, MockTransport) {
        let mock = MockTransport::default();
        mock.route("GET", "https://api.example.com/", 200, b"ok".to_vec());
        let c = PermissionedClient::new(
            Box::new(mock.clone()),
            Allowlist::new(allow.iter().copied()),
            max,
        );
        (c, mock)
    }

    #[test]
    fn allows_listed_domain_and_returns_response() {
        let (c, _mock) = client(&["api.example.com"], 10);
        let resp = c
            .fetch(&HttpRequest::get("https://api.example.com/v1"))
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"ok");
    }

    #[test]
    fn denies_unlisted_domain_before_transport() {
        let (c, mock) = client(&["api.example.com"], 10);
        let err = c
            .fetch(&HttpRequest::get("https://evil.example.org/steal"))
            .unwrap_err();
        assert_eq!(err, NetError::DomainDenied("evil.example.org".into()));
        assert_eq!(
            mock.request_count(),
            0,
            "transport must never see denied calls"
        );
    }

    #[test]
    fn denies_file_scheme() {
        let (c, mock) = client(&["api.example.com"], 10);
        let err = c
            .fetch(&HttpRequest::get("file:///etc/passwd"))
            .unwrap_err();
        assert!(matches!(err, NetError::SchemeDenied(_)));
        assert_eq!(mock.request_count(), 0);
    }

    #[test]
    fn request_quota_enforced() {
        let (c, mock) = client(&["api.example.com"], 2);
        let req = HttpRequest::get("https://api.example.com/v1");
        assert!(c.fetch(&req).is_ok());
        assert!(c.fetch(&req).is_ok());
        assert_eq!(c.fetch(&req).unwrap_err(), NetError::QuotaExceeded);
        assert_eq!(mock.request_count(), 2);
    }

    #[test]
    fn follows_redirect_within_allowlist() {
        let mock = MockTransport::default();
        mock.route_redirect("GET", "https://api.example.com/old", 302, "/new");
        mock.route("GET", "https://api.example.com/new", 200, b"moved".to_vec());
        let c = PermissionedClient::new(
            Box::new(mock.clone()),
            Allowlist::new(["api.example.com"]),
            10,
        );
        let resp = c
            .fetch(&HttpRequest::get("https://api.example.com/old"))
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"moved");
        // Both hops went through the transport (and the quota).
        assert_eq!(mock.request_count(), 2);
    }

    #[test]
    fn redirect_to_disallowed_host_denied() {
        // SDD §9: a 30x must not smuggle a request past the allowlist.
        let mock = MockTransport::default();
        mock.route_redirect(
            "GET",
            "https://api.example.com/old",
            302,
            "https://evil.example.org/steal",
        );
        mock.route("GET", "https://evil.example.org/", 200, b"gotcha".to_vec());
        let c = PermissionedClient::new(
            Box::new(mock.clone()),
            Allowlist::new(["api.example.com"]),
            10,
        );
        let err = c
            .fetch(&HttpRequest::get("https://api.example.com/old"))
            .unwrap_err();
        assert_eq!(err, NetError::DomainDenied("evil.example.org".into()));
        // The disallowed hop never reached the transport.
        assert_eq!(mock.request_count(), 1);
    }

    #[test]
    fn redirect_loop_hits_hop_limit() {
        let mock = MockTransport::default();
        mock.route_redirect("GET", "https://api.example.com/loop", 302, "/loop");
        let c = PermissionedClient::new(
            Box::new(mock.clone()),
            Allowlist::new(["api.example.com"]),
            100,
        );
        let err = c
            .fetch(&HttpRequest::get("https://api.example.com/loop"))
            .unwrap_err();
        assert_eq!(err, NetError::TooManyRedirects);
        assert_eq!(mock.request_count(), 6); // initial + 5 followed hops
    }

    fn ip(s: &str) -> std::net::IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn resolver_pins_checked_addresses_through_transport() {
        let mock = MockTransport::default();
        mock.route("GET", "https://api.example.com/", 200, b"ok".to_vec());
        let resolver = MockResolver::default();
        resolver.host("api.example.com", [ip("93.184.216.34")]);
        let c = PermissionedClient::new(
            Box::new(mock.clone()),
            Allowlist::new(["api.example.com"]),
            10,
        )
        .with_resolver(Box::new(resolver));
        c.fetch(&HttpRequest::get("https://api.example.com/v1"))
            .unwrap();
        let pinned = mock.pinned();
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].1, vec!["93.184.216.34:443".parse().unwrap()]);
    }

    #[test]
    fn name_resolving_to_non_global_address_denied() {
        // SDD §9: DNS rebinding — a permitted *name* must not reach loopback,
        // RFC-1918, link-local, CGNAT, ULA, or IPv4-mapped disguises thereof.
        for bad in [
            "127.0.0.1",
            "10.0.0.1",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.1.1",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:10.0.0.1",
        ] {
            let mock = MockTransport::default();
            mock.route("GET", "https://api.example.com/", 200, b"ok".to_vec());
            let resolver = MockResolver::default();
            resolver.host("api.example.com", [ip(bad)]);
            let c = PermissionedClient::new(
                Box::new(mock.clone()),
                Allowlist::new(["api.example.com"]),
                10,
            )
            .with_resolver(Box::new(resolver));
            let err = c
                .fetch(&HttpRequest::get("https://api.example.com/v1"))
                .unwrap_err();
            assert_eq!(
                err,
                NetError::AddressDenied {
                    host: "api.example.com".into(),
                    addr: ip(bad),
                },
                "{bad} must be denied"
            );
            assert_eq!(mock.request_count(), 0, "{bad} reached the transport");
        }
    }

    #[test]
    fn allowlisted_ip_literal_is_an_explicit_grant() {
        // The user approved this exact address in the manifest: no resolution,
        // no non-global policy — pin it as-is.
        let mock = MockTransport::default();
        mock.route("GET", "http://127.0.0.1:8080/", 200, b"ok".to_vec());
        let resolver = MockResolver::default();
        let c = PermissionedClient::new(Box::new(mock.clone()), Allowlist::new(["127.0.0.1"]), 10)
            .with_resolver(Box::new(resolver.clone()));
        c.fetch(&HttpRequest::get("http://127.0.0.1:8080/health"))
            .unwrap();
        assert!(resolver.queries().is_empty(), "literal must not resolve");
        assert_eq!(mock.pinned()[0].1, vec!["127.0.0.1:8080".parse().unwrap()]);
    }

    #[test]
    fn redirect_hop_re_resolves_and_denies_private_target() {
        let mock = MockTransport::default();
        mock.route_redirect(
            "GET",
            "https://api.example.com/old",
            302,
            "https://cdn.example.com/new",
        );
        mock.route("GET", "https://cdn.example.com/", 200, b"ok".to_vec());
        let resolver = MockResolver::default();
        resolver.host("api.example.com", [ip("93.184.216.34")]);
        resolver.host("cdn.example.com", [ip("192.168.1.7")]);
        let c = PermissionedClient::new(
            Box::new(mock.clone()),
            Allowlist::new(["api.example.com", "cdn.example.com"]),
            10,
        )
        .with_resolver(Box::new(resolver));
        let err = c
            .fetch(&HttpRequest::get("https://api.example.com/old"))
            .unwrap_err();
        assert_eq!(
            err,
            NetError::AddressDenied {
                host: "cdn.example.com".into(),
                addr: ip("192.168.1.7"),
            }
        );
        assert_eq!(mock.request_count(), 1, "second hop must be blocked");
    }

    #[test]
    fn resolution_failure_is_reported() {
        let mock = MockTransport::default();
        let c = PermissionedClient::new(
            Box::new(mock.clone()),
            Allowlist::new(["api.example.com"]),
            10,
        )
        .with_resolver(Box::new(MockResolver::default()));
        let err = c
            .fetch(&HttpRequest::get("https://api.example.com/v1"))
            .unwrap_err();
        assert!(matches!(err, NetError::ResolveFailed(_)));
        assert_eq!(mock.request_count(), 0);
    }

    #[test]
    fn see_other_downgrades_to_get_without_body() {
        let mock = MockTransport::default();
        mock.route_redirect("POST", "https://api.example.com/submit", 303, "/result");
        mock.route("GET", "https://api.example.com/result", 200, b"ok".to_vec());
        let c = PermissionedClient::new(
            Box::new(mock.clone()),
            Allowlist::new(["api.example.com"]),
            10,
        );
        let mut req = HttpRequest::get("https://api.example.com/submit");
        req.method = "POST".into();
        req.body = Some(b"payload".to_vec());
        let resp = c.fetch(&req).unwrap();
        assert_eq!(resp.status, 200);
        let hops = mock.requests();
        assert_eq!(hops[1].method, "GET");
        assert_eq!(hops[1].body, None);
    }
}
