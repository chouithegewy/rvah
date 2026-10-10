//! Guest side of ABI v0.1, for add-ons compiled to wasm (feature `guest`).
//!
//! Every host service is one JSON request through the `env.rvah_host_call`
//! import, answered as `{"Ok":…}` or `{"Err":AddonError}`. The [`Host`]
//! trait keeps add-on logic testable natively with a scripted host; on
//! wasm32, [`WasmHost`] is the real one. [`Op`] and [`Response`] are the
//! wire forms of the host's `AddonOp` and `AddonResponse`, and
//! [`export_guest!`](crate::export_guest) defines the two exports the host
//! calls, `rvah_alloc` and `rvah_invoke`.
//!
//! A call gets limited fuel, so answers are decoded straight into their
//! types, never through `serde_json::Value`, and an HTTP body (a JSON array
//! of byte values) is read once, by a small hand-written parser.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    AccountStatus, AddonError, AddonErrorCode, AuthPoll, Credentials, DeviceAuth, Filter,
    HttpResponse, ItemDetails, Page, ResolveResult,
};

pub trait Host {
    /// One host call: the `Ok` payload as JSON text, still undecoded.
    fn call(&self, request: Value) -> Result<Vec<u8>, AddonError>;
}

fn internal(msg: &str) -> AddonError {
    AddonError::new(AddonErrorCode::Internal, msg)
}

/// Decode straight into `T`, never through `serde_json::Value`: fuel per
/// call is limited, and it bounds how much of the history one call reads.
fn decode<T: DeserializeOwned>(payload: &[u8], what: &str) -> Result<T, AddonError> {
    serde_json::from_slice(payload).map_err(|e| internal(&format!("bad {what}: {e}")))
}

/// An `http_fetch` answer with the body left as JSON text, for [`byte_array`].
#[derive(Deserialize)]
struct RawHttpResponse<'a> {
    status: u16,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(borrow, default)]
    body: Option<&'a serde_json::value::RawValue>,
}

fn http_response(payload: &[u8]) -> Result<HttpResponse, AddonError> {
    let bad_body = || internal("bad http response body");
    // Only `byte_array` should read a long body, once: the decoder would
    // scan it again just to skip it.
    let (head, body) = match split_body(payload) {
        Some((head, body)) => (head, Some(byte_array(body).ok_or_else(bad_body)?)),
        None => (payload.to_vec(), None),
    };
    let raw: RawHttpResponse =
        serde_json::from_slice(&head).map_err(|e| internal(&format!("bad http response: {e}")))?;
    let body = match (body, raw.body) {
        (Some(body), _) => body,
        (None, Some(body)) => byte_array(body.get()).ok_or_else(bad_body)?,
        (None, None) => Vec::new(),
    };
    Ok(HttpResponse {
        status: raw.status,
        headers: raw.headers,
        body,
    })
}

