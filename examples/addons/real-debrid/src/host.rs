//! Guest-side view of the RVAH host services (ABI v0.1): every service is
//! one JSON request through `rvah_host_call`, answered as `{"Ok":…}` or
//! `{"Err":AddonError}`. A [`Host`] trait keeps the logic testable natively.

use std::collections::BTreeMap;

use rvah_addon_sdk::{AddonError, AddonErrorCode, HttpResponse};
use serde_json::{json, Value};

pub trait Host {
    fn call(&self, request: Value) -> Result<Value, AddonError>;
}

fn internal(msg: &str) -> AddonError {
    AddonError::new(AddonErrorCode::Internal, msg)
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
    serde_json::from_value(resp).map_err(|e| internal(&format!("bad http response: {e}")))
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
    serde_json::from_value(resp).map_err(|e| internal(&format!("bad http response: {e}")))
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
    Ok(resp["value"].as_str().map(str::to_string))
}

pub fn storage_get(host: &dyn Host, key: &str) -> Result<Option<Vec<u8>>, AddonError> {
    let resp = host.call(json!({ "fn": "storage_get", "key": key }))?;
    match &resp["value"] {
        Value::Null => Ok(None),
        value => {
            serde_json::from_value(value.clone()).map_err(|e| internal(&format!("storage: {e}")))
        }
    }
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
    fn call(&self, request: Value) -> Result<Value, AddonError> {
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
        let envelope: Value = serde_json::from_slice(response)
            .map_err(|e| internal(&format!("decode host response: {e}")))?;
        if let Some(err) = envelope.get("Err") {
            return Err(serde_json::from_value(err.clone())
                .unwrap_or_else(|_| internal("undecodable host error")));
        }
        envelope
            .get("Ok")
            .cloned()
            .ok_or_else(|| internal("host response missing Ok/Err"))
    }
}
