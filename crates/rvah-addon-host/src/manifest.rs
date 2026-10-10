//! `rvah.toml` manifest parsing and validation (FR-002, FR-003).
//!
//! The manifest is the security boundary: it declares identity, entry module,
//! capabilities, network/storage/secret permissions, and settings schema.

use std::collections::BTreeMap;

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

/// SDK version implemented by this host build. Manifests must declare a
/// compatible `sdk` range or installation is rejected (TC-ADD-003).
pub fn host_sdk_version() -> Version {
    Version::parse(rvah_addon_sdk::SDK_VERSION).expect("valid SDK version")
}

/// Convenience constant string form of the host SDK version.
pub const HOST_SDK_VERSION: &str = rvah_addon_sdk::SDK_VERSION;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Catalog,
    Search,
    Resolve,
    Auth,
    AccountStatus,
    /// Details pages: genres, trailers, ratings.
    Details,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct AddonSection {
    pub id: String,
    pub name: String,
    pub version: Version,
    /// Supported host SDK range, e.g. `">=0.1,<0.2"`.
    pub sdk: VersionReq,
    pub entry: String,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
pub struct RawPermissions {
    #[serde(default)]
    pub network: Vec<String>,
    /// Human-readable size such as "10MiB"; defaults to 1MiB.
    #[serde(default)]
    pub storage: Option<String>,
    #[serde(default)]
    pub secrets: Vec<String>,
}

/// Validated permissions with the storage quota resolved to bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionsSection {
    pub network: Vec<String>,
    pub storage_bytes: u64,
    pub secrets: Vec<String>,
}

