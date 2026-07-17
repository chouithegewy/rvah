//! Real-Debrid downloads add-on for RVAH (ABI v0.1 wasm guest).
//!
//! Catalog = your Real-Debrid downloads history; resolving an item plays
//! its direct (already unrestricted) link. Requires the `api_token` secret
//! (from <https://real-debrid.com/apitoken>), stored via `rvah secret-set`.

pub mod host;
pub mod rd;

use rvah_addon_sdk::{AccountStatus, AddonError, AddonErrorCode, Filter, Page, ResolveResult};
use serde::{Deserialize, Serialize};

use host::Host;

/// Mirror of the host's `AddonOp` wire shape (`rvah-addon-host`).
// Only `rvah_invoke` (wasm32) reaches these outside of tests.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Op {
    List {
        #[allow(dead_code)]
        path: Option<String>,
        cursor: Option<String>,
    },
    Search {
        #[allow(dead_code)]
        query: String,
        #[allow(dead_code)]
        #[serde(default)]
        filters: Vec<Filter>,
        #[allow(dead_code)]
        cursor: Option<String>,
    },
    Resolve {
        item_id: String,
        #[allow(dead_code)]
        source_id: Option<String>,
    },
    AccountStatus,
}

/// Mirror of the host's `AddonResponse` wire shape.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum Response {
    Page(Page),
    Resolve(ResolveResult),
    Account(AccountStatus),
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn dispatch(input: &[u8], host: &dyn Host) -> Result<Response, AddonError> {
    let op: Op = serde_json::from_slice(input)
        .map_err(|e| AddonError::new(AddonErrorCode::Malformed, format!("op: {e}")))?;
    match op {
        Op::List { cursor, .. } => rd::list(host, cursor.as_deref()).map(Response::Page),
        Op::Search { .. } => Err(AddonError::new(
            AddonErrorCode::Internal,
            "search is not declared by this add-on",
        )),
        Op::Resolve { item_id, .. } => rd::resolve(host, &item_id).map(Response::Resolve),
        Op::AccountStatus => rd::account(host).map(Response::Account),
    }
}

// ---- ABI v0.1 exports (wasm32) ----

