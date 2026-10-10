use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rvah_addon_host::{
    AddonManager, AddonOp, AddonResponse, AddonRuntime, Capability, InvocationCtx, Manifest,
};
use rvah_addon_sdk::{
    AccountStatus, AddonError, AddonErrorCode, AuthPoll, Credentials, Filter, HostApi, ItemDetails,
    Page, Release, ResolveResult, SourceRequest,
};
use rvah_auth::{DeviceCodeFlow, DeviceCodeState, PollOutcome, SecretVault};
use rvah_data::{AddonStatus, Database, DiagnosticEvent, FavoriteRecord};
use rvah_player::{
    validate_source_opts, PlayerBackend, PlayerController, PlayerEvent, PlayerState,
    SourceRejection,
};

use crate::errors::CoreError;
use crate::nav::NavHistory;
use crate::playback::{
    lock_tracker, PlaybackCapabilities, PlaybackDisplayMetadata, PlaybackLifecycleEvent,
    PlaybackSession, PlaybackSessionId, PlaybackTracker, PreparedPlayback,
};

pub const DEFAULT_INVOKE_TIMEOUT: Duration = Duration::from_secs(2);
/// A source search waits on every indexer the source asks.
pub const SOURCES_TIMEOUT: Duration = Duration::from_secs(45);
/// Playing a release takes a debrid service several requests.
pub const RELEASE_TIMEOUT: Duration = Duration::from_secs(20);
/// Consecutive failures before an add-on is quarantined (NFR Reliability).
pub const QUARANTINE_THRESHOLD: u32 = 3;

struct RegisteredAddon {
    runtime: Box<dyn AddonRuntime>,
    host: Arc<dyn HostApi>,
}

/// What the user needs to finish a device-code sign-in (UC-04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthPrompt {
    pub user_code: String,
    pub verification_url: String,
    /// The provider's minimum wait between polls.
    pub interval_secs: u32,
    /// Unix seconds after which the code is no longer accepted.
    pub expires_at: u64,
}

/// A device-code sign-in in progress. The device code stays in the core;
/// callers only ever see the user code and URL.
struct PendingAuth {
    flow: DeviceCodeFlow,
    device_code: String,
    interval: Duration,
    next_poll: Instant,
}

/// The application core: owns the add-on manager, per-add-on runtimes, the
/// player, and persistence. This is the entire UI-facing surface for Phase 1;
/// the CLI (mock UI) and later GUIs sit directly on top of it.
pub struct AppCore<B: PlayerBackend> {
    pub manager: AddonManager,
    db: Arc<Database>,
    vault: Arc<dyn SecretVault>,
    addons: HashMap<String, RegisteredAddon>,
    pub player: PlayerController<B>,
    pub nav: NavHistory,
    pub invoke_timeout: Duration,
    /// For `sources`; never shorter than `invoke_timeout`.
    pub sources_timeout: Duration,
    /// For `resolve_release`; never shorter than `invoke_timeout`.
    pub release_timeout: Duration,
    failures: HashMap<String, u32>,
    playback: Arc<Mutex<PlaybackTracker>>,
    sign_ins: HashMap<String, PendingAuth>,
}

impl<B: PlayerBackend> AppCore<B> {
    pub fn new(
        manager: AddonManager,
        db: Arc<Database>,
        vault: Arc<dyn SecretVault>,
        backend: B,
    ) -> Self {
        let playback = Arc::new(Mutex::new(PlaybackTracker::new(Arc::clone(&db))));
        Self {
            manager,
            db,
            vault,
            addons: HashMap::new(),
            player: PlayerController::new(backend),
            nav: NavHistory::new(),
            invoke_timeout: DEFAULT_INVOKE_TIMEOUT,
            sources_timeout: SOURCES_TIMEOUT,
            release_timeout: RELEASE_TIMEOUT,
            failures: HashMap::new(),
            playback,
            sign_ins: HashMap::new(),
        }
    }

    /// Attach a runtime + host bridge for an installed add-on. The runtime
    /// adapter is what lets fixtures (FR-027) and wasm (FR-020) coexist.
    pub fn register_runtime(
        &mut self,
        addon_id: impl Into<String>,
        runtime: Box<dyn AddonRuntime>,
        host: Arc<dyn HostApi>,
    ) {
        self.addons
            .insert(addon_id.into(), RegisteredAddon { runtime, host });
    }

