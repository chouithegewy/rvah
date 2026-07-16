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
        World {
            _tmp: tmp,
            handle: CoreService::spawn(core),
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
