//! End-to-end UI-service flows over a real core (fixture add-on, MockPlayer):
//! intents → CoreService thread → events → reducers.

use std::sync::Arc;
use std::time::Duration;

use rvah_addon_host::{AddonManager, Capability, InProcessRuntime, InstallDecision};
use rvah_auth::{MemoryVault, SecretVault};
use rvah_core::AppCore;
use rvah_data::Database;
use rvah_testkit::{
    write_fixture_package, CatalogEchoProvider, MockPlayer, NoopHost, ECHO_FIXTURE_ID,
};
use rvah_ui::{intent, reduce, CoreHandle, CoreService, Load, Screen, UiIntent, UiState};

struct World {
    _tmp: tempfile::TempDir,
    handle: CoreHandle,
    state: UiState,
}

impl World {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let manager = AddonManager::new(Arc::clone(&db), tmp.path().join("addons"));
        let (pkg, sig, pubkey) = write_fixture_package(tmp.path());
        manager
            .install(&pkg, &sig, &pubkey, InstallDecision::Approve)
            .unwrap();
        let vault: Arc<dyn SecretVault> = Arc::new(MemoryVault::new());
        let mut core = AppCore::new(manager, db, vault, MockPlayer::new());
        core.register_runtime(
            ECHO_FIXTURE_ID,
            Box::new(InProcessRuntime::new(
                Arc::new(CatalogEchoProvider),
                vec![Capability::Catalog, Capability::Search, Capability::Resolve],
            )),
            Arc::new(NoopHost::default()),
        );
        // Same factory shape rvah-app uses: fresh installs get the simulator
        // runtime immediately.
        let factory: rvah_ui::RuntimeFactory = Box::new(|_id, caps| {
            let caps = caps
                .iter()
                .filter_map(|c| match c.as_str() {
                    "catalog" => Some(Capability::Catalog),
                    "search" => Some(Capability::Search),
                    "resolve" => Some(Capability::Resolve),
                    _ => None,
                })
                .collect();
            (
                Box::new(InProcessRuntime::new(Arc::new(CatalogEchoProvider), caps))
                    as Box<dyn rvah_addon_host::AddonRuntime>,
                Arc::new(NoopHost::default()) as Arc<dyn rvah_addon_sdk::HostApi>,
            )
        });
        World {
            _tmp: tmp,
            handle: CoreService::spawn_with(core, Some(factory)),
            state: UiState::default(),
        }
    }

    fn send(&mut self, ui_intent: UiIntent) {
        if let Some(req) = intent(&mut self.state, ui_intent) {
            self.handle.requests.send(req).unwrap();
        }
    }

    /// Apply every event the core produces within the timeout.
    fn pump(&mut self) {
        while let Ok(event) = self.handle.events.recv_timeout(Duration::from_secs(2)) {
            reduce(&mut self.state, event);
            // Drain whatever arrived alongside it without waiting again.
            while let Ok(more) = self.handle.events.try_recv() {
                reduce(&mut self.state, more);
            }
            return;
        }
        panic!("no core event within timeout");
    }
}

#[test]
fn home_lists_installed_addon() {
    let mut w = World::new();
    w.send(UiIntent::OpenHome);
    w.pump();
    let Load::Loaded(home) = &w.state.home else {
        panic!("home not loaded: {:?}", w.state.home);
    };
    assert_eq!(home.addons.len(), 1);
    assert_eq!(home.addons[0].id, ECHO_FIXTURE_ID);
}

#[test]
fn browse_flow_loads_pages_and_history_works() {
    let mut w = World::new();
    w.send(UiIntent::OpenBrowse {
        addon_id: ECHO_FIXTURE_ID.into(),
        path: None,
    });
    w.pump();
    let Load::Loaded(page) = &w.state.browse else {
        panic!("browse not loaded");
    };
    assert_eq!(page.items.len(), 2);

    w.send(UiIntent::OpenBrowse {
        addon_id: ECHO_FIXTURE_ID.into(),
        path: Some("/movies".into()),
    });
    w.pump();
    w.send(UiIntent::NavBack);
    w.pump();
    assert_eq!(w.state.browse_loc.as_ref().unwrap().path, None);
    assert!(matches!(w.state.browse, Load::Loaded(_)));
}

#[test]
fn search_all_returns_normalized_results() {
    let mut w = World::new();
    w.send(UiIntent::OpenSearch { addon_id: None });
    w.send(UiIntent::SetSearchQuery("naruto".into()));
    w.send(UiIntent::SubmitSearch);
    w.pump();
    let Load::Loaded(results) = &w.state.search else {
        panic!("search not loaded");
    };
    assert_eq!(results.len(), 1);
    assert!(results[0].1.as_ref().unwrap().items.len() == 1);
}

#[test]
fn play_starts_session_and_playback_updates() {
    let mut w = World::new();
    w.send(UiIntent::PlayItem {
        addon_id: ECHO_FIXTURE_ID.into(),
        item_id: "m1".into(),
    });
    w.pump();
    assert_eq!(w.state.screen, Screen::Player);
    assert!(w
        .state
        .player
        .now_playing
        .as_deref()
        .unwrap()
        .contains("m1.mp4"));
    // Loading state reported via playback snapshot.
    assert!(w.state.player.state.is_some());
}

