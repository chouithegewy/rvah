//! Portable, encrypted RVAH profiles.
//!
//! A profile contains explicit add-on settings and secrets plus an optional
//! exact-package reconstruction recipe. It does not contain package bytes,
//! repository trust, permission grants, caches, diagnostics, favorites, or
//! playback history. The encrypted bytes are suitable for untrusted storage;
//! the 256-bit profile key must be enrolled separately on each device.

use std::collections::BTreeMap;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::TryRng;
#[cfg(feature = "native")]
use rvah_auth::{SecretVault, VaultError};
#[cfg(feature = "native")]
use rvah_data::{DataError, Database};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

const MAGIC: &[u8; 8] = b"RVAHPB1\0";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const FORMAT_VERSION: u16 = 1;
/// Keeps one synchronized profile comfortably below Veilid's total DHT-value
/// ceiling after record schema and subkey overhead.
const MAX_BUNDLE_BYTES: usize = 512 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct ProfileKey([u8; 32]);

impl ProfileKey {
    pub fn generate() -> Self {
        let mut bytes = [0_u8; 32];
        rand::rngs::SysRng
            .try_fill_bytes(&mut bytes)
            .expect("operating system random number generator failed");
        Self(bytes)
    }

    pub fn from_hex(value: &str) -> Result<Self, ProfileError> {
        let bytes = hex::decode(value.trim()).map_err(|_| ProfileError::InvalidKey)?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| ProfileError::InvalidKey)?;
        Ok(Self(bytes))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl Drop for ProfileKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Deliberately reveal neither the key nor a fingerprint in diagnostics.
impl std::fmt::Debug for ProfileKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProfileKey([REDACTED])")
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ProfileError {
    #[error("profile key must be exactly 32 bytes encoded as 64 hexadecimal characters")]
    InvalidKey,
    #[error("profile bundle is truncated or has an unknown format")]
    InvalidFormat,
    #[error("profile bundle uses unsupported version {0}")]
    UnsupportedVersion(u16),
    #[error("profile bundle authentication failed (wrong key or modified data)")]
    Authentication,
    #[error("profile bundle exceeds the 512 KiB portable-profile limit")]
    TooLarge,
    #[error("profile bundle serialization failed: {0}")]
    Serialization(String),
    #[cfg(feature = "native")]
    #[error("profile database operation failed: {0}")]
    Data(#[from] DataError),
    #[cfg(feature = "native")]
    #[error("profile secret-vault operation failed: {0}")]
    Vault(#[from] VaultError),
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddonRestore {
    /// Exact SHA-256 of the signed package expected from a trusted repository.
    pub package_sha256: String,
    /// Disabled add-ons remain disabled after an explicitly approved restore.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddonProfile {
    /// Informational source version. Import does not install or downgrade it.
    pub source_version: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, String>,
    /// Optional package recipe. The repository URL and trust key stay outside
    /// the bundle and must be supplied and approved on the target device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<AddonRestore>,
}

/// Plaintext exists only inside the trusted application boundary before it is
/// encrypted or after it is authenticated. Do not log or implement `Debug` for
/// this type: it contains provider credentials.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortableProfile {
    pub format_version: u16,
    pub created_at_unix: u64,
    pub addons: BTreeMap<String, AddonProfile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportSummary {
    pub addons: usize,
    pub settings: usize,
    pub secrets: usize,
}

impl PortableProfile {
    pub fn empty() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            created_at_unix: now_unix(),
            addons: BTreeMap::new(),
        }
    }

    /// Capture all explicitly configured installed add-ons. Defaults and
    /// runtime storage are excluded; secret values are never logged.
    #[cfg(feature = "native")]
    pub fn collect(db: &Database, vault: &dyn SecretVault) -> Result<Self, ProfileError> {
        let mut addons = BTreeMap::new();
        for addon in db.list_addons()? {
            let settings = db
                .list_settings(&addon.id)?
                .into_iter()
                .map(|setting| (setting.key, setting.value))
                .collect();
            let mut secrets = BTreeMap::new();
            for name in vault.list(&addon.id)? {
                if let Some(value) = vault.get(&addon.id, &name)? {
                    secrets.insert(name, value);
                }
            }
            addons.insert(
                addon.id,
                AddonProfile {
                    source_version: addon.version,
                    settings,
                    secrets,
                    restore: Some(AddonRestore {
                        package_sha256: addon.package_hash,
                        enabled: matches!(addon.status, rvah_data::AddonStatus::Enabled),
                    }),
                },
            );
        }
        Ok(Self {
            format_version: FORMAT_VERSION,
            created_at_unix: now_unix(),
            addons,
        })
    }

    /// Overlay profile values without installing packages, restoring
    /// permission grants, or deleting target-device values absent from the
    /// bundle. Configuration can therefore be imported before add-ons arrive.
    /// Best-effort rollback restores earlier values when a write fails.
    #[cfg(feature = "native")]
    pub fn apply(
        &self,
        db: &Database,
        vault: &dyn SecretVault,
    ) -> Result<ImportSummary, ProfileError> {
        if self.format_version != FORMAT_VERSION {
            return Err(ProfileError::UnsupportedVersion(self.format_version));
        }

        let mut old_secrets = Vec::new();
        let mut old_settings = Vec::new();
        for (addon_id, addon) in &self.addons {
            for name in addon.secrets.keys() {
                old_secrets.push((addon_id.clone(), name.clone(), vault.get(addon_id, name)?));
            }
            for key in addon.settings.keys() {
                old_settings.push((
                    addon_id.clone(),
                    key.clone(),
                    db.get_setting(addon_id, key)?,
                ));
            }
        }

        let mut applied_secrets = 0;
        for (addon_id, addon) in &self.addons {
            for (name, value) in &addon.secrets {
                if let Err(error) = vault.set(addon_id, name, value) {
                    rollback_secrets(vault, &old_secrets[..applied_secrets]);
                    return Err(error.into());
                }
                applied_secrets += 1;
            }
        }

        let mut applied_settings = 0;
        for (addon_id, addon) in &self.addons {
            for (key, value) in &addon.settings {
                if let Err(error) = db.set_setting(addon_id, key, value) {
                    rollback_settings(db, &old_settings[..applied_settings]);
                    rollback_secrets(vault, &old_secrets);
                    return Err(error.into());
                }
                applied_settings += 1;
            }
        }

        Ok(ImportSummary {
            addons: self.addons.len(),
            settings: applied_settings,
            secrets: applied_secrets,
        })
    }

    pub fn encrypt(&self, key: &ProfileKey) -> Result<Vec<u8>, ProfileError> {
        if self.format_version != FORMAT_VERSION {
            return Err(ProfileError::UnsupportedVersion(self.format_version));
        }
        let plaintext = Zeroizing::new(
            serde_json::to_vec(self)
                .map_err(|error| ProfileError::Serialization(error.to_string()))?,
        );
        if MAGIC.len() + NONCE_LEN + TAG_LEN + plaintext.len() > MAX_BUNDLE_BYTES {
            return Err(ProfileError::TooLarge);
        }
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key.0));
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &plaintext,
                    aad: MAGIC,
                },
            )
            .map_err(|_| ProfileError::Authentication)?;
        let mut output = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ciphertext.len());
        output.extend_from_slice(MAGIC);
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    pub fn decrypt(bytes: &[u8], key: &ProfileKey) -> Result<Self, ProfileError> {
        if bytes.len() > MAX_BUNDLE_BYTES {
            return Err(ProfileError::TooLarge);
        }
        if bytes.len() < MAGIC.len() + NONCE_LEN + TAG_LEN || &bytes[..MAGIC.len()] != MAGIC {
            return Err(ProfileError::InvalidFormat);
        }
        let nonce = Nonce::from_slice(&bytes[MAGIC.len()..MAGIC.len() + NONCE_LEN]);
        let ciphertext = &bytes[MAGIC.len() + NONCE_LEN..];
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key.0));
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    nonce,
                    Payload {
                        msg: ciphertext,
                        aad: MAGIC,
                    },
                )
                .map_err(|_| ProfileError::Authentication)?,
        );
        let profile: Self = serde_json::from_slice(&plaintext)
            .map_err(|error| ProfileError::Serialization(error.to_string()))?;
        if profile.format_version != FORMAT_VERSION {
            return Err(ProfileError::UnsupportedVersion(profile.format_version));
        }
        Ok(profile)
    }
}