    /// Snapshot of the active playback session.
    pub fn session(&self) -> Option<PlaybackSession> {
        self.tracker().session().cloned()
    }

    /// The playback tracker behind its own lock. Platform adapters that
    /// share the core across threads report native-player callbacks here so
    /// they never wait behind an add-on invocation holding the core.
    pub fn playback_tracker(&self) -> Arc<Mutex<PlaybackTracker>> {
        Arc::clone(&self.playback)
    }

    fn tracker(&self) -> MutexGuard<'_, PlaybackTracker> {
        lock_tracker(&self.playback)
    }

    fn ensure_enabled(&self, addon_id: &str) -> Result<(), CoreError> {
        let rec = self
            .db
            .get_addon(addon_id)?
            .ok_or_else(|| CoreError::AddonNotFound(addon_id.to_string()))?;
        match rec.status {
            AddonStatus::Enabled => Ok(()),
            _ => Err(CoreError::AddonDisabled(addon_id.to_string())),
        }
    }

    /// Invoke through the runtime with failure accounting: any provider error
    /// is logged, and `QUARANTINE_THRESHOLD` consecutive failures quarantine
    /// the add-on. An auth failure first gets one credential refresh and
    /// retry (SDD §7.3), so an expired access token never reaches the user.
    fn invoke(&mut self, addon_id: &str, op: AddonOp) -> Result<AddonResponse, CoreError> {
        self.ensure_enabled(addon_id)?;
        let auth_op = op.required_capability() == Capability::Auth;
        let mut result = self.call(addon_id, op.clone())?;
        if matches!(&result, Err(e) if e.code == AddonErrorCode::Auth)
            && !auth_op
            && self.refresh_credentials(addon_id)
        {
            result = self.call(addon_id, op)?;
        }
        match result {
            Ok(resp) => {
                self.failures.remove(addon_id);
                Ok(resp)
            }
            Err(err) => {
                self.record_failure(addon_id, &err);
                Err(CoreError::Provider(err))
            }
        }
    }

    /// One runtime call, without failure accounting.
    fn call(
        &self,
        addon_id: &str,
        op: AddonOp,
    ) -> Result<Result<AddonResponse, AddonError>, CoreError> {
        let addon = self
            .addons
            .get(addon_id)
            .ok_or_else(|| CoreError::AddonNotFound(addon_id.to_string()))?;
        let timeout = match &op {
            AddonOp::Sources { .. } => self.sources_timeout.max(self.invoke_timeout),
            AddonOp::ResolveRelease { .. } => self.release_timeout.max(self.invoke_timeout),
            _ => self.invoke_timeout,
        };
        let ctx = InvocationCtx {
            timeout,
            host: Arc::clone(&addon.host),
        };
        let op_name = match &op {
            AddonOp::List { .. } => "list",
            AddonOp::Search { .. } => "search",
            AddonOp::Resolve { .. } => "resolve",
            AddonOp::AccountStatus => "account_status",
            AddonOp::AuthBegin => "auth_begin",
            AddonOp::AuthPoll { .. } => "auth_poll",
            AddonOp::AuthRefresh => "auth_refresh",
            AddonOp::Details { .. } => "details",
            AddonOp::Sources { .. } => "sources",
            AddonOp::ResolveRelease { .. } => "resolve_release",
        };
        let started = std::time::Instant::now();
        let result = addon.runtime.invoke(op, &ctx);
        crate::debug_log!(
            "{addon_id} {op_name}: {} in {:.3}s",
            if result.is_ok() { "ok" } else { "error" },
            started.elapsed().as_secs_f64()
        );
        Ok(result)
    }

    /// After an auth failure, let an add-on that declares `auth` trade its
    /// stored refresh material for fresh credentials. True when new ones
    /// were stored and the failed call is worth one retry.
    fn refresh_credentials(&mut self, addon_id: &str) -> bool {
        let Ok(manifest) = self.installed_manifest(addon_id) else {
            return false;
        };
        if !manifest.addon.capabilities.contains(&Capability::Auth) {
            return false;
        }
        let outcome = match self.call(addon_id, AddonOp::AuthRefresh) {
            Ok(Ok(AddonResponse::Credentials(credentials))) => {
                self.store_credentials(addon_id, &manifest, &credentials)
            }
            Ok(Ok(other)) => Err(unexpected_response(&other)),
            Ok(Err(e)) => Err(CoreError::Provider(e)),
            Err(e) => Err(e),
        };
        match outcome {
            Ok(()) => {
                let _ = self.db.log_event(&DiagnosticEvent {
                    level: "info".into(),
                    addon_id: Some(addon_id.to_string()),
                    code: "auth_refresh".into(),
                    message: "credentials refreshed".into(),
                });
                true
            }
            // The original auth error is what gets recorded and shown.
            Err(e) => {
                crate::debug_log!("{addon_id} credential refresh failed: {e}");
                false
            }
        }
    }

    /// Store add-on supplied credentials, all or nothing on the names: every
    /// one must be a secret the manifest declares (FR-083).
    fn store_credentials(
        &self,
        addon_id: &str,
        manifest: &Manifest,
        credentials: &Credentials,
    ) -> Result<(), CoreError> {
        check_declared(manifest, credentials)?;
        for (name, value) in &credentials.0 {
            self.vault.set(addon_id, name, value)?;
        }
        Ok(())
    }

    fn record_failure(&mut self, addon_id: &str, err: &AddonError) {
        let count = self.failures.entry(addon_id.to_string()).or_insert(0);
        *count += 1;
        let _ = self.db.log_event(&DiagnosticEvent {
            level: "error".into(),
            addon_id: Some(addon_id.to_string()),
            code: format!("{:?}", err.code).to_lowercase(),
            message: err.message.clone(),
        });
        if *count >= QUARANTINE_THRESHOLD {
            let _ = self.db.set_addon_status(addon_id, AddonStatus::Quarantined);
            let _ = self.db.log_event(&DiagnosticEvent {
                level: "warn".into(),
                addon_id: Some(addon_id.to_string()),
                code: "quarantined".into(),
                message: format!("{addon_id} quarantined after {count} consecutive failures"),
            });
            self.failures.remove(addon_id);
        }
    }

    fn expect_page(resp: AddonResponse) -> Result<Page, CoreError> {
        match resp {
            AddonResponse::Page(page) => Ok(page),
            other => Err(unexpected_response(&other)),
        }
    }

    pub fn browse(
        &mut self,
        addon_id: &str,
        path: Option<String>,
        cursor: Option<String>,
    ) -> Result<Page, CoreError> {
        // A cursor continues the listing already on screen (FR-024): the
        // location is unchanged, so history gains no entry.
        let follow_up = cursor.is_some();
        let page = Self::expect_page(self.invoke(
            addon_id,
            AddonOp::List {
                path: path.clone(),
                cursor,
            },
        )?)?;
        let page = self.screened(addon_id, page)?;
        if !follow_up {
            self.nav
                .push(format!("{addon_id}:{}", path.as_deref().unwrap_or("/")));
        }
        Ok(page)
    }

    /// `cursor` is the previous page's `cursor_next`, or `None` for the first.
    pub fn search(
        &mut self,
        addon_id: &str,
        query: &str,
        cursor: Option<String>,
    ) -> Result<Page, CoreError> {
        let op = AddonOp::Search {
            query: query.to_string(),
            filters: Vec::<Filter>::new(),
            cursor,
        };
        let page = Self::expect_page(self.invoke(addon_id, op)?)?;
        self.screened(addon_id, page)
    }

    /// Details-page information for one item (capability `details`), with
    /// the listing artwork rule and only plain trailer keys.
    pub fn details(&mut self, addon_id: &str, item_id: &str) -> Result<ItemDetails, CoreError> {
        let op = AddonOp::Details {
            item_id: item_id.to_string(),
        };
        let mut details = match self.invoke(addon_id, op)? {
            AddonResponse::Details(details) => *details,
            other => return Err(unexpected_response(&other)),
        };
        let manifest = self.installed_manifest(addon_id)?;
        crate::artwork::screen_details(&mut details, &manifest.permissions.network);
        Ok(details)
    }

    /// `page` with only the artwork the UI may load (see `artwork`).
    fn screened(&self, addon_id: &str, mut page: Page) -> Result<Page, CoreError> {
        let manifest = self.installed_manifest(addon_id)?;
        crate::artwork::screen_page(&mut page, &manifest.permissions.network);
        Ok(page)
    }

    /// Fan a query out to every enabled add-on declaring search (FR-044).
    pub fn search_all(&mut self, query: &str) -> Vec<(String, Result<Page, CoreError>)> {
        let candidates: Vec<String> = match self.db.list_addons() {
            Ok(addons) => addons
                .into_iter()
                .filter(|a| {
                    a.status == AddonStatus::Enabled && a.capabilities.iter().any(|c| c == "search")
                })
                .map(|a| a.id)
                .collect(),
            Err(e) => return vec![("host".into(), Err(e.into()))],
        };
        candidates
            .into_iter()
            .map(|id| {
                let result = self.search(&id, query, None);
                (id, result)
            })
            .collect()
    }

    /// Resolve an item and prepare a source for a platform-native player.
    /// No backend command is executed here.
    pub fn prepare_playback(
        &mut self,
        addon_id: &str,
        item_id: &str,
        capabilities: PlaybackCapabilities,
    ) -> Result<PreparedPlayback, CoreError> {
        let resp = self.invoke(
            addon_id,
            AddonOp::Resolve {
                item_id: item_id.to_string(),
                source_id: None,
            },
        )?;
        self.prepare_resolved(resp, (addon_id, item_id), capabilities)
    }

    /// Releases of one film or episode from a `sources` add-on, each with a
    /// well-formed info hash (normalized to lowercase hex), once.
    pub fn sources(
        &mut self,
        addon_id: &str,
        request: SourceRequest,
    ) -> Result<Vec<Release>, CoreError> {
        let releases = match self.invoke(addon_id, AddonOp::Sources { request })? {
            AddonResponse::Releases(releases) => releases,
            other => return Err(unexpected_response(&other)),
        };
        let mut seen = std::collections::HashSet::new();
        Ok(releases
            .into_iter()
            .filter_map(|mut release| {
                release.info_hash = normalize_info_hash(&release.info_hash)?;
                seen.insert(release.info_hash.clone()).then_some(release)
            })
            .collect())
    }

    /// Play `release` through a `debrid` add-on. Progress is kept under
    /// `title` (the add-on and item the release is of), so resuming works
    /// whichever release plays next time.
    pub fn prepare_release(
        &mut self,
        debrid_id: &str,
        release: &Release,
        season: Option<u32>,
        episode: Option<u32>,
        title: (&str, &str),
        capabilities: PlaybackCapabilities,
    ) -> Result<PreparedPlayback, CoreError> {
        let resp = self.invoke(
            debrid_id,
            AddonOp::ResolveRelease {
                release: release.clone(),
                season,
                episode,
            },
        )?;
        self.prepare_resolved(resp, title, capabilities)
    }

    /// A playback session for a resolve answer, tracked under `progress`.
    fn prepare_resolved(
        &mut self,
        resp: AddonResponse,
        progress: (&str, &str),
        capabilities: PlaybackCapabilities,
    ) -> Result<PreparedPlayback, CoreError> {
        let (addon_id, item_id) = progress;
        let result = match resp {
            AddonResponse::Resolve(r) => r,
            other => {
                return Err(CoreError::Provider(AddonError::new(
                    AddonErrorCode::Malformed,
                    format!("provider returned unexpected response: {other:?}"),
                )))
            }
        };
        let sources = match result {
            ResolveResult::Playables(sources) => sources,
            ResolveResult::AuthRequired { provider_hint } => {
                return Err(CoreError::Provider(
                    AddonError::new(AddonErrorCode::Auth, provider_hint).user_visible(),
                ))
            }
            ResolveResult::Unavailable { reason } => {
                return Err(CoreError::Provider(
                    AddonError::new(AddonErrorCode::NotFound, reason).user_visible(),
                ))
            }
        };

        let now = unix_now();
        let mut last_rejection: Option<SourceRejection> = None;
        let source = sources.into_iter().find(|s| {
            if !capabilities.supports(s.protocol) {
                last_rejection = Some(SourceRejection::UnsupportedProtocol(
                    format!("{:?}", s.protocol).to_ascii_lowercase(),
                ));
                return false;
            }
            match validate_source_opts(s, now, capabilities.dash) {
                Ok(()) => true,
                Err(r) => {
                    last_rejection = Some(r);
                    false
                }
            }
        });
        let source = match source {
            Some(s) => s,
            None => {
                return Err(match last_rejection {
                    Some(r) => CoreError::SourceRejected(r),
                    None => CoreError::Provider(
                        AddonError::new(AddonErrorCode::NotFound, "no playable sources")
                            .user_visible(),
                    ),
                })
            }
        };

        let start_at = match self.db.get_watch_state(addon_id, item_id)? {
            Some(ws) if !ws.watched && ws.position_secs > 0.0 => ws.position_secs,
            _ => 0.0,
        };

        let session_id = self
            .tracker()
            .begin(addon_id, item_id, source.clone(), start_at);
        Ok(PreparedPlayback {
            session_id,
            subtitles: source.subtitles.clone(),
            source,
            resume_position_secs: start_at,
            display: PlaybackDisplayMetadata {
                title: item_id.to_string(),
                subtitle: Some(addon_id.to_string()),
            },
        })
    }

    /// Desktop adapter: load a prepared source into the configured backend.
    pub fn start_prepared_playback(
        &mut self,
        prepared: &PreparedPlayback,
    ) -> Result<(), CoreError> {
        if self.tracker().session().map(|s| s.id) != Some(prepared.session_id) {
            return Ok(());
        }
        self.player
            .load_source(&prepared.source, prepared.resume_position_secs)
            .map_err(CoreError::Player)
    }

    /// Compatibility adapter used by the desktop CLI and egui service.
    pub fn resolve_and_play(
        &mut self,
        addon_id: &str,
        item_id: &str,
    ) -> Result<PlaybackSession, CoreError> {
        let caps = PlaybackCapabilities::for_backend(self.player.backend());
        let prepared = self.prepare_playback(addon_id, item_id, caps)?;
        if let Err(e) = self.start_prepared_playback(&prepared) {
            // Nothing is playing; do not leave a session for callbacks.
            self.tracker().stop(prepared.session_id);
            return Err(e);
        }
        Ok(self.session().expect("prepared session exists"))
    }

    /// Record one native-player callback. Returns `false` for a stale session
    /// and intentionally performs no state or persistence mutation.
    pub fn report_playback_event(
        &mut self,
        session_id: PlaybackSessionId,
        event: PlaybackLifecycleEvent,
    ) -> Result<bool, CoreError> {
        self.tracker().report(session_id, event)
    }

    /// Pump player events and persist debounced watch state (FR-065). Playback
    /// position doubles as the debounce clock so tests stay deterministic.
    pub fn tick_playback(&mut self) -> Result<Vec<PlayerEvent>, CoreError> {
        let events = self.player.handle_events();
        let Some(session_id) = self.tracker().session().map(|session| session.id) else {
            return Ok(events);
        };
        let duration = self.player.duration_secs().unwrap_or(0.0);
        for event in &events {
            let lifecycle = match event {
                // Play during Loading already unpaused the backend; only a
                // load that settled paused is a pause.
                PlayerEvent::Loaded { .. } if self.player.state() == PlayerState::Paused => {
                    PlaybackLifecycleEvent::Paused {
                        position_secs: self.player.position_secs(),
                        duration_secs: self.player.duration_secs(),
                    }
                }
                PlayerEvent::Loaded { .. } => PlaybackLifecycleEvent::Buffering,
                PlayerEvent::Buffering => PlaybackLifecycleEvent::Buffering,
                PlayerEvent::Progress { position_secs } => PlaybackLifecycleEvent::Playing {
                    position_secs: *position_secs,
                    duration_secs: Some(duration),
                },
                PlayerEvent::Ended => PlaybackLifecycleEvent::Ended {
                    position_secs: duration,
                    duration_secs: Some(duration),
                },
                PlayerEvent::Error(error) => PlaybackLifecycleEvent::Error {
                    message: error.message.clone(),
                },
            };
            let _ = self.report_playback_event(session_id, lifecycle)?;
        }
        Ok(events)
    }

    pub fn stop_playback(&mut self, session_id: PlaybackSessionId) -> Result<bool, CoreError> {
        Ok(self.tracker().stop(session_id))
    }

    pub fn stop_desktop_playback(&mut self) -> Result<(), CoreError> {
        let session_id = self.tracker().session().map(|session| session.id);
        self.player.stop().map_err(CoreError::Player)?;
        if let Some(session_id) = session_id {
            let _ = self.stop_playback(session_id)?;
        }
        Ok(())
    }

    fn installed_manifest(&self, addon_id: &str) -> Result<Manifest, CoreError> {
        let rec = self
            .db
            .get_addon(addon_id)?
            .ok_or_else(|| CoreError::AddonNotFound(addon_id.to_string()))?;
        let manifest_path = self
            .manager
            .install_dir(addon_id, &rec.version)
            .join("manifest.toml");
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|e| CoreError::Data(format!("read {}: {e}", manifest_path.display())))?;
        Manifest::parse(&text).map_err(|e| CoreError::Data(format!("manifest: {e}")))
    }

    /// Start a device-code sign-in with the add-on's provider (UC-04),
    /// replacing any earlier attempt for the same add-on.
    pub fn auth_begin(&mut self, addon_id: &str) -> Result<AuthPrompt, CoreError> {
        self.sign_ins.remove(addon_id);
        let device = match self.invoke(addon_id, AddonOp::AuthBegin)? {
            AddonResponse::DeviceAuth(device) => device,
            other => return Err(unexpected_response(&other)),
        };
        // The user is sent to this address, so it must at least be https.
        if !device.verification_url.starts_with("https://") {
            return Err(CoreError::Provider(AddonError::new(
                AddonErrorCode::Malformed,
                "sign-in verification URL must be https",
            )));
        }
        let prompt = AuthPrompt {
            user_code: device.user_code.clone(),
            verification_url: device.verification_url.clone(),
            interval_secs: device.interval_secs,
            expires_at: unix_now() + u64::from(device.expires_in_secs),
        };
        let interval = Duration::from_secs(device.interval_secs.into());
        self.sign_ins.insert(
            addon_id.to_string(),
            PendingAuth {
                flow: DeviceCodeFlow::new(
                    device.user_code,
                    device.verification_url,
                    prompt.expires_at,
                    addon_id,
                ),
                device_code: device.device_code,
                interval,
                next_poll: Instant::now() + interval,
            },
        );
        Ok(prompt)
    }

    /// Ask the provider once whether the user approved the code. Credentials
    /// go straight to the vault. A poll sooner than the provider's interval
    /// reports `Pending` without calling it, so no UI can trip a rate limit.
    /// A terminal state ends the sign-in; an error leaves it pending.
    pub fn auth_poll(&mut self, addon_id: &str) -> Result<DeviceCodeState, CoreError> {
        let mut pending = self
            .sign_ins
            .remove(addon_id)
            .ok_or_else(|| CoreError::NoSignIn(addon_id.to_string()))?;
        let result = self.advance_sign_in(addon_id, &mut pending);
        if matches!(result, Ok(DeviceCodeState::Pending { .. }) | Err(_)) {
            self.sign_ins.insert(addon_id.to_string(), pending);
        }
        result
    }

    fn advance_sign_in(
        &mut self,
        addon_id: &str,
        pending: &mut PendingAuth,
    ) -> Result<DeviceCodeState, CoreError> {
        if Instant::now() < pending.next_poll {
            return Ok(pending.flow.state().clone());
        }
        pending.next_poll = Instant::now() + pending.interval;
        let manifest = self.installed_manifest(addon_id)?;
        let op = AddonOp::AuthPoll {
            device_code: pending.device_code.clone(),
        };
        let vault = Arc::clone(&self.vault);
        let mut poller = || -> Result<PollOutcome, CoreError> {
            match self.invoke(addon_id, op.clone())? {
                AddonResponse::AuthPoll(AuthPoll::Pending) => Ok(PollOutcome::Pending),
                AddonResponse::AuthPoll(AuthPoll::Denied) => Ok(PollOutcome::Denied),
                AddonResponse::AuthPoll(AuthPoll::Authorized(credentials)) => {
                    check_declared(&manifest, &credentials)?;
                    Ok(PollOutcome::Authorized {
                        secrets: credentials.0,
                    })
                }
                other => Err(unexpected_response(&other)),
            }
        };
        pending
            .flow
            .poll(unix_now(), &mut poller, vault.as_ref())
            .cloned()
    }

    /// Abandon a sign-in, e.g. when the user leaves the screen.
    pub fn auth_cancel(&mut self, addon_id: &str) {
        self.sign_ins.remove(addon_id);
    }

    /// The add-on's view of its account (FR-085), after a refresh if the
    /// stored token had expired.
    pub fn account_status(&mut self, addon_id: &str) -> Result<AccountStatus, CoreError> {
        match self.invoke(addon_id, AddonOp::AccountStatus)? {
            AddonResponse::Account(status) => Ok(status),
            other => Err(unexpected_response(&other)),
        }
    }

    /// Re-approve the permissions currently declared by the installed
    /// manifest (FR-046).
    pub fn grant_permissions(&mut self, addon_id: &str) -> Result<(), CoreError> {
        let manifest = self.installed_manifest(addon_id)?;
        let json = serde_json::to_string(&manifest.permissions)
            .map_err(|e| CoreError::Data(e.to_string()))?;
        self.db
            .record_permissions(addon_id, &json, &manifest.permission_hash())?;
        Ok(())
    }

    pub fn revoke_permissions(&mut self, addon_id: &str) -> Result<(), CoreError> {
        Ok(self.db.revoke_permissions(addon_id)?)
    }

    pub fn favorites_add(
        &mut self,
        addon_id: &str,
        item_id: &str,
        title: &str,
    ) -> Result<(), CoreError> {
        Ok(self.db.add_favorite(&FavoriteRecord {
            addon_id: addon_id.to_string(),
            item_id: item_id.to_string(),
            title: title.to_string(),
            artwork_json: "{}".into(),
        })?)
    }

    pub fn favorites_list(&self) -> Result<Vec<FavoriteRecord>, CoreError> {
        Ok(self.db.list_favorites()?)
    }

    pub fn favorites_remove(&mut self, addon_id: &str, item_id: &str) -> Result<(), CoreError> {
        Ok(self.db.remove_favorite(addon_id, item_id)?)
    }

    pub fn db(&self) -> &Arc<Database> {
        &self.db
    }

    pub fn vault(&self) -> &Arc<dyn SecretVault> {
        &self.vault
    }
}

