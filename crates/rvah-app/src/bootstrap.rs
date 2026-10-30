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

/// Where mpv's video goes.
pub enum PlayerLaunch {
    /// Into the app's native child window (`--wid`).
    Embedded(u64),
    /// mpv's own top-level window; `note` is shown in the toolbar.
    Standalone { note: Option<String> },
    /// No embeddable surface and no opt-in: mock player only.
    Disabled(String),
}

/// Decide playback from the outcome of creating the embedded video surface.
/// A separate mpv window is used only when embedding failed *and* the user
/// opted in with `--mpv-window` (Windows, macOS, Wayland-only sessions).
pub fn player_launch(embedded: Result<u64, String>, allow_mpv_window: bool) -> PlayerLaunch {
    match embedded {
        Ok(window_id) => PlayerLaunch::Embedded(window_id),
        Err(reason) if allow_mpv_window => PlayerLaunch::Standalone {
            note: Some(format!(
                "embedded video unavailable ({reason}); video opens in a separate mpv window"
            )),
        },
        Err(reason) => PlayerLaunch::Disabled(reason),
    }
}

pub fn wants_mpv_window(args: &[String]) -> bool {
    args.iter().any(|a| a == "--mpv-window")
}

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

/// Wall-clock budget for one add-on invocation when real network I/O can
/// happen inside it (matches the transport's own 30 s timeout).
#[cfg(feature = "wasm")]
const WASM_NETWORK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// HTTP request budget per add-on per app session (FR-081 quota).
#[cfg(feature = "wasm")]
const SESSION_REQUEST_QUOTA: u32 = 512;

/// A registrable execution pair: the runtime plus its host-services bridge.
type RuntimePair = (
    Box<dyn rvah_addon_host::AddonRuntime>,
    Arc<dyn rvah_addon_sdk::HostApi>,
);

/// The dev-simulator runtime: echo provider, no host services.
fn fixture_runtime(caps: &[String]) -> RuntimePair {
    let caps = caps.iter().filter_map(|c| capability_from_str(c)).collect();
    (
        Box::new(InProcessRuntime::new(
            Arc::new(CatalogEchoProvider) as Arc<dyn AddonProvider + Send + Sync>,
            caps,
        )),
        Arc::new(NoopHost::default()),
    )
}