pub const DEFAULT_STORAGE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SettingSpec {
    #[serde(rename = "type")]
    pub r#type: String,
    pub default: toml::Value,
    #[serde(default, rename = "enum")]
    pub r#enum: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ManifestError {
    #[error("missing required field: {0}")]
    MissingField(String),
    #[error("invalid version: {0}")]
    InvalidVersion(String),
    #[error("unsupported SDK range: {0}")]
    InvalidSdkRange(String),
    #[error("invalid add-on id: {0}")]
    InvalidId(String),
    #[error("invalid permission: {0}")]
    InvalidPermission(String),
    #[error("manifest parse error: {0}")]
    Toml(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct RawManifest {
    addon: AddonSection,
    #[serde(default)]
    permissions: RawPermissions,
    #[serde(default)]
    settings: BTreeMap<String, SettingSpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub addon: AddonSection,
    pub permissions: PermissionsSection,
    pub settings: BTreeMap<String, SettingSpec>,
}

impl Manifest {
    pub fn parse(toml_str: &str) -> Result<Manifest, ManifestError> {
        let raw: RawManifest = toml::from_str(toml_str).map_err(|e| {
            let msg = e.to_string();
            // Surface missing required fields distinctly from other syntax errors.
            if msg.contains("missing field") {
                let field = msg.split('`').nth(1).unwrap_or("unknown").to_string();
                ManifestError::MissingField(field)
            } else if msg.contains("expected string") && msg.contains("version") {
                ManifestError::InvalidVersion(msg)
            } else {
                ManifestError::Toml(msg)
            }
        })?;

        validate_id(&raw.addon.id)?;
        if raw.addon.entry.trim().is_empty() {
            return Err(ManifestError::MissingField("addon.entry".into()));
        }

        let mut network = Vec::new();
        for domain in &raw.permissions.network {
            validate_domain_rule(domain)?;
            network.push(domain.to_ascii_lowercase());
        }

        let storage_bytes = match &raw.permissions.storage {
            None => DEFAULT_STORAGE_BYTES,
            Some(s) => parse_size(s)
                .ok_or_else(|| ManifestError::InvalidPermission(format!("storage: {s}")))?,
        };

        Ok(Manifest {
            addon: raw.addon,
            permissions: PermissionsSection {
                network,
                storage_bytes,
                secrets: raw.permissions.secrets,
            },
            settings: raw.settings,
        })
    }

    /// Whether this add-on's declared SDK range matches the running host.
    pub fn is_sdk_compatible(&self) -> bool {
        self.addon.sdk.matches(&host_sdk_version())
    }

    /// Stable hash of the permission set, used to detect permission expansion
    /// across updates (TC-ADD-005).
    pub fn permission_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let canonical = serde_json::to_vec(&self.permissions).expect("serializable");
        hex::encode(Sha256::digest(canonical))
    }

    pub fn has_capability(&self, cap: Capability) -> bool {
        self.addon.capabilities.contains(&cap)
    }

    /// Default setting values from the schema, as display strings.
    pub fn default_settings(&self) -> BTreeMap<String, String> {
        self.settings
            .iter()
            .map(|(k, spec)| (k.clone(), toml_value_to_string(&spec.default)))
            .collect()
    }
}

fn toml_value_to_string(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Reverse-DNS style: at least two lowercase alphanumeric/hyphen labels.
fn validate_id(id: &str) -> Result<(), ManifestError> {
    let labels: Vec<&str> = id.split('.').collect();
    let label_ok = |l: &&str| {
        !l.is_empty()
            && l.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    if labels.len() < 2 || !labels.iter().all(label_ok) {
        return Err(ManifestError::InvalidId(id.to_string()));
    }
    Ok(())
}

/// Network permissions are bare domains, optionally with a leading `*.`
/// wildcard label. Schemes, ports, paths, and IPs are rejected.
fn validate_domain_rule(rule: &str) -> Result<(), ManifestError> {
    let err = || ManifestError::InvalidPermission(format!("network: {rule}"));
    let host = rule.strip_prefix("*.").unwrap_or(rule);
    if host.is_empty()
        || rule.contains("://")
        || rule.contains('/')
        || rule.contains(':')
        || host.contains('*')
    {
        return Err(err());
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 {
        return Err(err());
    }
    for l in &labels {
        if l.is_empty() || !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(err());
        }
    }
    // Reject IPv4 literals: wildcarding or allowlisting raw IPs is not allowed.
    if labels.iter().all(|l| l.chars().all(|c| c.is_ascii_digit())) {
        return Err(err());
    }
    Ok(())
}

/// Parse sizes like "512KiB", "10MiB", "1GiB", or plain byte counts.
fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = if let Some(n) = s.strip_suffix("GiB") {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("MiB") {
        (n, 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("KiB") {
        (n, 1024)
    } else if let Some(n) = s.strip_suffix('B') {
        (n, 1)
    } else {
        (s, 1)
    };
    let value: u64 = num.trim().parse().ok()?;
    value.checked_mul(mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
[addon]
id = "provider.example.video"
name = "Example Video Provider"
version = "1.2.0"
sdk = ">=0.1, <0.2"
entry = "module.wasm"
capabilities = ["catalog", "search", "resolve", "auth"]

[permissions]
network = ["api.example.com", "cdn.example.com"]
storage = "10MiB"
secrets = ["provider_token"]

[settings]
region = { type = "string", default = "us", enum = ["us", "gb", "ca"] }
"#;

    #[test]
    fn parses_full_manifest() {
        let m = Manifest::parse(FULL).unwrap();
        assert_eq!(m.addon.id, "provider.example.video");
        assert_eq!(m.addon.version, Version::new(1, 2, 0));
        assert!(m.is_sdk_compatible());
        assert!(m.has_capability(Capability::Resolve));
        assert!(!m.has_capability(Capability::AccountStatus));
        assert_eq!(
            m.permissions.network,
            vec!["api.example.com", "cdn.example.com"]
        );
        assert_eq!(m.permissions.storage_bytes, 10 * 1024 * 1024);
        assert_eq!(m.permissions.secrets, vec!["provider_token"]);
        assert_eq!(m.settings["region"].r#type, "string");
        assert_eq!(m.default_settings()["region"], "us");
    }

    #[test]
    fn rejects_missing_id() {
        let toml = FULL.replace("id = \"provider.example.video\"\n", "");
        match Manifest::parse(&toml) {
            Err(ManifestError::MissingField(f)) => assert_eq!(f, "id"),
            other => panic!("expected MissingField, got {other:?}"),
        }
    }

    #[test]
    fn rejects_bad_semver() {
        let toml = FULL.replace("version = \"1.2.0\"", "version = \"one.two\"");
        assert!(Manifest::parse(&toml).is_err());
    }

    #[test]
    fn rejects_unsupported_sdk_range() {
        let toml = FULL.replace(">=0.1, <0.2", ">=9.0");
        let m = Manifest::parse(&toml).unwrap();
        assert!(!m.is_sdk_compatible());
    }

    #[test]
    fn rejects_invalid_ids() {
        for bad in ["single", "Upper.Case", "a..b", "-bad.start", "trail-.dom"] {
            let toml = FULL.replace("provider.example.video", bad);
            assert!(
                matches!(Manifest::parse(&toml), Err(ManifestError::InvalidId(_))),
                "expected InvalidId for {bad}"
            );
        }
    }

    #[test]
    fn rejects_url_in_network_permission() {
        for bad in [
            "https://api.example.com",
            "api.example.com/path",
            "api.example.com:8080",
            "*",
            "*.com",
            "a.*.example.com",
            "127.0.0.1",
        ] {
            let toml = FULL.replace("\"api.example.com\"", &format!("\"{bad}\""));
            assert!(
                matches!(
                    Manifest::parse(&toml),
                    Err(ManifestError::InvalidPermission(_))
                ),
                "expected InvalidPermission for {bad}"
            );
        }
    }

    #[test]
    fn parses_storage_quota_mib() {
        assert_eq!(parse_size("10MiB"), Some(10 * 1024 * 1024));
        assert_eq!(parse_size("512KiB"), Some(512 * 1024));
        assert_eq!(parse_size("2048"), Some(2048));
        assert_eq!(parse_size("junk"), None);
        // Missing storage falls back to the default quota.
        let toml = FULL.replace("storage = \"10MiB\"\n", "");
        let m = Manifest::parse(&toml).unwrap();
        assert_eq!(m.permissions.storage_bytes, DEFAULT_STORAGE_BYTES);
    }

    #[test]
    fn permission_hash_changes_on_expansion() {
        let m1 = Manifest::parse(FULL).unwrap();
        let expanded = FULL.replace(
            "network = [\"api.example.com\", \"cdn.example.com\"]",
            "network = [\"api.example.com\", \"cdn.example.com\", \"evil.example.org\"]",
        );
        let m2 = Manifest::parse(&expanded).unwrap();
        assert_ne!(m1.permission_hash(), m2.permission_hash());
        assert_eq!(
            m1.permission_hash(),
            Manifest::parse(FULL).unwrap().permission_hash()
        );
    }
}
