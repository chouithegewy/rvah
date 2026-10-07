//! Containment parity for the Android (Wasmi) runtime: wall-clock deadline,
//! fuel, memory cap, and hostile host-call pointers.
#![cfg(feature = "wasm-wasmi")]

use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use rvah_addon_host::{
    AddonOp, AddonResponse, AddonRuntime, InvocationCtx, WasmiLimits, WasmiRuntime,
};
use rvah_addon_sdk::{AddonError, AddonErrorCode, HostApi, HttpRequest, HttpResponse, LogLevel};

/// Every `log` host call blocks, like a slow network fetch would.
struct SlowHost(Duration);

impl HostApi for SlowHost {
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
    fn log(&self, _level: LogLevel, _message: &str) {
        std::thread::sleep(self.0);
    }
}

const LOG_CALL: &str = r#"{"fn":"log","message":"tick"}"#;
const EMPTY_PAGE: &str = r#"{"Ok":{"kind":"page","value":{"items":[]}}}"#;

fn wat_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn packed(ptr: u64, len: usize) -> i64 {
    ((ptr << 32) | len as u64) as i64
}

/// Guest whose `rvah_invoke` runs `body`, then returns an empty page.
fn module(memory_pages: u32, body: &str) -> String {
    format!(
        r#"(module
  (import "env" "rvah_host_call" (func $host (param i32 i32) (result i64)))
  (memory (export "memory") {memory_pages})
  (data (i32.const 0) "{log}")
  (data (i32.const 1024) "{page}")
  (func (export "rvah_alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "rvah_invoke") (param i32 i32) (result i64)
    {body}
    (i64.const {ret})))"#,
        log = wat_escape(LOG_CALL),
        page = wat_escape(EMPTY_PAGE),
        ret = packed(1024, EMPTY_PAGE.len()),
    )
}

fn list() -> AddonOp {
    AddonOp::List {
        path: None,
        cursor: None,
    }
}

/// Invoke on another thread so a missing limit fails the test instead of
/// hanging it.
fn invoke_bounded(
    wat: String,
    limits: WasmiLimits,
    ctx_timeout: Duration,
    host: Arc<dyn HostApi>,
) -> (Result<AddonResponse, AddonError>, Duration) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let started = Instant::now();
        let runtime = WasmiRuntime::load(wat.as_bytes(), limits).unwrap();
        let ctx = InvocationCtx {
            timeout: ctx_timeout,
            host,
        };
        let result = runtime.invoke(list(), &ctx);
        let _ = tx.send((result, started.elapsed()));
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("invoke did not return: no wall-clock bound")
}

#[test]
fn blocking_host_calls_hit_the_wall_clock_deadline() {
    let body = format!(
        "(loop $l (drop (call $host (i32.const 0) (i32.const {}))) (br $l))",
        LOG_CALL.len()
    );
    let limits = WasmiLimits {
        fuel: u64::MAX / 2,
        timeout: Duration::from_millis(300),
        ..Default::default()
    };
    let (result, elapsed) = invoke_bounded(
        module(1, &body),
        limits,
        Duration::from_secs(30),
        Arc::new(SlowHost(Duration::from_millis(20))),
    );
    let err = result.expect_err("endless host-call loop must be stopped");
    assert_eq!(err.code, AddonErrorCode::Timeout, "{err:?}");
    assert!(elapsed < Duration::from_secs(3), "took {elapsed:?}");
}

#[test]
fn caller_timeout_wins_when_shorter() {
    let body = format!(
        "(loop $l (drop (call $host (i32.const 0) (i32.const {}))) (br $l))",
        LOG_CALL.len()
    );
    let limits = WasmiLimits {
        fuel: u64::MAX / 2,
        timeout: Duration::from_secs(60),
        ..Default::default()
    };
    let (result, elapsed) = invoke_bounded(
        module(1, &body),
        limits,
        Duration::from_millis(200),
        Arc::new(SlowHost(Duration::from_millis(20))),
    );
    assert_eq!(result.unwrap_err().code, AddonErrorCode::Timeout);
    assert!(elapsed < Duration::from_secs(3), "took {elapsed:?}");
}

#[test]
fn pure_compute_is_bounded_by_fuel() {
    let (result, _) = invoke_bounded(
        module(1, "(loop $l (br $l))"),
        WasmiLimits {
            fuel: 1_000_000,
            ..Default::default()
        },
        Duration::from_secs(30),
        Arc::new(SlowHost(Duration::ZERO)),
    );
    assert_eq!(result.unwrap_err().code, AddonErrorCode::Timeout);
}

#[test]
fn initial_memory_above_the_cap_is_refused() {
    // 512 pages = 32 MiB against the default 16 MiB ceiling.
    let (result, _) = invoke_bounded(
        module(512, ""),
        WasmiLimits::default(),
        Duration::from_secs(30),
        Arc::new(SlowHost(Duration::ZERO)),
    );
    assert!(result.is_err());
}

#[test]
fn hostile_host_call_pointers_do_not_panic_the_host() {
    // ptr = -1 sign-extends to usize::MAX; ptr + len must not overflow.
    let body = "(drop (call $host (i32.const -1) (i32.const 64)))
    (drop (call $host (i32.const 65000) (i32.const -1)))";
    let (result, _) = invoke_bounded(
        module(1, body),
        WasmiLimits::default(),
        Duration::from_secs(30),
        Arc::new(SlowHost(Duration::ZERO)),
    );
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn well_behaved_guest_still_answers() {
    let body = format!(
        "(drop (call $host (i32.const 0) (i32.const {})))",
        LOG_CALL.len()
    );
    let (result, _) = invoke_bounded(
        module(1, &body),
        WasmiLimits::default(),
        Duration::from_secs(30),
        Arc::new(SlowHost(Duration::ZERO)),
    );
    assert!(matches!(result, Ok(AddonResponse::Page(_))), "{result:?}");
}
