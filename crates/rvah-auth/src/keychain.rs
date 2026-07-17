//! OS-keychain [`SecretVault`] over the freedesktop Secret Service D-Bus
//! API (the alternative store SDD §9 lists). One keyring item per
//! `(addon_id, name)`, tagged with searchable attributes so `list`
//! enumerates properly and add-ons stay isolated from each other.

use std::collections::HashMap;

use secret_service::blocking::SecretService;
use secret_service::EncryptionType;

use crate::{SecretVault, VaultError};

const APPLICATION: &str = "rvah";

pub struct KeychainVault {
    service: SecretService<'static>,
}

fn ss_err(e: secret_service::Error) -> VaultError {
    VaultError::Io(format!("secret service: {e}"))
}

impl KeychainVault {
    /// Connect to the session's Secret Service (D-Bus transport encrypted
    /// via DH session keys). Fails if no daemon is reachable.
    pub fn connect() -> Result<Self, VaultError> {
        let service = SecretService::connect(EncryptionType::Dh).map_err(ss_err)?;
        Ok(Self { service })
    }

    fn attributes<'a>(addon_id: &'a str, name: Option<&'a str>) -> HashMap<&'a str, &'a str> {
        let mut attrs = HashMap::from([("application", APPLICATION), ("addon", addon_id)]);
        if let Some(name) = name {
            attrs.insert("name", name);
        }
        attrs
    }

    fn search(
        &self,
        addon_id: &str,
        name: Option<&str>,
    ) -> Result<Vec<secret_service::blocking::Item<'_>>, VaultError> {
        let results = self
            .service
            .search_items(Self::attributes(addon_id, name))
            .map_err(ss_err)?;
        // Locked items are unlocked on demand; the daemon may prompt.
        for item in &results.locked {
            item.unlock().map_err(ss_err)?;
        }
        let mut items = results.unlocked;
        items.extend(results.locked);
        Ok(items)
    }
}

impl SecretVault for KeychainVault {
    fn set(&self, addon_id: &str, name: &str, value: &str) -> Result<(), VaultError> {
        let collection = self.service.get_default_collection().map_err(ss_err)?;
        collection
            .create_item(
                &format!("RVAH secret: {addon_id}/{name}"),
                Self::attributes(addon_id, Some(name)),
                value.as_bytes(),
                true, // replace an existing item with the same attributes
                "text/plain",
            )
            .map_err(ss_err)?;
        Ok(())
    }

    fn get(&self, addon_id: &str, name: &str) -> Result<Option<String>, VaultError> {
        let items = self.search(addon_id, Some(name))?;
        let Some(item) = items.first() else {
            return Ok(None);
        };
        let bytes = item.get_secret().map_err(ss_err)?;
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|e| VaultError::Crypto(format!("secret not utf-8: {e}")))
    }

    fn delete(&self, addon_id: &str, name: &str) -> Result<(), VaultError> {
        for item in self.search(addon_id, Some(name))? {
            item.delete().map_err(ss_err)?;
        }
        Ok(())
    }

    fn list(&self, addon_id: &str) -> Result<Vec<String>, VaultError> {
        let mut names = Vec::new();
        for item in self.search(addon_id, None)? {
            let attrs = item.get_attributes().map_err(ss_err)?;
            if let Some(name) = attrs.get("name") {
                names.push(name.clone());
            }
        }
        names.sort();
        names.dedup();
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Skips (with a note) when no Secret Service daemon is reachable, so
    /// plain CI stays green; run locally against gnome-keyring.
    fn vault() -> Option<KeychainVault> {
        match KeychainVault::connect() {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("skipping keychain test: {e}");
                None
            }
        }
    }

    #[test]
    fn keychain_roundtrip_isolation_and_list() {
        let Some(vault) = vault() else { return };
        let a = "com.example.keychain-test-a";
        let b = "com.example.keychain-test-b";
        // Clean slate in case an earlier run aborted.
        for addon in [a, b] {
            for name in vault.list(addon).unwrap() {
                vault.delete(addon, &name).unwrap();
            }
        }

        vault.set(a, "token", "secret-a").unwrap();
        vault.set(a, "refresh", "secret-a2").unwrap();
        vault.set(b, "token", "secret-b").unwrap();

        assert_eq!(vault.get(a, "token").unwrap().as_deref(), Some("secret-a"));
        assert_eq!(vault.get(b, "token").unwrap().as_deref(), Some("secret-b"));
        assert_eq!(vault.list(a).unwrap(), vec!["refresh", "token"]);

        // Overwrite replaces, not duplicates.
        vault.set(a, "token", "rotated").unwrap();
        assert_eq!(vault.get(a, "token").unwrap().as_deref(), Some("rotated"));
        assert_eq!(vault.list(a).unwrap(), vec!["refresh", "token"]);

        // Delete is per (addon, name) and never crosses add-ons.
        vault.delete(a, "token").unwrap();
        assert_eq!(vault.get(a, "token").unwrap(), None);
        assert_eq!(vault.get(b, "token").unwrap().as_deref(), Some("secret-b"));

        for addon in [a, b] {
            for name in vault.list(addon).unwrap() {
                vault.delete(addon, &name).unwrap();
            }
            assert!(vault.list(addon).unwrap().is_empty());
        }
    }
}