/// The answer without its body, and the body array's text, when the host
/// wrote the body compactly as the first field (`{"body":[1,2],…}`, sorted
/// keys) or the last (`{…,"body":[1,2]}`, field order); otherwise `None`.
fn split_body(payload: &[u8]) -> Option<(Vec<u8>, &str)> {
    let is_item = |c: &&u8| c.is_ascii_digit() || **c == b',';
    let (array, head) = if let Some(rest) = payload.strip_prefix(br#"{"body":"#) {
        let len = 1 + rest.get(1..)?.iter().take_while(is_item).count() + 1;
        let (array, after) = rest.split_at_checked(len)?;
        let mut head = b"{".to_vec();
        match after {
            [b'}'] => head.push(b'}'),
            [b',', fields @ ..] => head.extend_from_slice(fields),
            _ => return None,
        }
        (array, head)
    } else {
        let rest = payload.strip_suffix(b"}")?;
        let len = 1
            + rest
                .strip_suffix(b"]")?
                .iter()
                .rev()
                .take_while(is_item)
                .count()
            + 1;
        let (fields, array) = rest.split_at_checked(rest.len().checked_sub(len)?)?;
        let mut head = fields.strip_suffix(br#","body":"#)?.to_vec();
        head.push(b'}');
        (array, head)
    };
    if array.first() != Some(&b'[') || array.last() != Some(&b']') {
        return None;
    }
    Some((head, std::str::from_utf8(array).ok()?))
}

/// `[91,123,34]` → bytes. The ABI sends a body as a JSON array of numbers,
/// and serde's generic sequence path spends several times the fuel per
/// byte of this loop; bodies are most of what a call reads.
fn byte_array(json: &str) -> Option<Vec<u8>> {
    let inner = json.trim().strip_prefix('[')?.strip_suffix(']')?;
    let mut bytes = Vec::with_capacity(inner.len() / 3);
    // The number being read, and whether the current item has one yet.
    let mut value: u16 = 0;
    let mut digits = false;
    let mut ended = false;
    for &c in inner.as_bytes() {
        match c {
            b'0'..=b'9' if !ended => {
                value = value * 10 + u16::from(c - b'0');
                if value > 255 {
                    return None;
                }
                digits = true;
            }
            b',' if digits => {
                bytes.push(value as u8);
                value = 0;
                digits = false;
                ended = false;
            }
            b' ' | b'\n' | b'\r' | b'\t' => ended = digits,
            _ => return None,
        }
    }
    if digits {
        bytes.push(value as u8);
    } else if !inner.trim().is_empty() {
        return None;
    }
    Some(bytes)
}

/// `{"value": …}`, the shape of the secret and storage answers.
#[derive(Deserialize)]
struct Answer<T> {
    value: Option<T>,
}

pub fn http_get(
    host: &dyn Host,
    url: &str,
    headers: BTreeMap<String, String>,
) -> Result<HttpResponse, AddonError> {
    let resp = host.call(json!({
        "fn": "http_fetch",
        "req": { "method": "GET", "url": url, "headers": headers },
    }))?;
    http_response(&resp)
}

/// POST `fields` as `application/x-www-form-urlencoded`.
pub fn http_post_form(
    host: &dyn Host,
    url: &str,
    fields: &[(&str, &str)],
) -> Result<HttpResponse, AddonError> {
    let body = form_encode(fields).into_bytes();
    let resp = host.call(json!({
        "fn": "http_fetch",
        "req": {
            "method": "POST",
            "url": url,
            "headers": { "content-type": "application/x-www-form-urlencoded" },
            "body": body,
        },
    }))?;
    http_response(&resp)
}

/// `k=v&k=v` with RFC 3986 unreserved characters kept and everything else
/// percent-encoded; also safe for query strings.
pub fn form_encode(fields: &[(&str, &str)]) -> String {
    fn encode(value: &str) -> String {
        value
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    }
    fields
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

pub fn secret_get(host: &dyn Host, name: &str) -> Result<Option<String>, AddonError> {
    let resp = host.call(json!({ "fn": "secret_get", "name": name }))?;
    Ok(decode::<Answer<String>>(&resp, "secret")?.value)
}

/// The add-on's setting `key` (its manifest default until the user changes it).
pub fn setting_get(host: &dyn Host, key: &str) -> Result<Option<String>, AddonError> {
    let resp = host.call(json!({ "fn": "setting_get", "key": key }))?;
    Ok(decode::<Answer<String>>(&resp, "setting")?.value)
}

pub fn storage_get(host: &dyn Host, key: &str) -> Result<Option<Vec<u8>>, AddonError> {
    let resp = host.call(json!({ "fn": "storage_get", "key": key }))?;
    Ok(decode::<Answer<Vec<u8>>>(&resp, "storage")?.value)
}

pub fn storage_set(host: &dyn Host, key: &str, value: &[u8]) -> Result<(), AddonError> {
    host.call(json!({ "fn": "storage_set", "key": key, "value": value }))?;
    Ok(())
}

pub fn log(host: &dyn Host, message: &str) {
    let _ = host.call(json!({ "fn": "log", "level": "info", "message": message }));
}

/// The real host, reached through the wasm import.
#[cfg(target_arch = "wasm32")]
pub struct WasmHost;

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    fn rvah_host_call(ptr: i32, len: i32) -> i64;
}

#[cfg(target_arch = "wasm32")]
impl Host for WasmHost {
    fn call(&self, request: Value) -> Result<Vec<u8>, AddonError> {
        let bytes = serde_json::to_vec(&request)
            .map_err(|e| internal(&format!("encode host call: {e}")))?;
        let packed = unsafe { rvah_host_call(bytes.as_ptr() as i32, bytes.len() as i32) };
        if packed == 0 {
            return Err(internal("host call plumbing failed"));
        }
        let ptr = (packed >> 32) as u32 as usize;
        let len = packed as u32 as usize;
        // The host wrote the response through our own rvah_alloc.
        let response = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
        ok_payload(response)
    }
}

/// The payload of a host answer, still undecoded. The host writes
/// `{"Ok":…}` compactly, so the payload is cut out without scanning it (a
/// scan of a long body costs as much fuel as decoding it); anything else
/// is decoded in full.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn ok_payload(response: &[u8]) -> Result<Vec<u8>, AddonError> {
    if let Some(payload) = response
        .strip_prefix(br#"{"Ok":"#)
        .and_then(|rest| rest.strip_suffix(b"}"))
    {
        return Ok(payload.to_vec());
    }
    #[derive(Deserialize)]
    enum Envelope<'a> {
        Ok(#[serde(borrow)] &'a serde_json::value::RawValue),
        Err(AddonError),
    }
    let envelope: Envelope = serde_json::from_slice(response)
        .map_err(|e| internal(&format!("decode host response: {e}")))?;
    match envelope {
        Envelope::Ok(payload) => Ok(payload.get().as_bytes().to_vec()),
        Envelope::Err(err) => Err(err),
    }
}

/// A scripted [`Host`] for add-on unit tests: canned HTTP answers by exact
/// URL (anything else is a 404), in-memory secrets, settings and storage,
/// and a record of every request. Answers are encoded as the real host
/// encodes them, so the decoders above are exercised too.
#[derive(Default)]
pub struct ScriptedHost {
    pub http: BTreeMap<String, (u16, Vec<u8>)>,
    pub secrets: BTreeMap<String, String>,
    pub settings: BTreeMap<String, String>,
    pub storage: std::sync::Mutex<BTreeMap<String, Vec<u8>>>,
    requests: std::sync::Mutex<Vec<crate::HttpRequest>>,
}

impl ScriptedHost {
    /// Answer GETs of `url` with `status` and `body` as JSON.
    pub fn route(&mut self, url: &str, status: u16, body: &impl Serialize) {
        let body = serde_json::to_vec(body).expect("route body serializes");
        self.http.insert(url.to_string(), (status, body));
    }

    /// Every request so far, in order.
    pub fn requests(&self) -> Vec<crate::HttpRequest> {
        self.requests.lock().expect("requests lock").clone()
    }

    /// The URL of every request so far, in order.
    pub fn urls(&self) -> Vec<String> {
        self.requests().into_iter().map(|r| r.url).collect()
    }
}

impl Host for ScriptedHost {
    fn call(&self, request: Value) -> Result<Vec<u8>, AddonError> {
        let answer = match request["fn"].as_str() {
            Some("http_fetch") => {
                let req: crate::HttpRequest = serde_json::from_value(request["req"].clone())
                    .map_err(|e| internal(&format!("scripted host: {e}")))?;
                let (status, body) = self
                    .http
                    .get(&req.url)
                    .cloned()
                    .unwrap_or((404, b"not routed".to_vec()));
                self.requests.lock().expect("requests lock").push(req);
                serde_json::to_value(HttpResponse {
                    status,
                    headers: BTreeMap::new(),
                    body,
                })
                .expect("answer serializes")
            }
            Some("secret_get") => {
                json!({ "value": request["name"].as_str().and_then(|n| self.secrets.get(n)) })
            }
            Some("setting_get") => {
                json!({ "value": request["key"].as_str().and_then(|k| self.settings.get(k)) })
            }
            Some("storage_get") => {
                let storage = self.storage.lock().expect("storage lock");
                json!({ "value": request["key"].as_str().and_then(|k| storage.get(k)) })
            }
            Some("storage_set") => {
                let value: Vec<u8> = serde_json::from_value(request["value"].clone())
                    .map_err(|e| internal(&format!("scripted host: {e}")))?;
                let key = request["key"].as_str().unwrap_or_default().to_string();
                self.storage
                    .lock()
                    .expect("storage lock")
                    .insert(key, value);
                Value::Null
            }
            Some("log") => Value::Null,
            other => return Err(internal(&format!("scripted host: no {other:?}"))),
        };
        // Encoded like the real host: sorted keys, compact.
        Ok(serde_json::to_vec(&answer).expect("answer serializes"))
    }
}

/// The host's `AddonOp` as a guest reads it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    List {
        path: Option<String>,
        cursor: Option<String>,
    },
    Search {
        query: String,
        #[serde(default)]
        filters: Vec<Filter>,
        cursor: Option<String>,
    },
    Resolve {
        item_id: String,
        source_id: Option<String>,
    },
    AccountStatus,
    AuthBegin,
    AuthPoll {
        device_code: String,
    },
    AuthRefresh,
    Details {
        item_id: String,
    },
}

