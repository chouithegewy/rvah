use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

use rvah_addon_host::Manifest;
use rvah_addon_sdk::Page;
use rvah_auth::{DeviceCodeFlow, DeviceCodeState, PollOutcome, Redactor};
use rvah_core::{AppCore, CoreError, ErrorClass};
use rvah_data::{AddonRecord, DiagnosticEvent, FavoriteRecord, WatchState};
use rvah_player::{PlayerBackend, PlayerState};

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerCommand {
    Play,
    Pause,
    Stop,
    Seek(f64),
    Volume(f32),
    /// Select a subtitle track by index (`None` = off).
    Subtitle(Option<usize>),
}

/// Everything the UI can ask of the domain core.
#[derive(Debug, Clone, PartialEq)]
pub enum CoreRequest {
    LoadHome,
    Browse {
        addon_id: String,
        path: Option<String>,
        cursor: Option<String>,
    },
    Search {
        addon_id: Option<String>,
        query: String,
    },
    Play {
        addon_id: String,
        item_id: String,
    },
    Player(PlayerCommand),
    TickPlayback,
    LoadPermissions {
        addon_id: String,
    },
    RevokePermissions {
        addon_id: String,
    },
    GrantPermissions {
        addon_id: String,
    },
    SetEnabled {
        addon_id: String,
        enabled: bool,
    },
    LoadSettings {
        addon_id: String,
    },
    SaveSetting {
        addon_id: String,
        key: String,
        value: String,
    },
    LoadDiagnostics,
    ExportDiagnostics {
        path: PathBuf,
    },
    AddFavorite {
        addon_id: String,
        item_id: String,
        title: String,
    },
    RemoveFavorite {
        addon_id: String,
        item_id: String,
    },
    StartAuth {
        addon_id: String,
    },
    PollAuth,
    /// Verify a package's signature and show its permission grant (UC-01).
    PreviewInstall {
        pkg: PathBuf,
        pubkey: PathBuf,
    },
    /// User approved the previewed grant: install (or update) the package.
    ConfirmInstall {
        pkg: PathBuf,
        pubkey: PathBuf,
    },
    Shutdown,
}

/// One editable setting: manifest schema merged with the stored value.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingEntry {
    pub key: String,
    pub value: String,
    pub value_type: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackSnapshot {
    pub state: PlayerState,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
}

/// Everything the core reports back to the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum CoreEvent {
    Home {
        addons: Vec<AddonRecord>,
        favorites: Vec<FavoriteRecord>,
        recents: Vec<WatchState>,
    },
    Page {
        addon_id: String,
        path: Option<String>,
        page: Page,
    },
    SearchResults {
        results: Vec<(String, Result<Page, String>)>,
    },
    PlaybackStarted {
        addon_id: String,
        item_id: String,
        url: String,
        /// Labels of the source's subtitle tracks, for the UI selector.
        subtitles: Vec<String>,
    },
    Playback(PlaybackSnapshot),
    Permissions {
        addon_id: String,
        granted_json: Option<String>,
    },
    Settings {
        addon_id: String,
        entries: Vec<SettingEntry>,
    },
    Diagnostics {
        events: Vec<DiagnosticEvent>,
    },
    Exported {
        path: PathBuf,
    },
    Auth {
        status: String,
        user_code: Option<String>,
        verification_url: Option<String>,
    },
    /// Signature-verified permission grant, awaiting the user's decision.
    InstallPreview {
        id: String,
        version: String,
        network: Vec<String>,
        storage_bytes: u64,
        secrets: Vec<String>,
    },
    Installed {
        id: String,
        version: String,
    },
    Failed {
        context: String,
        message: String,
        class: ErrorClass,
        retryable: bool,
    },
}

/// The UI's grip on the core thread.
pub struct CoreHandle {
    pub requests: Sender<CoreRequest>,
    pub events: Receiver<CoreEvent>,
}

