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
        Self::with_player(MockPlayer::new())
    }

    fn with_player(player: MockPlayer) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let manager = AddonManager::new(Arc::clone(&db), tmp.path().join("addons"));

        let (pkg, sig, pubkey) = write_fixture_package(tmp.path());
        manager
            .install(&pkg, &sig, &pubkey, InstallDecision::Approve)
            .unwrap();

        let mut core = AppCore::new(manager, db, Arc::new(MemoryVault::new()), player);
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

    w.core.stop_desktop_playback().unwrap();

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
fn dash_source_gated_by_backend_capability() {
    let mut dash = rvah_testkit::mp4_source("https://cdn.example.com/show.mpd");
    dash.protocol = rvah_addon_sdk::Protocol::Dash;

    // Default backend has no DASH support: rejected as unsupported.
    let mut w = World::new();
    w.add_addon(
        "com.rvah.fixtures.dash",
        &["resolve"],
        Arc::new(ResolverStaticProvider {
            sources: vec![dash.clone()],
        }),
        vec![Capability::Resolve],
    );
    let err = w
        .core
        .resolve_and_play("com.rvah.fixtures.dash", "x")
        .unwrap_err();
    assert!(matches!(
        err,
        CoreError::SourceRejected(rvah_player::SourceRejection::UnsupportedProtocol(_))
    ));

    // DASH-capable backend: the same source plays.
    let mut w = World::with_player(MockPlayer::with_dash());
    w.add_addon(
        "com.rvah.fixtures.dash",
        &["resolve"],
        Arc::new(ResolverStaticProvider {
            sources: vec![dash],
        }),
        vec![Capability::Resolve],
    );
    let session = w
        .core
        .resolve_and_play("com.rvah.fixtures.dash", "x")
        .unwrap();
    assert!(session.source.url.ends_with(".mpd"));
}

#[test]
fn prepare_playback_is_platform_neutral_and_stale_events_are_ignored() {
    let mut w = World::new();
    let first = w
        .core
        .prepare_playback(
            ECHO_FIXTURE_ID,
            "m1",
            rvah_core::PlaybackCapabilities::media3(),
        )
        .unwrap();
    assert_eq!(first.resume_position_secs, 0.0);
    assert_eq!(first.display.title, "m1");
    assert!(
        w.core.player.backend_mut().commands.is_empty(),
        "native preparation must not touch a desktop backend"
    );

    let second = w
        .core
        .prepare_playback(
            ECHO_FIXTURE_ID,
            "m1",
            rvah_core::PlaybackCapabilities::media3(),
        )
        .unwrap();
    assert_ne!(first.session_id, second.session_id);
    assert!(!w
        .core
        .report_playback_event(
            first.session_id,
            rvah_core::PlaybackLifecycleEvent::Ended {
                position_secs: 100.0,
                duration_secs: Some(100.0),
            },
        )
        .unwrap());
    assert_eq!(w.core.session().unwrap().id, second.session_id);
    assert!(w
        .core
        .report_playback_event(
            second.session_id,
            rvah_core::PlaybackLifecycleEvent::Playing {
                position_secs: 10.0,
                duration_secs: Some(100.0),
            },
        )
        .unwrap());
}

#[test]
fn protected_sources_fail_closed_before_reaching_player() {
    let mut protected = rvah_testkit::mp4_source("https://cdn.example.com/protected.mpd");
    protected.protocol = rvah_addon_sdk::Protocol::Dash;
    protected.drm = Some(rvah_addon_sdk::DrmScheme::Widevine);
    let mut w = World::with_player(MockPlayer::with_dash());
    w.add_addon(
        "com.rvah.fixtures.drm",
        &["resolve"],
        Arc::new(ResolverStaticProvider {
            sources: vec![protected],
        }),
        vec![Capability::Resolve],
    );
    let error = w
        .core
        .prepare_playback(
            "com.rvah.fixtures.drm",
            "x",
            rvah_core::PlaybackCapabilities::media3(),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        CoreError::SourceRejected(rvah_player::SourceRejection::UnsupportedDrm(_))
    ));
    assert!(w.core.player.backend_mut().commands.is_empty());
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

#[test]
fn failed_load_does_not_leave_a_session_behind() {
    let mut w = World::new();
    w.core.player.backend_mut().fail_next =
        Some(rvah_player::PlayerError::internal("load refused"));
    assert!(w.core.resolve_and_play(ECHO_FIXTURE_ID, "m1").is_err());
    assert_eq!(w.core.session(), None);
}

#[test]
fn loaded_after_play_reports_buffering_not_paused() {
    // Play during Loading unpauses the backend; the load completing must not
    // be reported to native-player observers as a pause.
    let mut w = World::new();
    w.core.resolve_and_play(ECHO_FIXTURE_ID, "m1").unwrap();
    w.core.player.play().unwrap();
    w.core
        .player
        .backend_mut()
        .script_events([PlayerEvent::Loaded {
            duration_secs: 100.0,
        }]);
    w.core.tick_playback().unwrap();
    assert_eq!(
        w.core.session().unwrap().lifecycle,
        rvah_core::PlaybackLifecycleEvent::Buffering
    );
}

/// Blocks inside `list` until released, standing in for a slow network call.
struct BlockingProvider {
    entered: std::sync::Mutex<std::sync::mpsc::Sender<()>>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl AddonProvider for BlockingProvider {
    fn list(
        &self,
        _ctx: &dyn rvah_addon_sdk::HostApi,
        _path: Option<&str>,
        _cursor: Option<&str>,
    ) -> Result<rvah_addon_sdk::Page, rvah_addon_sdk::AddonError> {
        let _ = self.entered.lock().unwrap().send(());
        let _ = self.release.lock().unwrap().recv();
        Ok(rvah_addon_sdk::Page::default())
    }
}

#[test]
fn playback_events_do_not_wait_for_an_addon_invocation() {
    use std::sync::{mpsc, Mutex};
    use std::time::Duration;

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut w = World::new();
    w.add_addon(
        "com.rvah.fixtures.slow",
        &["catalog"],
        Arc::new(BlockingProvider {
            entered: Mutex::new(entered_tx),
            release: Mutex::new(release_rx),
        }),
        vec![Capability::Catalog],
    );
    w.core.invoke_timeout = Duration::from_secs(10);
    let prepared = w
        .core
        .prepare_playback(
            ECHO_FIXTURE_ID,
            "m1",
            rvah_core::PlaybackCapabilities::media3(),
        )
        .unwrap();
    let tracker = w.core.playback_tracker();
    let core = Arc::new(Mutex::new(w.core));

    // An add-on call holds the core lock for its whole duration.
    let browsing = {
        let core = Arc::clone(&core);
        std::thread::spawn(move || {
            let _ = core
                .lock()
                .unwrap()
                .browse("com.rvah.fixtures.slow", None, None);
        })
    };
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("slow add-on entered");

    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let recorded = tracker.lock().unwrap().report(
            prepared.session_id,
            rvah_core::PlaybackLifecycleEvent::Playing {
                position_secs: 30.0,
                duration_secs: Some(100.0),
            },
        );
        let _ = done_tx.send(recorded.map_err(|e| e.to_string()));
    });
    let recorded = done_rx.recv_timeout(Duration::from_secs(2));
    release_tx.send(()).unwrap();
    browsing.join().unwrap();
    assert_eq!(
        recorded.expect("playback report blocked behind the add-on call"),
        Ok(true)
    );
}
