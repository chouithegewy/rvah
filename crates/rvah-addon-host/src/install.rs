use std::path::{Path, PathBuf};
use std::sync::Arc;

use ed25519_dalek::VerifyingKey;
use rvah_data::{AddonRecord, AddonStatus, Database};

use crate::manifest::{Manifest, ManifestError};
use crate::package::{
    extract_package, read_package_file, sha256_file, verify_package, PackageError,
};

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Package(#[from] PackageError),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("add-on requires SDK {required} but host provides {host}")]
    SdkIncompatible { required: String, host: String },
    #[error("add-on already installed: {0} (use update)")]
    AlreadyInstalled(String),
    #[error("add-on not installed: {0}")]
    NotInstalled(String),
    #[error("update changes permissions and needs re-approval (TC-ADD-005)")]
    PermissionExpansionNeedsApproval,
    #[error("user denied the requested permissions")]
    PermissionsDenied,
    #[error("data error: {0}")]
    Data(String),
    #[error("io error: {0}")]
    Io(String),
}

impl From<rvah_data::DataError> for InstallError {
    fn from(e: rvah_data::DataError) -> Self {
        InstallError::Data(e.to_string())
    }
}

/// Whether the user approved the permission grant shown at install/update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallDecision {
    Approve,
    Deny,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InstalledAddon {
    pub id: String,
    pub version: String,
    pub install_dir: PathBuf,
}

/// Add-on lifecycle: signed install, permission-diffed update, enable/disable,
/// uninstall with optional data purge, and integrity re-verification
/// (FR-001..FR-007).
pub struct AddonManager {
    db: Arc<Database>,
    install_root: PathBuf,
}

impl AddonManager {
    pub fn new(db: Arc<Database>, install_root: impl Into<PathBuf>) -> Self {
        Self {
            db,
            install_root: install_root.into(),
        }
    }

    pub fn db(&self) -> &Arc<Database> {
        &self.db
    }

    pub fn install_dir(&self, id: &str, version: &str) -> PathBuf {
        self.install_root.join(id).join(version)
    }

    fn stored_package_path(&self, id: &str) -> PathBuf {
        self.install_root.join(id).join("package.rvah")
    }

    /// Verify signature, validate the manifest, and read it — the shared
    /// front half of install and update.
    fn verify_and_parse(
        &self,
        pkg_path: &Path,
        sig: &[u8; 64],
        pubkey: &VerifyingKey,
    ) -> Result<(Manifest, String), InstallError> {
        verify_package(pkg_path, sig, pubkey)?;
        let manifest_bytes = read_package_file(pkg_path, "manifest.toml")?;
        let manifest_str = String::from_utf8(manifest_bytes)
            .map_err(|e| PackageError::Malformed(e.to_string()))?;
        let manifest = Manifest::parse(&manifest_str)?;
        if !manifest.is_sdk_compatible() {
            return Err(InstallError::SdkIncompatible {
                required: manifest.addon.sdk.to_string(),
                host: crate::manifest::HOST_SDK_VERSION.to_string(),
            });
        }
        let hash = hex::encode(sha256_file(pkg_path)?);
        Ok((manifest, hash))
    }

    fn materialize(
        &self,
        pkg_path: &Path,
        manifest: &Manifest,
        hash: &str,
        status: AddonStatus,
    ) -> Result<InstalledAddon, InstallError> {
        let id = &manifest.addon.id;
        let version = manifest.addon.version.to_string();
        let dir = self.install_dir(id, &version);
        extract_package(pkg_path, &dir)?;
        std::fs::copy(pkg_path, self.stored_package_path(id))
            .map_err(|e| InstallError::Io(e.to_string()))?;

        let permissions_json = serde_json::to_string(&manifest.permissions)
            .map_err(|e| InstallError::Data(e.to_string()))?;
        self.db
            .record_permissions(id, &permissions_json, &manifest.permission_hash())?;
        self.db.upsert_addon(&AddonRecord {
            id: id.clone(),
            version: version.clone(),
            package_hash: hash.to_string(),
            status,
            install_source: pkg_path.display().to_string(),
            sdk_range: manifest.addon.sdk.to_string(),
            capabilities: manifest
                .addon
                .capabilities
                .iter()
                .map(|c| format!("{c:?}").to_lowercase())
                .collect(),
        })?;
        Ok(InstalledAddon {
            id: id.clone(),
            version,
            install_dir: dir,
        })
    }

