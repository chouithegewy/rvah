//! Application wiring (SDD §4 `rvah-app`): data directory, database, add-on
//! manager, runtime registration, player backend selection, core thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rvah_addon_host::{AddonManager, Capability, InProcessRuntime};
use rvah_addon_sdk::AddonProvider;
use rvah_auth::{EncryptedFileVault, SecretVault};
use rvah_core::AppCore;
use rvah_data::Database;
use rvah_player::{MockPlayer, PlayerBackend};
use rvah_player_mpv::MpvBackend;
use rvah_testkit::{CatalogEchoProvider, NoopHost};
use rvah_ui::{CoreHandle, CoreService};

pub struct Bootstrapped {
    pub handle: CoreHandle,
    /// Set when the app fell back to the mock player.
    pub backend_note: Option<String>,
}

pub fn default_data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(dir).join("rvah");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/share/rvah");
    }
    PathBuf::from("rvah-data")
}

fn capability_from_str(s: &str) -> Option<Capability> {
    match s {
        "catalog" => Some(Capability::Catalog),
        "search" => Some(Capability::Search),
        "resolve" => Some(Capability::Resolve),
        "auth" => Some(Capability::Auth),
        "account_status" | "accountstatus" => Some(Capability::AccountStatus),
        _ => None,
    }
}

/// Pick mpv when available, otherwise fall back to the mock player so the UI
/// stays usable on machines without mpv. The IPC endpoint is a Unix socket
/// on unix and a named pipe on Windows (`MpvIpc::default_endpoint`).
fn select_backend(data_dir: &Path) -> (Box<dyn PlayerBackend + Send>, Option<String>) {
    if MpvBackend::find_mpv().is_some() {
        let endpoint = rvah_player_mpv::MpvIpc::default_endpoint(data_dir);
        if cfg!(unix) {
            let _ = std::fs::remove_file(&endpoint);
        }
        match MpvBackend::spawn(&endpoint) {
            Ok(backend) => return (Box::new(backend), None),
            Err(e) => {
                return (
                    Box::new(MockPlayer::new()),
                    Some(format!(
                        "mpv launch failed ({}); using mock player",
                        e.message
                    )),
                )
            }
        }
    }
    (
        Box::new(MockPlayer::new()),
        Some("mpv not found on PATH; using mock player".into()),
    )
}

/// The OS keychain when built with `--features keychain` and a Secret
/// Service daemon answers; the encrypted file vault otherwise.
fn select_vault(data_dir: &Path) -> Result<Arc<dyn SecretVault>, String> {
    #[cfg(feature = "keychain")]
    if let Ok(vault) = rvah_auth::KeychainVault::connect() {
        return Ok(Arc::new(vault));
    }
    Ok(Arc::new(
        EncryptedFileVault::open(data_dir.join("secrets.vault"), data_dir.join("vault.key"))
            .map_err(|e| e.to_string())?,
    ))
}

pub fn bootstrap(data_dir: &Path) -> Result<Bootstrapped, String> {
    std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let db = Arc::new(Database::open(&data_dir.join("rvah.sqlite")).map_err(|e| e.to_string())?);
    let manager = AddonManager::new(Arc::clone(&db), data_dir.join("addons"));
    let vault = select_vault(data_dir)?;

    let (backend, backend_note) = select_backend(data_dir);
    let mut core = AppCore::new(manager, Arc::clone(&db), vault, backend);

    // Phase 2 runtime story: installed add-ons run on the in-process
    // simulator provider, gated by their installed capabilities (FR-027).
    for rec in db.list_addons().map_err(|e| e.to_string())? {
        let caps: Vec<Capability> = rec
            .capabilities
            .iter()
            .filter_map(|c| capability_from_str(c))
            .collect();
        let provider: Arc<dyn AddonProvider + Send + Sync> = Arc::new(CatalogEchoProvider);
        core.register_runtime(
            rec.id,
            Box::new(InProcessRuntime::new(provider, caps)),
            Arc::new(NoopHost::default()),
        );
    }

    // GUI installs (UC-01) register the same simulator runtime immediately.
    let factory: rvah_ui::RuntimeFactory = Box::new(|_id, caps| {
        let caps = caps.iter().filter_map(|c| capability_from_str(c)).collect();
        (
            Box::new(InProcessRuntime::new(
                Arc::new(CatalogEchoProvider) as Arc<dyn AddonProvider + Send + Sync>,
                caps,
            )) as Box<dyn rvah_addon_host::AddonRuntime>,
            Arc::new(NoopHost::default()) as Arc<dyn rvah_addon_sdk::HostApi>,
        )
    });

    Ok(Bootstrapped {
        handle: CoreService::spawn_with(core, Some(factory)),
        backend_note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvah_ui::{CoreEvent, CoreRequest};
    use std::time::Duration;

    #[test]
    fn bootstrap_headless_serves_home() {
        let tmp = tempfile::tempdir().unwrap();
        let booted = bootstrap(tmp.path()).unwrap();
        // Without mpv the note must say we fell back to the mock player.
        assert!(booted.backend_note.is_some() || MpvBackend::find_mpv().is_some());

        booted.handle.requests.send(CoreRequest::LoadHome).unwrap();
        let event = booted
            .handle
            .events
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        let CoreEvent::Home { addons, .. } = event else {
            panic!("expected home event, got {event:?}");
        };
        assert!(addons.is_empty());
    }

    #[test]
    fn default_data_dir_is_stable() {
        let dir = default_data_dir();
        assert!(dir.to_string_lossy().contains("rvah"));
    }
}
