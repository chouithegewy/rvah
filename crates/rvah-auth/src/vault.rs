use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, Nonce};
use rand::RngCore;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum VaultError {
    #[error("vault io error: {0}")]
    Io(String),
    #[error("vault crypto error: {0}")]
    Crypto(String),
}

/// Host-mediated secret storage, isolated per add-on (FR-083).
pub trait SecretVault: Send + Sync {
    fn set(&self, addon_id: &str, name: &str, value: &str) -> Result<(), VaultError>;
    fn get(&self, addon_id: &str, name: &str) -> Result<Option<String>, VaultError>;
    fn delete(&self, addon_id: &str, name: &str) -> Result<(), VaultError>;
    fn list(&self, addon_id: &str) -> Result<Vec<String>, VaultError>;
}

type SecretMap = BTreeMap<String, BTreeMap<String, String>>;

/// In-memory vault for tests and the developer simulator.
#[derive(Default)]
pub struct MemoryVault {
    data: Mutex<SecretMap>,
}

impl MemoryVault {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretVault for MemoryVault {
    fn set(&self, addon_id: &str, name: &str, value: &str) -> Result<(), VaultError> {
        self.data
            .lock()
            .unwrap()
            .entry(addon_id.to_string())
            .or_default()
            .insert(name.to_string(), value.to_string());
        Ok(())
    }

    fn get(&self, addon_id: &str, name: &str) -> Result<Option<String>, VaultError> {
        Ok(self
            .data
            .lock()
            .unwrap()
            .get(addon_id)
            .and_then(|m| m.get(name))
            .cloned())
    }

    fn delete(&self, addon_id: &str, name: &str) -> Result<(), VaultError> {
        if let Some(m) = self.data.lock().unwrap().get_mut(addon_id) {
            m.remove(name);
        }
        Ok(())
    }

    fn list(&self, addon_id: &str) -> Result<Vec<String>, VaultError> {
        Ok(self
            .data
            .lock()
            .unwrap()
            .get(addon_id)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default())
    }
}

/// AES-256-GCM encrypted vault file; a fresh random nonce is used per write.
///
/// The key file is auto-generated (32 random bytes, hex-encoded, mode 0600 on
/// unix) when missing. Opening an existing vault with the wrong key fails with
/// [`VaultError::Crypto`].
pub struct EncryptedFileVault {
    path: PathBuf,
    cipher: Aes256Gcm,
    data: Mutex<SecretMap>,
}

impl EncryptedFileVault {
    pub fn open(path: impl AsRef<Path>, key_path: impl AsRef<Path>) -> Result<Self, VaultError> {
        let key = load_or_create_key(key_path.as_ref())?;
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
        let path = path.as_ref().to_path_buf();
        let data = if path.exists() {
            let raw = std::fs::read(&path).map_err(|e| VaultError::Io(e.to_string()))?;
            decrypt_map(&cipher, &raw)?
        } else {
            SecretMap::new()
        };
        Ok(Self {
            path,
            cipher,
            data: Mutex::new(data),
        })
    }

    fn persist(&self, data: &SecretMap) -> Result<(), VaultError> {
        let plaintext =
            serde_json::to_vec(data).map_err(|e| VaultError::Crypto(e.to_string()))?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_slice())
            .map_err(|e| VaultError::Crypto(e.to_string()))?;
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&ciphertext);
        std::fs::write(&self.path, blob).map_err(|e| VaultError::Io(e.to_string()))
    }
}

fn load_or_create_key(key_path: &Path) -> Result<[u8; 32], VaultError> {
    if key_path.exists() {
        let hex_str = std::fs::read_to_string(key_path).map_err(|e| VaultError::Io(e.to_string()))?;
        let bytes = hex::decode(hex_str.trim()).map_err(|e| VaultError::Crypto(e.to_string()))?;
        bytes
            .try_into()
            .map_err(|_| VaultError::Crypto("key file must hold 32 bytes".into()))
    } else {
        let mut key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        std::fs::write(key_path, hex::encode(key)).map_err(|e| VaultError::Io(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| VaultError::Io(e.to_string()))?;
        }
        Ok(key)
    }
}

