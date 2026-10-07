//! Portable `.rvah` ABI v0.1 runtime for Android. Wasmi avoids Wasmtime's
//! 32-bit target limitation while retaining fuel and memory enforcement.
//!
//! Wasmi has no epoch interruption, so the wall-clock budget is enforced at
//! host-call boundaries (before and after every `rvah_host_call`, which is
//! where a guest can block on the network); pure guest compute between host
//! calls is bounded by fuel.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rvah_addon_sdk::{AddonError, AddonErrorCode, HostApi};
use wasmi::{
    core::TrapCode, Caller, Config, Engine, Extern, Linker, Module, Store, StoreLimits,
    StoreLimitsBuilder,
};

use crate::abi::{dispatch_host_call, guest_range};
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
    deadline: Instant,
}

/// Trap raised from a host call once the invocation's wall-clock budget is
/// spent.
#[derive(Debug)]
struct DeadlineExceeded;

impl std::fmt::Display for DeadlineExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("wall-clock deadline")
    }
}

impl wasmi::core::HostError for DeadlineExceeded {}

pub struct WasmiRuntime {
    engine: Engine,
    module: Module,
    limits: WasmiLimits,
}

fn internal(message: impl std::fmt::Display) -> AddonError {
    AddonError::new(AddonErrorCode::Internal, message.to_string())
}

fn guest_error(error: wasmi::Error) -> AddonError {
    if error.downcast_ref::<DeadlineExceeded>().is_some() {
        return AddonError::new(AddonErrorCode::Timeout, "wall-clock deadline");
    }
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

fn host_call(caller: &mut Caller<'_, StoreData>, ptr: i32, len: i32) -> Result<i64, wasmi::Error> {
    let deadline = caller.data().deadline;
    if Instant::now() >= deadline {
        return Err(wasmi::Error::host(DeadlineExceeded));
    }
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return Ok(0);
    };
    let Some(request) = guest_range(ptr, len)
        .and_then(|range| memory.data(&*caller).get(range))
        .map(<[u8]>::to_vec)
    else {
        return Ok(0);
    };
    let response = dispatch_host_call(caller.data().host.as_ref(), &request);
    // The call may have blocked (network); do not resume a guest that is
    // already over budget.
    if Instant::now() >= deadline {
        return Err(wasmi::Error::host(DeadlineExceeded));
    }
    let Some(Extern::Func(alloc)) = caller.get_export("rvah_alloc") else {
        return Ok(0);
    };
    let Ok(alloc) = alloc.typed::<i32, i32>(&*caller) else {
        return Ok(0);
    };
    let Ok(response_ptr) = alloc.call(&mut *caller, response.len() as i32) else {
        return Ok(0);
    };
    if memory
        .write(&mut *caller, response_ptr as u32 as usize, &response)
        .is_err()
    {
        return Ok(0);
    }
    Ok((((response_ptr as u32 as u64) << 32) | response.len() as u64) as i64)
}

impl AddonRuntime for WasmiRuntime {
    fn invoke(&self, op: AddonOp, ctx: &InvocationCtx) -> Result<AddonResponse, AddonError> {
        let data = StoreData {
            limits: StoreLimitsBuilder::new()
                .memory_size(self.limits.max_memory_bytes)
                .build(),
            host: Arc::clone(&ctx.host),
            deadline: Instant::now() + self.limits.timeout.min(ctx.timeout),
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
            .write(&mut store, ptr as u32 as usize, &payload)
            .map_err(internal)?;
        let packed = invoke
            .call(&mut store, (ptr, payload.len() as i32))
            .map_err(guest_error)?;
        if packed == 0 {
            return Err(internal("guest returned empty response"));
        }
        let bytes = guest_range((packed >> 32) as i32, packed as i32)
            .and_then(|range| memory.data(&store).get(range))
            .ok_or_else(|| internal("guest response out of bounds"))?;
        serde_json::from_slice::<Result<AddonResponse, AddonError>>(bytes).map_err(|error| {
            AddonError::new(AddonErrorCode::Malformed, format!("guest JSON: {error}"))
        })?
    }
}