    /// Fresh install: signature → manifest validation → SDK check → duplicate
    /// check → explicit permission grant → extract and record.
    pub fn install(
        &self,
        pkg_path: &Path,
        sig: &[u8; 64],
        pubkey: &VerifyingKey,
        decision: InstallDecision,
    ) -> Result<InstalledAddon, InstallError> {
        let (manifest, hash) = self.verify_and_parse(pkg_path, sig, pubkey)?;
        if self.db.get_addon(&manifest.addon.id)?.is_some() {
            return Err(InstallError::AlreadyInstalled(manifest.addon.id));
        }
        if decision == InstallDecision::Deny {
            return Err(InstallError::PermissionsDenied);
        }
        self.materialize(pkg_path, &manifest, &hash, AddonStatus::Enabled)
    }

    /// Update in place. Settings, storage, and secrets are preserved
    /// (TC-ADD-004); a changed permission set requires re-approval
    /// (TC-ADD-005).
    pub fn update(
        &self,
        pkg_path: &Path,
        sig: &[u8; 64],
        pubkey: &VerifyingKey,
        decision: InstallDecision,
    ) -> Result<InstalledAddon, InstallError> {
        let (manifest, hash) = self.verify_and_parse(pkg_path, sig, pubkey)?;
        let existing = self
            .db
            .get_addon(&manifest.addon.id)?
            .ok_or_else(|| InstallError::NotInstalled(manifest.addon.id.clone()))?;
        let granted = self.db.get_permission_hash(&manifest.addon.id)?;
        if granted.as_deref() != Some(manifest.permission_hash().as_str())
            && decision != InstallDecision::Approve
        {
            return Err(InstallError::PermissionExpansionNeedsApproval);
        }
        self.materialize(pkg_path, &manifest, &hash, existing.status)
    }

    pub fn enable(&self, id: &str) -> Result<(), InstallError> {
        self.require_installed(id)?;
        Ok(self.db.set_addon_status(id, AddonStatus::Enabled)?)
    }

    pub fn disable(&self, id: &str) -> Result<(), InstallError> {
        self.require_installed(id)?;
        Ok(self.db.set_addon_status(id, AddonStatus::Disabled)?)
    }

