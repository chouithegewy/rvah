use std::sync::{Arc, Mutex};

use rvah_addon_sdk::{AddonError, AddonErrorCode, HostApi, HttpRequest, HttpResponse, LogLevel};
use rvah_auth::{Redactor, SecretVault};
use rvah_data::{Database, DiagnosticEvent};
use rvah_net::{Allowlist, NetError, PermissionedClient, Transport};

use crate::manifest::Manifest;

/// The concrete [`HostApi`] handed to a running add-on: every service is
/// scoped to the add-on's id and gated by its granted manifest permissions
/// (FR-080..FR-084). Log output is redacted before persistence (TC-SEC-003).
pub struct BridgedHostApi {
    addon_id: String,
    client: PermissionedClient,
    db: Arc<Database>,
    vault: Arc<dyn SecretVault>,
    redactor: Mutex<Redactor>,
    allowed_secrets: Vec<String>,
    default_settings: std::collections::BTreeMap<String, String>,
    storage_quota: u64,
}

impl BridgedHostApi {
    pub fn new(
        manifest: &Manifest,
        db: Arc<Database>,
        vault: Arc<dyn SecretVault>,
        transport: Box<dyn Transport>,
        max_requests: u32,
    ) -> Self {
        let allowlist = Allowlist::new(manifest.permissions.network.iter().cloned());
        Self {
            addon_id: manifest.addon.id.clone(),
            client: PermissionedClient::new(transport, allowlist, max_requests),
            db,
            vault,
            redactor: Mutex::new(Redactor::new()),
            allowed_secrets: manifest.permissions.secrets.clone(),
            default_settings: manifest.default_settings(),
            storage_quota: manifest.permissions.storage_bytes,
        }
    }
}

fn net_to_addon_error(err: NetError) -> AddonError {
    let code = match &err {
        NetError::DomainDenied(_) | NetError::SchemeDenied(_) | NetError::AddressDenied { .. } => {
            AddonErrorCode::PermissionDenied
        }
        NetError::QuotaExceeded => AddonErrorCode::Quota,
        NetError::Transport(_) | NetError::TooManyRedirects | NetError::ResolveFailed(_) => {
            AddonErrorCode::Network
        }
        NetError::InvalidUrl(_) => AddonErrorCode::Malformed,
    };
    AddonError::new(code, err.to_string())
}

impl HostApi for BridgedHostApi {
    fn http_fetch(&self, req: HttpRequest) -> Result<HttpResponse, AddonError> {
        self.client.fetch(&req).map_err(net_to_addon_error)
    }

    fn setting_get(&self, key: &str) -> Option<String> {
        match self.db.get_setting(&self.addon_id, key) {
            Ok(Some(v)) => Some(v),
            _ => self.default_settings.get(key).cloned(),
        }
    }

    fn secret_get(&self, name: &str) -> Result<Option<String>, AddonError> {
        // Only secrets declared in the manifest are reachable (TC-RUN-006).
        if !self.allowed_secrets.iter().any(|s| s == name) {
            return Err(AddonError::new(
                AddonErrorCode::PermissionDenied,
                format!("secret {name:?} not declared in manifest"),
            ));
        }
        let value = self
            .vault
            .get(&self.addon_id, name)
            .map_err(|e| AddonError::new(AddonErrorCode::Internal, e.to_string()))?;
        if let Some(v) = &value {
            // Anything read from the vault must never appear in diagnostics.
            self.redactor.lock().unwrap().register_secret(v.clone());
        }
        Ok(value)
    }

    fn storage_get(&self, key: &str) -> Option<Vec<u8>> {
        self.db.storage_get(&self.addon_id, key).ok().flatten()
    }

    fn storage_set(&self, key: &str, value: &[u8]) -> Result<(), AddonError> {
        self.db
            .storage_set(&self.addon_id, key, value, self.storage_quota)
            .map_err(|e| match e {
                rvah_data::DataError::StorageQuotaExceeded { .. } => {
                    AddonError::new(AddonErrorCode::Quota, e.to_string())
                }
                other => AddonError::new(AddonErrorCode::Internal, other.to_string()),
            })
    }