#[no_mangle]
pub extern "C" fn rvah_alloc(len: i32) -> i32 {
    let mut buf = vec![0u8; len.max(0) as usize];
    let ptr = buf.as_mut_ptr();
    // Leaked on purpose: the store lives for one invocation only.
    std::mem::forget(buf);
    ptr as i32
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn rvah_invoke(ptr: i32, len: i32) -> i64 {
    let input = unsafe { std::slice::from_raw_parts(ptr as *const u8, len.max(0) as usize) };
    let result: Result<Response, AddonError> = dispatch(input, &host::WasmHost);
    let out = serde_json::to_vec(&result).unwrap_or_default();
    if out.is_empty() {
        return 0;
    }
    let out_ptr = rvah_alloc(out.len() as i32);
    unsafe {
        std::ptr::copy_nonoverlapping(out.as_ptr(), out_ptr as *mut u8, out.len());
    }
    (((out_ptr as u64) << 32) | out.len() as u64) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvah_addon_sdk::PageItem;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// Scripted host: canned HTTP responses per URL, in-memory storage and
    /// secrets, recorded log lines.
    #[derive(Default)]
    struct MockHost {
        secrets: BTreeMap<String, String>,
        http: BTreeMap<String, (u16, Vec<u8>)>,
        storage: Mutex<BTreeMap<String, Vec<u8>>>,
        http_calls: Mutex<Vec<String>>,
    }

    impl Host for MockHost {
        fn call(&self, request: Value) -> Result<Value, AddonError> {
            match request["fn"].as_str().unwrap() {
                "http_fetch" => {
                    let url = request["req"]["url"].as_str().unwrap().to_string();
                    assert!(
                        request["req"]["headers"]["authorization"]
                            .as_str()
                            .is_some_and(|h| h.starts_with("Bearer ")),
                        "every API call must send the bearer token"
                    );
                    self.http_calls.lock().unwrap().push(url.clone());
                    let (status, body) = self
                        .http
                        .get(&url)
                        .cloned()
                        .unwrap_or((404, b"not routed".to_vec()));
                    Ok(json!({ "status": status, "headers": {}, "body": body }))
                }
                "secret_get" => {
                    let name = request["name"].as_str().unwrap();
                    Ok(json!({ "value": self.secrets.get(name) }))
                }
                "storage_get" => {
                    let key = request["key"].as_str().unwrap();
                    Ok(json!({ "value": self.storage.lock().unwrap().get(key) }))
                }
                "storage_set" => {
                    let key = request["key"].as_str().unwrap().to_string();
                    let value: Vec<u8> = serde_json::from_value(request["value"].clone()).unwrap();
                    self.storage.lock().unwrap().insert(key, value);
                    Ok(Value::Null)
                }
                "log" => Ok(Value::Null),
                other => panic!("unexpected host call {other}"),
            }
        }
    }

    fn entry(id: &str, filename: &str, download: &str) -> Value {
        json!({
            "id": id,
            "filename": filename,
            "mimeType": "video/mp4",
            "filesize": 1073741824u64,
            "link": "https://real-debrid.com/d/original",
            "host": "example.com",
            "download": download,
            "generated": "2026-07-16T10:00:00.000Z",
            "streamable": 1
        })
    }

    fn rd_host() -> MockHost {
        let mut host = MockHost::default();
        host.secrets.insert("api_token".into(), "tok-abc".into());
        host.http.insert(
            "https://api.real-debrid.com/rest/1.0/downloads?page=1&limit=50".into(),
            (
                200,
                serde_json::to_vec(&json!([
                    entry(
                        "d1",
                        "Movie.One.2024.mkv",
                        "https://x.download.real-debrid.com/d1"
                    ),
                    entry(
                        "d2",
                        "Show.S01E01.mkv",
                        "https://x.download.real-debrid.com/d2"
                    ),
                ]))
                .unwrap(),
            ),
        );
        host
    }

    #[test]
    fn list_maps_downloads_history_to_media_items() {
        let host = rd_host();
        let page = rd::list(&host, None).unwrap();
        assert_eq!(page.items.len(), 2);
        let PageItem::Media(m) = &page.items[0] else {
            panic!("expected media");
        };
        assert_eq!(m.id, "d1");
        assert_eq!(m.title, "Movie.One.2024.mkv");
        assert_eq!(m.synopsis.as_deref(), Some("1.0 GiB"));
        // Two entries < page limit: history exhausted, no cursor.
        assert_eq!(page.cursor_next, None);
    }

    #[test]
    fn resolve_uses_the_list_cache_without_refetching() {
        let host = rd_host();
        rd::list(&host, None).unwrap();
        let calls_after_list = host.http_calls.lock().unwrap().len();

        let result = rd::resolve(&host, "d2").unwrap();
        let ResolveResult::Playables(sources) = result else {
            panic!("expected playables, got {result:?}");
        };
        assert_eq!(sources[0].url, "https://x.download.real-debrid.com/d2");
        assert_eq!(
            host.http_calls.lock().unwrap().len(),
            calls_after_list,
            "cached resolve must not hit the API"
        );
    }

    #[test]
    fn resolve_falls_back_to_history_scan_and_reports_missing() {
        let host = rd_host();
        // No prior list: cache is cold, resolve scans page 1.
        let result = rd::resolve(&host, "d1").unwrap();
        assert!(matches!(result, ResolveResult::Playables(_)));

        let result = rd::resolve(&host, "nope").unwrap();
        let ResolveResult::Unavailable { reason } = result else {
            panic!("expected unavailable, got {result:?}");
        };
        assert!(reason.contains("nope"));
    }

    #[test]
    fn missing_token_and_rejected_token_are_auth_errors() {
        let host = MockHost::default(); // no secret
        let err = rd::list(&host, None).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Auth);
        assert!(err.message.contains("api_token"));

        let mut host = rd_host();
        host.http.insert(
            "https://api.real-debrid.com/rest/1.0/downloads?page=1&limit=50".into(),
            (401, b"bad token".to_vec()),
        );
        let err = rd::list(&host, None).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Auth);
    }

    #[test]
    fn account_status_reads_user() {
        let mut host = rd_host();
        host.http.insert(
            "https://api.real-debrid.com/rest/1.0/user".into(),
            (
                200,
                serde_json::to_vec(&json!({
                    "id": 1, "username": "david", "type": "premium",
                    "premium": 12345, "expiration": "2027-01-01T00:00:00.000Z"
                }))
                .unwrap(),
            ),
        );
        let status = rd::account(&host).unwrap();
        assert_eq!(
            status,
            AccountStatus::SignedIn {
                display_name: Some("david".into())
            }
        );
    }

    #[test]
    fn dispatch_speaks_the_host_wire_format() {
        let host = rd_host();
        let response = dispatch(br#"{"op":"list","path":null,"cursor":null}"#, &host).unwrap();
        let wire = serde_json::to_value(&response).unwrap();
        assert_eq!(wire["kind"], "page");
        assert_eq!(wire["value"]["items"][0]["kind"], "media");

        // Undeclared op → structured error, not a panic.
        let err = dispatch(br#"{"op":"search","query":"x","cursor":null}"#, &host).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Internal);
    }
}
