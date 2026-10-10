//! End-to-end flows over real install + runtime + player wiring (STP §6):
//! browse, pagination, quarantine, playback with resume, search, favorites,
//! device-code sign-in and credential refresh.

use std::sync::Arc;

use rvah_addon_host::{AddonManager, Capability, InProcessRuntime, InstallDecision};
use rvah_addon_sdk::{AddonErrorCode, AddonProvider, PageItem};
use rvah_auth::DeviceCodeState;
use rvah_auth::MemoryVault;
use rvah_core::{AppCore, CoreError, ErrorClass};
use rvah_data::{AddonStatus, Database};
use rvah_player::{PlayerEvent, PlayerState};
use rvah_testkit::{
    write_fixture_package, CatalogEchoProvider, DeviceAuthProvider, FailingProvider,
    LargeCatalogProvider, MockPlayer, NoopHost, ResolverStaticProvider, ECHO_FIXTURE_ID,
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
        self.add_addon_with_secrets(id, caps, &[], provider, runtime_caps);
    }

    fn add_addon_with_secrets(
        &mut self,
        id: &str,
        caps: &[&str],
        secrets: &[&str],
        provider: Arc<dyn AddonProvider + Send + Sync>,
        runtime_caps: Vec<Capability>,
    ) {
        self.add_addon_with_permissions(id, caps, &[], secrets, provider, runtime_caps);
    }

    fn add_addon_with_permissions(
        &mut self,
        id: &str,
        caps: &[&str],
        network: &[&str],
        secrets: &[&str],
        provider: Arc<dyn AddonProvider + Send + Sync>,
        runtime_caps: Vec<Capability>,
    ) {
        let (pkg, sig, pubkey) = rvah_testkit::write_fixture_package_with_permissions(
            self._tmp.path(),
            id,
            caps,
            network,
            secrets,
        );
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
fn search_follows_cursor_pages() {
    let mut w = World::new();
    w.add_addon(
        "com.rvah.fixtures.large",
        &["search"],
        Arc::new(LargeCatalogProvider {
            total: 15,
            page_size: 10,
        }),
        vec![Capability::Search],
    );

    let first = w
        .core
        .search("com.rvah.fixtures.large", "item", None)
        .unwrap();
    assert_eq!(first.items.len(), 10);
    let second = w
        .core
        .search("com.rvah.fixtures.large", "item", first.cursor_next)
        .unwrap();
    assert_eq!(second.items.len(), 5);
    assert_eq!(second.cursor_next, None);
}

#[test]
fn following_a_cursor_stays_on_the_same_history_entry() {
    // A follow-up page extends the listing; it is not a new location, so
    // Back must not land on the same directory again.
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
    w.core
        .browse("com.rvah.fixtures.large", None, None)
        .unwrap();
    w.core
        .browse("com.rvah.fixtures.large", None, Some("10".into()))
        .unwrap();
    assert_eq!(w.core.nav.current(), Some("com.rvah.fixtures.large:/"));
    assert_eq!(w.core.nav.back(), None);
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
    let page = w.core.search(ECHO_FIXTURE_ID, "naruto", None).unwrap();
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

const LINK_ID: &str = "com.rvah.fixtures.link";

/// A catalog add-on that signs in by device code and declares two secrets.
fn linked_world(provider: &Arc<DeviceAuthProvider>) -> World {
    let mut w = World::new();
    w.add_addon_with_secrets(
        LINK_ID,
        &["catalog", "auth"],
        &["api_token", "oauth_client"],
        Arc::clone(provider) as Arc<dyn AddonProvider + Send + Sync>,
        vec![Capability::Catalog, Capability::Auth],
    );
    w
}

fn stored(w: &World, name: &str) -> Option<String> {
    w.core.vault().get(LINK_ID, name).unwrap()
}

#[test]
fn device_code_sign_in_stores_credentials_only_in_the_vault() {
    // UC-04: the user sees a code and URL; approval lands in the vault.
    let provider = Arc::new(DeviceAuthProvider::new(
        2,
        &[("api_token", "tok-1"), ("oauth_client", "refresh-1")],
    ));
    let mut w = linked_world(&provider);

    let prompt = w.core.auth_begin(LINK_ID).unwrap();
    assert_eq!(prompt.user_code, "WXYZ-1234");
    assert_eq!(prompt.verification_url, "https://example.com/device");

    assert!(matches!(
        w.core.auth_poll(LINK_ID).unwrap(),
        DeviceCodeState::Pending { .. }
    ));
    assert_eq!(stored(&w, "api_token"), None);
    assert_eq!(
        w.core.auth_poll(LINK_ID).unwrap(),
        DeviceCodeState::Authorized
    );
    assert_eq!(stored(&w, "api_token").as_deref(), Some("tok-1"));
    assert_eq!(stored(&w, "oauth_client").as_deref(), Some("refresh-1"));

    // The flow is over; another poll has nothing to advance.
    assert!(matches!(
        w.core.auth_poll(LINK_ID),
        Err(CoreError::NoSignIn(_))
    ));
}

#[test]
fn sign_in_refuses_secret_names_the_manifest_does_not_declare() {
    let provider = Arc::new(DeviceAuthProvider::new(
        1,
        &[("api_token", "tok-1"), ("someone_elses", "x")],
    ));
    let mut w = linked_world(&provider);
    w.core.auth_begin(LINK_ID).unwrap();
    let err = w.core.auth_poll(LINK_ID).unwrap_err();
    assert!(
        matches!(&err, CoreError::Provider(e) if e.code == AddonErrorCode::PermissionDenied),
        "{err}"
    );
    assert!(w.core.vault().list(LINK_ID).unwrap().is_empty());
}

#[test]
fn polls_sooner_than_the_interval_never_reach_the_provider() {
    let provider = Arc::new(DeviceAuthProvider::new(1, &[("api_token", "t")]).with_interval(60));
    let mut w = linked_world(&provider);
    w.core.auth_begin(LINK_ID).unwrap();
    for _ in 0..3 {
        assert!(matches!(
            w.core.auth_poll(LINK_ID).unwrap(),
            DeviceCodeState::Pending { .. }
        ));
    }
    assert_eq!(provider.polls(), 0);
    w.core.auth_cancel(LINK_ID);
    assert!(matches!(
        w.core.auth_poll(LINK_ID),
        Err(CoreError::NoSignIn(_))
    ));
}

#[test]
fn an_expired_token_is_refreshed_once_and_the_call_retried() {
    // SDD §7.3: refresh is handled centrally, invisibly to the user.
    let provider = Arc::new(
        DeviceAuthProvider::new(1, &[("api_token", "tok-1")])
            .refreshing_to(&[("api_token", "tok-2")]),
    );
    let mut w = linked_world(&provider);
    let page = w.core.browse(LINK_ID, None, None).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(provider.refreshes(), 1);
    assert_eq!(stored(&w, "api_token").as_deref(), Some("tok-2"));
    let events = w.core.db().recent_events(10).unwrap();
    assert!(events
        .iter()
        .any(|e| e.code == "auth_refresh" && e.level == "info"));
    assert!(
        !events.iter().any(|e| e.level == "error"),
        "a recovered call is not a failure: {events:?}"
    );
}

#[test]
fn a_failed_refresh_surfaces_the_original_auth_error() {
    let provider = Arc::new(DeviceAuthProvider::new(1, &[("api_token", "tok-1")]));
    let mut w = linked_world(&provider);
    let err = w.core.browse(LINK_ID, None, None).unwrap_err();
    assert!(
        matches!(&err, CoreError::Provider(e) if e.code == AddonErrorCode::Auth && e.message == "token expired"),
        "{err}"
    );
    assert_eq!(err.class(), ErrorClass::UserActionable);
    assert_eq!(provider.refreshes(), 1);
}

#[test]
fn add_ons_without_the_auth_capability_are_never_refreshed() {
    // The runtime could serve a refresh, but the manifest never declared
    // `auth`, so the core must not ask.
    let provider = Arc::new(
        DeviceAuthProvider::new(1, &[("api_token", "t")]).refreshing_to(&[("api_token", "t2")]),
    );
    let mut w = World::new();
    w.add_addon_with_secrets(
        LINK_ID,
        &["catalog"],
        &["api_token"],
        Arc::clone(&provider) as Arc<dyn AddonProvider + Send + Sync>,
        vec![Capability::Catalog, Capability::Auth],
    );
    assert!(w.core.browse(LINK_ID, None, None).is_err());
    assert_eq!(provider.refreshes(), 0);
}

#[test]
fn account_status_reports_the_linked_account_after_a_refresh() {
    // FR-085: the settings screen can say who is signed in.
    let provider = Arc::new(
        DeviceAuthProvider::new(1, &[("api_token", "tok-1")])
            .refreshing_to(&[("api_token", "tok-2")]),
    );
    let mut w = World::new();
    w.add_addon_with_secrets(
        LINK_ID,
        &["catalog", "auth", "account_status"],
        &["api_token", "oauth_client"],
        Arc::clone(&provider) as Arc<dyn AddonProvider + Send + Sync>,
        vec![
            Capability::Catalog,
            Capability::Auth,
            Capability::AccountStatus,
        ],
    );
    assert_eq!(
        w.core.account_status(LINK_ID).unwrap(),
        rvah_addon_sdk::AccountStatus::SignedIn {
            display_name: Some("fixture-user".into())
        }
    );
    assert_eq!(provider.refreshes(), 1);
}

const ART_ID: &str = "com.rvah.fixtures.art";

fn with_artwork(poster: &str, fanart: &str, thumb: &str) -> rvah_addon_sdk::Artwork {
    rvah_addon_sdk::Artwork {
        poster: Some(poster.into()),
        fanart: Some(fanart.into()),
        thumb: Some(thumb.into()),
    }
}

#[test]
fn artwork_reaches_the_ui_only_over_https_from_declared_hosts() {
    use rvah_addon_sdk::{ContentType, DirectoryItem, MediaItem, Page};
    let page = Page {
        items: vec![
            PageItem::Media(MediaItem {
                id: "m1".into(),
                content_type: ContentType::Movie,
                title: "Poster Movie".into(),
                year: None,
                season: None,
                episode: None,
                runtime_secs: None,
                synopsis: None,
                rating: None,
                artwork: with_artwork(
                    "https://img.example.com/p/m1.jpg",
                    // Not https.
                    "http://img.example.com/f/m1.jpg",
                    // Not a host the add-on declared.
                    "https://tracker.example.net/pixel.gif",
                ),
                provider_ids: Default::default(),
            }),
            PageItem::Directory(DirectoryItem {
                id: "d1".into(),
                title: "Row".into(),
                subtitle: None,
                artwork: with_artwork(
                    // A subdomain under a declared wildcard.
                    "https://a.cdn.example.org/d1.jpg",
                    "https://user@img.example.com/d1.jpg",
                    "not a url",
                ),
                path: "/d1".into(),
                badges: vec![],
            }),
        ],
        cursor_next: None,
        total_hint: None,
    };
    let mut w = World::new();
    w.add_addon_with_permissions(
        ART_ID,
        &["catalog", "search"],
        &["img.example.com", "*.cdn.example.org"],
        &[],
        Arc::new(rvah_testkit::StaticPageProvider(page)),
        vec![Capability::Catalog, Capability::Search],
    );
    let browsed = w.core.browse(ART_ID, None, None).unwrap();
    let searched = w.core.search(ART_ID, "anything", None).unwrap();
    for page in [browsed, searched] {
        let PageItem::Media(m) = &page.items[0] else {
            panic!("expected media first");
        };
        assert_eq!(
            m.artwork,
            rvah_addon_sdk::Artwork {
                poster: Some("https://img.example.com/p/m1.jpg".into()),
                fanart: None,
                thumb: None,
            }
        );
        let PageItem::Directory(d) = &page.items[1] else {
            panic!("expected a directory second");
        };
        assert_eq!(
            d.artwork,
            rvah_addon_sdk::Artwork {
                poster: Some("https://a.cdn.example.org/d1.jpg".into()),
                fanart: None,
                thumb: None,
            }
        );
    }
}

#[test]
fn details_get_the_same_artwork_rule_and_only_plain_trailer_keys() {
    use rvah_addon_sdk::{ContentType, ItemDetails, MediaItem, Rating, Trailer, VideoSite};
    let trailer = |key: &str| Trailer {
        name: format!("Trailer {key}"),
        site: VideoSite::YouTube,
        key: key.into(),
    };
    let details = ItemDetails {
        item: MediaItem {
            id: "movie:1".into(),
            content_type: ContentType::Movie,
            title: "Detailed".into(),
            year: Some(2026),
            season: None,
            episode: None,
            runtime_secs: Some(5400),
            synopsis: Some("A coyote sues.".into()),
            rating: Some(7.4),
            artwork: with_artwork(
                "https://img.example.com/p.jpg",
                "https://img.example.com/f.jpg",
                "https://elsewhere.example.net/t.jpg",
            ),
            provider_ids: Default::default(),
        },
        series_title: None,
        tagline: None,
        genres: vec!["Comedy".into()],
        released: Some("2026-03-14".into()),
        trailers: vec![
            trailer("dQw4w9WgXcQ"),
            trailer("x?list=evil"),
            trailer("../watch"),
            trailer(""),
            trailer("a_b-C9"),
        ],
        ratings: vec![Rating {
            source: "rotten_tomatoes".into(),
            value: "93%".into(),
        }],
    };
    let mut w = World::new();
    w.add_addon_with_permissions(
        ART_ID,
        &["details"],
        &["img.example.com"],
        &[],
        Arc::new(rvah_testkit::StaticDetailsProvider(details)),
        vec![Capability::Details],
    );
    let shown = w.core.details(ART_ID, "movie:1").unwrap();
    assert_eq!(shown.item.title, "Detailed");
    assert_eq!(
        shown.item.artwork.poster.as_deref(),
        Some("https://img.example.com/p.jpg")
    );
    assert_eq!(
        shown.item.artwork.fanart.as_deref(),
        Some("https://img.example.com/f.jpg")
    );
    assert_eq!(shown.item.artwork.thumb, None);
    let keys: Vec<&str> = shown.trailers.iter().map(|t| t.key.as_str()).collect();
    assert_eq!(keys, ["dQw4w9WgXcQ", "a_b-C9"]);
    assert_eq!(shown.ratings[0].value, "93%");

    // An add-on without the capability is refused before any call.
    let err = w.core.details(ECHO_FIXTURE_ID, "m1").unwrap_err();
    assert!(matches!(err, CoreError::Provider(e) if e.code == AddonErrorCode::PermissionDenied));
}

const SOURCES_ID: &str = "com.rvah.fixtures.sources";
const DEBRID_ID: &str = "com.rvah.fixtures.debrid";

fn release(name: &str, info_hash: &str) -> rvah_addon_sdk::Release {
    rvah_addon_sdk::Release {
        name: name.into(),
        info_hash: info_hash.into(),
        size_bytes: Some(4 << 30),
        seeders: Some(120),
        origin: Some("Indexer".into()),
    }
}

fn episode_request() -> rvah_addon_sdk::SourceRequest {
    rvah_addon_sdk::SourceRequest {
        kind: rvah_addon_sdk::SourceKind::Episode,
        imdb_id: Some("tt0944947".into()),
        title: "Acme Corp".into(),
        year: None,
        season: Some(1),
        episode: Some(2),
    }
}

#[test]
fn sources_keep_one_of_each_well_formed_release() {
    let hex = "0123456789ABCDEF0123456789abcdef01234567";
    // The same hash in base32 (RFC 4648), as some indexers write it.
    let base32 = "AERUKZ4JVPG66AJDIVTYTK6N54ASGRLH";
    let releases = vec![
        release("Acme.Corp.S01E02.1080p", hex),
        release("Acme.Corp.S01E02.1080p.again", base32),
        release(
            "Acme.Corp.S01E02.720p",
            "ffffffffffffffffffffffffffffffffffffffff",
        ),
        release("broken", "not-a-hash"),
        release("too short", "abc123"),
    ];
    let mut w = World::new();
    w.add_addon(
        SOURCES_ID,
        &["sources"],
        Arc::new(rvah_testkit::StaticSourcesProvider {
            releases,
            delay: std::time::Duration::ZERO,
        }),
        vec![Capability::Sources],
    );
    let found = w.core.sources(SOURCES_ID, episode_request()).unwrap();
    let hashes: Vec<&str> = found.iter().map(|r| r.info_hash.as_str()).collect();
    assert_eq!(
        hashes,
        [
            "0123456789abcdef0123456789abcdef01234567",
            "ffffffffffffffffffffffffffffffffffffffff"
        ]
    );
}

#[test]
fn a_source_search_may_take_longer_than_other_calls() {
    let mut w = World::new();
    w.add_addon(
        SOURCES_ID,
        &["sources"],
        Arc::new(rvah_testkit::StaticSourcesProvider {
            releases: vec![],
            delay: std::time::Duration::from_millis(150),
        }),
        vec![Capability::Sources],
    );
    w.core.invoke_timeout = std::time::Duration::from_millis(50);
    w.core.sources_timeout = std::time::Duration::from_millis(1_000);
    assert!(w.core.sources(SOURCES_ID, episode_request()).is_ok());
    w.core.sources_timeout = std::time::Duration::from_millis(60);
    let err = w.core.sources(SOURCES_ID, episode_request()).unwrap_err();
    assert!(matches!(err, CoreError::Provider(e) if e.code == AddonErrorCode::Timeout));
}

#[test]
fn a_release_plays_through_the_debrid_add_on_and_resumes_by_title() {
    let debrid = Arc::new(rvah_testkit::DebridProvider::new(
        rvah_addon_sdk::ResolveResult::Playables(vec![rvah_testkit::mp4_source(
            "https://cdn.example.com/acme-s01e02.mp4",
        )]),
    ));
    let mut w = World::new();
    w.add_addon(
        DEBRID_ID,
        &["debrid"],
        Arc::clone(&debrid) as Arc<dyn AddonProvider + Send + Sync>,
        vec![Capability::Debrid],
    );
    // Watched before, through another release of the same episode.
    w.core
        .db()
        .set_watch_state(&rvah_data::WatchState {
            addon_id: "org.themoviedb.discover".into(),
            item_id: "tv:1399:s1e2".into(),
            position_secs: 120.0,
            duration_secs: 3000.0,
            watched: false,
            last_played_at: 1,
        })
        .unwrap();
    let chosen = release("Acme.Corp.S01.1080p", &"c".repeat(40));
    let prepared = w
        .core
        .prepare_release(
            DEBRID_ID,
            &chosen,
            Some(1),
            Some(2),
            ("org.themoviedb.discover", "tv:1399:s1e2"),
            rvah_core::PlaybackCapabilities::media3(),
        )
        .unwrap();
    assert_eq!(
        prepared.source.url,
        "https://cdn.example.com/acme-s01e02.mp4"
    );
    assert_eq!(prepared.resume_position_secs, 120.0);
    assert_eq!(
        debrid.seen.lock().unwrap().as_slice(),
        [(chosen, Some(1), Some(2))]
    );
}

#[test]
fn a_release_that_is_not_ready_says_so() {
    let mut w = World::new();
    w.add_addon(
        DEBRID_ID,
        &["debrid"],
        Arc::new(rvah_testkit::DebridProvider::new(
            rvah_addon_sdk::ResolveResult::Unavailable {
                reason: "not cached on Real-Debrid".into(),
            },
        )),
        vec![Capability::Debrid],
    );
    let err = w
        .core
        .prepare_release(
            DEBRID_ID,
            &release("Film.2026.2160p", &"d".repeat(40)),
            None,
            None,
            ("org.themoviedb.discover", "movie:1"),
            rvah_core::PlaybackCapabilities::media3(),
        )
        .unwrap_err();
    assert!(
        matches!(&err, CoreError::Provider(e) if e.code == AddonErrorCode::NotFound && e.user_visible && e.message.contains("not cached")),
        "{err:?}"
    );
}