/// A guest's answer, in the host's `AddonResponse` wire form.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Response {
    Page(Page),
    Resolve(ResolveResult),
    Account(AccountStatus),
    DeviceAuth(DeviceAuth),
    AuthPoll(AuthPoll),
    Credentials(Credentials),
    Details(Box<ItemDetails>),
}

/// The op the host sent, or a `Malformed` error.
pub fn read_op(input: &[u8]) -> Result<Op, AddonError> {
    serde_json::from_slice(input)
        .map_err(|e| AddonError::new(AddonErrorCode::Malformed, format!("op: {e}")))
}

/// `rvah_alloc`: a buffer for the host to write into. Leaked on purpose:
/// the store lives for one invocation only.
pub fn alloc(len: i32) -> i32 {
    let mut buf = vec![0u8; len.max(0) as usize];
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr as i32
}

/// `rvah_invoke`: hand the op the host wrote at `ptr` to `dispatch`, and
/// return the answer's address and length packed into one `i64`.
#[cfg(target_arch = "wasm32")]
pub fn invoke(
    ptr: i32,
    len: i32,
    dispatch: impl FnOnce(&[u8], &dyn Host) -> Result<Response, AddonError>,
) -> i64 {
    let input = unsafe { std::slice::from_raw_parts(ptr as *const u8, len.max(0) as usize) };
    let result = dispatch(input, &WasmHost);
    let out = serde_json::to_vec(&result).unwrap_or_default();
    if out.is_empty() {
        return 0;
    }
    let out_ptr = alloc(out.len() as i32);
    unsafe {
        std::ptr::copy_nonoverlapping(out.as_ptr(), out_ptr as *mut u8, out.len());
    }
    (((out_ptr as u64) << 32) | out.len() as u64) as i64
}

