//! Add-on repositories (FR-008, SDD §4 `rvah-repo`): signed index metadata,
//! version/update checks with channels, deprecation notices, and package
//! download + install through the existing signed-package pipeline.
//!
//! Trust model (MVP): a single ed25519 repository key signs the index *and*
//! every package it lists. The index signature is verified before the index
//! is parsed; package bytes are hash-checked against the index entry and
//! then re-verified by `AddonManager` at install time.

use std::path::Path;

use ed25519_dalek::VerifyingKey;
use rvah_addon_host::{AddonManager, InstallDecision, InstallError, InstalledAddon};
use rvah_addon_sdk::HttpRequest;
use rvah_data::Database;
use rvah_net::Transport;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("repository transport error: {0}")]
    Transport(String),
    #[error("repository index signature verification failed")]
    BadSignature,
    #[error("malformed repository data: {0}")]
    Malformed(String),
    #[error("package hash mismatch for {id}: expected {expected}")]
    HashMismatch { id: String, expected: String },
    #[error("add-on not in repository: {0}")]
    NotFound(String),
    #[error(transparent)]
    Install(#[from] InstallError),
    #[error("io error: {0}")]
    Io(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoEntry {
    pub id: String,
    pub version: semver::Version,
    /// Update channel, e.g. "stable" or "beta" (SDD Phase 4 update channels).
    #[serde(default = "default_channel")]
    pub channel: String,
    pub url: String,
    /// Hex sha256 of the package bytes.
    pub sha256: String,
    /// Hex ed25519 signature of the package (repo key, detached).
    pub sig: String,
    /// Human-readable deprecation notice, if the entry is deprecated (FR-008).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<String>,
}

fn default_channel() -> String {
    "stable".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoIndex {
    pub name: String,
    pub generated_at: u64,
    pub addons: Vec<RepoEntry>,
}

/// An available upgrade (or deprecation) for an installed add-on.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateInfo {
    pub id: String,
    pub installed: String,
    pub available: semver::Version,
    pub deprecated: Option<String>,
}

/// Verify `sig` (ed25519 over sha256 of `bytes`) — the same detached scheme
/// packages use.
fn verify_detached(bytes: &[u8], sig_hex: &str, key: &VerifyingKey) -> Result<(), RepoError> {
    use ed25519_dalek::{Signature, Verifier};
    let sig_bytes: [u8; 64] = hex::decode(sig_hex.trim())
        .map_err(|e| RepoError::Malformed(format!("signature hex: {e}")))?
        .try_into()
        .map_err(|_| RepoError::Malformed("signature must be 64 bytes".into()))?;
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    key.verify(&digest, &Signature::from_bytes(&sig_bytes))
        .map_err(|_| RepoError::BadSignature)
}

fn fetch(transport: &dyn Transport, url: &str) -> Result<Vec<u8>, RepoError> {
    let resp = transport
        .execute(&HttpRequest::get(url))
        .map_err(|e| RepoError::Transport(e.to_string()))?;
    if resp.status != 200 {
        return Err(RepoError::Transport(format!(
            "GET {url} returned {}",
            resp.status
        )));
    }
    Ok(resp.body)
}

/// `Transport` over the local filesystem: accepts `file://` URLs or plain
/// paths. Lets the whole repository flow run offline (dev repos, tests).
pub struct FileTransport;

impl Transport for FileTransport {
    fn execute(
        &self,
        req: &HttpRequest,
    ) -> Result<rvah_addon_sdk::HttpResponse, rvah_net::NetError> {
        let path = req.url.strip_prefix("file://").unwrap_or(&req.url);
        match std::fs::read(path) {
            Ok(body) => Ok(rvah_addon_sdk::HttpResponse {
                status: 200,
                truncated: false,
                headers: Default::default(),
                body,
            }),
            Err(e) => Err(rvah_net::NetError::Transport(format!("{path}: {e}"))),
        }
    }
}

/// Pick a transport for a repository URL: HTTP(S) goes over the network,
/// anything else is treated as a local path.
pub fn transport_for(url: &str) -> Box<dyn Transport> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Box::new(rvah_net::HttpTransport::default())
    } else {
        Box::new(FileTransport)
    }
}

/// A remote repository plus the key we trust it under.
pub struct Repository {
    pub index_url: String,
    pub pubkey: VerifyingKey,
}

impl Repository {
    pub fn new(index_url: impl Into<String>, pubkey: VerifyingKey) -> Self {
        Self {
            index_url: index_url.into(),
            pubkey,
        }
    }

    /// Fetch and signature-verify the index. Nothing is parsed before the
    /// signature checks out (SDD §9 "insecure update").
    pub fn fetch_index(&self, transport: &dyn Transport) -> Result<RepoIndex, RepoError> {
        let index_bytes = fetch(transport, &self.index_url)?;
        let sig_bytes = fetch(transport, &format!("{}.sig", self.index_url))?;
        let sig_hex = String::from_utf8(sig_bytes)
            .map_err(|e| RepoError::Malformed(format!("signature file: {e}")))?;
        verify_detached(&index_bytes, &sig_hex, &self.pubkey)?;
        serde_json::from_slice(&index_bytes).map_err(|e| RepoError::Malformed(e.to_string()))
    }