fn decrypt_map(cipher: &Aes256Gcm, raw: &[u8]) -> Result<SecretMap, VaultError> {
    if raw.len() < 12 {
        return Err(VaultError::Crypto("vault file truncated".into()));
    }
    let (nonce, ciphertext) = raw.split_at(12);
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| VaultError::Crypto("decryption failed (wrong key or corrupt vault)".into()))?;
    serde_json::from_slice(&plaintext).map_err(|e| VaultError::Crypto(e.to_string()))
}

impl SecretVault for EncryptedFileVault {
    fn set(&self, addon_id: &str, name: &str, value: &str) -> Result<(), VaultError> {
        let mut data = self.data.lock().unwrap();
        data.entry(addon_id.to_string())
            .or_default()
            .insert(name.to_string(), value.to_string());
        self.persist(&data)
    }

    fn get(&self, addon_id: &str, name: &str) -> Result<Option<String>, VaultError> {
        Ok(self
            .data
            .lock()
            .unwrap()
            .get(addon_id)
            .and_then(|m| m.get(name))
            .cloned())
    }

    fn delete(&self, addon_id: &str, name: &str) -> Result<(), VaultError> {
        let mut data = self.data.lock().unwrap();
        if let Some(m) = data.get_mut(addon_id) {
            m.remove(name);
        }
        self.persist(&data)
    }

    fn list(&self, addon_id: &str) -> Result<Vec<String>, VaultError> {
        Ok(self
            .data
            .lock()
            .unwrap()
            .get(addon_id)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default())
    }
}

/// Device-code authorization state machine (UC-04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceCodeState {
    Pending {
        user_code: String,
        verification_url: String,
        expires_at: u64,
    },
    Authorized,
    Expired,
    Denied,
}

/// What the injected poller observed at the authorization server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollOutcome {
    Pending,
    Authorized { token: String },
    Denied,
}

/// Drives a device-code grant: poll until authorized, denied, or expired.
/// The token is only ever written to the vault, never returned (FR-084).
pub struct DeviceCodeFlow {
    state: DeviceCodeState,
    addon_id: String,
    secret_name: String,
}

impl DeviceCodeFlow {
    pub fn new(
        user_code: impl Into<String>,
        verification_url: impl Into<String>,
        expires_at: u64,
        addon_id: impl Into<String>,
        secret_name: impl Into<String>,
    ) -> Self {
        Self {
            state: DeviceCodeState::Pending {
                user_code: user_code.into(),
                verification_url: verification_url.into(),
                expires_at,
            },
            addon_id: addon_id.into(),
            secret_name: secret_name.into(),
        }
    }

    pub fn state(&self) -> &DeviceCodeState {
        &self.state
    }

