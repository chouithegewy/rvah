//! Real-Debrid downloads add-on for RVAH (ABI v0.1 wasm guest).
//!
//! Catalog = your Real-Debrid downloads history; resolving an item plays
//! its direct (already unrestricted) link. Sign in with a device code
//! (Link account, see [`auth`]) or set the `api_token` secret from
//! <https://real-debrid.com/apitoken> via `rvah secret-set`.

pub mod auth;
pub mod host;
pub mod rd;

use rvah_addon_sdk::{
    AccountStatus, AddonError, AddonErrorCode, AuthPoll, Credentials, DeviceAuth, Filter, Page,
    ResolveResult,
};
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
        query: String,
        #[allow(dead_code)]
        #[serde(default)]
        filters: Vec<Filter>,
        cursor: Option<String>,
    },
    Resolve {
        item_id: String,
        #[allow(dead_code)]
        source_id: Option<String>,
    },
    AccountStatus,
    AuthBegin,
    AuthPoll {
        device_code: String,
    },
    AuthRefresh,
}

/// Mirror of the host's `AddonResponse` wire shape.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum Response {
    Page(Page),
    Resolve(ResolveResult),
    Account(AccountStatus),
    DeviceAuth(DeviceAuth),
    AuthPoll(AuthPoll),
    Credentials(Credentials),
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn dispatch(input: &[u8], host: &dyn Host) -> Result<Response, AddonError> {
    let op: Op = serde_json::from_slice(input)
        .map_err(|e| AddonError::new(AddonErrorCode::Malformed, format!("op: {e}")))?;
    match op {
        Op::List { cursor, .. } => rd::list(host, cursor.as_deref()).map(Response::Page),
        Op::Search { query, cursor, .. } => {
            rd::search(host, &query, cursor.as_deref()).map(Response::Page)
        }
        Op::Resolve { item_id, .. } => rd::resolve(host, &item_id).map(Response::Resolve),
        Op::AccountStatus => rd::account(host).map(Response::Account),
        Op::AuthBegin => auth::begin(host).map(Response::DeviceAuth),
        Op::AuthPoll { device_code } => auth::poll(host, &device_code).map(Response::AuthPoll),
        Op::AuthRefresh => auth::refresh(host).map(Response::Credentials),
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
    use rvah_addon_sdk::{AuthPoll, PageItem};
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// Scripted host: canned HTTP responses per URL, in-memory storage and
    /// secrets, recorded requests.
    #[derive(Default)]
    struct MockHost {
        secrets: BTreeMap<String, String>,
        http: BTreeMap<String, (u16, Vec<u8>)>,
        storage: Mutex<BTreeMap<String, Vec<u8>>>,
        http_calls: Mutex<Vec<String>>,
        /// (method, url, body) of every request, in order.
        requests: Mutex<Vec<(String, String, String)>>,
    }

    impl MockHost {
        fn route(&mut self, url: &str, status: u16, body: Value) {
            self.http
                .insert(url.into(), (status, serde_json::to_vec(&body).unwrap()));
        }

        fn requests(&self) -> Vec<(String, String, String)> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Host for MockHost {
        fn call(&self, request: Value) -> Result<Vec<u8>, AddonError> {
            self.answer(request)
                .map(|payload| serde_json::to_vec(&payload).unwrap())
        }
    }

    impl MockHost {
        fn answer(&self, request: Value) -> Result<Value, AddonError> {
            match request["fn"].as_str().unwrap() {
                "http_fetch" => {
                    let url = request["req"]["url"].as_str().unwrap().to_string();
                    if url.starts_with("https://api.real-debrid.com/rest/") {
                        assert!(
                            request["req"]["headers"]["authorization"]
                                .as_str()
                                .is_some_and(|h| h.starts_with("Bearer ")),
                            "every REST call must send the bearer token"
                        );
                    }
                    let body: Vec<u8> = match &request["req"]["body"] {
                        Value::Null => Vec::new(),
                        body => serde_json::from_value(body.clone()).unwrap(),
                    };
                    self.requests.lock().unwrap().push((
                        request["req"]["method"].as_str().unwrap().to_string(),
                        url.clone(),
                        String::from_utf8(body).unwrap(),
                    ));
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

        // Unknown op → structured error, not a panic.
        let err = dispatch(br#"{"op":"subtitles","item_id":"x"}"#, &host).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Malformed);
    }

    const CODE_URL: &str =
        "https://api.real-debrid.com/oauth/v2/device/code?client_id=X245A4XAIBGVM&new_credentials=yes";
    const CREDENTIALS_URL: &str =
        "https://api.real-debrid.com/oauth/v2/device/credentials?client_id=X245A4XAIBGVM&code=dev-code-1";
    const TOKEN_URL: &str = "https://api.real-debrid.com/oauth/v2/token";

    fn token_response(access: &str, refresh: &str) -> Value {
        json!({
            "access_token": access, "expires_in": 3600,
            "token_type": "Bearer", "refresh_token": refresh
        })
    }

    #[test]
    fn begin_asks_real_debrid_for_a_device_code() {
        let mut host = MockHost::default();
        host.route(
            CODE_URL,
            200,
            json!({
                "device_code": "dev-code-1", "user_code": "ABCDEFGH",
                "interval": 5, "expires_in": 1800,
                "verification_url": "https://real-debrid.com/device"
            }),
        );
        let device = auth::begin(&host).unwrap();
        assert_eq!(device.user_code, "ABCDEFGH");
        assert_eq!(device.verification_url, "https://real-debrid.com/device");
        assert_eq!(device.device_code, "dev-code-1");
        assert_eq!((device.interval_secs, device.expires_in_secs), (5, 1800));
    }

    #[test]
    fn poll_is_pending_until_the_user_approves() {
        let mut host = MockHost::default();
        host.route(
            CREDENTIALS_URL,
            403,
            json!({ "error": "authorization_pending" }),
        );
        assert_eq!(auth::poll(&host, "dev-code-1").unwrap(), AuthPoll::Pending);
        assert_eq!(host.requests().len(), 1, "no token request before approval");
    }

    #[test]
    fn approval_trades_the_device_code_for_tokens() {
        let mut host = MockHost::default();
        host.route(
            CREDENTIALS_URL,
            200,
            json!({ "client_id": "cid-1", "client_secret": "csecret-1" }),
        );
        host.route(TOKEN_URL, 200, token_response("access-1", "refresh-1"));
        let AuthPoll::Authorized(credentials) = auth::poll(&host, "dev-code-1").unwrap() else {
            panic!("expected approval");
        };
        assert_eq!(credentials.0["api_token"], "access-1");
        let client: Value = serde_json::from_str(&credentials.0["oauth_client"]).unwrap();
        assert_eq!(
            client,
            json!({ "client_id": "cid-1", "client_secret": "csecret-1", "refresh_token": "refresh-1" })
        );

        let (method, url, body) = host.requests().pop().unwrap();
        assert_eq!((method.as_str(), url.as_str()), ("POST", TOKEN_URL));
        assert_eq!(
            body,
            "client_id=cid-1&client_secret=csecret-1&code=dev-code-1\
             &grant_type=http%3A%2F%2Foauth.net%2Fgrant_type%2Fdevice%2F1.0"
        );
    }

    #[test]
    fn refresh_trades_the_stored_refresh_token_for_new_tokens() {
        let mut host = MockHost::default();
        host.secrets.insert(
            "oauth_client".into(),
            json!({ "client_id": "cid-1", "client_secret": "csecret-1", "refresh_token": "refresh-1" })
                .to_string(),
        );
        host.route(TOKEN_URL, 200, token_response("access-2", "refresh-2"));
        let credentials = auth::refresh(&host).unwrap();
        assert_eq!(credentials.0["api_token"], "access-2");
        assert!(credentials.0["oauth_client"].contains("refresh-2"));
        let (_, _, body) = host.requests().pop().unwrap();
        assert!(body.contains("&code=refresh-1&"), "{body}");

        host.route(
            TOKEN_URL,
            401,
            json!({ "error": "bad_token", "error_code": 8 }),
        );
        assert_eq!(auth::refresh(&host).unwrap_err().code, AddonErrorCode::Auth);
    }

    #[test]
    fn refresh_without_a_device_link_is_an_auth_error() {
        // A pasted api_token has nothing to refresh with.
        let host = rd_host();
        let err = auth::refresh(&host).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Auth);
        assert!(host.requests().is_empty());
    }

    #[test]
    fn dispatch_speaks_the_auth_wire_format() {
        let mut host = MockHost::default();
        host.route(
            CODE_URL,
            200,
            json!({
                "device_code": "dev-code-1", "user_code": "ABCDEFGH",
                "interval": 5, "expires_in": 1800,
                "verification_url": "https://real-debrid.com/device"
            }),
        );
        host.route(CREDENTIALS_URL, 403, json!({}));
        let begin =
            serde_json::to_value(dispatch(br#"{"op":"auth_begin"}"#, &host).unwrap()).unwrap();
        assert_eq!(begin["kind"], "device_auth");
        assert_eq!(begin["value"]["user_code"], "ABCDEFGH");
        let poll = serde_json::to_value(
            dispatch(br#"{"op":"auth_poll","device_code":"dev-code-1"}"#, &host).unwrap(),
        )
        .unwrap();
        assert_eq!(
            poll,
            json!({ "kind": "auth_poll", "value": { "status": "pending" } })
        );
    }

    #[test]
    fn an_expired_link_asks_the_host_to_refresh_instead_of_signing_out() {
        let mut host = rd_host();
        host.route(
            "https://api.real-debrid.com/rest/1.0/user",
            401,
            json!({ "error": "bad_token", "error_code": 8 }),
        );
        // A pasted token that is rejected: simply signed out.
        assert_eq!(rd::account(&host).unwrap(), AccountStatus::SignedOut);
        // A device link can be refreshed, so report it as an auth failure.
        host.secrets.insert("oauth_client".into(), "{}".into());
        assert_eq!(rd::account(&host).unwrap_err().code, AddonErrorCode::Auth);
    }

    fn search_url(page: u32) -> String {
        format!("https://api.real-debrid.com/rest/1.0/downloads?page={page}&limit=100")
    }

    /// One page of `count` downloads named `prefix.N.mkv`, ids `prefix-N`.
    fn history(prefix: &str, count: usize) -> Value {
        Value::Array(
            (0..count)
                .map(|n| {
                    entry(
                        &format!("{prefix}-{n}"),
                        &format!("{prefix}.{n}.mkv"),
                        &format!("https://x.download.real-debrid.com/{prefix}-{n}"),
                    )
                })
                .collect(),
        )
    }

    fn titles(page: &Page) -> Vec<String> {
        page.items
            .iter()
            .map(|item| match item {
                PageItem::Media(m) => m.title.clone(),
                other => panic!("expected media, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn search_matches_every_word_in_any_order_ignoring_case_and_separators() {
        let name = "Coyote.vs.Acme.2026.2160p.WEB-DL.mkv";
        assert!(rd::Query::new("coyote acme").matches(name));
        assert!(rd::Query::new("ACME Coyote").matches(name));
        assert!(rd::Query::new("coyote vs. acme (2026)").matches(name));
        assert!(rd::Query::new("2160").matches(name));
        assert!(rd::Query::new("web dl").matches(name));
        assert!(!rd::Query::new("coyote roadrunner").matches(name));
        assert!(!rd::Query::new("2025").matches(name));
    }

    #[test]
    fn search_scans_the_history_and_returns_only_matches() {
        let mut host = rd_host();
        let mut first = history("Other", 100);
        first[3] = entry(
            "m1",
            "Coyote.vs.Acme.2026.mkv",
            "https://x.download.real-debrid.com/m1",
        );
        first[60] = entry(
            "m2",
            "coyote_vs_acme_extras.mp4",
            "https://x.download.real-debrid.com/m2",
        );
        host.route(&search_url(1), 200, first);
        let mut last = history("More", 7);
        last[6] = entry(
            "m3",
            "Acme Coyote Shorts.mkv",
            "https://x.download.real-debrid.com/m3",
        );
        host.route(&search_url(2), 200, last);

        let page = rd::search(&host, "coyote acme", None).unwrap();
        assert_eq!(
            titles(&page),
            [
                "Coyote.vs.Acme.2026.mkv",
                "coyote_vs_acme_extras.mp4",
                "Acme Coyote Shorts.mkv"
            ]
        );
        // Page 2 was short: the whole history has been searched.
        assert_eq!(page.cursor_next, None);

        // Matches resolve from the cache, without another scan.
        let before = host.http_calls.lock().unwrap().len();
        let ResolveResult::Playables(sources) = rd::resolve(&host, "m3").unwrap() else {
            panic!("expected playables");
        };
        assert_eq!(sources[0].url, "https://x.download.real-debrid.com/m3");
        assert_eq!(host.http_calls.lock().unwrap().len(), before);
    }

    #[test]
    fn search_reads_a_bounded_number_of_pages_per_call() {
        let mut host = rd_host();
        for page in 1..=rd::SEARCH_SCAN_PAGES + 1 {
            host.route(&search_url(page), 200, history(&format!("P{page}"), 100));
        }
        // Past the end of the history.
        host.route(&search_url(rd::SEARCH_SCAN_PAGES + 2), 200, json!([]));
        let page = rd::search(&host, "nothing matches", None).unwrap();
        assert!(page.items.is_empty());
        let next = (rd::SEARCH_SCAN_PAGES + 1).to_string();
        assert_eq!(page.cursor_next.as_deref(), Some(next.as_str()));
        assert_eq!(
            host.http_calls.lock().unwrap().len(),
            rd::SEARCH_SCAN_PAGES as usize
        );

        // "Load more" carries on from the next unread page.
        let last = rd::SEARCH_SCAN_PAGES + 1;
        let more = rd::search(&host, &format!("p{last}.71"), Some(&next)).unwrap();
        assert_eq!(titles(&more), [format!("P{last}.71.mkv")]);
    }

    #[test]
    fn a_query_without_words_finds_nothing_without_calling_the_api() {
        let host = rd_host();
        let page = rd::search(&host, " .-_ ", None).unwrap();
        assert!(page.items.is_empty());
        assert_eq!(page.cursor_next, None);
        assert!(host.http_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn dispatch_answers_search_with_a_page() {
        let mut host = rd_host();
        host.route(
            &search_url(1),
            200,
            json!([entry(
                "d1",
                "Movie.One.2024.mkv",
                "https://x.download.real-debrid.com/d1"
            )]),
        );
        let out = serde_json::to_value(
            dispatch(
                br#"{"op":"search","query":"movie one","cursor":null}"#,
                &host,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(out["kind"], "page");
        assert_eq!(out["value"]["items"].as_array().unwrap().len(), 1);
    }
}