/// Builds the runtime + host bridge for a freshly installed add-on so it is
/// usable without restarting the app. Args: addon id, declared capabilities.
pub type RuntimeFactory = Box<
    dyn Fn(
            &str,
            &[String],
        ) -> (
            Box<dyn rvah_addon_host::AddonRuntime>,
            std::sync::Arc<dyn rvah_addon_sdk::HostApi>,
        ) + Send,
>;

/// Runs `AppCore` on a dedicated thread so the UI never blocks on SQLite,
/// add-on invocations, or player IPC.
pub struct CoreService<B: PlayerBackend> {
    core: AppCore<B>,
    auth: Option<(String, DeviceCodeFlow)>,
    auth_polls: u32,
    events: Sender<CoreEvent>,
    runtime_factory: Option<RuntimeFactory>,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl<B: PlayerBackend + Send + 'static> CoreService<B> {
    pub fn spawn(core: AppCore<B>) -> CoreHandle {
        Self::spawn_with(core, None)
    }

    pub fn spawn_with(core: AppCore<B>, runtime_factory: Option<RuntimeFactory>) -> CoreHandle {
        let (req_tx, req_rx) = channel::<CoreRequest>();
        let (event_tx, event_rx) = channel::<CoreEvent>();
        std::thread::spawn(move || {
            let mut service = CoreService {
                core,
                auth: None,
                auth_polls: 0,
                events: event_tx,
                runtime_factory,
            };
            while let Ok(request) = req_rx.recv() {
                if matches!(request, CoreRequest::Shutdown) {
                    break;
                }
                service.handle(request);
            }
        });
        CoreHandle {
            requests: req_tx,
            events: event_rx,
        }
    }

    fn send(&self, event: CoreEvent) {
        let _ = self.events.send(event);
    }

    fn fail(&self, context: &str, err: CoreError) {
        self.send(CoreEvent::Failed {
            context: context.to_string(),
            message: err.to_string(),
            class: err.class(),
            retryable: err.retryable(),
        });
    }

    fn snapshot(&self) -> PlaybackSnapshot {
        PlaybackSnapshot {
            state: self.core.player.state(),
            position_secs: self.core.player.position_secs(),
            duration_secs: self.core.player.duration_secs(),
        }
    }

