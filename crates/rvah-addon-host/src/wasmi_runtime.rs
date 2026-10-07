//! Portable `.rvah` ABI v0.1 runtime for Android. Wasmi avoids Wasmtime's
//! 32-bit target limitation while retaining fuel and memory enforcement.

use std::sync::Arc;
use std::time::Duration;

use rvah_addon_sdk::{AddonError, AddonErrorCode, HostApi, LogLevel};
use wasmi::{
    core::TrapCode, Caller, Config, Engine, Extern, Linker, Module, Store, StoreLimits,
    StoreLimitsBuilder,
};

use crate::runtime::{AddonOp, AddonResponse, AddonRuntime, InvocationCtx};

#[derive(Debug, Clone, Copy)]
pub struct WasmiLimits {
    pub max_memory_bytes: usize,
    pub fuel: u64,
    pub timeout: Duration,
}

impl Default for WasmiLimits {
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

pub struct WasmiRuntime {
    engine: Engine,
    module: Module,
    limits: WasmiLimits,
}

fn internal(message: impl std::fmt::Display) -> AddonError {
    AddonError::new(AddonErrorCode::Internal, message.to_string())
}

fn guest_error(error: wasmi::Error) -> AddonError {
    match error.as_trap_code() {
        Some(TrapCode::OutOfFuel) => AddonError::new(AddonErrorCode::Timeout, "fuel exhausted"),
        _ => internal(format!("guest trapped: {error}")),
    }
}

impl WasmiRuntime {
    pub fn load(module_bytes: &[u8], limits: WasmiLimits) -> Result<Self, AddonError> {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, module_bytes)
            .map_err(|e| AddonError::new(AddonErrorCode::Malformed, format!("bad module: {e}")))?;
        Ok(Self {
            engine,
            module,
            limits,
        })
    }
}

fn dispatch_host_call(host: &dyn HostApi, request: &[u8]) -> Vec<u8> {
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
            level: Option<String>,
            message: String,
        },
    }
    fn ok(value: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({ "Ok": value })).expect("host response serializes")
    }
    fn err(error: &AddonError) -> Vec<u8> {
        serde_json::to_vec(&json!({ "Err": error })).expect("host error serializes")
    }
    let call: HostCall = match serde_json::from_slice(request) {
        Ok(call) => call,
        Err(error) => {
            return err(&AddonError::new(
                AddonErrorCode::Malformed,
                format!("host call: {error}"),
            ))
        }
    };
    match call {
        HostCall::HttpFetch { req } => match host.http_fetch(req) {
            Ok(response) => ok(serde_json::to_value(response).expect("response serializes")),
            Err(error) => err(&error),
        },
        HostCall::SettingGet { key } => ok(json!({ "value": host.setting_get(&key) })),
        HostCall::SecretGet { name } => match host.secret_get(&name) {
            Ok(value) => ok(json!({ "value": value })),
            Err(error) => err(&error),
        },
        HostCall::StorageGet { key } => ok(json!({ "value": host.storage_get(&key) })),
        HostCall::StorageSet { key, value } => match host.storage_set(&key, &value) {
            Ok(()) => ok(Value::Null),
            Err(error) => err(&error),
        },
        HostCall::StorageDelete { key } => {
            host.storage_delete(&key);
            ok(Value::Null)
        }
        HostCall::Log { level, message } => {
            let level = match level.as_deref() {
                Some("error") => LogLevel::Error,
                Some("warn") => LogLevel::Warn,
                Some("debug") => LogLevel::Debug,
                Some("trace") => LogLevel::Trace,
                _ => LogLevel::Info,
            };
            host.log(level, &message);
            ok(Value::Null)
        }
    }
}

fn host_call(caller: &mut Caller<'_, StoreData>, ptr: i32, len: i32) -> i64 {
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return 0;
    };
    let Some(request) = memory
        .data(&*caller)
        .get(ptr as usize..ptr as usize + len as usize)
        .map(<[u8]>::to_vec)
    else {
        return 0;
    };
    let response = dispatch_host_call(caller.data().host.as_ref(), &request);
    let Some(Extern::Func(alloc)) = caller.get_export("rvah_alloc") else {
        return 0;
    };
    let Ok(alloc) = alloc.typed::<i32, i32>(&*caller) else {
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

impl AddonRuntime for WasmiRuntime {
    fn invoke(&self, op: AddonOp, ctx: &InvocationCtx) -> Result<AddonResponse, AddonError> {
        let data = StoreData {
            limits: StoreLimitsBuilder::new()
                .memory_size(self.limits.max_memory_bytes)
                .build(),
            host: Arc::clone(&ctx.host),
        };
        let mut store = Store::new(&self.engine, data);
        store.limiter(|data| &mut data.limits);
        store.set_fuel(self.limits.fuel).map_err(internal)?;
        let mut linker = Linker::new(&self.engine);
        linker
            .func_wrap(
                "env",
                "rvah_host_call",
                |mut caller: Caller<'_, StoreData>, ptr: i32, len: i32| {
                    host_call(&mut caller, ptr, len)
                },
            )
            .map_err(internal)?;
        let instance = linker
            .instantiate(&mut store, &self.module)
            .and_then(|pre| pre.start(&mut store))
            .map_err(guest_error)?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| internal("guest exports no memory"))?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&store, "rvah_alloc")
            .map_err(internal)?;
        let invoke = instance
            .get_typed_func::<(i32, i32), i64>(&store, "rvah_invoke")
            .map_err(internal)?;
        let payload = serde_json::to_vec(&op).map_err(internal)?;
        let ptr = alloc
            .call(&mut store, payload.len() as i32)
            .map_err(guest_error)?;
        memory
            .write(&mut store, ptr as usize, &payload)
            .map_err(internal)?;
        let packed = invoke
            .call(&mut store, (ptr, payload.len() as i32))
            .map_err(guest_error)?;
        if packed == 0 {
            return Err(internal("guest returned empty response"));
        }
        let response_ptr = (packed >> 32) as u32 as usize;
        let response_len = packed as u32 as usize;
        let bytes = memory
            .data(&store)
            .get(response_ptr..response_ptr + response_len)
            .ok_or_else(|| internal("guest response out of bounds"))?;
        serde_json::from_slice::<Result<AddonResponse, AddonError>>(bytes).map_err(|error| {
            AddonError::new(AddonErrorCode::Malformed, format!("guest JSON: {error}"))
        })?
    }
}