#[test]
fn permissions_revoke_and_regrant_roundtrip() {
    let mut w = World::new();
    w.send(UiIntent::OpenPermissions {
        addon_id: ECHO_FIXTURE_ID.into(),
    });
    w.pump();
    assert!(matches!(w.state.permissions, Load::Loaded(Some(_))));

    w.send(UiIntent::Revoke {
        addon_id: ECHO_FIXTURE_ID.into(),
    });
    w.pump();
    assert!(matches!(w.state.permissions, Load::Loaded(None)));

    w.send(UiIntent::Grant {
        addon_id: ECHO_FIXTURE_ID.into(),
    });
    w.pump();
    assert!(matches!(w.state.permissions, Load::Loaded(Some(_))));
}

#[test]
fn settings_load_from_manifest_schema() {
    let mut w = World::new();
    w.send(UiIntent::OpenSettings {
        addon_id: ECHO_FIXTURE_ID.into(),
    });
    w.pump();
    // Echo fixture manifest declares no settings — loaded but empty.
    assert_eq!(w.state.settings, Load::Loaded(vec![]));
}

#[test]
fn diagnostics_export_writes_redacted_bundle() {
    let mut w = World::new();
    let path = w._tmp.path().join("bundle.json");
    w.send(UiIntent::OpenDiagnostics);
    w.pump();
    w.send(UiIntent::ExportDiagnostics { path: path.clone() });
    w.pump();
    assert!(w
        .state
        .export_status
        .as_deref()
        .unwrap()
        .contains("exported"));
    let bundle: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(bundle["addons"][0]["id"], ECHO_FIXTURE_ID);
    assert!(bundle["events"].is_array());
}

/// Package a fixture add-on and write hex pubkey + sig files, GUI-style.
fn packaged_addon(dir: &std::path::Path, id: &str) -> (String, String) {
    let (pkg, sig, pubkey) = rvah_testkit::write_named_fixture_package(dir, id, &["catalog"]);
    let sig_path = format!("{}.sig", pkg.display());
    std::fs::write(&sig_path, hex::encode(sig)).unwrap();
    let pub_path = dir.join(format!("{id}.pub"));
    std::fs::write(&pub_path, hex::encode(pubkey.to_bytes())).unwrap();
    (
        pkg.to_str().unwrap().to_string(),
        pub_path.to_str().unwrap().to_string(),
    )
}

#[test]
fn gui_install_preview_approve_flow() {
    let mut w = World::new();
    let (pkg, pubkey) = packaged_addon(w._tmp.path(), "com.example.guiinstall");

    w.send(UiIntent::OpenInstall);
    w.send(UiIntent::SetInstallPkg(pkg));
    w.send(UiIntent::SetInstallPubkey(pubkey));
    w.send(UiIntent::PreviewInstall);
    w.pump();
    let Load::Loaded(preview) = &w.state.install else {
        panic!("preview not loaded: {:?}", w.state.install);
    };
    assert_eq!(preview.id, "com.example.guiinstall");
    assert_eq!(preview.storage_bytes, 1024 * 1024);

    w.send(UiIntent::ConfirmInstall);
    // Confirm emits Installed then a refreshed Home; pump until both landed.
    w.pump();
    if !matches!(w.state.home, Load::Loaded(_)) {
        w.pump();
    }
    assert_eq!(w.state.screen, Screen::Home);
    assert!(w
        .state
        .install_done
        .as_deref()
        .unwrap()
        .contains("guiinstall"));
    let Load::Loaded(home) = &w.state.home else {
        panic!("home not refreshed");
    };
    assert!(home.addons.iter().any(|a| a.id == "com.example.guiinstall"));

    // The factory registered a runtime: the new add-on browses immediately.
    w.send(UiIntent::OpenBrowse {
        addon_id: "com.example.guiinstall".into(),
        path: None,
    });
    w.pump();
    assert!(matches!(w.state.browse, Load::Loaded(_)));
}

#[test]
fn gui_install_rejects_tampered_package() {
    let mut w = World::new();
    let (pkg, pubkey) = packaged_addon(w._tmp.path(), "com.example.tamperedgui");
    let mut bytes = std::fs::read(&pkg).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    std::fs::write(&pkg, bytes).unwrap();

    w.send(UiIntent::OpenInstall);
    w.send(UiIntent::SetInstallPkg(pkg));
    w.send(UiIntent::SetInstallPubkey(pubkey));
    w.send(UiIntent::PreviewInstall);
    w.pump();
    let Load::Failed(error) = &w.state.install else {
        panic!("expected failed install, got {:?}", w.state.install);
    };
    assert!(error.message.contains("signature"));
    // Approve without a verified preview is refused client-side too.
    assert_eq!(
        rvah_ui::intent(&mut w.state, UiIntent::ConfirmInstall),
        None
    );
}

#[test]
fn auth_flow_reaches_authorized_via_polling() {
    let mut w = World::new();
    w.send(UiIntent::OpenAuth {
        addon_id: ECHO_FIXTURE_ID.into(),
    });
    w.pump();
    let Load::Loaded(auth) = &w.state.auth else {
        panic!("auth not loaded");
    };
    assert_eq!(auth.status, "pending");
    assert_eq!(auth.user_code.as_deref(), Some("RVAH-1234"));

    for _ in 0..3 {
        w.send(UiIntent::PollAuth);
        w.pump();
    }
    let Load::Loaded(auth) = &w.state.auth else {
        panic!("auth not loaded");
    };
    assert_eq!(auth.status, "authorized");
}