    fn handle(&mut self, request: CoreRequest) {
        match request {
            CoreRequest::LoadHome => self.load_home(),
            CoreRequest::Browse {
                addon_id,
                path,
                cursor,
            } => match self.core.browse(&addon_id, path.clone(), cursor) {
                Ok(page) => self.send(CoreEvent::Page {
                    addon_id,
                    path,
                    page,
                }),
                Err(e) => self.fail("browse", e),
            },
            CoreRequest::Search { addon_id, query } => {
                let results = match addon_id {
                    Some(id) => vec![(
                        id.clone(),
                        self.core.search(&id, &query).map_err(|e| e.to_string()),
                    )],
                    None => self
                        .core
                        .search_all(&query)
                        .into_iter()
                        .map(|(id, r)| (id, r.map_err(|e| e.to_string())))
                        .collect(),
                };
                self.send(CoreEvent::SearchResults { results });
            }
            CoreRequest::Play { addon_id, item_id } => {
                match self.core.resolve_and_play(&addon_id, &item_id) {
                    Ok(session) => {
                        let subtitles = session
                            .source
                            .subtitles
                            .iter()
                            .map(|t| t.label.clone().unwrap_or_else(|| t.lang.clone()))
                            .collect();
                        self.send(CoreEvent::PlaybackStarted {
                            addon_id: session.addon_id,
                            item_id: session.item_id,
                            url: session.source.url,
                            subtitles,
                        });
                        self.send(CoreEvent::Playback(self.snapshot()));
                    }
                    Err(e) => self.fail("play", e),
                }
            }
            CoreRequest::Player(cmd) => {
                let result = match cmd {
                    PlayerCommand::Play => self.core.player.play(),
                    PlayerCommand::Pause => self.core.player.pause(),
                    PlayerCommand::Stop => {
                        return match self.core.stop_playback() {
                            Ok(()) => self.send(CoreEvent::Playback(self.snapshot())),
                            Err(e) => self.fail("player", e),
                        };
                    }
                    PlayerCommand::Seek(secs) => self.core.player.seek(secs),
                    PlayerCommand::Volume(v) => self.core.player.set_volume(v),
                    PlayerCommand::Subtitle(index) => self.core.player.select_subtitle(index),
                };
                if let Err(e) = result {
                    self.fail("player", CoreError::Player(e));
                }
                self.send(CoreEvent::Playback(self.snapshot()));
            }
            CoreRequest::TickPlayback => match self.core.tick_playback() {
                Ok(_) => self.send(CoreEvent::Playback(self.snapshot())),
                Err(e) => self.fail("playback", e),
            },
            CoreRequest::LoadPermissions { addon_id } => self.load_permissions(&addon_id),
            CoreRequest::RevokePermissions { addon_id } => {
                if let Err(e) = self.core.revoke_permissions(&addon_id) {
                    return self.fail("permissions", e);
                }
                self.load_permissions(&addon_id);
            }
            CoreRequest::GrantPermissions { addon_id } => {
                if let Err(e) = self.core.grant_permissions(&addon_id) {
                    return self.fail("permissions", e);
                }
                self.load_permissions(&addon_id);
            }
            CoreRequest::SetEnabled { addon_id, enabled } => {
                let result = if enabled {
                    self.core.manager.enable(&addon_id)
                } else {
                    self.core.manager.disable(&addon_id)
                };
                match result {
                    Ok(()) => self.load_home(),
                    Err(e) => self.fail("addons", CoreError::Data(e.to_string())),
                }
            }
            CoreRequest::LoadSettings { addon_id } => self.load_settings(&addon_id),
            CoreRequest::SaveSetting {
                addon_id,
                key,
                value,
            } => {
                if let Err(e) = self.core.db().set_setting(&addon_id, &key, &value) {
                    return self.fail("settings", e.into());
                }
                self.load_settings(&addon_id);
            }
            CoreRequest::LoadDiagnostics => match self.core.db().recent_events(100) {
                Ok(events) => self.send(CoreEvent::Diagnostics { events }),
                Err(e) => self.fail("diagnostics", e.into()),
            },
            CoreRequest::ExportDiagnostics { path } => self.export_diagnostics(path),
            CoreRequest::AddFavorite {
                addon_id,
                item_id,
                title,
            } => match self.core.favorites_add(&addon_id, &item_id, &title) {
                Ok(()) => self.load_home(),
                Err(e) => self.fail("favorites", e),
            },
            CoreRequest::RemoveFavorite { addon_id, item_id } => {
                match self.core.favorites_remove(&addon_id, &item_id) {
                    Ok(()) => self.load_home(),
                    Err(e) => self.fail("favorites", e),
                }
            }
            CoreRequest::StartAuth { addon_id } => {
                // Simulated device-code issuance (UC-04); a real provider
                // integration supplies these values from its auth endpoint.
                let flow = DeviceCodeFlow::new(
                    "RVAH-1234",
                    "https://example.com/activate",
                    unix_now() + 300,
                    addon_id.clone(),
                    "oauth_token",
                );
                self.auth = Some((addon_id, flow));
                self.auth_polls = 0;
                self.send_auth_state();
            }
            CoreRequest::PollAuth => {
                let Some((_, flow)) = self.auth.as_mut() else {
                    return;
                };
                self.auth_polls += 1;
                // Simulated poller: authorize on the third poll.
                let authorize = self.auth_polls >= 3;
                let mut poller = move || {
                    if authorize {
                        PollOutcome::Authorized {
                            token: "simulated-token".into(),
                        }
                    } else {
                        PollOutcome::Pending
                    }
                };
                let vault = std::sync::Arc::clone(self.core.vault());
                if let Err(e) = flow.poll(unix_now(), &mut poller, vault.as_ref()) {
                    return self.fail("auth", CoreError::Data(e.to_string()));
                }
                self.send_auth_state();
            }
            CoreRequest::PreviewInstall { pkg, pubkey } => self.preview_install(&pkg, &pubkey),
            CoreRequest::ConfirmInstall { pkg, pubkey } => self.confirm_install(&pkg, &pubkey),
            CoreRequest::Shutdown => {}
        }
    }

