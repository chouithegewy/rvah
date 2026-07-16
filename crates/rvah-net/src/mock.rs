use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use rvah_addon_sdk::{HttpRequest, HttpResponse};

use crate::{NetError, Resolve, Transport};

#[derive(Clone)]
struct Route {
    method: String,
    url_prefix: String,
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// `(url, addrs)` of one pinned execution.
type PinnedRequest = (String, Vec<SocketAddr>);

/// In-memory transport for tests: canned responses per method + URL prefix,
/// records every request it actually receives.
#[derive(Clone, Default)]
pub struct MockTransport {
    routes: Arc<Mutex<Vec<Route>>>,
    requests: Arc<Mutex<Vec<HttpRequest>>>,
    pinned: Arc<Mutex<Vec<PinnedRequest>>>,
}

impl MockTransport {
    pub fn route(&self, method: &str, url_prefix: &str, status: u16, body: Vec<u8>) {
        self.routes.lock().unwrap().push(Route {
            method: method.to_string(),
            url_prefix: url_prefix.to_string(),
            status,
            headers: Vec::new(),
            body,
        });
    }

    /// A canned redirect: `status` (e.g. 302) with a `Location` header.
    pub fn route_redirect(&self, method: &str, url_prefix: &str, status: u16, location: &str) {
        self.routes.lock().unwrap().push(Route {
            method: method.to_string(),
            url_prefix: url_prefix.to_string(),
            status,
            headers: vec![("Location".into(), location.to_string())],
            body: Vec::new(),
        });
    }

    pub fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// `(url, addrs)` per pinned execution, in order.
    pub fn pinned(&self) -> Vec<PinnedRequest> {
        self.pinned.lock().unwrap().clone()
    }
}

impl Transport for MockTransport {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        self.requests.lock().unwrap().push(req.clone());
        let routes = self.routes.lock().unwrap();
        let hit = routes
            .iter()
            .find(|r| r.method == req.method && req.url.starts_with(&r.url_prefix));
        match hit {
            Some(r) => Ok(HttpResponse {
                status: r.status,
                headers: r.headers.iter().cloned().collect(),
                body: r.body.clone(),
            }),
            None => Err(NetError::Transport(format!(
                "no mock route for {}",
                req.url
            ))),
        }
    }

    fn execute_pinned(
        &self,
        req: &HttpRequest,
        addrs: &[SocketAddr],
    ) -> Result<HttpResponse, NetError> {
        self.pinned
            .lock()
            .unwrap()
            .push((req.url.clone(), addrs.to_vec()));
        self.execute(req)
    }
}

/// Scripted resolver for tests; unknown hosts fail with `ResolveFailed`.
#[derive(Clone, Default)]
pub struct MockResolver {
    hosts: Arc<Mutex<HashMap<String, Vec<IpAddr>>>>,
    queries: Arc<Mutex<Vec<String>>>,
}

impl MockResolver {
    pub fn host(&self, name: &str, addrs: impl IntoIterator<Item = IpAddr>) {
        self.hosts
            .lock()
            .unwrap()
            .insert(name.to_string(), addrs.into_iter().collect());
    }

    pub fn queries(&self) -> Vec<String> {
        self.queries.lock().unwrap().clone()
    }
}

impl Resolve for MockResolver {
    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, NetError> {
        self.queries.lock().unwrap().push(host.to_string());
        self.hosts
            .lock()
            .unwrap()
            .get(host)
            .cloned()
            .ok_or_else(|| NetError::ResolveFailed(host.to_string()))
    }
}