    /// Remove the add-on; `purge` also deletes its settings, storage,
    /// favorites, and watch state (TC-ADD-007/008).
    pub fn uninstall(&self, id: &str, purge: bool) -> Result<(), InstallError> {
        self.require_installed(id)?;
        self.db.remove_addon(id, purge)?;
        let dir = self.install_root.join(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| InstallError::Io(e.to_string()))?;
        }
        Ok(())
    }

    /// Re-hash the stored package against the hash recorded at install time
    /// (TC-SEC-001).
    pub fn verify_installed(&self, id: &str) -> Result<bool, InstallError> {
        let rec = self.require_installed(id)?;
        let hash = hex::encode(sha256_file(&self.stored_package_path(id))?);
        Ok(hash == rec.package_hash)
    }

    fn require_installed(&self, id: &str) -> Result<AddonRecord, InstallError> {
        self.db
            .get_addon(id)?
            .ok_or_else(|| InstallError::NotInstalled(id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{keygen, package_dir, sign_package};
    use ed25519_dalek::SigningKey;

    fn manifest_toml(version: &str, sdk: &str, network: &[&str]) -> String {
        let net = network
            .iter()
            .map(|d| format!("\"{d}\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            r#"
[addon]
id = "com.example.demo"
name = "Demo"
version = "{version}"
sdk = "{sdk}"
entry = "main"
capabilities = ["catalog", "resolve"]

[permissions]
network = [{net}]
storage = "1MiB"
"#
        )
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        manager: AddonManager,
        db: Arc<Database>,
        signing: SigningKey,
        verifying: VerifyingKey,
        work: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let db = Arc::new(Database::open_in_memory().unwrap());
            let manager = AddonManager::new(Arc::clone(&db), tmp.path().join("addons"));
            let (signing, verifying) = keygen();
            let work = tmp.path().to_path_buf();
            Fixture { _tmp: tmp, manager, db, signing, verifying, work }
        }

        fn build_package(&self, name: &str, manifest: &str) -> (PathBuf, [u8; 64]) {
            let src = self.work.join(name);
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(src.join("manifest.toml"), manifest).unwrap();
            std::fs::write(src.join("main"), "entry").unwrap();
            let pkg = self.work.join(format!("{name}.rvah"));
            package_dir(&src, &pkg).unwrap();
            let sig = sign_package(&pkg, &self.signing).unwrap();
            (pkg, sig)
        }
    }

    #[test]
    fn install_flow_records_addon_and_extracts() {
        let f = Fixture::new();
        let (pkg, sig) = f.build_package("demo", &manifest_toml("1.0.0", ">=0.1, <0.2", &[]));
        let installed = f
            .manager
            .install(&pkg, &sig, &f.verifying, InstallDecision::Approve)
            .unwrap();
        assert_eq!(installed.id, "com.example.demo");
        assert!(installed.install_dir.join("manifest.toml").exists());
        let rec = f.db.get_addon("com.example.demo").unwrap().unwrap();
        assert_eq!(rec.status, AddonStatus::Enabled);
        assert!(f.manager.verify_installed("com.example.demo").unwrap());

        // Deny leaves nothing behind for a fresh id.
        let (pkg2, sig2) = f.build_package("demo2", &manifest_toml("1.0.0", ">=0.1, <0.2", &[])
            .replace("com.example.demo", "com.example.other"));
        assert!(matches!(
            f.manager.install(&pkg2, &sig2, &f.verifying, InstallDecision::Deny),
            Err(InstallError::PermissionsDenied)
        ));
        assert!(f.db.get_addon("com.example.other").unwrap().is_none());

        // Duplicate install is refused.
        assert!(matches!(
            f.manager.install(&pkg, &sig, &f.verifying, InstallDecision::Approve),
            Err(InstallError::AlreadyInstalled(_))
        ));
    }

    #[test]
    fn install_rejects_bad_sdk_range() {
        // TC-ADD-003: declared SDK range excludes this host.
        let f = Fixture::new();
        let (pkg, sig) = f.build_package("demo", &manifest_toml("1.0.0", ">=9.0", &[]));
        assert!(matches!(
            f.manager.install(&pkg, &sig, &f.verifying, InstallDecision::Approve),
            Err(InstallError::SdkIncompatible { .. })
        ));
    }

    #[test]
    fn update_with_expanded_permissions_requires_reapproval() {
        let f = Fixture::new();
        let (pkg, sig) = f.build_package("v1", &manifest_toml("1.0.0", ">=0.1, <0.2", &[]));
        f.manager
            .install(&pkg, &sig, &f.verifying, InstallDecision::Approve)
            .unwrap();
        f.db.set_setting("com.example.demo", "region", "eu").unwrap();

        // Same permissions: update sails through without re-approval.
        let (pkg2, sig2) = f.build_package("v2", &manifest_toml("1.0.1", ">=0.1, <0.2", &[]));
        f.manager
            .update(&pkg2, &sig2, &f.verifying, InstallDecision::Deny)
            .unwrap();

        // Expanded permissions: blocked unless approved (TC-ADD-005).
        let (pkg3, sig3) = f.build_package(
            "v3",
            &manifest_toml("1.0.2", ">=0.1, <0.2", &["api.example.com"]),
        );
        assert!(matches!(
            f.manager.update(&pkg3, &sig3, &f.verifying, InstallDecision::Deny),
            Err(InstallError::PermissionExpansionNeedsApproval)
        ));
        let updated = f
            .manager
            .update(&pkg3, &sig3, &f.verifying, InstallDecision::Approve)
            .unwrap();
        assert_eq!(updated.version, "1.0.2");

        // Settings survived the updates (TC-ADD-004).
        assert_eq!(
            f.db.get_setting("com.example.demo", "region").unwrap().as_deref(),
            Some("eu")
        );
    }

    #[test]
    fn uninstall_purges_or_retains() {
        // TC-ADD-007/008: purge flag controls user-data removal.
        let f = Fixture::new();
        let (pkg, sig) = f.build_package("demo", &manifest_toml("1.0.0", ">=0.1, <0.2", &[]));

        f.manager
            .install(&pkg, &sig, &f.verifying, InstallDecision::Approve)
            .unwrap();
        f.db.storage_set("com.example.demo", "k", b"v", 1024).unwrap();
        f.manager.uninstall("com.example.demo", false).unwrap();
        assert!(f.db.get_addon("com.example.demo").unwrap().is_none());
        assert!(!f.manager.install_dir("com.example.demo", "1.0.0").exists());
        assert_eq!(
            f.db.storage_get("com.example.demo", "k").unwrap().as_deref(),
            Some(b"v".as_slice())
        );

        f.manager
            .install(&pkg, &sig, &f.verifying, InstallDecision::Approve)
            .unwrap();
        f.manager.uninstall("com.example.demo", true).unwrap();
        assert!(f.db.storage_get("com.example.demo", "k").unwrap().is_none());
    }
}