    /// Load and check everything needed to trust a package file:
    /// pinned pubkey (hex), detached signature (`<pkg>.sig`, hex), signature
    /// verification — nothing is parsed before this passes (TC-SEC-001).
    fn verify_pkg(
        pkg: &std::path::Path,
        pubkey: &std::path::Path,
    ) -> Result<
        (
            rvah_addon_host::Manifest,
            [u8; 64],
            ed25519_dalek::VerifyingKey,
        ),
        String,
    > {
        let key_hex = std::fs::read_to_string(pubkey)
            .map_err(|e| format!("read {}: {e}", pubkey.display()))?;
        let key_bytes: [u8; 32] = hex::decode(key_hex.trim())
            .map_err(|e| format!("pubkey: {e}"))?
            .try_into()
            .map_err(|_| "pubkey must be 32 bytes".to_string())?;
        let key = ed25519_dalek::VerifyingKey::from_bytes(&key_bytes)
            .map_err(|e| format!("pubkey: {e}"))?;
        let sig_path = format!("{}.sig", pkg.display());
        let sig_hex =
            std::fs::read_to_string(&sig_path).map_err(|e| format!("read {sig_path}: {e}"))?;
        let sig: [u8; 64] = hex::decode(sig_hex.trim())
            .map_err(|e| format!("signature: {e}"))?
            .try_into()
            .map_err(|_| "signature must be 64 bytes".to_string())?;
        rvah_addon_host::verify_package(pkg, &sig, &key).map_err(|e| e.to_string())?;
        let manifest_bytes =
            rvah_addon_host::read_package_file(pkg, "manifest.toml").map_err(|e| e.to_string())?;
        let manifest =
            Manifest::parse(&String::from_utf8(manifest_bytes).map_err(|e| e.to_string())?)
                .map_err(|e| format!("invalid manifest: {e}"))?;
        Ok((manifest, sig, key))
    }

    fn preview_install(&self, pkg: &std::path::Path, pubkey: &std::path::Path) {
        match Self::verify_pkg(pkg, pubkey) {
            Ok((manifest, _, _)) => self.send(CoreEvent::InstallPreview {
                id: manifest.addon.id.clone(),
                version: manifest.addon.version.to_string(),
                network: manifest.permissions.network.clone(),
                storage_bytes: manifest.permissions.storage_bytes,
                secrets: manifest.permissions.secrets.clone(),
            }),
            Err(message) => self.fail("install", CoreError::Data(message)),
        }
    }

    fn confirm_install(&mut self, pkg: &std::path::Path, pubkey: &std::path::Path) {
        let (manifest, sig, key) = match Self::verify_pkg(pkg, pubkey) {
            Ok(parts) => parts,
            Err(message) => return self.fail("install", CoreError::Data(message)),
        };
        let exists = match self.core.db().get_addon(&manifest.addon.id) {
            Ok(rec) => rec.is_some(),
            Err(e) => return self.fail("install", e.into()),
        };
        let result = if exists {
            self.core
                .manager
                .update(pkg, &sig, &key, rvah_addon_host::InstallDecision::Approve)
        } else {
            self.core
                .manager
                .install(pkg, &sig, &key, rvah_addon_host::InstallDecision::Approve)
        };
        match result {
            Ok(installed) => {
                // Make the new add-on usable immediately.
                if let Some(factory) = &self.runtime_factory {
                    if let Ok(Some(rec)) = self.core.db().get_addon(&installed.id) {
                        let (runtime, host) = factory(&rec.id, &rec.capabilities);
                        self.core.register_runtime(rec.id, runtime, host);
                    }
                }
                self.send(CoreEvent::Installed {
                    id: installed.id,
                    version: installed.version,
                });
                self.load_home();
            }
            Err(e) => self.fail("install", CoreError::Data(e.to_string())),
        }
    }