    fn storage_delete(&self, key: &str) {
        let _ = self.db.storage_delete(&self.addon_id, key);
    }

    fn log(&self, level: LogLevel, message: &str) {
        let redacted = self.redactor.lock().unwrap().redact(message);
        let _ = self.db.log_event(&DiagnosticEvent {
            level: format!("{level:?}").to_lowercase(),
            addon_id: Some(self.addon_id.clone()),
            code: "addon_log".into(),
            message: redacted,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvah_auth::MemoryVault;
    use rvah_net::MockTransport;

    const MANIFEST: &str = r#"
[addon]
id = "com.example.bridge"
name = "Bridge Test"
version = "1.0.0"
sdk = ">=0.1, <0.2"
entry = "main"
capabilities = ["catalog"]

[permissions]
network = ["api.example.com"]
storage = "1KiB"
secrets = ["api_token"]

[settings.region]
type = "string"
default = "us"
"#;

    fn bridge() -> (
        BridgedHostApi,
        Arc<Database>,
        Arc<MemoryVault>,
        MockTransport,
    ) {
        let manifest = Manifest::parse(MANIFEST).unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let vault = Arc::new(MemoryVault::new());
        let mock = MockTransport::default();
        mock.route("GET", "https://api.example.com/", 200, b"ok".to_vec());
        let api = BridgedHostApi::new(
            &manifest,
            Arc::clone(&db),
            vault.clone() as Arc<dyn SecretVault>,
            Box::new(mock.clone()),
            100,
        );
        (api, db, vault, mock)
    }

    #[test]
    fn bridged_api_denies_unlisted_domain() {
        let (api, _db, _vault, mock) = bridge();
        let err = api
            .http_fetch(HttpRequest::get("https://evil.example.org/steal"))
            .unwrap_err();
        assert_eq!(err.code, AddonErrorCode::PermissionDenied);
        // Denied before the transport ever saw it (FR-081).
        assert_eq!(mock.request_count(), 0);

        assert!(api
            .http_fetch(HttpRequest::get("https://api.example.com/v1"))
            .is_ok());
    }

    #[test]
    fn bridged_api_denies_undeclared_secret() {
        let (api, _db, vault, _mock) = bridge();
        vault
            .set("com.example.bridge", "other_secret", "x")
            .unwrap();
        let err = api.secret_get("other_secret").unwrap_err();
        assert_eq!(err.code, AddonErrorCode::PermissionDenied);

        vault
            .set("com.example.bridge", "api_token", "tok-123456")
            .unwrap();
        assert_eq!(
            api.secret_get("api_token").unwrap().as_deref(),
            Some("tok-123456")
        );
    }

    #[test]
    fn log_redacts_fetched_secrets() {
        let (api, db, vault, _mock) = bridge();
        vault
            .set("com.example.bridge", "api_token", "tok-123456")
            .unwrap();
        api.secret_get("api_token").unwrap();
        api.log(LogLevel::Info, "request with tok-123456 failed");
        let events = db.recent_events(10).unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].message.contains("tok-123456"));
        assert!(events[0].message.contains("[REDACTED]"));
    }

    #[test]
    fn settings_fall_back_to_manifest_default() {
        let (api, db, _vault, _mock) = bridge();
        assert_eq!(api.setting_get("region").as_deref(), Some("us"));
        db.set_setting("com.example.bridge", "region", "eu")
            .unwrap();
        assert_eq!(api.setting_get("region").as_deref(), Some("eu"));
        assert_eq!(api.setting_get("missing"), None);
    }

    #[test]
    fn storage_respects_manifest_quota() {
        let (api, _db, _vault, _mock) = bridge();
        api.storage_set("small", b"x").unwrap();
        assert_eq!(api.storage_get("small").as_deref(), Some(b"x".as_slice()));
        let err = api.storage_set("big", &[0u8; 2048]).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Quota);
    }
}
