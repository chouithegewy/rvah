//! End-to-end flows over real install + runtime + player wiring (STP §6):
//! browse, pagination, quarantine, playback with resume, search, favorites.

use std::sync::Arc;

use rvah_addon_host::{AddonManager, Capability, InProcessRuntime, InstallDecision};
use rvah_addon_sdk::{AddonErrorCode, AddonProvider, PageItem};
use rvah_auth::MemoryVault;
use rvah_core::{AppCore, CoreError, ErrorClass};
use rvah_data::{AddonStatus, Database};
use rvah_player::{PlayerEvent, PlayerState};
use rvah_testkit::{
    write_fixture_package, CatalogEchoProvider, FailingProvider, LargeCatalogProvider, MockPlayer,
    NoopHost, ResolverStaticProvider, ECHO_FIXTURE_ID,
};

struct World {
    _tmp: tempfile::TempDir,
    core: AppCore<MockPlayer>,
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

        let mut core = AppCore::new(manager, db, Arc::new(MemoryVault::new()), MockPlayer::new());
        core.register_runtime(
            ECHO_FIXTURE_ID,
            Box::new(InProcessRuntime::new(
                Arc::new(CatalogEchoProvider),
                vec![Capability::Catalog, Capability::Search, Capability::Resolve],
            )),
            Arc::new(NoopHost::default()),
        );
        World { _tmp: tmp, core }
    }

    /// Install another fixture add-on and register the given provider for it.
    fn add_addon(
        &mut self,
        id: &str,
        caps: &[&str],
        provider: Arc<dyn AddonProvider + Send + Sync>,
        runtime_caps: Vec<Capability>,
    ) {
        let (pkg, sig, pubkey) =
            rvah_testkit::write_named_fixture_package(self._tmp.path(), id, caps);
        self.core
            .manager
            .install(&pkg, &sig, &pubkey, InstallDecision::Approve)
            .unwrap();
        self.core.register_runtime(
            id,
            Box::new(InProcessRuntime::new(provider, runtime_caps)),
            Arc::new(NoopHost::default()),
        );
    }
}

#[test]
fn browse_catalog_echo_root_and_nested() {
    let mut w = World::new();
    let root = w.core.browse(ECHO_FIXTURE_ID, None, None).unwrap();
    assert_eq!(root.items.len(), 2);
    let PageItem::Directory(movies) = &root.items[0] else {
        panic!("expected directory");
    };
    assert_eq!(movies.path, "/movies");

    let nested = w
        .core
        .browse(ECHO_FIXTURE_ID, Some("/movies".into()), None)
        .unwrap();
    assert_eq!(nested.items.len(), 2);
    assert!(matches!(nested.items[0], PageItem::Media(_)));
}

#[test]
fn pagination_over_large_catalog() {
    let mut w = World::new();
    w.add_addon(
        "com.rvah.fixtures.large",
        &["catalog"],
        Arc::new(LargeCatalogProvider {
            total: 25,
            page_size: 10,
        }),
        vec![Capability::Catalog],
    );

    let mut all = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = w
            .core
            .browse("com.rvah.fixtures.large", None, cursor.clone())
            .unwrap();
        assert_eq!(page.total_hint, Some(25));
        all.extend(page.items);
        pages += 1;
        match page.cursor_next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(all.len(), 25);
}

#[test]
fn malformed_provider_response_surfaces_error_host_survives() {
    // TC-RUN-003: invalid provider output becomes a Developer-class error and
    // the host keeps serving other add-ons.
    let mut w = World::new();
    w.add_addon(
        "com.rvah.fixtures.bad",
        &["catalog"],
        Arc::new(FailingProvider {
            code: AddonErrorCode::Malformed,
        }),
        vec![Capability::Catalog],
    );

    let err = w
        .core
        .browse("com.rvah.fixtures.bad", None, None)
        .unwrap_err();
    assert!(matches!(&err, CoreError::Provider(e) if e.code == AddonErrorCode::Malformed));
    assert_eq!(err.class(), ErrorClass::Developer);

    // Host survives: the echo add-on still works.
    assert!(w.core.browse(ECHO_FIXTURE_ID, None, None).is_ok());
}

#[test]
fn repeated_failures_quarantine_addon() {
    // TC-NFR-005: three consecutive failures quarantine the offender.
    let mut w = World::new();
    let id = "com.rvah.fixtures.flaky";
    w.add_addon(
        id,
        &["catalog"],
        Arc::new(FailingProvider {
            code: AddonErrorCode::Internal,
        }),
        vec![Capability::Catalog],
    );

    for _ in 0..3 {
        assert!(matches!(
            w.core.browse(id, None, None),
            Err(CoreError::Provider(_))
        ));
    }
    let rec = w.core.db().get_addon(id).unwrap().unwrap();
    assert_eq!(rec.status, AddonStatus::Quarantined);

    // Further calls are refused up front.
    assert!(matches!(
        w.core.browse(id, None, None),
        Err(CoreError::AddonDisabled(_))
    ));
    // And the event trail says why.
    let events = w.core.db().recent_events(10).unwrap();
    assert!(events.iter().any(|e| e.code == "quarantined"));
}

