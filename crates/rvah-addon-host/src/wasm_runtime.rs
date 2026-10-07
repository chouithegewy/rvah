//! Best-effort Wasmtime sandbox implementing [`AddonRuntime`] (FR-020).
//!
//! ABI v0: the guest exports `memory`, `rvah_alloc(len: i32) -> i32`, and
//! `rvah_invoke(ptr: i32, len: i32) -> i64` (packed `ptr << 32 | len` of a
//! JSON `Result<AddonResponse, AddonError>`); the host passes a JSON
//! [`AddonOp`]. The optional import `env.rvah_host_call(ptr, len) -> i64`
//! dispatches the full JSON-serialized [`HostApi`] (ABI v0.1): `http_fetch`,
//! `setting_get`, `secret_get`, `storage_get/set/delete`, `log`. Responses
//! are written back through the guest's `rvah_alloc` and returned packed.
//!
//! Enforcement: `StoreLimits` caps guest memory (TC-RUN-005); fuel metering
//! plus an epoch watchdog bound CPU and wall-clock time (TC-RUN-004).

use std::sync::Arc;
use std::time::Duration;

use rvah_addon_sdk::{AddonError, AddonErrorCode, HostApi};
use wasmtime::{
    Caller, Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder, Trap,
};

use crate::abi::{dispatch_host_call, guest_range};
use crate::runtime::{AddonOp, AddonResponse, AddonRuntime, InvocationCtx};

#[derive(Debug, Clone, Copy)]
pub struct WasmLimits {
    pub max_memory_bytes: usize,
    pub fuel: u64,
    pub timeout: Duration,
}

impl Default for WasmLimits {
    fn default() -> Self {
        Self {
            max_memory_bytes: 16 * 1024 * 1024,
            fuel: 50_000_000,
            timeout: Duration::from_secs(2),
        }
    }
}

struct StoreData {
    limits: StoreLimits,
    host: Arc<dyn HostApi>,
}

pub struct WasmRuntime {
    engine: Engine,
    module: Module,
    limits: WasmLimits,
}

fn internal(msg: impl std::fmt::Display) -> AddonError {
    AddonError::new(AddonErrorCode::Internal, msg.to_string())
}

fn trap_to_error(e: wasmtime::Error) -> AddonError {
    match e.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => AddonError::new(AddonErrorCode::Timeout, "fuel exhausted"),
        Some(Trap::Interrupt) => AddonError::new(AddonErrorCode::Timeout, "wall-clock deadline"),
        _ => internal(format!("guest trapped: {e}")),
    }
}

impl WasmRuntime {
    /// Compile a module (`.wasm` bytes or WAT text) under sandbox limits.
    pub fn load(module_bytes: &[u8], limits: WasmLimits) -> Result<Self, AddonError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config).map_err(internal)?;
        let module = Module::new(&engine, module_bytes)
            .map_err(|e| AddonError::new(AddonErrorCode::Malformed, format!("bad module: {e}")))?;
        Ok(Self {
            engine,
            module,
            limits,
        })
    }
}

/// FFI shim: read the request from guest memory, dispatch, write the
/// response back through the guest's own allocator, return packed ptr/len.
/// `0` signals a plumbing failure (bad pointers, missing exports).
fn host_call(caller: &mut Caller<'_, StoreData>, ptr: i32, len: i32) -> i64 {
    let Some(wasmtime::Extern::Memory(memory)) = caller.get_export("memory") else {
        return 0;
    };
    let Some(request) = guest_range(ptr, len)
        .and_then(|range| memory.data(&caller).get(range))
        .map(|b| b.to_vec())
    else {
        return 0;
    };
    let host = Arc::clone(&caller.data().host);
    let response = dispatch_host_call(host.as_ref(), &request);

    let Some(wasmtime::Extern::Func(alloc)) = caller.get_export("rvah_alloc") else {
        return 0;
    };
    let Ok(alloc) = alloc.typed::<i32, i32>(&caller) else {
        return 0;
    };
    let Ok(response_ptr) = alloc.call(&mut *caller, response.len() as i32) else {
        return 0;
    };
    if memory
        .write(&mut *caller, response_ptr as usize, &response)
        .is_err()
    {
        return 0;
    }
    (((response_ptr as u64) << 32) | response.len() as u64) as i64
}