/// Load an installed add-on's wasm module and pair it with the real
/// permission bridge (allowlist + DNS pinning + quotas + redacted logs).
/// `Ok(None)` means the manifest's entry is not a wasm module.
#[cfg(feature = "wasm")]
fn wasm_runtime_for(
    dir: &Path,
    db: &Arc<Database>,
    vault: &Arc<dyn SecretVault>,
) -> Result<Option<RuntimePair>, String> {
    use rvah_addon_host::{BridgedHostApi, CapabilityGated, Manifest, WasmLimits, WasmRuntime};

    let manifest_text =
        std::fs::read_to_string(dir.join("manifest.toml")).map_err(|e| e.to_string())?;
    let manifest = Manifest::parse(&manifest_text).map_err(|e| e.to_string())?;
    if !manifest.addon.entry.ends_with(".wasm") {
        return Ok(None);
    }
    let module = std::fs::read(dir.join(&manifest.addon.entry)).map_err(|e| e.to_string())?;
    let limits = WasmLimits {
        timeout: WASM_NETWORK_TIMEOUT,
        ..Default::default()
    };
    let runtime = WasmRuntime::load(&module, limits).map_err(|e| e.message)?;
    let gated = CapabilityGated::new(runtime, manifest.addon.capabilities.clone());
    let host = BridgedHostApi::new(
        &manifest,
        Arc::clone(db),
        Arc::clone(vault),
        Box::new(rvah_net::HttpTransport::default()),
        SESSION_REQUEST_QUOTA,
    )
    .with_resolver(Box::new(rvah_net::SystemResolver));
    Ok(Some((Box::new(gated), Arc::new(host))))
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
fn select_backend(
    data_dir: &Path,
    launch: PlayerLaunch,
) -> (Box<dyn PlayerBackend + Send>, Option<String>) {
    if let PlayerLaunch::Disabled(reason) = &launch {
        return (
            Box::new(MockPlayer::new()),
            Some(format!(
                "embedded video unavailable ({reason}); playback disabled \
                 (run with --mpv-window to use a separate mpv window)"
            )),
        );
    }
    if MpvBackend::find_mpv().is_some() {
        let endpoint = rvah_player_mpv::MpvIpc::default_endpoint(data_dir);
        if cfg!(unix) {
            let _ = std::fs::remove_file(&endpoint);
        }
        let (spawned, note) = match launch {
            PlayerLaunch::Embedded(window_id) => {
                (MpvBackend::spawn_embedded(&endpoint, window_id), None)
            }
            PlayerLaunch::Standalone { note } => (MpvBackend::spawn(&endpoint), note),
            PlayerLaunch::Disabled(_) => unreachable!(),
        };
        match spawned {
            Ok(backend) => return (Box::new(backend), note),
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

#[cfg(test)]
pub fn bootstrap(data_dir: &Path) -> Result<Bootstrapped, String> {
    bootstrap_with_player(data_dir, PlayerLaunch::Standalone { note: None })
}

pub fn bootstrap_with_player(
    data_dir: &Path,
    launch: PlayerLaunch,
) -> Result<Bootstrapped, String> {
    std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
    let db = Arc::new(Database::open(&data_dir.join("rvah.sqlite")).map_err(|e| e.to_string())?);
    let manager = AddonManager::new(Arc::clone(&db), data_dir.join("addons"));
    let vault = select_vault(data_dir)?;

    let (backend, backend_note) = select_backend(data_dir, launch);
    let mut core = AppCore::new(manager, Arc::clone(&db), Arc::clone(&vault), backend);

    // Installed add-ons with a wasm entry run in the sandbox against the
    // real permission bridge; anything else keeps the in-process simulator
    // provider, gated by its installed capabilities (FR-027).
    #[cfg_attr(not(feature = "wasm"), allow(unused_mut))]
    let mut any_wasm = false;
    for rec in db.list_addons().map_err(|e| e.to_string())? {
        #[cfg(feature = "wasm")]
        {
            let dir = core.manager.install_dir(&rec.id, &rec.version);
            match wasm_runtime_for(&dir, &db, &vault) {
                Ok(Some((runtime, host))) => {
                    core.register_runtime(rec.id.clone(), runtime, host);
                    any_wasm = true;
                    continue;
                }
                Ok(None) => {}
                Err(note) => eprintln!(
                    "rvah: {}: wasm load failed ({note}); using fixture provider",
                    rec.id
                ),
            }
        }
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
    #[cfg(feature = "wasm")]
    if any_wasm {
        // Fixture providers answer instantly; real add-ons talk to the
        // network inside an invocation.
        core.invoke_timeout = WASM_NETWORK_TIMEOUT;
    }
    #[cfg(not(feature = "wasm"))]
    let _ = any_wasm;

    // GUI installs (UC-01) register their runtime immediately: the wasm
    // sandbox when the package ships a module, the simulator otherwise.
    #[cfg(feature = "wasm")]
    let factory: rvah_ui::RuntimeFactory = {
        let db = Arc::clone(&db);
        let vault = Arc::clone(&vault);
        let addons_root = data_dir.join("addons");
        Box::new(move |id, caps| {
            if let Ok(Some(rec)) = db.get_addon(id) {
                let dir = addons_root.join(id).join(&rec.version);
                if let Ok(Some(pair)) = wasm_runtime_for(&dir, &db, &vault) {
                    return pair;
                }
            }
            fixture_runtime(caps)
        })
    };
    #[cfg(not(feature = "wasm"))]
    let factory: rvah_ui::RuntimeFactory = Box::new(|_id, caps| fixture_runtime(caps));

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
    fn separate_mpv_window_only_when_embedding_fails_and_opted_in() {
        assert!(matches!(
            player_launch(Ok(7), false),
            PlayerLaunch::Embedded(7)
        ));
        assert!(matches!(
            player_launch(Ok(7), true),
            PlayerLaunch::Embedded(7)
        ));
        assert!(matches!(
            player_launch(Err("no X11".into()), false),
            PlayerLaunch::Disabled(reason) if reason == "no X11"
        ));
        match player_launch(Err("no X11".into()), true) {
            PlayerLaunch::Standalone { note: Some(note) } => {
                assert!(note.contains("no X11"), "{note}");
                assert!(note.contains("separate mpv window"), "{note}");
            }
            _ => panic!("opt-in must fall back to a separate mpv window"),
        }
    }

    #[test]
    fn mpv_window_flag_is_parsed() {
        assert!(wants_mpv_window(&["--mpv-window".to_string()]));
        assert!(!wants_mpv_window(&["--data-dir".into(), "/x".into()]));
    }

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

    /// A packaged wasm add-on browses through the sandbox, not the fixture
    /// provider (the WAT guest returns a directory the echo provider never
    /// would).
    #[cfg(feature = "wasm")]
    #[test]
    fn bootstrap_runs_installed_wasm_addon() {
        use rvah_addon_host::{package_dir, sign_package, AddonManager, InstallDecision};

        const MANIFEST: &str = r#"
[addon]
id = "com.example.wasmdemo"
name = "Wasm Demo"
version = "1.0.0"
sdk = ">=0.1, <0.2"
entry = "module.wasm"
capabilities = ["catalog"]

[permissions]
network = []
storage = "1KiB"
"#;
        // Same shape as the wasm_runtime echo fixture (wasmtime accepts WAT).
        let page_json = r#"{"Ok":{"kind":"page","value":{"items":[{"kind":"directory","id":"w1","title":"From Wasm","path":"/w"}]}}}"#;
        let module = format!(
            r#"(module
  (memory (export "memory") 2)
  (data (i32.const 1024) "{data}")
  (global $heap (mut i32) (i32.const 65536))
  (func (export "rvah_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    global.get $heap
    local.set $ptr
    global.get $heap
    local.get $len
    i32.add
    global.set $heap
    local.get $ptr)
  (func (export "rvah_invoke") (param i32 i32) (result i64)
    i64.const {packed}))"#,
            data = page_json.replace('\\', "\\\\").replace('"', "\\\""),
            packed = (1024u64 << 32) | page_json.len() as u64,
        );

        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("manifest.toml"), MANIFEST).unwrap();
        std::fs::write(src.join("module.wasm"), module).unwrap();
        let pkg = tmp.path().join("demo.rvah");
        package_dir(&src, &pkg).unwrap();
        let (signing, verifying) = rvah_addon_host::keygen();
        let sig = sign_package(&pkg, &signing).unwrap();

        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        {
            let db = Arc::new(Database::open(&data_dir.join("rvah.sqlite")).unwrap());
            let manager = AddonManager::new(db, data_dir.join("addons"));
            manager
                .install(&pkg, &sig, &verifying, InstallDecision::Approve)
                .unwrap();
        }

        let booted = bootstrap(&data_dir).unwrap();
        booted
            .handle
            .requests
            .send(CoreRequest::Browse {
                addon_id: "com.example.wasmdemo".into(),
                path: None,
                cursor: None,
            })
            .unwrap();
        let event = booted
            .handle
            .events
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let CoreEvent::Page { page, .. } = event else {
            panic!("expected page event, got {event:?}");
        };
        let rvah_addon_sdk::PageItem::Directory(dir) = &page.items[0] else {
            panic!("expected directory item");
        };
        assert_eq!(dir.title, "From Wasm", "page must come from the sandbox");
    }
}
