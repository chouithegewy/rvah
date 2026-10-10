//! Guest-side view of the RVAH host services (ABI v0.1): every service is
//! one JSON request through `rvah_host_call`, answered as `{"Ok":…}` or
//! `{"Err":AddonError}`. A [`Host`] trait keeps the logic testable natively.

use std::collections::BTreeMap;

use rvah_addon_sdk::{AddonError, AddonErrorCode, HttpResponse};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

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
pub(crate) fn byte_array(json: &str) -> Option<Vec<u8>> {
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
