use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{AccountStatus, AddonError, AddonErrorCode, Filter, Page, ResolveResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        HttpRequest {
            method: "GET".into(),
            url: url.into(),
            headers: BTreeMap::new(),
            body: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body: Vec<u8>,
}

/// Host services exposed to add-ons. Every call is capability-checked by the
/// host against the add-on's manifest; add-ons never get raw OS access.
pub trait HostApi: Send + Sync {
    fn http_fetch(&self, req: HttpRequest) -> Result<HttpResponse, AddonError>;
    fn setting_get(&self, key: &str) -> Option<String>;
    fn secret_get(&self, name: &str) -> Result<Option<String>, AddonError>;
    fn storage_get(&self, key: &str) -> Option<Vec<u8>>;
    fn storage_set(&self, key: &str, value: &[u8]) -> Result<(), AddonError>;
    fn storage_delete(&self, key: &str);
    fn log(&self, level: LogLevel, message: &str);
}

fn unimplemented_capability(name: &str) -> AddonError {
    AddonError::new(
        AddonErrorCode::Internal,
        format!("capability not implemented: {name}"),
    )
}

/// The narrow provider interface an add-on implements. Default methods report
/// an internal error so providers only implement declared capabilities.
pub trait AddonProvider {
    fn list(
        &self,
        _ctx: &dyn HostApi,
        _path: Option<&str>,
        _cursor: Option<&str>,
    ) -> Result<Page, AddonError> {
        Err(unimplemented_capability("catalog"))
    }

    fn search(
        &self,
        _ctx: &dyn HostApi,
        _query: &str,
        _filters: &[Filter],
        _cursor: Option<&str>,
    ) -> Result<Page, AddonError> {
        Err(unimplemented_capability("search"))
    }

    fn resolve(
        &self,
        _ctx: &dyn HostApi,
        _item_id: &str,
        _source_id: Option<&str>,
    ) -> Result<ResolveResult, AddonError> {
        Err(unimplemented_capability("resolve"))
    }

    fn account_status(&self, _ctx: &dyn HostApi) -> Result<AccountStatus, AddonError> {
        Err(unimplemented_capability("account_status"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Nothing;
    impl AddonProvider for Nothing {}

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

    #[test]
    fn default_provider_methods_report_unimplemented() {
        let p = Nothing;
        let err = p.list(&NullHost, None, None).unwrap_err();
        assert_eq!(err.code, AddonErrorCode::Internal);
        assert!(err.message.contains("catalog"));
        let err = p.resolve(&NullHost, "x", None).unwrap_err();
        assert!(err.message.contains("resolve"));
    }
}
