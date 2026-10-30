//! RVAH add-on host: manifest security boundary, signed packaging, sandboxed
//! runtime invocation with quotas, capability-checked host API bridge, and the
//! install/update/disable/uninstall lifecycle.

#[cfg(any(feature = "wasm", feature = "wasm-wasmi"))]
mod abi;
#[cfg(feature = "native")]
pub mod host_api;
#[cfg(feature = "native")]
pub mod install;
pub mod manifest;
pub mod package;
pub mod runtime;
#[cfg(feature = "wasm")]
pub mod wasm_runtime;
#[cfg(feature = "wasm-wasmi")]
mod wasmi_runtime;

#[cfg(feature = "native")]
pub use host_api::BridgedHostApi;
#[cfg(feature = "native")]
pub use install::{AddonManager, InstallDecision, InstallError, InstalledAddon};
pub use manifest::{
    AddonSection, Capability, Manifest, ManifestError, PermissionsSection, SettingSpec,
    HOST_SDK_VERSION,
};
pub use package::{
    extract_package, keygen, package_dir, read_package_file, sha256_file, sign_package,
    verify_package, PackageError,
};
pub use runtime::{
    AddonOp, AddonResponse, AddonRuntime, CapabilityGated, InProcessRuntime, InvocationCtx,
};
#[cfg(feature = "wasm")]
pub use wasm_runtime::{WasmLimits, WasmRuntime};
#[cfg(feature = "wasm-wasmi")]
pub use wasmi_runtime::{WasmiLimits, WasmiRuntime};
