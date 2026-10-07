#![cfg(all(feature = "wasm", feature = "wasm-wasmi"))]

use std::sync::Arc;
use std::time::Duration;

use rvah_addon_host::{
    AddonOp, AddonRuntime, InvocationCtx, WasmLimits, WasmRuntime, WasmiLimits, WasmiRuntime,
};
use rvah_addon_sdk::{AddonError, HostApi, HttpRequest, HttpResponse, LogLevel};

struct NullHost;

impl HostApi for NullHost {
    fn http_fetch(&self, _req: HttpRequest) -> Result<HttpResponse, AddonError> {
        unreachable!()
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

fn fixture() -> String {
    let response = r#"{"Ok":{"kind":"page","value":{"items":[]}}}"#;
    let escaped = response.replace('\\', "\\\\").replace('"', "\\\"");
    let packed = (((1024_u64) << 32) | response.len() as u64) as i64;
    format!(
        r#"(module
  (memory (export "memory") 1)
  (data (i32.const 1024) "{escaped}")
  (func (export "rvah_alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "rvah_invoke") (param i32 i32) (result i64) (i64.const {packed})))"#
    )
}

#[test]
fn wasmtime_and_wasmi_return_identical_abi_response() {
    let bytes = fixture();
    let op = AddonOp::List {
        path: None,
        cursor: None,
    };
    let ctx = InvocationCtx {
        timeout: Duration::from_secs(1),
        host: Arc::new(NullHost),
    };
    let desktop = WasmRuntime::load(bytes.as_bytes(), WasmLimits::default())
        .unwrap()
        .invoke(op.clone(), &ctx)
        .unwrap();
    let android = WasmiRuntime::load(bytes.as_bytes(), WasmiLimits::default())
        .unwrap()
        .invoke(op, &ctx)
        .unwrap();
    assert_eq!(desktop, android);
}
