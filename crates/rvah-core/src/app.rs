use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rvah_addon_host::{
    AddonManager, AddonOp, AddonResponse, AddonRuntime, InvocationCtx, Manifest,
};
use rvah_addon_sdk::{AddonError, AddonErrorCode, Filter, HostApi, Page, ResolveResult};
use rvah_auth::SecretVault;
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
/// Consecutive failures before an add-on is quarantined (NFR Reliability).
pub const QUARANTINE_THRESHOLD: u32 = 3;

struct RegisteredAddon {
    runtime: Box<dyn AddonRuntime>,
    host: Arc<dyn HostApi>,
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
    failures: HashMap<String, u32>,
    playback: Arc<Mutex<PlaybackTracker>>,
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
            failures: HashMap::new(),
            playback,
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
    /// the add-on (TC-NFR-005). The host always survives (FR-023).
    fn invoke(&mut self, addon_id: &str, op: AddonOp) -> Result<AddonResponse, CoreError> {
        self.ensure_enabled(addon_id)?;
        let addon = self
            .addons
            .get(addon_id)
            .ok_or_else(|| CoreError::AddonNotFound(addon_id.to_string()))?;
        let ctx = InvocationCtx {
            timeout: self.invoke_timeout,
            host: Arc::clone(&addon.host),
        };
        let op_name = match &op {
            AddonOp::List { .. } => "list",
            AddonOp::Search { .. } => "search",
            AddonOp::Resolve { .. } => "resolve",
            AddonOp::AccountStatus => "account_status",
        };
        let started = std::time::Instant::now();
        let result = addon.runtime.invoke(op, &ctx);
        crate::debug_log!(
            "{addon_id} {op_name}: {} in {:.3}s",
            if result.is_ok() { "ok" } else { "error" },
            started.elapsed().as_secs_f64()
        );
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
            other => Err(CoreError::Provider(AddonError::new(
                AddonErrorCode::Malformed,
                format!("provider returned unexpected response: {other:?}"),
            ))),
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
        Self::expect_page(self.invoke(addon_id, op)?)
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

    /// Re-approve the permissions currently declared by the installed
    /// manifest (FR-046).
    pub fn grant_permissions(&mut self, addon_id: &str) -> Result<(), CoreError> {
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
        let manifest =
            Manifest::parse(&text).map_err(|e| CoreError::Data(format!("manifest: {e}")))?;
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

pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