    fn send_auth_state(&self) {
        let Some((_, flow)) = &self.auth else { return };
        let event = match flow.state() {
            DeviceCodeState::Pending {
                user_code,
                verification_url,
                ..
            } => CoreEvent::Auth {
                status: "pending".into(),
                user_code: Some(user_code.clone()),
                verification_url: Some(verification_url.clone()),
            },
            DeviceCodeState::Authorized => CoreEvent::Auth {
                status: "authorized".into(),
                user_code: None,
                verification_url: None,
            },
            DeviceCodeState::Expired => CoreEvent::Auth {
                status: "expired".into(),
                user_code: None,
                verification_url: None,
            },
            DeviceCodeState::Denied => CoreEvent::Auth {
                status: "denied".into(),
                user_code: None,
                verification_url: None,
            },
        };
        self.send(event);
    }

    fn load_home(&self) {
        let db = self.core.db();
        match (
            db.list_addons(),
            db.list_favorites(),
            db.recent_watch_states(10),
        ) {
            (Ok(addons), Ok(favorites), Ok(recents)) => self.send(CoreEvent::Home {
                addons,
                favorites,
                recents,
            }),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => self.fail("home", e.into()),
        }
    }

    fn load_permissions(&self, addon_id: &str) {
        match self.core.db().get_permissions_json(addon_id) {
            Ok(granted_json) => self.send(CoreEvent::Permissions {
                addon_id: addon_id.to_string(),
                granted_json,
            }),
            Err(e) => self.fail("permissions", e.into()),
        }
    }

    fn load_settings(&self, addon_id: &str) {
        let rec = match self.core.db().get_addon(addon_id) {
            Ok(Some(rec)) => rec,
            Ok(None) => {
                return self.fail("settings", CoreError::AddonNotFound(addon_id.to_string()))
            }
            Err(e) => return self.fail("settings", e.into()),
        };
        let manifest_path = self
            .core
            .manager
            .install_dir(addon_id, &rec.version)
            .join("manifest.toml");
        let manifest = std::fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|text| Manifest::parse(&text).ok());
        let mut entries = Vec::new();
        if let Some(manifest) = manifest {
            let defaults = manifest.default_settings();
            for (key, spec) in &manifest.settings {
                let stored = self.core.db().get_setting(addon_id, key).ok().flatten();
                entries.push(SettingEntry {
                    key: key.clone(),
                    value: stored
                        .or_else(|| defaults.get(key).cloned())
                        .unwrap_or_default(),
                    value_type: spec.r#type.clone(),
                    options: spec.r#enum.clone().unwrap_or_default(),
                });
            }
        }
        self.send(CoreEvent::Settings {
            addon_id: addon_id.to_string(),
            entries,
        });
    }

    /// Diagnostics bundle export (FR-103, Phase 2 deliverable): JSON file with
    /// installed add-ons and recent events, pattern-scrubbed once more on the
    /// way out.
    fn export_diagnostics(&self, path: PathBuf) {
        let db = self.core.db();
        let (addons, events) = match (db.list_addons(), db.recent_events(200)) {
            (Ok(a), Ok(e)) => (a, e),
            (Err(e), _) | (_, Err(e)) => return self.fail("export", e.into()),
        };
        let redactor = Redactor::new();
        let bundle = serde_json::json!({
            "exported_at": unix_now(),
            "addons": addons.iter().map(|a| serde_json::json!({
                "id": a.id, "version": a.version, "status": a.status,
                "capabilities": a.capabilities,
            })).collect::<Vec<_>>(),
            "events": events.iter().map(|e| serde_json::json!({
                "level": e.level, "addon_id": e.addon_id, "code": e.code,
                "message": redactor.redact(&e.message),
            })).collect::<Vec<_>>(),
        });
        let result = serde_json::to_vec_pretty(&bundle)
            .map_err(|e| e.to_string())
            .and_then(|bytes| std::fs::write(&path, bytes).map_err(|e| e.to_string()));
        match result {
            Ok(()) => self.send(CoreEvent::Exported { path }),
            Err(e) => self.fail("export", CoreError::Data(e)),
        }
    }
}
