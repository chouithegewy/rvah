//! Best-effort Wasmtime sandbox implementing [`AddonRuntime`] (FR-020).
//!
//! ABI v0: the guest exports `memory`, `rvah_alloc(len: i32) -> i32`, and
//! `rvah_invoke(ptr: i32, len: i32) -> i64` (packed `ptr << 32 | len` of a
//! JSON `Result<AddonResponse, AddonError>`); the host passes a JSON
//! [`AddonOp`]. The optional import `env.rvah_host_call(ptr, len) -> i64`
//! dispatches JSON host-API calls; v0 implements `log` only — richer
//! dispatch rides on the same [`HostApi`] bridge later.
//!
//! Enforcement: `StoreLimits` caps guest memory (TC-RUN-005); fuel metering
//! plus an epoch watchdog bound CPU and wall-clock time (TC-RUN-004).

use std::sync::Arc;
use std::time::Duration;

use rvah_addon_sdk::{AddonError, AddonErrorCode, HostApi, LogLevel};
use wasmtime::{
    Caller, Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder, Trap,
};

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
        Ok(Self { engine, module, limits })
    }
}

/// v0 host-call dispatch: `{"fn":"log","level":"info","message":"..."}`.
/// Unknown functions are ignored (return 0) rather than trapping the guest.
fn host_call(caller: &mut Caller<'_, StoreData>, ptr: i32, len: i32) -> i64 {
    let Some(wasmtime::Extern::Memory(memory)) = caller.get_export("memory") else {
        return 0;
    };
    let Some(bytes) = memory
        .data(&caller)
        .get(ptr as usize..(ptr as usize + len as usize))
    else {
        return 0;
    };
    let Ok(req) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return 0;
    };
    if req["fn"] == "log" {
        let level = match req["level"].as_str() {
            Some("error") => LogLevel::Error,
            Some("warn") => LogLevel::Warn,
            Some("debug") => LogLevel::Debug,
            Some("trace") => LogLevel::Trace,
            _ => LogLevel::Info,
        };
        let message = req["message"].as_str().unwrap_or_default().to_string();
        caller.data().host.log(level, &message);
    }
    0
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
        let rptr = (packed >> 32) as u32 as usize;
        let rlen = packed as u32 as usize;
        let bytes = memory
            .data(&store)
            .get(rptr..rptr + rlen)
            .ok_or_else(|| internal("guest response out of bounds"))?;
        serde_json::from_slice::<Result<AddonResponse, AddonError>>(bytes)
            .map_err(|e| AddonError::new(AddonErrorCode::Malformed, format!("guest JSON: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvah_addon_sdk::{HttpRequest, HttpResponse, PageItem};

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
            .invoke(AddonOp::List { path: None, cursor: None }, &ctx())
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
            .invoke(AddonOp::List { path: None, cursor: None }, &ctx())
            .unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Timeout);
        assert!(err.message.contains("fuel"));
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
            .invoke(AddonOp::List { path: None, cursor: None }, &ctx())
            .unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Quota);
    }
}