fn now_unix() -> u64 {
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0)
    }

    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    {
        (js_sys::Date::now() / 1_000.0) as u64
    }
}

#[cfg(feature = "native")]
fn rollback_secrets(vault: &dyn SecretVault, values: &[(String, String, Option<String>)]) {
    for (addon_id, name, old) in values.iter().rev() {
        match old {
            Some(value) => {
                let _ = vault.set(addon_id, name, value);
            }
            None => {
                let _ = vault.delete(addon_id, name);
            }
        }
    }
}

#[cfg(feature = "native")]
fn rollback_settings(db: &Database, values: &[(String, String, Option<String>)]) {
    for (addon_id, key, old) in values.iter().rev() {
        match old {
            Some(value) => {
                let _ = db.set_setting(addon_id, key, value);
            }
            None => {
                let _ = db.delete_setting(addon_id, key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvah_auth::MemoryVault;
    use rvah_data::{AddonRecord, AddonStatus};

    fn addon(id: &str) -> AddonRecord {
        AddonRecord {
            id: id.into(),
            version: "1.2.3".into(),
            package_hash: "hash".into(),
            status: AddonStatus::Enabled,
            install_source: "test".into(),
            sdk_range: ">=0.1, <0.2".into(),
            capabilities: vec!["auth".into()],
        }
    }

    #[test]
    fn encrypted_profile_roundtrips_settings_and_secrets() {
        let source = Database::open_in_memory().unwrap();
        let source_vault = MemoryVault::new();
        source.upsert_addon(&addon("com.example.service")).unwrap();
        source
            .set_setting("com.example.service", "region", "US")
            .unwrap();
        source_vault
            .set("com.example.service", "refresh_token", "never-log-me")
            .unwrap();

        let key = ProfileKey::generate();
        let encrypted = PortableProfile::collect(&source, &source_vault)
            .unwrap()
            .encrypt(&key)
            .unwrap();
        let profile = PortableProfile::decrypt(&encrypted, &key).unwrap();

        let target = Database::open_in_memory().unwrap();
        let target_vault = MemoryVault::new();
        let summary = profile.apply(&target, &target_vault).unwrap();
        assert_eq!(summary.addons, 1);
        assert_eq!(summary.settings, 1);
        assert_eq!(summary.secrets, 1);
        assert_eq!(
            target
                .get_setting("com.example.service", "region")
                .unwrap()
                .as_deref(),
            Some("US")
        );
        assert_eq!(
            target_vault
                .get("com.example.service", "refresh_token")
                .unwrap()
                .as_deref(),
            Some("never-log-me")
        );
    }

    #[test]
    fn wrong_key_and_tampering_are_rejected_without_plaintext_errors() {
        let profile = PortableProfile {
            format_version: FORMAT_VERSION,
            created_at_unix: 0,
            addons: BTreeMap::new(),
        };
        let encrypted = profile.encrypt(&ProfileKey::generate()).unwrap();
        let error = match PortableProfile::decrypt(&encrypted, &ProfileKey::generate()) {
            Err(error) => error,
            Ok(_) => panic!("a profile decrypted with the wrong key"),
        };
        assert_eq!(
            error.to_string(),
            "profile bundle authentication failed (wrong key or modified data)"
        );

        let key = ProfileKey::generate();
        let mut encrypted = profile.encrypt(&key).unwrap();
        let last = encrypted.len() - 1;
        encrypted[last] ^= 1;
        assert!(matches!(
            PortableProfile::decrypt(&encrypted, &key),
            Err(ProfileError::Authentication)
        ));
    }

    #[test]
    fn profile_key_debug_is_redacted() {
        let key = ProfileKey::from_hex(&"ab".repeat(32)).unwrap();
        let debug = format!("{key:?}");
        assert!(!debug.contains("abab"));
        assert!(debug.contains("REDACTED"));
    }
}
