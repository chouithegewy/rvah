//! Typed UniFFI surface consumed by the Kotlin/Compose TV application.
//! Blocking methods are intentionally synchronous; Kotlin calls them from
//! `Dispatchers.IO` so Rust owns policy while Media3 owns playback surfaces.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rvah_addon_host::{
    AddonManager, AddonRuntime, BridgedHostApi, CapabilityGated, Manifest, WasmiLimits,
    WasmiRuntime,
};
use rvah_addon_sdk::{AccountStatus, Page, PageItem, Protocol, SubtitleFormat};
use rvah_auth::{DeviceCodeState, SecretVault, VaultError};
use rvah_core::{
    lock_tracker, AppCore, PlaybackCapabilities, PlaybackLifecycleEvent, PlaybackSessionId,
    PlaybackTracker, PreparedPlayback,
};
use rvah_data::Database;
use rvah_player::MockPlayer;

uniffi::setup_scaffolding!();

const REQUEST_QUOTA: u32 = 512;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum AndroidCoreError {
    #[error("storage: {reason}")]
    Storage { reason: String },
    #[error("add-on: {reason}")]
    Addon { reason: String },
    #[error("playback: {reason}")]
    Playback { reason: String },
    /// The add-on needs an account: the TV offers Link account here.
    #[error("sign-in required: {reason}")]
    SignInRequired { reason: String },
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SecretRead {
    pub value: Option<Vec<u8>>,
    pub error: Option<String>,
}

/// Kotlin implements this with one AES-GCM key in Android Keystore. Rust
/// supplies only namespaced keys and plaintext bytes at this narrow boundary.
#[uniffi::export(callback_interface)]
pub trait AndroidSecretStore: Send + Sync {
    fn put(&self, key: String, value: Vec<u8>) -> Option<String>;
    fn get(&self, key: String) -> SecretRead;
    fn delete(&self, key: String) -> Option<String>;
    fn list(&self, prefix: String) -> Vec<String>;
}

struct KeystoreVault {
    store: Box<dyn AndroidSecretStore>,
}

fn secret_key(addon_id: &str, name: &str) -> String {
    format!("rvah/{addon_id}/{name}")
}

impl SecretVault for KeystoreVault {
    fn set(&self, addon_id: &str, name: &str, value: &str) -> Result<(), VaultError> {
        self.store
            .put(secret_key(addon_id, name), value.as_bytes().to_vec())
            .map_or(Ok(()), |error| Err(VaultError::Crypto(error)))
    }

    fn get(&self, addon_id: &str, name: &str) -> Result<Option<String>, VaultError> {
        let result = self.store.get(secret_key(addon_id, name));
        if let Some(error) = result.error {
            return Err(VaultError::Crypto(error));
        }
        result
            .value
            .map(String::from_utf8)
            .transpose()
            .map_err(|error| VaultError::Crypto(error.to_string()))
    }

    fn delete(&self, addon_id: &str, name: &str) -> Result<(), VaultError> {
        self.store
            .delete(secret_key(addon_id, name))
            .map_or(Ok(()), |error| Err(VaultError::Crypto(error)))
    }

    fn list(&self, addon_id: &str) -> Result<Vec<String>, VaultError> {
        let prefix = format!("rvah/{addon_id}/");
        Ok(self
            .store
            .list(prefix.clone())
            .into_iter()
            .filter_map(|key| key.strip_prefix(&prefix).map(str::to_owned))
            .collect())
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AddonRecord {
    pub id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
    /// `enabled`, `disabled`, or `quarantined`.
    pub status: String,
    /// Manifest capabilities (`catalog`, `search`, `resolve`, ...).
    pub capabilities: Vec<String>,
}

/// What the user is asked to approve before an install (FR-005). Nothing is
/// written until `install_signed` is called with `permission_hash`.
#[derive(Debug, Clone, uniffi::Record)]
pub struct InstallPreview {
    pub id: String,
    pub name: String,
    pub version: String,
    pub capabilities: Vec<String>,
    pub network: Vec<String>,
    pub storage_bytes: u64,
    pub secrets: Vec<String>,
    /// The trusted key (hex) whose signature verified.
    pub signer_key_hex: String,
    pub permission_hash: String,
    pub is_update: bool,
}

/// Whether a declared secret has a value. Values never cross back to Kotlin.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SecretStatus {
    pub name: String,
    pub is_set: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SettingRecord {
    pub key: String,
    pub value: String,
    pub value_type: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct DiagnosticRecord {
    pub level: String,
    pub addon_id: Option<String>,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum CatalogEntry {
    Directory {
        id: String,
        title: String,
        subtitle: Option<String>,
        path: String,
        poster: Option<String>,
    },
    /// Artwork URLs are https on a host the add-on declared (core rule).
    Media {
        id: String,
        title: String,
        synopsis: Option<String>,
        year: Option<u16>,
        rating: Option<f32>,
        poster: Option<String>,
        fanart: Option<String>,
    },
}

/// A details page (capability `details`).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ItemDetailsRecord {
    pub id: String,
    pub title: String,
    pub year: Option<u16>,
    pub synopsis: Option<String>,
    pub runtime_secs: Option<u32>,
    pub rating: Option<f32>,
    pub poster: Option<String>,
    pub fanart: Option<String>,
    pub tagline: Option<String>,
    pub genres: Vec<String>,
    /// ISO 8601 date.
    pub released: Option<String>,
    pub trailers: Vec<TrailerLink>,
    pub ratings: Vec<RatingRecord>,
}

/// A trailer page, built here from the site's video key.
#[derive(Debug, Clone, uniffi::Record)]
pub struct TrailerLink {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct RatingRecord {
    pub source: String,
    pub value: String,
}

impl From<rvah_addon_sdk::ItemDetails> for ItemDetailsRecord {
    fn from(details: rvah_addon_sdk::ItemDetails) -> Self {
        let item = details.item;
        ItemDetailsRecord {
            id: item.id,
            title: item.title,
            year: item.year,
            synopsis: item.synopsis,
            runtime_secs: item.runtime_secs,
            rating: item.rating,
            poster: item.artwork.poster,
            fanart: item.artwork.fanart,
            tagline: details.tagline,
            genres: details.genres,
            released: details.released,
            trailers: details
                .trailers
                .into_iter()
                .map(|trailer| TrailerLink {
                    url: match trailer.site {
                        rvah_addon_sdk::VideoSite::YouTube => {
                            format!("https://www.youtube.com/watch?v={}", trailer.key)
                        }
                    },
                    name: trailer.name,
                })
                .collect(),
            ratings: details
                .ratings
                .into_iter()
                .map(|rating| RatingRecord {
                    source: rating.source,
                    value: rating.value,
                })
                .collect(),
        }
    }
}

/// What the TV shows to finish linking an account by device code (UC-04).
#[derive(Debug, Clone, uniffi::Record)]
pub struct SignInPrompt {
    pub user_code: String,
    pub verification_url: String,
    /// The provider's minimum wait between polls.
    pub interval_secs: u32,
    /// Unix seconds after which the code stops working.
    pub expires_at_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum SignInState {
    /// The user has not entered the code yet.
    Waiting,
    /// Credentials are in the vault.
    Linked,
    Expired,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum AccountState {
    SignedOut,
    SignedIn { display_name: Option<String> },
    Expired,
}

/// One page of a catalog or search listing. Pass `next_cursor` back to get
/// the page after it; `None` means this was the last page (FR-024).
#[derive(Debug, Clone, uniffi::Record)]
pub struct CatalogPage {
    pub entries: Vec<CatalogEntry>,
    pub next_cursor: Option<String>,
}

impl From<Page> for CatalogPage {
    fn from(page: Page) -> Self {
        Self {
            entries: page.items.into_iter().map(catalog_entry).collect(),
            next_cursor: page.cursor_next,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct HeaderRecord {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum NativeProtocol {
    Progressive,
    Hls,
    Dash,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum NativeSubtitleFormat {
    WebVtt,
    SubRip,
    Ttml,
    Ssa,
    Ass,
    Unknown,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct NativeSubtitle {
    pub url: String,
    pub language: String,
    pub label: Option<String>,
    pub format: NativeSubtitleFormat,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct NativePreparedPlayback {
    pub session_id: u64,
    pub url: String,
    pub protocol: NativeProtocol,
    pub headers: Vec<HeaderRecord>,
    pub resume_position_ms: u64,
    pub subtitles: Vec<NativeSubtitle>,
    pub title: String,
    pub subtitle: Option<String>,
}

impl From<PreparedPlayback> for NativePreparedPlayback {
    fn from(value: PreparedPlayback) -> Self {
        let protocol = match value.source.protocol {
            Protocol::Hls => NativeProtocol::Hls,
            Protocol::Dash => NativeProtocol::Dash,
            Protocol::Http | Protocol::Https => NativeProtocol::Progressive,
            Protocol::File => unreachable!("file sources are rejected by the core"),
        };
        let subtitles = value
            .subtitles
            .into_iter()
            .map(|track| {
                let format = match track.effective_format() {
                    Some(SubtitleFormat::WebVtt) => NativeSubtitleFormat::WebVtt,
                    Some(SubtitleFormat::SubRip) => NativeSubtitleFormat::SubRip,
                    Some(SubtitleFormat::Ttml) => NativeSubtitleFormat::Ttml,
                    Some(SubtitleFormat::Ssa) => NativeSubtitleFormat::Ssa,
                    Some(SubtitleFormat::Ass) => NativeSubtitleFormat::Ass,
                    None => NativeSubtitleFormat::Unknown,
                };
                NativeSubtitle {
                    url: track.url,
                    language: track.lang,
                    label: track.label,
                    format,
                }
            })
            .collect();
        Self {
            session_id: value.session_id.0,
            url: value.source.url,
            protocol,
            headers: value
                .source
                .headers
                .into_iter()
                .map(|(name, value)| HeaderRecord { name, value })
                .collect(),
            resume_position_ms: (value.resume_position_secs.max(0.0) * 1000.0) as u64,
            subtitles,
            title: value.display.title,
            subtitle: value.display.subtitle,
        }
    }
}

impl NativePreparedPlayback {
    /// Prefer the catalog title the user picked over the resolver's item id.
    fn with_title(mut self, title: Option<String>) -> Self {
        if let Some(title) = title.filter(|t| !t.trim().is_empty()) {
            self.title = title;
        }
        self
    }
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum NativePlaybackEvent {
    Preparing,
    Buffering,
    Playing {
        position_ms: u64,
        duration_ms: Option<u64>,
    },
    Paused {
        position_ms: u64,
        duration_ms: Option<u64>,
    },
    Ended {
        position_ms: u64,
        duration_ms: Option<u64>,
    },
    Error {
        message: String,
    },
}

fn seconds(milliseconds: u64) -> f64 {
    milliseconds as f64 / 1000.0
}

fn duration(value: Option<u64>) -> Option<f64> {
    value.map(seconds)
}

fn lifecycle(event: NativePlaybackEvent) -> PlaybackLifecycleEvent {
    match event {
        NativePlaybackEvent::Preparing => PlaybackLifecycleEvent::Preparing,
        NativePlaybackEvent::Buffering => PlaybackLifecycleEvent::Buffering,
        NativePlaybackEvent::Playing {
            position_ms,
            duration_ms,
        } => PlaybackLifecycleEvent::Playing {
            position_secs: seconds(position_ms),
            duration_secs: duration(duration_ms),
        },
        NativePlaybackEvent::Paused {
            position_ms,
            duration_ms,
        } => PlaybackLifecycleEvent::Paused {
            position_secs: seconds(position_ms),
            duration_secs: duration(duration_ms),
        },
        NativePlaybackEvent::Ended {
            position_ms,
            duration_ms,
        } => PlaybackLifecycleEvent::Ended {
            position_secs: seconds(position_ms),
            duration_secs: duration(duration_ms),
        },
        NativePlaybackEvent::Error { message } => PlaybackLifecycleEvent::Error { message },
    }
}

type AndroidAppCore = AppCore<MockPlayer>;
type RuntimePair = (Box<dyn AddonRuntime>, Arc<dyn rvah_addon_sdk::HostApi>);

/// Locking: `core` serializes add-on invocations (browse, search, resolve,
/// install), which can block on the network for up to the invoke timeout.
/// Everything a TV UI needs while that happens — native-player callbacks
/// (`playback`), and database-only reads and writes (`db`, `manager`) — uses
/// its own handle and never waits for `core`.
#[derive(uniffi::Object)]
pub struct AndroidCore {
    core: Mutex<AndroidAppCore>,
    playback: Arc<Mutex<PlaybackTracker>>,
    db: Arc<Database>,
    vault: Arc<dyn SecretVault>,
    manager: AddonManager,
}

impl AndroidCore {
    fn installed_manifest(&self, addon_id: &str) -> Result<Manifest, AndroidCoreError> {
        let addon = self
            .db
            .get_addon(addon_id)
            .map_err(storage_error)?
            .ok_or_else(|| AndroidCoreError::Addon {
                reason: format!("add-on not installed: {addon_id}"),
            })?;
        read_manifest(&self.manager.install_dir(addon_id, &addon.version))
    }

    /// A panic in one call must not wedge every later call across the FFI
    /// boundary, so lock poisoning is ignored.
    fn core(&self) -> MutexGuard<'_, AndroidAppCore> {
        self.core
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[uniffi::export]
impl AndroidCore {
    #[uniffi::constructor]
    pub fn open(
        data_dir: String,
        secrets: Box<dyn AndroidSecretStore>,
    ) -> Result<Arc<Self>, AndroidCoreError> {
        let data_dir = std::path::PathBuf::from(data_dir);
        std::fs::create_dir_all(&data_dir).map_err(|error| AndroidCoreError::Storage {
            reason: error.to_string(),
        })?;
        let db = Arc::new(
            Database::open(&data_dir.join("rvah.sqlite")).map_err(|error| {
                AndroidCoreError::Storage {
                    reason: error.to_string(),
                }
            })?,
        );
        let manager = AddonManager::new(Arc::clone(&db), data_dir.join("addons"));
        let vault: Arc<dyn SecretVault> = Arc::new(KeystoreVault { store: secrets });
        let mut core = AppCore::new(
            AddonManager::new(Arc::clone(&db), data_dir.join("addons")),
            Arc::clone(&db),
            Arc::clone(&vault),
            MockPlayer::new(),
        );
        core.invoke_timeout = Duration::from_secs(30);
        for addon in db
            .list_addons()
            .map_err(|error| AndroidCoreError::Storage {
                reason: error.to_string(),
            })?
        {
            let dir = core.manager.install_dir(&addon.id, &addon.version);
            match load_wasmi_runtime(&dir, &db, &vault) {
                Ok((runtime, host)) => core.register_runtime(addon.id, runtime, host),
                // Keep starting; the add-on simply stays unusable, and the
                // reason is visible on the Diagnostics screen.
                Err(error) => {
                    let _ = db.log_event(&rvah_data::DiagnosticEvent {
                        level: "error".into(),
                        addon_id: Some(addon.id),
                        code: "runtime_load".into(),
                        message: error.to_string(),
                    });
                }
            }
        }
        Ok(Arc::new(Self {
            playback: core.playback_tracker(),
            core: Mutex::new(core),
            db,
            vault,
            manager,
        }))
    }

    pub fn addons(&self) -> Result<Vec<AddonRecord>, AndroidCoreError> {
        let addons = self.db.list_addons().map_err(storage_error)?;
        Ok(addons
            .into_iter()
            .map(|addon| {
                let manifest = read_manifest(&self.manager.install_dir(&addon.id, &addon.version));
                AddonRecord {
                    name: manifest
                        .as_ref()
                        .map(|manifest| manifest.addon.name.clone())
                        .unwrap_or_else(|_| addon.id.clone()),
                    capabilities: manifest.as_ref().map(capability_names).unwrap_or_default(),
                    enabled: addon.status == rvah_data::AddonStatus::Enabled,
                    status: match addon.status {
                        rvah_data::AddonStatus::Enabled => "enabled",
                        rvah_data::AddonStatus::Disabled => "disabled",
                        rvah_data::AddonStatus::Quarantined => "quarantined",
                    }
                    .into(),
                    id: addon.id,
                    version: addon.version,
                }
            })
            .collect())
    }

    pub fn settings(&self, addon_id: String) -> Result<Vec<SettingRecord>, AndroidCoreError> {
        let manifest = self.installed_manifest(&addon_id)?;
        let defaults = manifest.default_settings();
        manifest
            .settings
            .iter()
            .map(|(key, spec)| {
                let value = self
                    .db
                    .get_setting(&addon_id, key)
                    .map_err(storage_error)?
                    .or_else(|| defaults.get(key).cloned())
                    .unwrap_or_default();
                Ok(SettingRecord {
                    key: key.clone(),
                    value,
                    value_type: spec.r#type.clone(),
                    options: spec.r#enum.clone().unwrap_or_default(),
                })
            })
            .collect()
    }

    /// Store a setting the add-on's manifest declares, checked against its
    /// declared type and options.
    pub fn save_setting(
        &self,
        addon_id: String,
        key: String,
        value: String,
    ) -> Result<(), AndroidCoreError> {
        let manifest = self.installed_manifest(&addon_id)?;
        let spec = manifest
            .settings
            .get(&key)
            .ok_or_else(|| AndroidCoreError::Addon {
                reason: format!("{addon_id} declares no setting {key}"),
            })?;
        let valid = match (&spec.r#enum, spec.r#type.as_str()) {
            (Some(options), _) => options.contains(&value),
            (None, "bool") => value == "true" || value == "false",
            (None, "int" | "integer") => value.parse::<i64>().is_ok(),
            (None, "float" | "number") => value.parse::<f64>().is_ok(),
            (None, _) => true,
        };
        if !valid {
            return Err(AndroidCoreError::Addon {
                reason: format!("invalid value for {key}"),
            });
        }
        self.db
            .set_setting(&addon_id, &key, &value)
            .map_err(storage_error)
    }

    pub fn diagnostics(&self, limit: u32) -> Result<Vec<DiagnosticRecord>, AndroidCoreError> {
        self.db
            .recent_events(limit.min(500))
            .map(|events| {
                events
                    .into_iter()
                    .map(|event| DiagnosticRecord {
                        level: event.level,
                        addon_id: event.addon_id,
                        code: event.code,
                        message: event.message,
                    })
                    .collect()
            })
            .map_err(storage_error)
    }

    /// Verify a package against the build's trusted keys and describe what
    /// installing it would grant. Installs nothing.
    pub fn preview_install(
        &self,
        package_path: String,
        trusted_keys_hex: Vec<String>,
    ) -> Result<InstallPreview, AndroidCoreError> {
        let package = std::path::PathBuf::from(package_path);
        let (_, _, signer_key_hex) = verify_trusted(&package, &trusted_keys_hex)?;
        let manifest = package_manifest(&package)?;
        let is_update = self
            .db
            .get_addon(&manifest.addon.id)
            .map_err(storage_error)?
            .is_some();
        Ok(InstallPreview {
            id: manifest.addon.id.clone(),
            name: manifest.addon.name.clone(),
            version: manifest.addon.version.to_string(),
            capabilities: capability_names(&manifest),
            network: manifest.permissions.network.clone(),
            storage_bytes: manifest.permissions.storage_bytes,
            secrets: manifest.permissions.secrets.clone(),
            signer_key_hex,
            permission_hash: manifest.permission_hash(),
            is_update,
        })
    }

    /// Install (or update) a package the user approved via
    /// [`AndroidCore::preview_install`]. The signature is re-verified against
    /// the trusted keys and the package's permissions must still hash to
    /// `approved_permission_hash`, so a swapped package cannot ride on an
    /// earlier approval.
    pub fn install_signed(
        &self,
        package_path: String,
        trusted_keys_hex: Vec<String>,
        approved_permission_hash: String,
    ) -> Result<AddonRecord, AndroidCoreError> {
        let package = std::path::PathBuf::from(package_path);
        let (key, signature, _) = verify_trusted(&package, &trusted_keys_hex)?;
        let manifest = package_manifest(&package)?;
        if manifest.permission_hash() != approved_permission_hash {
            return Err(AndroidCoreError::Addon {
                reason: "the package's permissions differ from what was approved".into(),
            });
        }
        let exists = self
            .db
            .get_addon(&manifest.addon.id)
            .map_err(storage_error)?
            .is_some();
        let approve = rvah_addon_host::InstallDecision::Approve;
        let installed = if exists {
            self.manager.update(&package, &signature, &key, approve)
        } else {
            self.manager.install(&package, &signature, &key, approve)
        }
        .map_err(|error| AndroidCoreError::Addon {
            reason: error.to_string(),
        })?;
        let dir = self.manager.install_dir(&installed.id, &installed.version);
        let (runtime, host) = load_wasmi_runtime(&dir, &self.db, &self.vault)?;
        self.core()
            .register_runtime(installed.id.clone(), runtime, host);
        self.addons()?
            .into_iter()
            .find(|addon| addon.id == installed.id)
            .ok_or_else(|| AndroidCoreError::Storage {
                reason: format!("{} missing after install", installed.id),
            })
    }

    /// Re-enable a disabled or quarantined add-on (e.g. after fixing its
    /// credentials).
    pub fn enable_addon(&self, addon_id: String) -> Result<(), AndroidCoreError> {
        self.manager
            .enable(&addon_id)
            .map_err(|error| AndroidCoreError::Addon {
                reason: error.to_string(),
            })
    }

    /// Store a secret the add-on's manifest declares.
    pub fn set_secret(
        &self,
        addon_id: String,
        name: String,
        value: String,
    ) -> Result<(), AndroidCoreError> {
        let manifest = self.installed_manifest(&addon_id)?;
        if !manifest.permissions.secrets.contains(&name) {
            return Err(AndroidCoreError::Addon {
                reason: format!("{addon_id} declares no secret {name}"),
            });
        }
        let value = value.trim();
        if value.is_empty() {
            return Err(AndroidCoreError::Addon {
                reason: "secret value is empty".into(),
            });
        }
        self.vault
            .set(&addon_id, &name, value)
            .map_err(|error| AndroidCoreError::Storage {
                reason: error.to_string(),
            })
    }

    pub fn secret_status(&self, addon_id: String) -> Result<Vec<SecretStatus>, AndroidCoreError> {
        let manifest = self.installed_manifest(&addon_id)?;
        let stored = self
            .vault
            .list(&addon_id)
            .map_err(|error| AndroidCoreError::Storage {
                reason: error.to_string(),
            })?;
        Ok(manifest
            .permissions
            .secrets
            .iter()
            .map(|name| SecretStatus {
                name: name.clone(),
                is_set: stored.contains(name),
            })
            .collect())
    }

    /// `cursor` is the previous page's `next_cursor`, or `None` for the first.
    pub fn browse(
        &self,
        addon_id: String,
        path: Option<String>,
        cursor: Option<String>,
    ) -> Result<CatalogPage, AndroidCoreError> {
        self.core()
            .browse(&addon_id, path, cursor)
            .map(CatalogPage::from)
            .map_err(playback_error)
    }

    /// `cursor` is the previous page's `next_cursor`, or `None` for the first.
    pub fn search(
        &self,
        addon_id: String,
        query: String,
        cursor: Option<String>,
    ) -> Result<CatalogPage, AndroidCoreError> {
        self.core()
            .search(&addon_id, &query, cursor)
            .map(CatalogPage::from)
            .map_err(playback_error)
    }

    /// `title` is the catalog title the user selected, shown in the media
    /// session instead of the resolver's item id.
    pub fn prepare_playback(
        &self,
        addon_id: String,
        item_id: String,
        title: Option<String>,
    ) -> Result<NativePreparedPlayback, AndroidCoreError> {
        self.core()
            .prepare_playback(&addon_id, &item_id, PlaybackCapabilities::media3())
            .map(|prepared| NativePreparedPlayback::from(prepared).with_title(title))
            .map_err(playback_error)
    }

    /// Never waits for an in-flight add-on call (see [`AndroidCore`]).
    pub fn report_playback_event(
        &self,
        session_id: u64,
        event: NativePlaybackEvent,
    ) -> Result<bool, AndroidCoreError> {
        lock_tracker(&self.playback)
            .report(PlaybackSessionId(session_id), lifecycle(event))
            .map_err(playback_error)
    }

    /// Start linking the add-on's account by device code. The device code
    /// and the resulting credentials never cross into Kotlin.
    pub fn begin_sign_in(&self, addon_id: String) -> Result<SignInPrompt, AndroidCoreError> {
        let prompt = self.core().auth_begin(&addon_id).map_err(addon_error)?;
        Ok(SignInPrompt {
            user_code: prompt.user_code,
            verification_url: prompt.verification_url,
            interval_secs: prompt.interval_secs,
            expires_at_unix: prompt.expires_at,
        })
    }

    /// Check once for approval. The core paces calls to the provider's
    /// interval, so polling more often is harmless. Linked, Expired and
    /// Denied end the sign-in.
    pub fn poll_sign_in(&self, addon_id: String) -> Result<SignInState, AndroidCoreError> {
        Ok(
            match self.core().auth_poll(&addon_id).map_err(addon_error)? {
                DeviceCodeState::Pending { .. } => SignInState::Waiting,
                DeviceCodeState::Authorized => SignInState::Linked,
                DeviceCodeState::Expired => SignInState::Expired,
                DeviceCodeState::Denied => SignInState::Denied,
            },
        )
    }

    pub fn cancel_sign_in(&self, addon_id: String) {
        self.core().auth_cancel(&addon_id);
    }

    pub fn account_status(&self, addon_id: String) -> Result<AccountState, AndroidCoreError> {
        Ok(
            match self.core().account_status(&addon_id).map_err(addon_error)? {
                AccountStatus::SignedOut => AccountState::SignedOut,
                AccountStatus::SignedIn { display_name } => AccountState::SignedIn { display_name },
                AccountStatus::Expired => AccountState::Expired,
            },
        )
    }

    /// Details-page information for one item (capability `details`).
    pub fn details(
        &self,
        addon_id: String,
        item_id: String,
    ) -> Result<ItemDetailsRecord, AndroidCoreError> {
        self.core()
            .details(&addon_id, &item_id)
            .map(ItemDetailsRecord::from)
            .map_err(addon_error)
    }

    /// Never waits for an in-flight add-on call (see [`AndroidCore`]).
    pub fn stop_playback(&self, session_id: u64) -> Result<bool, AndroidCoreError> {
        Ok(lock_tracker(&self.playback).stop(PlaybackSessionId(session_id)))
    }
}

fn playback_error(error: rvah_core::CoreError) -> AndroidCoreError {
    sign_in_required(&error).unwrap_or_else(|| AndroidCoreError::Playback {
        reason: error.to_string(),
    })
}

/// An add-on's `Auth` failure, which a token refresh did not fix.
fn sign_in_required(error: &rvah_core::CoreError) -> Option<AndroidCoreError> {
    match error {
        rvah_core::CoreError::Provider(e) if e.code == rvah_addon_sdk::AddonErrorCode::Auth => {
            let reason = if e.user_visible {
                e.message.clone()
            } else {
                "sign-in required".to_owned()
            };
            Some(AndroidCoreError::SignInRequired { reason })
        }
        _ => None,
    }
}

/// Route the core's debug lines (add-on calls and timings, credential
/// refreshes) to logcat under the tag `rvah`. Debug builds turn this on at
/// startup; `adb logcat -s rvah` shows them.
#[uniffi::export]
pub fn set_debug_logging(enabled: bool) {
    rvah_core::debug::set_sink(Some(logcat));
    rvah_core::debug::set_enabled(enabled);
}

#[cfg(target_os = "android")]
fn logcat(line: &str) {
    use std::ffi::{c_char, c_int, CString};
    #[link(name = "log")]
    extern "C" {
        fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }
    const ANDROID_LOG_DEBUG: c_int = 3;
    let text = CString::new(line.replace('\0', " ")).unwrap_or_default();
    // SAFETY: both pointers are valid NUL-terminated strings for the call.
    unsafe {
        __android_log_write(ANDROID_LOG_DEBUG, c"rvah".as_ptr(), text.as_ptr());
    }
}

/// Host builds (tests) have no logcat.
#[cfg(not(target_os = "android"))]
fn logcat(line: &str) {
    eprintln!("{line}");
}

fn addon_error(error: rvah_core::CoreError) -> AndroidCoreError {
    sign_in_required(&error).unwrap_or_else(|| AndroidCoreError::Addon {
        reason: error.to_string(),
    })
}

fn storage_error(error: rvah_data::DataError) -> AndroidCoreError {
    AndroidCoreError::Storage {
        reason: error.to_string(),
    }
}

fn catalog_entry(item: PageItem) -> CatalogEntry {
    match item {
        PageItem::Directory(item) => CatalogEntry::Directory {
            id: item.id,
            title: item.title,
            subtitle: item.subtitle,
            path: item.path,
            poster: item.artwork.poster,
        },
        PageItem::Media(item) => CatalogEntry::Media {
            id: item.id,
            title: item.title,
            synopsis: item.synopsis,
            year: item.year,
            rating: item.rating,
            poster: item.artwork.poster,
            fanart: item.artwork.fanart,
        },
    }
}

fn capability_names(manifest: &Manifest) -> Vec<String> {
    manifest
        .addon
        .capabilities
        .iter()
        .filter_map(|cap| serde_json::to_value(cap).ok()?.as_str().map(str::to_owned))
        .collect()
}

/// Verify `<package>.sig` (hex) against each trusted key in turn; returns the
/// key, the signature, and the matching key's hex.
fn verify_trusted(
    package: &std::path::Path,
    trusted_keys_hex: &[String],
) -> Result<(ed25519_dalek::VerifyingKey, [u8; 64], String), AndroidCoreError> {
    let invalid = |reason: String| AndroidCoreError::Addon { reason };
    let signature_hex = std::fs::read_to_string(format!("{}.sig", package.display()))
        .map_err(|error| invalid(format!("signature file: {error}")))?;
    let signature: [u8; 64] = hex::decode(signature_hex.trim())
        .map_err(|error| invalid(format!("invalid signature: {error}")))?
        .try_into()
        .map_err(|_| invalid("signature must be 64 bytes".into()))?;
    for key_hex in trusted_keys_hex {
        let Ok(bytes) = <[u8; 32]>::try_from(hex::decode(key_hex.trim()).unwrap_or_default())
        else {
            continue;
        };
        let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(&bytes) else {
            continue;
        };
        if rvah_addon_host::verify_package(package, &signature, &key).is_ok() {
            return Ok((key, signature, hex::encode(bytes)));
        }
    }
    Err(invalid(
        "package is not signed by a trusted key for this build".into(),
    ))
}

fn package_manifest(package: &std::path::Path) -> Result<Manifest, AndroidCoreError> {
    let bytes = rvah_addon_host::read_package_file(package, "manifest.toml").map_err(|error| {
        AndroidCoreError::Addon {
            reason: error.to_string(),
        }
    })?;
    let manifest = Manifest::parse(&String::from_utf8_lossy(&bytes)).map_err(|error| {
        AndroidCoreError::Addon {
            reason: error.to_string(),
        }
    })?;
    if !manifest.is_sdk_compatible() {
        return Err(AndroidCoreError::Addon {
            reason: format!("{} needs SDK {}", manifest.addon.id, manifest.addon.sdk),
        });
    }
    Ok(manifest)
}

fn read_manifest(dir: &std::path::Path) -> Result<Manifest, AndroidCoreError> {
    let text = std::fs::read_to_string(dir.join("manifest.toml")).map_err(|error| {
        AndroidCoreError::Storage {
            reason: error.to_string(),
        }
    })?;
    Manifest::parse(&text).map_err(|error| AndroidCoreError::Addon {
        reason: error.to_string(),
    })
}

fn load_wasmi_runtime(
    dir: &std::path::Path,
    db: &Arc<Database>,
    vault: &Arc<dyn SecretVault>,
) -> Result<RuntimePair, AndroidCoreError> {
    let manifest = read_manifest(dir)?;
    if !manifest.addon.entry.ends_with(".wasm") {
        return Err(AndroidCoreError::Addon {
            reason: "Android add-ons must provide a .wasm entry".into(),
        });
    }
    let module = std::fs::read(dir.join(&manifest.addon.entry)).map_err(|error| {
        AndroidCoreError::Addon {
            reason: error.to_string(),
        }
    })?;
    let runtime = WasmiRuntime::load(
        &module,
        WasmiLimits {
            timeout: Duration::from_secs(30),
            ..Default::default()
        },
    )
    .map_err(|error| AndroidCoreError::Addon {
        reason: error.message,
    })?;
    let runtime = CapabilityGated::new(runtime, manifest.addon.capabilities.clone());
    let host = BridgedHostApi::new(
        &manifest,
        Arc::clone(db),
        Arc::clone(vault),
        Box::new(rvah_net::UreqTransport::default()),
        REQUEST_QUOTA,
    )
    .with_resolver(Box::new(rvah_net::SystemResolver));
    Ok((Box::new(runtime), Arc::new(host)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_logging_switches_the_core_flag() {
        set_debug_logging(true);
        assert!(rvah_core::debug::enabled());
        set_debug_logging(false);
        assert!(!rvah_core::debug::enabled());
    }

    fn movie() -> rvah_addon_sdk::MediaItem {
        rvah_addon_sdk::MediaItem {
            id: "movie:1".into(),
            content_type: rvah_addon_sdk::ContentType::Movie,
            title: "Coyote vs. Acme".into(),
            year: Some(2026),
            season: None,
            episode: None,
            runtime_secs: Some(5400),
            synopsis: Some("A coyote sues.".into()),
            rating: Some(7.4),
            artwork: rvah_addon_sdk::Artwork {
                poster: Some("https://image.tmdb.org/t/p/w342/p.jpg".into()),
                fanart: Some("https://image.tmdb.org/t/p/w780/f.jpg".into()),
                thumb: None,
            },
            provider_ids: Default::default(),
        }
    }

    #[test]
    fn catalog_entries_carry_artwork_and_rating() {
        let CatalogEntry::Media {
            poster,
            fanart,
            rating,
            ..
        } = catalog_entry(PageItem::Media(movie()))
        else {
            panic!("expected media");
        };
        assert_eq!(
            poster.as_deref(),
            Some("https://image.tmdb.org/t/p/w342/p.jpg")
        );
        assert_eq!(
            fanart.as_deref(),
            Some("https://image.tmdb.org/t/p/w780/f.jpg")
        );
        assert_eq!(rating, Some(7.4));
    }

    #[test]
    fn details_build_the_trailer_link_from_the_key() {
        let record = ItemDetailsRecord::from(rvah_addon_sdk::ItemDetails {
            item: movie(),
            tagline: Some("Beep beep.".into()),
            genres: vec!["Comedy".into(), "Animation".into()],
            released: Some("2026-03-14".into()),
            trailers: vec![rvah_addon_sdk::Trailer {
                name: "Official Trailer".into(),
                site: rvah_addon_sdk::VideoSite::YouTube,
                key: "dQw4w9WgXcQ".into(),
            }],
            ratings: vec![rvah_addon_sdk::Rating {
                source: "rotten_tomatoes".into(),
                value: "93%".into(),
            }],
        });
        assert_eq!(record.title, "Coyote vs. Acme");
        assert_eq!(record.year, Some(2026));
        assert_eq!(record.genres, ["Comedy", "Animation"]);
        assert_eq!(record.trailers[0].name, "Official Trailer");
        assert_eq!(
            record.trailers[0].url,
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
        assert_eq!(record.ratings[0].source, "rotten_tomatoes");
        assert_eq!(record.ratings[0].value, "93%");
        assert_eq!(
            record.fanart.as_deref(),
            Some("https://image.tmdb.org/t/p/w780/f.jpg")
        );
    }

    #[test]
    fn auth_failures_ask_for_sign_in() {
        use rvah_addon_sdk::{AddonError, AddonErrorCode};
        use rvah_core::CoreError;

        let shown = || {
            let mut error = AddonError::new(AddonErrorCode::Auth, "not signed in to Real-Debrid");
            error.user_visible = true;
            CoreError::Provider(error)
        };
        for mapped in [addon_error(shown()), playback_error(shown())] {
            match mapped {
                AndroidCoreError::SignInRequired { reason } => {
                    assert_eq!(reason, "not signed in to Real-Debrid")
                }
                other => panic!("expected SignInRequired, got {other:?}"),
            }
        }

        // An add-on's internal wording stays out of the UI.
        let hidden = CoreError::Provider(AddonError::new(AddonErrorCode::Auth, "401 from /user"));
        match playback_error(hidden) {
            AndroidCoreError::SignInRequired { reason } => assert_eq!(reason, "sign-in required"),
            other => panic!("expected SignInRequired, got {other:?}"),
        }

        let offline = CoreError::Provider(AddonError::new(AddonErrorCode::Network, "offline"));
        assert!(matches!(
            playback_error(offline),
            AndroidCoreError::Playback { .. }
        ));
    }

    #[test]
    fn source_conversion_maps_protocol_headers_subtitles_and_resume() {
        let mut source = rvah_testkit::mp4_source("https://cdn.example.com/master.m3u8");
        source.protocol = Protocol::Hls;
        source
            .headers
            .insert("Referer".into(), "https://example.com".into());
        source.subtitles.push(rvah_addon_sdk::SubtitleTrack {
            url: "https://cdn.example.com/en.vtt".into(),
            lang: "en".into(),
            label: Some("English".into()),
            format: None,
        });
        let native = NativePreparedPlayback::from(PreparedPlayback {
            session_id: PlaybackSessionId(9),
            source,
            resume_position_secs: 12.5,
            subtitles: vec![rvah_addon_sdk::SubtitleTrack {
                url: "https://cdn.example.com/en.vtt".into(),
                lang: "en".into(),
                label: Some("English".into()),
                format: None,
            }],
            display: rvah_core::PlaybackDisplayMetadata {
                title: "Fixture".into(),
                subtitle: None,
            },
        });
        assert!(matches!(native.protocol, NativeProtocol::Hls));
        assert_eq!(native.resume_position_ms, 12_500);
        assert_eq!(native.headers[0].name, "Referer");
        assert!(matches!(
            native.subtitles[0].format,
            NativeSubtitleFormat::WebVtt
        ));
    }
}

#[cfg(test)]
mod tv_flow_tests {
    use std::collections::BTreeMap;

    use super::*;

    #[derive(Default)]
    struct MemoryStore(Mutex<BTreeMap<String, Vec<u8>>>);

    impl AndroidSecretStore for MemoryStore {
        fn put(&self, key: String, value: Vec<u8>) -> Option<String> {
            self.0.lock().unwrap().insert(key, value);
            None
        }
        fn get(&self, key: String) -> SecretRead {
            SecretRead {
                value: self.0.lock().unwrap().get(&key).cloned(),
                error: None,
            }
        }
        fn delete(&self, key: String) -> Option<String> {
            self.0.lock().unwrap().remove(&key);
            None
        }
        fn list(&self, prefix: String) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .keys()
                .filter(|key| key.starts_with(&prefix))
                .cloned()
                .collect()
        }
    }

    const ID: &str = "com.example.tv";
    const MANIFEST: &str = r#"
[addon]
id = "com.example.tv"
name = "TV Fixture"
version = "0.1.0"
sdk = ">=0.1, <0.2"
entry = "module.wasm"
capabilities = ["catalog", "search", "auth", "account_status"]

[permissions]
network = ["api.example.com"]
secrets = ["api_token"]

[settings]
quality = { type = "string", default = "1080p", enum = ["720p", "1080p"] }
subtitles = { type = "bool", default = false }
"#;

    const EMPTY_PAGE: &str = r#"{"items":[]}"#;

    const DEVICE_AUTH: &str = r#"{"Ok":{"kind":"device_auth","value":{"user_code":"WXYZ-1234","verification_url":"https://example.com/device","device_code":"dev-1","interval_secs":0,"expires_in_secs":600}}}"#;
    const APPROVED: &str = r#"{"Ok":{"kind":"auth_poll","value":{"status":"authorized","credentials":{"api_token":"tok-1"}}}}"#;
    const SIGNED_IN: &str =
        r#"{"Ok":{"kind":"account","value":{"kind":"signed_in","display_name":"fixture-user"}}}"#;

    /// Wasm guest (WAT; Wasmi accepts text). It tells ops apart by one byte
    /// of the request (`{"op":"auth_begin"...`): `auth_begin` gets a device
    /// code, `auth_poll` an approval with an `api_token`, `account_status` a
    /// signed-in account, and anything else `page`.
    fn module(page: &str) -> String {
        let page = format!(r#"{{"Ok":{{"kind":"page","value":{page}}}}}"#);
        let escape = |json: &str| json.replace('"', "\\\"");
        let at = |offset: i64, json: &str| (offset << 32) | json.len() as i64;
        format!(
            r#"(module
  (memory (export "memory") 1)
  (data (i32.const 1024) "{page_data}")
  (data (i32.const 2048) "{begin_data}")
  (data (i32.const 3072) "{poll_data}")
  (data (i32.const 3584) "{account_data}")
  (func (export "rvah_alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "rvah_invoke") (param $ptr i32) (param i32) (result i64)
    (if (i32.eq (i32.load8_u offset=12 (local.get $ptr)) (i32.const 98))
      (then (return (i64.const {begin}))))
    (if (i32.eq (i32.load8_u offset=12 (local.get $ptr)) (i32.const 112))
      (then (return (i64.const {poll}))))
    (if (i32.eq (i32.load8_u offset=8 (local.get $ptr)) (i32.const 99))
      (then (return (i64.const {account}))))
    (i64.const {page_at})))"#,
            page_data = escape(&page),
            begin_data = escape(DEVICE_AUTH),
            poll_data = escape(APPROVED),
            account_data = escape(SIGNED_IN),
            begin = at(2048, DEVICE_AUTH),
            poll = at(3072, APPROVED),
            account = at(3584, SIGNED_IN),
            page_at = at(1024, &page),
        )
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        core: Arc<AndroidCore>,
        pkg: String,
        key_hex: String,
        other_key_hex: String,
    }

    fn fixture() -> Fixture {
        fixture_with(EMPTY_PAGE)
    }

    fn fixture_with(page: &str) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("addon");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("manifest.toml"), MANIFEST).unwrap();
        std::fs::write(src.join("module.wasm"), module(page)).unwrap();
        let pkg = tmp.path().join("tv.rvah");
        rvah_addon_host::package_dir(&src, &pkg).unwrap();
        let (signing, verifying) = rvah_addon_host::keygen();
        let sig = rvah_addon_host::sign_package(&pkg, &signing).unwrap();
        std::fs::write(tmp.path().join("tv.rvah.sig"), hex::encode(sig)).unwrap();
        let (_, other) = rvah_addon_host::keygen();
        let core = AndroidCore::open(
            tmp.path().join("data").display().to_string(),
            Box::new(MemoryStore::default()),
        )
        .unwrap();
        Fixture {
            core,
            pkg: pkg.display().to_string(),
            key_hex: hex::encode(verifying.to_bytes()),
            other_key_hex: hex::encode(other.to_bytes()),
            _tmp: tmp,
        }
    }

    fn installed() -> Fixture {
        installed_with(EMPTY_PAGE)
    }

    fn installed_with(page: &str) -> Fixture {
        let f = fixture_with(page);
        let keys = vec![f.key_hex.clone()];
        let preview = f.core.preview_install(f.pkg.clone(), keys.clone()).unwrap();
        f.core
            .install_signed(f.pkg.clone(), keys, preview.permission_hash)
            .unwrap();
        f
    }

    #[test]
    fn preview_lists_permissions_and_the_trusted_signer() {
        let f = fixture();
        let preview = f
            .core
            .preview_install(
                f.pkg.clone(),
                vec![f.other_key_hex.clone(), f.key_hex.clone()],
            )
            .unwrap();
        assert_eq!(preview.id, ID);
        assert_eq!(preview.name, "TV Fixture");
        assert_eq!(preview.version, "0.1.0");
        assert_eq!(
            preview.capabilities,
            ["catalog", "search", "auth", "account_status"]
        );
        assert_eq!(preview.network, ["api.example.com"]);
        assert_eq!(preview.secrets, ["api_token"]);
        assert_eq!(preview.signer_key_hex, f.key_hex);
        assert!(!preview.is_update);
        assert!(
            f.core.addons().unwrap().is_empty(),
            "preview installs nothing"
        );
    }

    #[test]
    fn preview_refuses_packages_from_untrusted_signers() {
        let f = fixture();
        assert!(f
            .core
            .preview_install(f.pkg.clone(), vec![f.other_key_hex.clone()])
            .is_err());
        assert!(f.core.preview_install(f.pkg.clone(), vec![]).is_err());
    }

    #[test]
    fn install_requires_the_approved_permission_hash() {
        let f = fixture();
        let keys = vec![f.key_hex.clone()];
        assert!(f
            .core
            .install_signed(f.pkg.clone(), keys.clone(), "not-what-was-shown".into())
            .is_err());
        assert!(f.core.addons().unwrap().is_empty());

        let preview = f.core.preview_install(f.pkg.clone(), keys.clone()).unwrap();
        let record = f
            .core
            .install_signed(f.pkg.clone(), keys, preview.permission_hash)
            .unwrap();
        assert_eq!(record.name, "TV Fixture");
        let addons = f.core.addons().unwrap();
        assert_eq!(addons.len(), 1);
        assert_eq!(addons[0].status, "enabled");
        assert_eq!(
            addons[0].capabilities,
            ["catalog", "search", "auth", "account_status"]
        );
        let page = f.core.browse(ID.into(), None, None).unwrap();
        assert!(page.entries.is_empty());
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn browse_and_search_hand_back_the_next_cursor() {
        let f = installed_with(
            r#"{"items":[{"kind":"media","id":"m1","content_type":"movie","title":"One"}],"cursor_next":"p2"}"#,
        );
        for page in [
            f.core.browse(ID.into(), None, None).unwrap(),
            f.core.browse(ID.into(), None, Some("p2".into())).unwrap(),
            f.core.search(ID.into(), "one".into(), None).unwrap(),
            f.core
                .search(ID.into(), "one".into(), Some("p2".into()))
                .unwrap(),
        ] {
            assert_eq!(page.next_cursor.as_deref(), Some("p2"));
            assert!(matches!(&page.entries[..], [CatalogEntry::Media { id, .. }] if id == "m1"));
        }
    }

    #[test]
    fn reinstalling_the_same_package_is_an_update() {
        let f = installed();
        let keys = vec![f.key_hex.clone()];
        let preview = f.core.preview_install(f.pkg.clone(), keys.clone()).unwrap();
        assert!(preview.is_update);
        f.core
            .install_signed(f.pkg.clone(), keys, preview.permission_hash)
            .unwrap();
        assert_eq!(f.core.addons().unwrap().len(), 1);
    }

    #[test]
    fn secrets_only_for_declared_names_and_never_read_back() {
        let f = installed();
        assert!(f
            .core
            .set_secret(ID.into(), "undeclared".into(), "x".into())
            .is_err());
        let before = f.core.secret_status(ID.into()).unwrap();
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].name, "api_token");
        assert!(!before[0].is_set);

        f.core
            .set_secret(ID.into(), "api_token".into(), "tok".into())
            .unwrap();
        assert!(f.core.secret_status(ID.into()).unwrap()[0].is_set);
        assert!(f
            .core
            .set_secret(ID.into(), "api_token".into(), "   ".into())
            .is_err());
    }

    #[test]
    fn settings_are_validated_against_the_manifest() {
        let f = installed();
        let save =
            |key: &str, value: &str| f.core.save_setting(ID.into(), key.into(), value.into());
        assert!(save("quality", "4k").is_err());
        assert!(save("subtitles", "maybe").is_err());
        assert!(save("undeclared", "x").is_err());
        save("quality", "720p").unwrap();
        save("subtitles", "true").unwrap();
        let settings = f.core.settings(ID.into()).unwrap();
        let value = |key: &str| {
            settings
                .iter()
                .find(|s| s.key == key)
                .map(|s| s.value.clone())
                .unwrap()
        };
        assert_eq!(value("quality"), "720p");
        assert_eq!(value("subtitles"), "true");
    }

    #[test]
    fn quarantined_addon_can_be_re_enabled() {
        let f = installed();
        f.core
            .db
            .set_addon_status(ID, rvah_data::AddonStatus::Quarantined)
            .unwrap();
        assert_eq!(f.core.addons().unwrap()[0].status, "quarantined");
        f.core.enable_addon(ID.into()).unwrap();
        assert_eq!(f.core.addons().unwrap()[0].status, "enabled");
    }

    #[test]
    fn display_title_override_ignores_blank_titles() {
        let prepared = NativePreparedPlayback::from(PreparedPlayback {
            session_id: PlaybackSessionId(1),
            source: rvah_testkit::mp4_source("https://cdn.example.com/a.mp4"),
            resume_position_secs: 0.0,
            subtitles: vec![],
            display: rvah_core::PlaybackDisplayMetadata {
                title: "opaque-id-123".into(),
                subtitle: None,
            },
        });
        assert_eq!(
            prepared
                .clone()
                .with_title(Some("A Real Title".into()))
                .title,
            "A Real Title"
        );
        assert_eq!(
            prepared.with_title(Some("  ".into())).title,
            "opaque-id-123"
        );
    }

    #[test]
    fn device_code_sign_in_stores_the_token_without_showing_it() {
        let f = installed();
        let prompt = f.core.begin_sign_in(ID.into()).unwrap();
        assert_eq!(prompt.user_code, "WXYZ-1234");
        assert_eq!(prompt.verification_url, "https://example.com/device");
        assert!(prompt.expires_at_unix > 0);
        assert_eq!(f.core.poll_sign_in(ID.into()).unwrap(), SignInState::Linked);
        let secrets = f.core.secret_status(ID.into()).unwrap();
        assert!(secrets.iter().any(|s| s.name == "api_token" && s.is_set));
        // The sign-in is over; there is nothing left to poll.
        assert!(f.core.poll_sign_in(ID.into()).is_err());
    }

    #[test]
    fn a_cancelled_sign_in_cannot_be_polled() {
        let f = installed();
        f.core.begin_sign_in(ID.into()).unwrap();
        f.core.cancel_sign_in(ID.into());
        assert!(f.core.poll_sign_in(ID.into()).is_err());
    }

    #[test]
    fn account_status_names_the_signed_in_user() {
        let f = installed();
        assert_eq!(
            f.core.account_status(ID.into()).unwrap(),
            AccountState::SignedIn {
                display_name: Some("fixture-user".into())
            }
        );
    }
}
