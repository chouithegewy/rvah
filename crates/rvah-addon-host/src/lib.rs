//! RVAH add-on host: manifest security boundary, signed packaging, sandboxed
//! runtime invocation with quotas, capability-checked host API bridge, and the
//! install/update/disable/uninstall lifecycle.

pub mod manifest;

pub use manifest::{
    AddonSection, Capability, Manifest, ManifestError, PermissionsSection, SettingSpec,
    HOST_SDK_VERSION,
};