    /// Compare installed add-ons against the index, within one channel.
    /// Deprecated entries are reported even without a newer version (FR-008).
    pub fn check_updates(
        index: &RepoIndex,
        db: &Database,
        channel: &str,
    ) -> Result<Vec<UpdateInfo>, RepoError> {
        let installed = db
            .list_addons()
            .map_err(|e| RepoError::Malformed(e.to_string()))?;
        let mut updates = Vec::new();
        for addon in installed {
            let Some(entry) = index
                .addons
                .iter()
                .filter(|e| e.id == addon.id && e.channel == channel)
                .max_by(|a, b| a.version.cmp(&b.version))
            else {
                continue;
            };
            let newer = semver::Version::parse(&addon.version)
                .map(|v| entry.version > v)
                .unwrap_or(false);
            if newer || entry.deprecated.is_some() {
                updates.push(UpdateInfo {
                    id: addon.id,
                    installed: addon.version,
                    available: entry.version.clone(),
                    deprecated: entry.deprecated.clone(),
                });
            }
        }
        Ok(updates)
    }

    /// Download an entry's package, hash-check it against the index, and run
    /// it through the normal signed install/update pipeline (permission
    /// expansion still requires approval there).
    pub fn install_entry(
        &self,
        transport: &dyn Transport,
        entry: &RepoEntry,
        manager: &AddonManager,
        work_dir: &Path,
        decision: InstallDecision,
    ) -> Result<InstalledAddon, RepoError> {
        let bytes = fetch(transport, &entry.url)?;
        let digest = hex::encode(Sha256::digest(&bytes));
        if !digest.eq_ignore_ascii_case(entry.sha256.trim()) {
            return Err(RepoError::HashMismatch {
                id: entry.id.clone(),
                expected: entry.sha256.clone(),
            });
        }
        let pkg_path = work_dir.join(format!("{}-{}.rvah", entry.id, entry.version));
        std::fs::write(&pkg_path, &bytes).map_err(|e| RepoError::Io(e.to_string()))?;
        let sig: [u8; 64] = hex::decode(entry.sig.trim())
            .map_err(|e| RepoError::Malformed(format!("package sig hex: {e}")))?
            .try_into()
            .map_err(|_| RepoError::Malformed("package sig must be 64 bytes".into()))?;

        let already_installed = manager
            .db()
            .get_addon(&entry.id)
            .map_err(|e| RepoError::Malformed(e.to_string()))?
            .is_some();
        let installed = if already_installed {
            manager.update(&pkg_path, &sig, &self.pubkey, decision)?
        } else {
            manager.install(&pkg_path, &sig, &self.pubkey, decision)?
        };
        Ok(installed)
    }
}

