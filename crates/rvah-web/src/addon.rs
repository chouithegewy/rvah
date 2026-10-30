//! Browser add-ons use the same signature, manifest and fuel-limited ABI as native.
use rvah_addon_host::{
    AddonOp, AddonRuntime, Capability, InvocationCtx, Manifest, WasmiLimits, WasmiRuntime,
};
use rvah_addon_sdk::{AddonError, AddonErrorCode, HostApi, HttpRequest, HttpResponse, LogLevel};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct BrowserAddon {
    manifest: Manifest,
    runtime: WasmiRuntime,
}

fn denied(message: &str) -> AddonError {
    AddonError::new(AddonErrorCode::PermissionDenied, message)
}
struct BrowserHost {
    settings: BTreeMap<String, String>,
}
impl HostApi for BrowserHost {
    fn http_fetch(&self, _: HttpRequest) -> Result<HttpResponse, AddonError> {
        Err(denied(
            "browser ABI does not support synchronous HTTP; use a host-owned playback transport",
        ))
    }
    fn setting_get(&self, key: &str) -> Option<String> {
        self.settings.get(key).cloned()
    }
    fn secret_get(&self, _: &str) -> Result<Option<String>, AddonError> {
        Err(denied("browser add-ons cannot access host/profile secrets"))
    }
    fn storage_get(&self, _: &str) -> Option<Vec<u8>> {
        None
    }
    fn storage_set(&self, _: &str, _: &[u8]) -> Result<(), AddonError> {
        Err(denied("browser add-on storage unavailable"))
    }
    fn storage_delete(&self, _: &str) {}
    fn log(&self, _: LogLevel, _: &str) {} // Never forward untrusted guest logs containing settings.
}

impl BrowserAddon {
    fn load(archive: &[u8], signature: &[u8], public_key: &[u8]) -> Result<Self, String> {
        let mut files =
            rvah_addon_host::package::verified_package_bytes(archive, signature, public_key)
                .map_err(|e| e.to_string())?;
        let text = files
            .remove("manifest.toml")
            .ok_or("manifest.toml missing")?;
        if text.len() > 64 * 1024 {
            return Err("manifest exceeds 64 KiB".into());
        }
        let manifest = Manifest::parse(std::str::from_utf8(&text).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if !manifest.is_sdk_compatible() {
            return Err("unsupported add-on SDK range".into());
        }
        let module = files
            .get(&manifest.addon.entry)
            .ok_or("manifest entry module missing")?;
        let runtime =
            WasmiRuntime::load(module, WasmiLimits::default()).map_err(|e| e.to_string())?;
        Ok(Self { manifest, runtime })
    }

    fn call(&self, operation: &str, grants: &str, settings: &str) -> Result<String, String> {
        if operation.len() > 64 * 1024 || grants.len() > 4096 || settings.len() > 1024 * 1024 {
            return Err("add-on invocation input exceeds limit".into());
        }
        let op: AddonOp = serde_json::from_str(operation).map_err(|e| e.to_string())?;
        let grants: Vec<Capability> = serde_json::from_str(grants).map_err(|e| e.to_string())?;
        if !matches!(
            op,
            AddonOp::List { .. } | AddonOp::Search { .. } | AddonOp::Resolve { .. }
        ) {
            return Err("unsupported browser operation".into());
        }
        if !self.manifest.has_capability(op.required_capability())
            || !grants.contains(&op.required_capability())
        {
            return Err("operation capability is not declared and approved".into());
        }
        let supplied: BTreeMap<String, String> =
            serde_json::from_str(settings).map_err(|e| e.to_string())?;
        let mut settings = self.manifest.default_settings();
        for (key, value) in supplied {
            if !self.manifest.settings.contains_key(&key) {
                return Err(format!("undeclared setting: {key}"));
            }
            settings.insert(key, value);
        }
        let response = self
            .runtime
            .invoke(
                op,
                &InvocationCtx {
                    timeout: Duration::from_secs(2),
                    host: Arc::new(BrowserHost { settings }),
                },
            )
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&response).map_err(|e| e.to_string())
    }
}