impl AddonRuntime for WasmRuntime {
    fn invoke(&self, op: AddonOp, ctx: &InvocationCtx) -> Result<AddonResponse, AddonError> {
        let data = StoreData {
            limits: StoreLimitsBuilder::new()
                .memory_size(self.limits.max_memory_bytes)
                .build(),
            host: Arc::clone(&ctx.host),
        };
        let mut store = Store::new(&self.engine, data);
        store.limiter(|d| &mut d.limits);
        store.set_fuel(self.limits.fuel).map_err(internal)?;
        store.set_epoch_deadline(1);

        // Watchdog: bump the epoch once the wall-clock budget elapses. Firing
        // after a finished invoke is harmless (the store is gone by then).
        let engine = self.engine.clone();
        let timeout = self.limits.timeout.min(ctx.timeout);
        std::thread::spawn(move || {
            std::thread::sleep(timeout);
            engine.increment_epoch();
        });

        let mut linker: Linker<StoreData> = Linker::new(&self.engine);
        linker
            .func_wrap(
                "env",
                "rvah_host_call",
                |mut caller: Caller<'_, StoreData>, ptr: i32, len: i32| -> i64 {
                    host_call(&mut caller, ptr, len)
                },
            )
            .map_err(internal)?;

        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(trap_to_error)?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| internal("guest exports no memory"))?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, "rvah_alloc")
            .map_err(internal)?;
        let invoke = instance
            .get_typed_func::<(i32, i32), i64>(&mut store, "rvah_invoke")
            .map_err(internal)?;

        let payload = serde_json::to_vec(&op).map_err(internal)?;
        let ptr = alloc
            .call(&mut store, payload.len() as i32)
            .map_err(trap_to_error)?;
        memory
            .write(&mut store, ptr as usize, &payload)
            .map_err(|e| internal(format!("payload write: {e}")))?;

        let packed = invoke
            .call(&mut store, (ptr, payload.len() as i32))
            .map_err(trap_to_error)?;
        if packed == 0 {
            return Err(internal("guest returned empty response"));
        }
        let bytes = guest_range((packed >> 32) as i32, packed as i32)
            .and_then(|range| memory.data(&store).get(range))
            .ok_or_else(|| internal("guest response out of bounds"))?;
        serde_json::from_slice::<Result<AddonResponse, AddonError>>(bytes)
            .map_err(|e| AddonError::new(AddonErrorCode::Malformed, format!("guest JSON: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvah_addon_sdk::{HttpRequest, HttpResponse, LogLevel, PageItem};

    struct NullHost;
    impl HostApi for NullHost {
        fn http_fetch(&self, _req: HttpRequest) -> Result<HttpResponse, AddonError> {
            Err(AddonError::new(AddonErrorCode::PermissionDenied, "no"))
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

    fn ctx() -> InvocationCtx {
        InvocationCtx {
            timeout: Duration::from_secs(5),
            host: Arc::new(NullHost),
        }
    }

    fn wat_escape(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }

    fn pack(ptr: u32, len: u32) -> i64 {
        (((ptr as u64) << 32) | len as u64) as i64
    }

    /// Guest fixture equivalent of `fixtures/wasm-echo`, written in WAT so
    /// tests need no wasm32 toolchain: returns a static one-directory page.
    fn echo_wat() -> String {
        let json = r#"{"Ok":{"kind":"page","value":{"items":[{"kind":"directory","id":"w1","title":"Wasm Echo","path":"/w"}]}}}"#;
        format!(
            r#"(module
  (memory (export "memory") 2)
  (data (i32.const 1024) "{data}")
  (global $heap (mut i32) (i32.const 65536))
  (func (export "rvah_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    global.get $heap
    local.set $ptr
    global.get $heap
    local.get $len
    i32.add
    global.set $heap
    local.get $ptr)
  (func (export "rvah_invoke") (param i32 i32) (result i64)
    i64.const {packed}))"#,
            data = wat_escape(json),
            packed = pack(1024, json.len() as u32),
        )
    }

    #[test]
    fn wasm_echo_lists_page() {
        let rt = WasmRuntime::load(
            echo_wat().as_bytes(),
            WasmLimits {
                max_memory_bytes: 1024 * 1024,
                fuel: 1_000_000,
                timeout: Duration::from_secs(1),
            },
        )
        .unwrap();
        let resp = rt
            .invoke(
                AddonOp::List {
                    path: None,
                    cursor: None,
                },
                &ctx(),
            )
            .unwrap();
        let AddonResponse::Page(page) = resp else {
            panic!("expected page, got {resp:?}");
        };
        assert_eq!(page.items.len(), 1);
        let PageItem::Directory(d) = &page.items[0] else {
            panic!("expected directory");
        };
        assert_eq!(d.title, "Wasm Echo");
    }

    #[test]
    fn wasm_infinite_loop_trapped_by_fuel() {
        // TC-RUN-004: a spinning guest burns its fuel and is trapped.
        let wat = r#"(module
  (memory (export "memory") 1)
  (func (export "rvah_alloc") (param i32) (result i32) i32.const 0)
  (func (export "rvah_invoke") (param i32 i32) (result i64)
    (loop $spin br $spin)
    i64.const 0))"#;
        let rt = WasmRuntime::load(
            wat.as_bytes(),
            WasmLimits {
                max_memory_bytes: 1024 * 1024,
                fuel: 100_000,
                timeout: Duration::from_secs(10),
            },
        )
        .unwrap();
        let err = rt
            .invoke(
                AddonOp::List {
                    path: None,
                    cursor: None,
                },
                &ctx(),
            )
            .unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Timeout);
        assert!(err.message.contains("fuel"));
    }

    /// Host with in-memory storage and recorded logs for dispatch tests.
    #[derive(Default)]
    struct RecordingHost {
        storage: std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
        logs: std::sync::Mutex<Vec<String>>,
    }

    impl HostApi for RecordingHost {
        fn http_fetch(&self, _req: HttpRequest) -> Result<HttpResponse, AddonError> {
            Ok(HttpResponse {
                status: 200,
                headers: Default::default(),
                body: b"fetched".to_vec(),
            })
        }
        fn setting_get(&self, key: &str) -> Option<String> {
            (key == "region").then(|| "us".to_string())
        }
        fn secret_get(&self, _name: &str) -> Result<Option<String>, AddonError> {
            Err(AddonError::new(
                AddonErrorCode::PermissionDenied,
                "secret not declared in manifest",
            ))
        }
        fn storage_get(&self, key: &str) -> Option<Vec<u8>> {
            self.storage.lock().unwrap().get(key).cloned()
        }
        fn storage_set(&self, key: &str, value: &[u8]) -> Result<(), AddonError> {
            self.storage
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_vec());
            Ok(())
        }
        fn storage_delete(&self, key: &str) {
            self.storage.lock().unwrap().remove(key);
        }
        fn log(&self, level: LogLevel, message: &str) {
            self.logs
                .lock()
                .unwrap()
                .push(format!("{level:?}:{message}"));
        }
    }

    fn call(host: &dyn HostApi, request: serde_json::Value) -> serde_json::Value {
        let response = dispatch_host_call(host, &serde_json::to_vec(&request).unwrap());
        serde_json::from_slice(&response).unwrap()
    }

    #[test]
    fn dispatch_covers_full_host_api() {
        use serde_json::json;
        let host = RecordingHost::default();

        // http_fetch passes through the bridge.
        let resp = call(
            &host,
            json!({"fn":"http_fetch","req":{"method":"GET","url":"https://api.example.com/v1"}}),
        );
        assert_eq!(resp["Ok"]["status"], 200);

        // settings read manifest-default/stored values.
        let resp = call(&host, json!({"fn":"setting_get","key":"region"}));
        assert_eq!(resp["Ok"]["value"], "us");
        let resp = call(&host, json!({"fn":"setting_get","key":"missing"}));
        assert_eq!(resp["Ok"]["value"], serde_json::Value::Null);

        // secret denial surfaces verbatim (policy stays in the bridge).
        let resp = call(&host, json!({"fn":"secret_get","name":"tok"}));
        assert_eq!(resp["Err"]["code"], "permission_denied");

        // storage roundtrip, then delete.
        let resp = call(&host, json!({"fn":"storage_set","key":"k","value":[1,2,3]}));
        assert!(resp.get("Ok").is_some());
        let resp = call(&host, json!({"fn":"storage_get","key":"k"}));
        assert_eq!(resp["Ok"]["value"], json!([1, 2, 3]));
        call(&host, json!({"fn":"storage_delete","key":"k"}));
        let resp = call(&host, json!({"fn":"storage_get","key":"k"}));
        assert_eq!(resp["Ok"]["value"], serde_json::Value::Null);

        // log recorded with level.
        call(
            &host,
            json!({"fn":"log","level":"warn","message":"careful"}),
        );
        assert_eq!(host.logs.lock().unwrap().as_slice(), ["Warn:careful"]);

        // malformed request is an error, not a crash.
        let resp = call(&host, json!({"fn":"no_such_op"}));
        assert_eq!(resp["Err"]["code"], "malformed");
    }

    #[test]
    fn wasm_guest_host_call_roundtrip() {
        // Guest logs through the import, then returns a static empty page —
        // proves the request-read + alloc + write-back plumbing end to end.
        let log_req = r#"{"fn":"log","level":"info","message":"hello from wasm"}"#;
        let ok_json = r#"{"Ok":{"kind":"page","value":{"items":[]}}}"#;
        let wat = format!(
            r#"(module
  (import "env" "rvah_host_call" (func $host_call (param i32 i32) (result i64)))
  (memory (export "memory") 2)
  (data (i32.const 1024) "{log_data}")
  (data (i32.const 2048) "{ok_data}")
  (global $heap (mut i32) (i32.const 65536))
  (func (export "rvah_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    global.get $heap
    local.set $ptr
    global.get $heap
    local.get $len
    i32.add
    global.set $heap
    local.get $ptr)
  (func (export "rvah_invoke") (param i32 i32) (result i64)
    (drop (call $host_call (i32.const 1024) (i32.const {log_len})))
    i64.const {packed_ok}))"#,
            log_data = wat_escape(log_req),
            ok_data = wat_escape(ok_json),
            log_len = log_req.len(),
            packed_ok = pack(2048, ok_json.len() as u32),
        );
        let rt = WasmRuntime::load(wat.as_bytes(), WasmLimits::default()).unwrap();
        let host = Arc::new(RecordingHost::default());
        let ctx = InvocationCtx {
            timeout: Duration::from_secs(5),
            host: Arc::clone(&host) as Arc<dyn HostApi>,
        };
        let resp = rt
            .invoke(
                AddonOp::List {
                    path: None,
                    cursor: None,
                },
                &ctx,
            )
            .unwrap();
        assert!(matches!(resp, AddonResponse::Page(_)));
        assert_eq!(
            host.logs.lock().unwrap().as_slice(),
            ["Info:hello from wasm"]
        );
    }

    #[test]
    fn hostile_host_call_pointers_do_not_panic_the_host() {
        // ptr = -1 sign-extends to usize::MAX; ptr + len must not overflow.
        let ok_json = r#"{"Ok":{"kind":"page","value":{"items":[]}}}"#;
        let wat = format!(
            r#"(module
  (import "env" "rvah_host_call" (func $host_call (param i32 i32) (result i64)))
  (memory (export "memory") 1)
  (data (i32.const 2048) "{ok_data}")
  (func (export "rvah_alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "rvah_invoke") (param i32 i32) (result i64)
    (drop (call $host_call (i32.const -1) (i32.const 64)))
    (drop (call $host_call (i32.const 65000) (i32.const -1)))
    i64.const {packed_ok}))"#,
            ok_data = wat_escape(ok_json),
            packed_ok = pack(2048, ok_json.len() as u32),
        );
        let rt = WasmRuntime::load(wat.as_bytes(), WasmLimits::default()).unwrap();
        let resp = rt.invoke(
            AddonOp::List {
                path: None,
                cursor: None,
            },
            &ctx(),
        );
        assert!(matches!(resp, Ok(AddonResponse::Page(_))), "{resp:?}");
    }

    #[test]
    fn wasm_memory_hog_capped() {
        // TC-RUN-005: memory.grow beyond StoreLimits fails inside the guest.
        let err_json = r#"{"Err":{"code":"quota","message":"memory grow denied by host cap","retryable":false,"user_visible":false}}"#;
        let ok_json = r#"{"Ok":{"kind":"page","value":{"items":[]}}}"#;
        let wat = format!(
            r#"(module
  (memory (export "memory") 1)
  (data (i32.const 512) "{err_data}")
  (data (i32.const 2048) "{ok_data}")
  (func (export "rvah_alloc") (param i32) (result i32) i32.const 4096)
  (func (export "rvah_invoke") (param i32 i32) (result i64)
    (if (result i64) (i32.eq (memory.grow (i32.const 64)) (i32.const -1))
      (then i64.const {packed_err})
      (else i64.const {packed_ok}))))"#,
            err_data = wat_escape(err_json),
            ok_data = wat_escape(ok_json),
            packed_err = pack(512, err_json.len() as u32),
            packed_ok = pack(2048, ok_json.len() as u32),
        );
        // Cap: 2 pages (128 KiB) — the 64-page (4 MiB) grow must be denied.
        let rt = WasmRuntime::load(
            wat.as_bytes(),
            WasmLimits {
                max_memory_bytes: 2 * 64 * 1024,
                fuel: 1_000_000,
                timeout: Duration::from_secs(1),
            },
        )
        .unwrap();
        let err = rt
            .invoke(
                AddonOp::List {
                    path: None,
                    cursor: None,
                },
                &ctx(),
            )
            .unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Quota);
    }
}
