use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rvah_addon_host::{
    AddonManager, AddonOp, AddonResponse, AddonRuntime, InvocationCtx, Manifest,
};
use rvah_addon_sdk::{
    AddonError, AddonErrorCode, Filter, HostApi, Page, PlayableSource, ResolveResult,
};
use rvah_auth::SecretVault;
use rvah_data::{AddonStatus, Database, DiagnosticEvent, FavoriteRecord, WatchState};
use rvah_player::{
    validate_source_opts, PlayerBackend, PlayerController, PlayerEvent, ProgressTracker,
    SourceRejection,
};

use crate::errors::CoreError;
use crate::nav::NavHistory;

pub const DEFAULT_INVOKE_TIMEOUT: Duration = Duration::from_secs(2);
/// Consecutive failures before an add-on is quarantined (NFR Reliability).
pub const QUARANTINE_THRESHOLD: u32 = 3;
const WATCHED_THRESHOLD: f32 = 0.9;
const PROGRESS_DEBOUNCE_SECS: f64 = 5.0;

/// An in-flight playback started by [`AppCore::resolve_and_play`].
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackSession {
    pub addon_id: String,
    pub item_id: String,
    pub source: PlayableSource,
}

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
    session: Option<PlaybackSession>,
    progress: ProgressTracker,
}

impl<B: PlayerBackend> AppCore<B> {
    pub fn new(
        manager: AddonManager,
        db: Arc<Database>,
        vault: Arc<dyn SecretVault>,
        backend: B,
    ) -> Self {
        Self {
            manager,
            db,
            vault,
            addons: HashMap::new(),
            player: PlayerController::new(backend),
            nav: NavHistory::new(),
            invoke_timeout: DEFAULT_INVOKE_TIMEOUT,
            failures: HashMap::new(),
            session: None,
            progress: ProgressTracker::new(WATCHED_THRESHOLD, PROGRESS_DEBOUNCE_SECS),
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

    pub fn session(&self) -> Option<&PlaybackSession> {
        self.session.as_ref()
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
        match addon.runtime.invoke(op, &ctx) {
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
        let page = Self::expect_page(self.invoke(
            addon_id,
            AddonOp::List {
                path: path.clone(),
                cursor,
            },
        )?)?;
        self.nav
            .push(format!("{addon_id}:{}", path.as_deref().unwrap_or("/")));
        Ok(page)
    }

    pub fn search(&mut self, addon_id: &str, query: &str) -> Result<Page, CoreError> {
        let op = AddonOp::Search {
            query: query.to_string(),
            filters: Vec::<Filter>::new(),
            cursor: None,
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
                let result = self.search(&id, query);
                (id, result)
            })
            .collect()
    }

    /// Resolve an item, validate its sources, and start playback at the
    /// persisted resume position (FR-060, FR-065, TC-PLY-008).
    pub fn resolve_and_play(
        &mut self,
        addon_id: &str,
        item_id: &str,
    ) -> Result<PlaybackSession, CoreError> {
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
        // DASH passes validation only when the active backend can play it.
        let allow_dash = self.player.backend().supports_dash();
        let mut last_rejection: Option<SourceRejection> = None;
        let source = sources
            .into_iter()
            .find(|s| match validate_source_opts(s, now, allow_dash) {
                Ok(()) => true,
                Err(r) => {
                    last_rejection = Some(r);
                    false
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

        self.player
            .load_source(&source, start_at)
            .map_err(CoreError::Player)?;
        self.progress = ProgressTracker::new(WATCHED_THRESHOLD, PROGRESS_DEBOUNCE_SECS);
        let session = PlaybackSession {
            addon_id: addon_id.to_string(),
            item_id: item_id.to_string(),
            source,
        };
        self.session = Some(session.clone());
        Ok(session)
    }

    /// Pump player events and persist debounced watch state (FR-065). Playback
    /// position doubles as the debounce clock so tests stay deterministic.
    pub fn tick_playback(&mut self) -> Result<Vec<PlayerEvent>, CoreError> {
        let events = self.player.handle_events();
        let Some(session) = self.session.clone() else {
            return Ok(events);
        };
        let duration = self.player.duration_secs().unwrap_or(0.0);
        for event in &events {
            match event {
                PlayerEvent::Progress { position_secs } => {
                    if let Some(update) =
                        self.progress
                            .on_progress(*position_secs, duration, *position_secs)
                    {
                        self.db.set_watch_state(&WatchState {
                            addon_id: session.addon_id.clone(),
                            item_id: session.item_id.clone(),
                            position_secs: update.position_secs,
                            duration_secs: update.duration_secs,
                            watched: update.watched,
                            last_played_at: unix_now(),
                        })?;
                    }
                }
                PlayerEvent::Ended => {
                    self.db.set_watch_state(&WatchState {
                        addon_id: session.addon_id.clone(),
                        item_id: session.item_id.clone(),
                        position_secs: duration,
                        duration_secs: duration,
                        watched: true,
                        last_played_at: unix_now(),
                    })?;
                    self.session = None;
                }
                _ => {}
            }
        }
        Ok(events)
    }

    pub fn stop_playback(&mut self) -> Result<(), CoreError> {
        self.player.stop().map_err(CoreError::Player)?;
        self.session = None;
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

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