#[wasm_bindgen]
impl BrowserAddon {
    #[wasm_bindgen(constructor)]
    pub fn new(
        archive: &[u8],
        signature: &[u8],
        public_key: &[u8],
    ) -> Result<BrowserAddon, JsValue> {
        Self::load(archive, signature, public_key).map_err(crate::js_error)
    }
    pub fn manifest_json(&self) -> Result<String, JsValue> {
        serde_json::to_string(&serde_json::json!({"addon": self.manifest.addon, "permissions": self.manifest.permissions, "settings": self.manifest.settings})).map_err(crate::js_error)
    }
    pub fn invoke(&self, operation: &str, grants: &str, settings: &str) -> Result<String, JsValue> {
        self.call(operation, grants, settings)
            .map_err(crate::js_error)
    }
}

/// The trusted host reads only this add-on's settings; profile keys and secrets
/// are never passed to the guest runtime.
#[wasm_bindgen]
pub fn addon_profile_settings(bundle: &[u8], key: &str, id: &str) -> Result<String, JsValue> {
    let profile = crate::decrypt(bundle, key)?;
    serde_json::to_string(
        &profile
            .addons
            .get(id)
            .map(|a| a.settings.clone())
            .unwrap_or_default(),
    )
    .map_err(crate::js_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn package(module: &str) -> (Vec<u8>, [u8; 64], [u8; 32]) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(
            src.join("manifest.toml"),
            r#"
[addon]
id = "test.browser"
name = "Browser test"
version = "1.0.0"
sdk = ">=0.1,<0.2"
entry = "main.wasm"
capabilities = ["catalog"]
[settings.catalog]
type = "string"
default = "[]"
"#,
        )
        .unwrap();
        std::fs::write(src.join("main.wasm"), module).unwrap();
        let file = dir.path().join("test.rvah");
        rvah_addon_host::package_dir(&src, &file).unwrap();
        let (sign, key) = rvah_addon_host::keygen();
        let signature = rvah_addon_host::sign_package(&file, &sign).unwrap();
        (std::fs::read(file).unwrap(), signature, key.to_bytes())
    }
    const LOOP: &str = r#"(module (memory (export "memory") 1)
        (func (export "rvah_alloc") (param i32) (result i32) i32.const 0)
        (func (export "rvah_invoke") (param i32 i32) (result i64)
            (loop $forever br $forever) i64.const 0))"#;
    #[test]
    fn signed_package_tamper_and_wrong_key_rejected() {
        let (mut bytes, sig, key) = package(LOOP);
        assert!(BrowserAddon::load(&bytes, &sig, &[0; 32]).is_err());
        bytes[0] ^= 1;
        assert!(BrowserAddon::load(&bytes, &sig, &key).is_err());
    }
    #[test]
    fn grants_settings_and_fuel_are_enforced() {
        let (bytes, sig, key) = package(LOOP);
        let addon = BrowserAddon::load(&bytes, &sig, &key).unwrap();
        let op = r#"{"op":"list","path":null,"cursor":null}"#;
        assert!(addon.call(op, "[]", "{}").unwrap_err().contains("approved"));
        assert!(addon
            .call(op, r#"["catalog"]"#, r#"{"profile_key":"secret"}"#)
            .unwrap_err()
            .contains("undeclared setting"));
        assert!(addon
            .call(op, r#"["catalog"]"#, "{}")
            .unwrap_err()
            .contains("fuel"));
    }
    #[test]
    fn host_never_exposes_secrets_or_storage() {
        let host = BrowserHost {
            settings: BTreeMap::new(),
        };
        assert!(host.secret_get("profile_key").is_err());
        assert!(host.storage_set("media", &[1]).is_err());
        assert!(host.setting_get("profile_key").is_none());
    }
}
