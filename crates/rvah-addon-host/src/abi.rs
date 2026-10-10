//! Shared pieces of the `.rvah` guest ABI v0.1 (see `wasm_runtime` for the
//! contract). Both sandboxes (Wasmtime on desktop, Wasmi on Android) use
//! this single implementation so their host-call semantics cannot drift.

use rvah_addon_sdk::{AddonError, AddonErrorCode, HostApi, LogLevel};

/// Guest memory range for a wasm32 `(ptr, len)` pair. Both are unsigned in
/// the guest's view, and the sum is checked so hostile values cannot
/// overflow (or panic in debug builds) on any host.
pub(crate) fn guest_range(ptr: i32, len: i32) -> Option<std::ops::Range<usize>> {
    let start = ptr as u32 as usize;
    Some(start..start.checked_add(len as u32 as usize)?)
}

fn parse_level(level: Option<&str>) -> LogLevel {
    match level {
        Some("error") => LogLevel::Error,
        Some("warn") => LogLevel::Warn,
        Some("debug") => LogLevel::Debug,
        Some("trace") => LogLevel::Trace,
        _ => LogLevel::Info,
    }
}

/// ABI v0.1 host-call dispatch: JSON request in, JSON `{"Ok":…}/{"Err":…}`
/// response out. Pure translation — capability and permission policy lives
/// entirely in the [`HostApi`] implementation (the bridge).
pub(crate) fn dispatch_host_call(host: &dyn HostApi, request: &[u8]) -> Vec<u8> {
    use rvah_addon_sdk::HttpRequest;
    use serde_json::{json, Value};

    #[derive(serde::Deserialize)]
    #[serde(tag = "fn", rename_all = "snake_case")]
    enum HostCall {
        HttpFetch {
            req: HttpRequest,
        },
        SettingGet {
            key: String,
        },
        SecretGet {
            name: String,
        },
        StorageGet {
            key: String,
        },
        StorageSet {
            key: String,
            value: Vec<u8>,
        },
        StorageDelete {
            key: String,
        },
        Log {
            #[serde(default)]
            level: Option<String>,
            message: String,
        },
        /// The time, in Unix seconds: add-ons have no clock of their own.
        Now,
    }

    fn ok(value: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({ "Ok": value })).expect("host response serializes")
    }
    fn err(error: &AddonError) -> Vec<u8> {
        serde_json::to_vec(&json!({ "Err": error })).expect("host error serializes")
    }

    let call: HostCall = match serde_json::from_slice(request) {
        Ok(call) => call,
        Err(e) => {
            return err(&AddonError::new(
                AddonErrorCode::Malformed,
                format!("host call: {e}"),
            ))
        }
    };
    match call {
        HostCall::HttpFetch { req } => match host.http_fetch(req) {
            Ok(resp) => ok(serde_json::to_value(resp).expect("response serializes")),
            Err(e) => err(&e),
        },
        HostCall::SettingGet { key } => ok(json!({ "value": host.setting_get(&key) })),
        HostCall::SecretGet { name } => match host.secret_get(&name) {
            Ok(value) => ok(json!({ "value": value })),
            Err(e) => err(&e),
        },
        HostCall::StorageGet { key } => ok(json!({ "value": host.storage_get(&key) })),
        HostCall::StorageSet { key, value } => match host.storage_set(&key, &value) {
            Ok(()) => ok(Value::Null),
            Err(e) => err(&e),
        },
        HostCall::StorageDelete { key } => {
            host.storage_delete(&key);
            ok(Value::Null)
        }
        HostCall::Log { level, message } => {
            host.log(parse_level(level.as_deref()), &message);
            ok(Value::Null)
        }
        HostCall::Now => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            ok(json!({ "value": now }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvah_addon_sdk::{HttpRequest, HttpResponse};

    struct Silent;
    impl HostApi for Silent {
        fn http_fetch(&self, _req: HttpRequest) -> Result<HttpResponse, AddonError> {
            Err(AddonError::new(
                AddonErrorCode::PermissionDenied,
                "no network",
            ))
        }
        fn setting_get(&self, _key: &str) -> Option<String> {
            None
        }
        fn secret_get(&self, _name: &str) -> Result<Option<String>, AddonError> {
            Ok(None)
        }
        fn storage_get(&self, _key: &str) -> Option<Vec<u8>> {
            None
        }
        fn storage_set(&self, _key: &str, _value: &[u8]) -> Result<(), AddonError> {
            Ok(())
        }
        fn storage_delete(&self, _key: &str) {}
        fn log(&self, _level: LogLevel, _message: &str) {}
    }

    #[test]
    fn add_ons_can_ask_the_time() {
        let answer: serde_json::Value =
            serde_json::from_slice(&dispatch_host_call(&Silent, br#"{"fn":"now"}"#)).unwrap();
        let now = answer["Ok"]["value"].as_u64().unwrap();
        // After 2026-01-01, before 2100.
        assert!((1_767_225_600..4_102_444_800).contains(&now), "{now}");
    }
}
