use std::sync::{Arc, Mutex};

use rvah_addon_sdk::{HttpRequest, HttpResponse};

use crate::{NetError, Transport};

#[derive(Clone)]
struct Route {
    method: String,
    url_prefix: String,
    status: u16,
    body: Vec<u8>,
}

/// In-memory transport for tests: canned responses per method + URL prefix,
/// records every request it actually receives.
#[derive(Clone, Default)]
pub struct MockTransport {
    routes: Arc<Mutex<Vec<Route>>>,
    requests: Arc<Mutex<Vec<HttpRequest>>>,
}

impl MockTransport {
    pub fn route(&self, method: &str, url_prefix: &str, status: u16, body: Vec<u8>) {
        self.routes.lock().unwrap().push(Route {
            method: method.to_string(),
            url_prefix: url_prefix.to_string(),
            status,
            body,
        });
    }

    pub fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    pub fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
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
                headers: Default::default(),
                body: r.body.clone(),
            }),
            None => Err(NetError::Transport(format!(
                "no mock route for {}",
                req.url
            ))),
        }
    }
}