    /// Advance the flow: expiry is checked before the poller runs, so an
    /// expired grant never stores a token (TC-AUTH-002).
    pub fn poll(
        &mut self,
        now_unix: u64,
        poller: &mut dyn FnMut() -> PollOutcome,
        vault: &dyn SecretVault,
    ) -> Result<&DeviceCodeState, VaultError> {
        if let DeviceCodeState::Pending { expires_at, .. } = &self.state {
            if now_unix >= *expires_at {
                self.state = DeviceCodeState::Expired;
                return Ok(&self.state);
            }
            match poller() {
                PollOutcome::Pending => {}
                PollOutcome::Authorized { token } => {
                    vault.set(&self.addon_id, &self.secret_name, &token)?;
                    self.state = DeviceCodeState::Authorized;
                }
                PollOutcome::Denied => self.state = DeviceCodeState::Denied,
            }
        }
        Ok(&self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_roundtrip_and_isolation_by_addon() {
        let vault = MemoryVault::new();
        vault.set("com.example.a", "token", "s3cr3t").unwrap();
        assert_eq!(
            vault.get("com.example.a", "token").unwrap().as_deref(),
            Some("s3cr3t")
        );
        // Another add-on must not see it (FR-083).
        assert_eq!(vault.get("com.example.b", "token").unwrap(), None);
        assert_eq!(vault.list("com.example.a").unwrap(), vec!["token"]);
        assert!(vault.list("com.example.b").unwrap().is_empty());
        vault.delete("com.example.a", "token").unwrap();
        assert_eq!(vault.get("com.example.a", "token").unwrap(), None);
    }

    #[test]
    fn encrypted_vault_persists_across_open() {
        let dir = tempfile::tempdir().unwrap();
        let vault_path = dir.path().join("secrets.vault");
        let key_path = dir.path().join("vault.key");
        {
            let vault = EncryptedFileVault::open(&vault_path, &key_path).unwrap();
            vault.set("com.example.a", "token", "persist-me").unwrap();
        }
        // Ciphertext on disk must not contain the plaintext secret.
        let raw = std::fs::read(&vault_path).unwrap();
        assert!(!raw.windows(10).any(|w| w == b"persist-me"));

        let vault = EncryptedFileVault::open(&vault_path, &key_path).unwrap();
        assert_eq!(
            vault.get("com.example.a", "token").unwrap().as_deref(),
            Some("persist-me")
        );
    }

    #[test]
    fn encrypted_vault_wrong_key_fails() {
        let dir = tempfile::tempdir().unwrap();
        let vault_path = dir.path().join("secrets.vault");
        {
            let vault =
                EncryptedFileVault::open(&vault_path, dir.path().join("right.key")).unwrap();
            vault.set("com.example.a", "token", "s3cr3t").unwrap();
        }
        let err = EncryptedFileVault::open(&vault_path, dir.path().join("wrong.key"))
            .err()
            .expect("open with wrong key must fail");
        assert!(matches!(err, VaultError::Crypto(_)));
    }

    #[test]
    fn device_code_success_stores_token() {
        let vault = MemoryVault::new();
        let mut flow = DeviceCodeFlow::new("ABCD-1234", "https://ex.com/activate", 1_000, "com.example.a", "oauth_token");
        let mut outcomes = vec![PollOutcome::Pending, PollOutcome::Authorized { token: "tok-99".into() }].into_iter();
        let mut poller = move || outcomes.next().unwrap();

        assert!(matches!(
            flow.poll(10, &mut poller, &vault).unwrap(),
            DeviceCodeState::Pending { .. }
        ));
        assert_eq!(
            flow.poll(20, &mut poller, &vault).unwrap(),
            &DeviceCodeState::Authorized
        );
        assert_eq!(
            vault.get("com.example.a", "oauth_token").unwrap().as_deref(),
            Some("tok-99")
        );
    }

    #[test]
    fn device_code_expiry_stores_nothing() {
        let vault = MemoryVault::new();
        let mut flow = DeviceCodeFlow::new("ABCD-1234", "https://ex.com/activate", 1_000, "com.example.a", "oauth_token");
        // Poller would authorize, but the code is already expired.
        let mut poller = || PollOutcome::Authorized { token: "late".into() };
        assert_eq!(
            flow.poll(1_000, &mut poller, &vault).unwrap(),
            &DeviceCodeState::Expired
        );
        assert_eq!(vault.get("com.example.a", "oauth_token").unwrap(), None);
        // Terminal state: further polls do not resurrect the flow.
        assert_eq!(
            flow.poll(2_000, &mut poller, &vault).unwrap(),
            &DeviceCodeState::Expired
        );
    }

    #[test]
    fn device_code_denied_is_terminal() {
        let vault = MemoryVault::new();
        let mut flow =
            DeviceCodeFlow::new("X", "https://ex.com", 1_000, "com.example.a", "oauth_token");
        let mut poller = || PollOutcome::Denied;
        assert_eq!(
            flow.poll(1, &mut poller, &vault).unwrap(),
            &DeviceCodeState::Denied
        );
        assert_eq!(vault.get("com.example.a", "oauth_token").unwrap(), None);
    }
}
