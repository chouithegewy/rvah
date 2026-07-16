use std::sync::atomic::{AtomicU32, Ordering};

use rvah_addon_sdk::{HttpRequest, HttpResponse};
use url::Url;

use crate::Allowlist;

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
}

/// Pluggable HTTP execution. The real transport is [`UreqTransport`];
/// tests use [`crate::MockTransport`].
pub trait Transport: Send + Sync {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError>;
}

/// Real transport backed by `ureq`.
pub struct UreqTransport {
    agent: ureq::Agent,
}

impl Default for UreqTransport {
    fn default() -> Self {
        UreqTransport {
            agent: ureq::AgentBuilder::new()
                .timeout(std::time::Duration::from_secs(30))
                .build(),
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
}

/// The client handed (indirectly) to add-ons: allowlist + scheme + quota
/// checks run before the transport ever sees the request.
pub struct PermissionedClient {
    transport: Box<dyn Transport>,
    allowlist: Allowlist,
    max_requests: u32,
    used: AtomicU32,
}

impl PermissionedClient {
    pub fn new(transport: Box<dyn Transport>, allowlist: Allowlist, max_requests: u32) -> Self {
        PermissionedClient {
            transport,
            allowlist,
            max_requests,
            used: AtomicU32::new(0),
        }
    }

    pub fn fetch(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        let url = Url::parse(&req.url).map_err(|e| NetError::InvalidUrl(e.to_string()))?;
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
        let used = self.used.fetch_add(1, Ordering::SeqCst);
        if used >= self.max_requests {
            return Err(NetError::QuotaExceeded);
        }
        self.transport.execute(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MockTransport;

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
}