#[test]
fn resolve_direct_mp4_and_full_playback_with_resume() {
    // TC-PLY-001 + TC-PLY-008.
    let mut w = World::new();

    let session = w.core.resolve_and_play(ECHO_FIXTURE_ID, "m1").unwrap();
    assert_eq!(session.source.url, "https://cdn.example.com/m1.mp4");
    assert_eq!(w.core.player.backend_mut().last_start_at, Some(0.0));

    w.core
        .player
        .backend_mut()
        .script_events([PlayerEvent::Loaded {
            duration_secs: 100.0,
        }]);
    w.core.tick_playback().unwrap();
    assert_eq!(w.core.player.state(), PlayerState::Paused);
    w.core.player.play().unwrap();

    w.core
        .player
        .backend_mut()
        .script_events([PlayerEvent::Progress {
            position_secs: 50.0,
        }]);
    w.core.tick_playback().unwrap();
    let ws = w
        .core
        .db()
        .get_watch_state(ECHO_FIXTURE_ID, "m1")
        .unwrap()
        .unwrap();
    assert_eq!(ws.position_secs, 50.0);
    assert!(!ws.watched);

    w.core.stop_playback().unwrap();

    // Second session resumes from the stored position (TC-PLY-008).
    w.core.resolve_and_play(ECHO_FIXTURE_ID, "m1").unwrap();
    assert_eq!(w.core.player.backend_mut().last_start_at, Some(50.0));

    w.core.player.backend_mut().script_events([
        PlayerEvent::Loaded {
            duration_secs: 100.0,
        },
        PlayerEvent::Progress {
            position_secs: 95.0,
        },
        PlayerEvent::Ended,
    ]);
    w.core.tick_playback().unwrap();
    let ws = w
        .core
        .db()
        .get_watch_state(ECHO_FIXTURE_ID, "m1")
        .unwrap()
        .unwrap();
    assert!(ws.watched);
    assert_eq!(w.core.session(), None);

    // Watched items start from the beginning next time.
    w.core.resolve_and_play(ECHO_FIXTURE_ID, "m1").unwrap();
    assert_eq!(w.core.player.backend_mut().last_start_at, Some(0.0));
}

#[test]
fn expired_source_rejected_with_retryable_error() {
    // TC-PLY-005: stale URLs never reach the player; the error invites retry.
    let mut w = World::new();
    let mut expired = rvah_testkit::mp4_source("https://cdn.example.com/old.mp4");
    expired.expires_at = Some(1); // long past
    w.add_addon(
        "com.rvah.fixtures.resolver",
        &["resolve"],
        Arc::new(ResolverStaticProvider {
            sources: vec![expired],
        }),
        vec![Capability::Resolve],
    );

    let err = w
        .core
        .resolve_and_play("com.rvah.fixtures.resolver", "anything")
        .unwrap_err();
    assert!(matches!(
        err,
        CoreError::SourceRejected(rvah_player::SourceRejection::Expired)
    ));
    assert!(err.retryable());
    assert_eq!(err.class(), ErrorClass::Recoverable);
    assert_eq!(w.core.player.state(), PlayerState::Idle);
}

#[test]
fn search_single_addon() {
    let mut w = World::new();
    let page = w.core.search(ECHO_FIXTURE_ID, "naruto").unwrap();
    assert_eq!(page.items.len(), 1);
    let PageItem::Media(m) = &page.items[0] else {
        panic!("expected media");
    };
    assert_eq!(m.title, "result: naruto");

    let all = w.core.search_all("naruto");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].0, ECHO_FIXTURE_ID);
    assert!(all[0].1.is_ok());
}

#[test]
fn nav_history_back_forward() {
    // FR-042: browsing records history that back/forward walk.
    let mut w = World::new();
    w.core.browse(ECHO_FIXTURE_ID, None, None).unwrap();
    w.core
        .browse(ECHO_FIXTURE_ID, Some("/movies".into()), None)
        .unwrap();
    assert_eq!(w.core.nav.current(), Some("com.rvah.fixtures.echo:/movies"));
    assert_eq!(w.core.nav.back(), Some("com.rvah.fixtures.echo:/"));
    assert_eq!(w.core.nav.forward(), Some("com.rvah.fixtures.echo:/movies"));
}

#[test]
fn favorites_roundtrip() {
    let mut w = World::new();
    w.core
        .favorites_add(ECHO_FIXTURE_ID, "m1", "First Movie")
        .unwrap();
    let favs = w.core.favorites_list().unwrap();
    assert_eq!(favs.len(), 1);
    assert_eq!(favs[0].title, "First Movie");
    w.core.favorites_remove(ECHO_FIXTURE_ID, "m1").unwrap();
    assert!(w.core.favorites_list().unwrap().is_empty());
}