/// Build a signed index for a set of packaged add-ons — the publisher side,
/// used by the CLI and tests.
pub fn build_index(
    name: &str,
    entries: Vec<RepoEntry>,
    signing: &ed25519_dalek::SigningKey,
) -> (Vec<u8>, String) {
    use ed25519_dalek::Signer;
    let index = RepoIndex {
        name: name.to_string(),
        generated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        addons: entries,
    };
    let bytes = serde_json::to_vec_pretty(&index).expect("index serializes");
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let sig = hex::encode(signing.sign(&digest).to_bytes());
    (bytes, sig)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvah_addon_host::{keygen, package_dir, sign_package};
    use rvah_net::MockTransport;
    use rvah_testkit::fixture_manifest;
    use std::sync::Arc;

    const REPO_URL: &str = "https://repo.example.com/index.json";

    struct RepoWorld {
        tmp: tempfile::TempDir,
        transport: MockTransport,
        repo: Repository,
        db: Arc<Database>,
        manager: AddonManager,
        entry: RepoEntry,
    }

    /// Build a one-add-on repository served over MockTransport.
    fn world(version: &str, deprecated: Option<&str>) -> RepoWorld {
        let tmp = tempfile::tempdir().unwrap();
        let (signing, pubkey) = keygen();

        // Author + package the add-on.
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let manifest = fixture_manifest("com.example.repo", &["catalog"], &[], &[])
            .replace("version = \"1.0.0\"", &format!("version = \"{version}\""));
        std::fs::write(src.join("manifest.toml"), manifest).unwrap();
        std::fs::write(src.join("main"), "entry").unwrap();
        let pkg = tmp.path().join("pkg.rvah");
        let hash = package_dir(&src, &pkg).unwrap();
        let sig = sign_package(&pkg, &signing).unwrap();
        let pkg_bytes = std::fs::read(&pkg).unwrap();

        let entry = RepoEntry {
            id: "com.example.repo".into(),
            version: semver::Version::parse(version).unwrap(),
            channel: "stable".into(),
            url: "https://repo.example.com/pkg.rvah".into(),
            sha256: hex::encode(hash),
            sig: hex::encode(sig),
            deprecated: deprecated.map(String::from),
        };
        let (index_bytes, index_sig) = build_index("test-repo", vec![entry.clone()], &signing);

        let transport = MockTransport::default();
        // Longest route first: MockTransport matches by URL prefix.
        transport.route(
            "GET",
            &format!("{REPO_URL}.sig"),
            200,
            index_sig.into_bytes(),
        );
        transport.route("GET", REPO_URL, 200, index_bytes);
        transport.route("GET", "https://repo.example.com/pkg.rvah", 200, pkg_bytes);

        let db = Arc::new(Database::open_in_memory().unwrap());
        let manager = AddonManager::new(Arc::clone(&db), tmp.path().join("addons"));
        RepoWorld {
            tmp,
            transport,
            repo: Repository::new(REPO_URL, pubkey),
            db,
            manager,
            entry,
        }
    }

    #[test]
    fn fetch_verifies_index_signature() {
        let w = world("1.0.0", None);
        let index = w.repo.fetch_index(&w.transport).unwrap();
        assert_eq!(index.name, "test-repo");
        assert_eq!(index.addons.len(), 1);
    }

    #[test]
    fn tampered_index_rejected() {
        let w = world("1.0.0", None);
        let mut index = w.repo.fetch_index(&w.transport).unwrap();
        // Re-serve a modified index with the old signature.
        index.addons[0].url = "https://evil.example.com/pkg.rvah".into();
        let tampered = MockTransport::default();
        let sig = fetch(&w.transport, &format!("{REPO_URL}.sig")).unwrap();
        tampered.route("GET", &format!("{REPO_URL}.sig"), 200, sig);
        tampered.route(
            "GET",
            REPO_URL,
            200,
            serde_json::to_vec_pretty(&index).unwrap(),
        );
        assert!(matches!(
            w.repo.fetch_index(&tampered),
            Err(RepoError::BadSignature)
        ));
    }

    #[test]
    fn install_from_repo_end_to_end() {
        let w = world("1.0.0", None);
        let index = w.repo.fetch_index(&w.transport).unwrap();
        let installed = w
            .repo
            .install_entry(
                &w.transport,
                &index.addons[0],
                &w.manager,
                w.tmp.path(),
                InstallDecision::Approve,
            )
            .unwrap();
        assert_eq!(installed.id, "com.example.repo");
        assert!(w.db.get_addon("com.example.repo").unwrap().is_some());
    }

    #[test]
    fn download_hash_mismatch_rejected() {
        let w = world("1.0.0", None);
        let mut entry = w.entry.clone();
        entry.sha256 = hex::encode([0u8; 32]);
        let err = w
            .repo
            .install_entry(
                &w.transport,
                &entry,
                &w.manager,
                w.tmp.path(),
                InstallDecision::Approve,
            )
            .unwrap_err();
        assert!(matches!(err, RepoError::HashMismatch { .. }));
        assert!(w.db.get_addon("com.example.repo").unwrap().is_none());
    }

    #[test]
    fn check_updates_finds_newer_in_channel_only() {
        // Install 1.0.0, then serve an index with 1.1.0 stable and 2.0.0 beta.
        let w = world("1.0.0", None);
        let index = w.repo.fetch_index(&w.transport).unwrap();
        w.repo
            .install_entry(
                &w.transport,
                &index.addons[0],
                &w.manager,
                w.tmp.path(),
                InstallDecision::Approve,
            )
            .unwrap();

        let mut stable = w.entry.clone();
        stable.version = semver::Version::parse("1.1.0").unwrap();
        let mut beta = w.entry.clone();
        beta.version = semver::Version::parse("2.0.0").unwrap();
        beta.channel = "beta".into();
        let index = RepoIndex {
            name: "test-repo".into(),
            generated_at: 0,
            addons: vec![stable, beta],
        };

        let updates = Repository::check_updates(&index, &w.db, "stable").unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].available.to_string(), "1.1.0");

        let beta_updates = Repository::check_updates(&index, &w.db, "beta").unwrap();
        assert_eq!(beta_updates[0].available.to_string(), "2.0.0");
    }

    #[test]
    fn deprecation_notice_surfaces_without_new_version() {
        let w = world("1.0.0", Some("unmaintained; switch to com.example.new"));
        let index = w.repo.fetch_index(&w.transport).unwrap();
        w.repo
            .install_entry(
                &w.transport,
                &index.addons[0],
                &w.manager,
                w.tmp.path(),
                InstallDecision::Approve,
            )
            .unwrap();
        let updates = Repository::check_updates(&index, &w.db, "stable").unwrap();
        assert_eq!(updates.len(), 1);
        assert!(updates[0]
            .deprecated
            .as_deref()
            .unwrap()
            .contains("unmaintained"));
    }
}