/// A BitTorrent v1 info hash as 40 lowercase hex digits, from hex or the
/// 32-character base32 form; `None` for anything else.
fn normalize_info_hash(hash: &str) -> Option<String> {
    let hash = hash.trim();
    if hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(hash.to_ascii_lowercase());
    }
    if hash.len() != 32 {
        return None;
    }
    let mut bits: u64 = 0;
    let mut count = 0;
    let mut bytes = Vec::with_capacity(20);
    for c in hash.bytes() {
        let value = match c.to_ascii_uppercase() {
            c @ b'A'..=b'Z' => c - b'A',
            c @ b'2'..=b'7' => c - b'2' + 26,
            _ => return None,
        };
        bits = (bits << 5) | u64::from(value);
        count += 5;
        if count >= 8 {
            count -= 8;
            bytes.push((bits >> count) as u8);
        }
    }
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn unexpected_response(response: &AddonResponse) -> CoreError {
    // `Debug` of every response type omits secret values (FR-084).
    CoreError::Provider(AddonError::new(
        AddonErrorCode::Malformed,
        format!("provider returned unexpected response: {response:?}"),
    ))
}

fn check_declared(manifest: &Manifest, credentials: &Credentials) -> Result<(), CoreError> {
    if credentials.0.is_empty() {
        return Err(CoreError::Provider(AddonError::new(
            AddonErrorCode::Malformed,
            "sign-in returned no credentials",
        )));
    }
    match credentials
        .0
        .keys()
        .find(|name| !manifest.permissions.secrets.contains(name))
    {
        Some(name) => Err(CoreError::Provider(AddonError::new(
            AddonErrorCode::PermissionDenied,
            format!("secret {name:?} not declared in manifest"),
        ))),
        None => Ok(()),
    }
}

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