/// Define the ABI exports, `rvah_alloc` and (on wasm32) `rvah_invoke`,
/// around `dispatch: fn(&[u8], &dyn Host) -> Result<Response, AddonError>`.
#[macro_export]
macro_rules! export_guest {
    ($dispatch:path) => {
        #[no_mangle]
        pub extern "C" fn rvah_alloc(len: i32) -> i32 {
            $crate::guest::alloc(len)
        }

        #[cfg(target_arch = "wasm32")]
        #[no_mangle]
        pub extern "C" fn rvah_invoke(ptr: i32, len: i32) -> i64 {
            $crate::guest::invoke(ptr, len, $dispatch)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_arrays_decode_like_serde_does() {
        let all: Vec<u8> = (0..=255).collect();
        for bytes in [Vec::new(), vec![0], vec![255, 0, 9], all] {
            let json = serde_json::to_string(&bytes).unwrap();
            assert_eq!(byte_array(&json), Some(bytes));
        }
        assert_eq!(
            byte_array(" [ 1, 22 ,\n333 ] ".replace("333", "200").as_str()),
            Some(vec![1, 22, 200])
        );
    }

    #[test]
    fn malformed_byte_arrays_are_rejected() {
        for bad in [
            "", "[", "1,2", "[256]", "[1,]", "[,1]", "[1,,2]", "[1 2]", "[-1]", "[1.5]", "[\"a\"]",
            "[1e2]",
        ] {
            assert_eq!(byte_array(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn host_answers_yield_their_payload_or_error() {
        assert_eq!(
            ok_payload(br#"{"Ok":{"value":"x"}}"#).unwrap(),
            br#"{"value":"x"}"#
        );
        assert_eq!(ok_payload(br#"{"Ok":null}"#).unwrap(), b"null");
        assert_eq!(ok_payload(br#"{ "Ok" : [1] }"#).unwrap(), b"[1]");
        let err = ok_payload(
            br#"{"Err":{"code":"auth","message":"no","retryable":false,"user_visible":true}}"#,
        )
        .unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Auth);
        assert!(ok_payload(b"garbage").is_err());
    }

    #[test]
    fn the_scripted_host_answers_like_the_real_one() {
        let mut host = ScriptedHost::default();
        host.route("https://api.example.com/a", 200, &json!({ "ok": true }));
        host.secrets.insert("key".into(), "s3cret".into());
        host.settings.insert("region".into(), "US".into());
        let resp = http_get(&host, "https://api.example.com/a", BTreeMap::new()).unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, br#"{"ok":true}"#);
        assert_eq!(
            http_get(&host, "https://api.example.com/b", BTreeMap::new())
                .unwrap()
                .status,
            404
        );
        assert_eq!(
            host.urls(),
            ["https://api.example.com/a", "https://api.example.com/b"]
        );
        assert_eq!(secret_get(&host, "key").unwrap().as_deref(), Some("s3cret"));
        assert_eq!(secret_get(&host, "other").unwrap(), None);
        assert_eq!(setting_get(&host, "region").unwrap().as_deref(), Some("US"));
        storage_set(&host, "k", b"v").unwrap();
        assert_eq!(storage_get(&host, "k").unwrap().as_deref(), Some(&b"v"[..]));
    }

    #[test]
    fn http_answers_keep_status_headers_and_body() {
        let payload = br#"{"status":404,"headers":{"x-total-count":"7"},"body":[104,105]}"#;
        let resp = http_response(payload).unwrap();
        assert_eq!(resp.status, 404);
        assert_eq!(resp.headers["x-total-count"], "7");
        assert_eq!(resp.body, b"hi");
        assert!(http_response(br#"{"status":204}"#).unwrap().body.is_empty());
        let resp = http_response(br#"{"status":200,"body":[]}"#).unwrap();
        assert!(resp.body.is_empty());
    }

    #[test]
    fn the_body_is_cut_off_without_being_scanned() {
        for (payload, head) in [
            (
                &br#"{"body":[1,22],"headers":{"a":"b"},"status":200}"#[..],
                &br#"{"headers":{"a":"b"},"status":200}"#[..],
            ),
            (br#"{"body":[1,22]}"#, br#"{}"#),
            (
                br#"{"status":200,"headers":{"a":"b"},"body":[1,22]}"#,
                br#"{"status":200,"headers":{"a":"b"}}"#,
            ),
        ] {
            let (cut, body) = split_body(payload).unwrap();
            assert_eq!(cut, head, "{}", String::from_utf8_lossy(payload));
            assert_eq!(body, "[1,22]");
        }
        // A header that looks like a body is still a header.
        let tricky = br#"{"body":[7],"headers":{"x":",\"body\":[9]"},"status":200}"#;
        let resp = http_response(tricky).unwrap();
        assert_eq!(resp.headers["x"], r#","body":[9]"#);
        assert_eq!(resp.body, [7]);
        let tricky = br#"{"headers":{"x":"{\"body\":[9]"},"status":200}"#;
        assert!(split_body(tricky).is_none());
        assert!(http_response(tricky).unwrap().body.is_empty());
        // Not a compact shape: decoded in full instead.
        for other in [
            &br#"{"status":200}"#[..],
            br#"{"headers":{},"body":[1],"status":200}"#,
            br#"{"status":200, "body": [1] }"#,
            br#"{"body":[1 ],"status":200}"#,
        ] {
            assert!(
                split_body(other).is_none(),
                "{}",
                String::from_utf8_lossy(other)
            );
            http_response(other).unwrap();
        }
    }
}
