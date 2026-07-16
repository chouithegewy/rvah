//! RVAH add-on host: manifest security boundary, signed packaging, sandboxed
//! runtime invocation with quotas, capability-checked host API bridge, and the
//! install/update/disable/uninstall lifecycle.

pub mod host_api;
pub mod install;
pub mod manifest;
pub mod package;
pub mod runtime;

pub use host_api::BridgedHostApi;
pub use install::{AddonManager, InstallDecision, InstallError, InstalledAddon};
pub use manifest::{
    AddonSection, Capability, Manifest, ManifestError, PermissionsSection, SettingSpec,
    HOST_SDK_VERSION,
};
pub use package::{
    extract_package, keygen, package_dir, read_package_file, sha256_file, sign_package,
    verify_package, PackageError,
};
pub use runtime::{AddonOp, AddonResponse, AddonRuntime, InProcessRuntime, InvocationCtx};
