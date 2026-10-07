//! Typed UniFFI surface consumed by the Kotlin/Compose TV application.
//! Blocking methods are intentionally synchronous; Kotlin calls them from
//! `Dispatchers.IO` so Rust owns policy while Media3 owns playback surfaces.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rvah_addon_host::{
    AddonManager, AddonRuntime, BridgedHostApi, CapabilityGated, Manifest, WasmiLimits,
    WasmiRuntime,
};
use rvah_addon_sdk::{PageItem, Protocol, SubtitleFormat};
use rvah_auth::{SecretVault, VaultError};
use rvah_core::{
    AppCore, PlaybackCapabilities, PlaybackLifecycleEvent, PlaybackSessionId, PreparedPlayback,
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
    },
    Media {
        id: String,
        title: String,
        synopsis: Option<String>,
        year: Option<u16>,
    },
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

#[derive(uniffi::Object)]
pub struct AndroidCore {
    core: Mutex<AndroidAppCore>,
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
            manager,
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
            if let Ok((runtime, host)) = load_wasmi_runtime(&dir, &db, &vault) {
                core.register_runtime(addon.id, runtime, host);
            }
        }
        Ok(Arc::new(Self {
            core: Mutex::new(core),
        }))
    }

    pub fn addons(&self) -> Result<Vec<AddonRecord>, AndroidCoreError> {
        let core = self.core.lock().unwrap();
        core.db()
            .list_addons()
            .map(|addons| {
                addons
                    .into_iter()
                    .map(|addon| AddonRecord {
                        name: addon.id.clone(),
                        id: addon.id,
                        version: addon.version,
                        enabled: addon.status == rvah_data::AddonStatus::Enabled,
                    })
                    .collect()
            })
            .map_err(|error| AndroidCoreError::Storage {
                reason: error.to_string(),
            })
    }

    pub fn settings(&self, addon_id: String) -> Result<Vec<SettingRecord>, AndroidCoreError> {
        let core = self.core.lock().unwrap();
        let addon = core
            .db()
            .get_addon(&addon_id)
            .map_err(storage_error)?
            .ok_or_else(|| AndroidCoreError::Addon {
                reason: format!("add-on not installed: {addon_id}"),
            })?;
        let path = core
            .manager
            .install_dir(&addon_id, &addon.version)
            .join("manifest.toml");
        let manifest = Manifest::parse(&std::fs::read_to_string(path).map_err(|error| {
            AndroidCoreError::Storage {
                reason: error.to_string(),
            }
        })?)
        .map_err(|error| AndroidCoreError::Addon {
            reason: error.to_string(),
        })?;
        let defaults = manifest.default_settings();
        manifest
            .settings
            .iter()
            .map(|(key, spec)| {
                let value = core
                    .db()
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

    pub fn save_setting(
        &self,
        addon_id: String,
        key: String,
        value: String,
    ) -> Result<(), AndroidCoreError> {
        self.core
            .lock()
            .unwrap()
            .db()
            .set_setting(&addon_id, &key, &value)
            .map_err(storage_error)
    }

    pub fn diagnostics(&self, limit: u32) -> Result<Vec<DiagnosticRecord>, AndroidCoreError> {
        self.core
            .lock()
            .unwrap()
            .db()
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

    /// Install only after detached Ed25519 verification. Release Kotlin code
    /// supplies keys from its configured trust set; debug may supply only the
    /// dedicated fixture key.
    pub fn install_signed(
        &self,
        package_path: String,
        public_key_hex: String,
    ) -> Result<AddonRecord, AndroidCoreError> {
        let package = std::path::PathBuf::from(package_path);
        let signature_hex =
            std::fs::read_to_string(format!("{}.sig", package.display())).map_err(|error| {
                AndroidCoreError::Storage {
                    reason: error.to_string(),
                }
            })?;
        let signature: [u8; 64] = hex::decode(signature_hex.trim())
            .map_err(|error| AndroidCoreError::Addon {
                reason: format!("invalid signature: {error}"),
            })?
            .try_into()
            .map_err(|_| AndroidCoreError::Addon {
                reason: "signature must be 64 bytes".into(),
            })?;
        let key_bytes: [u8; 32] = hex::decode(public_key_hex.trim())
            .map_err(|error| AndroidCoreError::Addon {
                reason: format!("invalid public key: {error}"),
            })?
            .try_into()
            .map_err(|_| AndroidCoreError::Addon {
                reason: "public key must be 32 bytes".into(),
            })?;
        let key = ed25519_dalek::VerifyingKey::from_bytes(&key_bytes).map_err(|error| {
            AndroidCoreError::Addon {
                reason: format!("invalid public key: {error}"),
            }
        })?;
        let mut core = self.core.lock().unwrap();
        let installed = core
            .manager
            .install(
                &package,
                &signature,
                &key,
                rvah_addon_host::InstallDecision::Approve,
            )
            .map_err(|error| AndroidCoreError::Addon {
                reason: error.to_string(),
            })?;
        let db = Arc::clone(core.db());
        let vault = Arc::clone(core.vault());
        let dir = core.manager.install_dir(&installed.id, &installed.version);
        let (runtime, host) = load_wasmi_runtime(&dir, &db, &vault)?;
        core.register_runtime(installed.id.clone(), runtime, host);
        Ok(AddonRecord {
            name: installed.id.clone(),
            id: installed.id,
            version: installed.version,
            enabled: true,
        })
    }

    pub fn browse(
        &self,
        addon_id: String,
        path: Option<String>,
    ) -> Result<Vec<CatalogEntry>, AndroidCoreError> {
        let page = self
            .core
            .lock()
            .unwrap()
            .browse(&addon_id, path, None)
            .map_err(playback_error)?;
        Ok(page.items.into_iter().map(catalog_entry).collect())
    }

    pub fn search(
        &self,
        addon_id: String,
        query: String,
    ) -> Result<Vec<CatalogEntry>, AndroidCoreError> {
        let page = self
            .core
            .lock()
            .unwrap()
            .search(&addon_id, &query)
            .map_err(playback_error)?;
        Ok(page.items.into_iter().map(catalog_entry).collect())
    }

    pub fn prepare_playback(
        &self,
        addon_id: String,
        item_id: String,
    ) -> Result<NativePreparedPlayback, AndroidCoreError> {
        self.core
            .lock()
            .unwrap()
            .prepare_playback(&addon_id, &item_id, PlaybackCapabilities::media3())
            .map(NativePreparedPlayback::from)
            .map_err(playback_error)
    }

    pub fn report_playback_event(
        &self,
        session_id: u64,
        event: NativePlaybackEvent,
    ) -> Result<bool, AndroidCoreError> {
        self.core
            .lock()
            .unwrap()
            .report_playback_event(PlaybackSessionId(session_id), lifecycle(event))
            .map_err(playback_error)
    }

    pub fn stop_playback(&self, session_id: u64) -> Result<bool, AndroidCoreError> {
        self.core
            .lock()
            .unwrap()
            .stop_playback(PlaybackSessionId(session_id))
            .map_err(playback_error)
    }
}

fn playback_error(error: rvah_core::CoreError) -> AndroidCoreError {
    AndroidCoreError::Playback {
        reason: error.to_string(),
    }
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
        },
        PageItem::Media(item) => CatalogEntry::Media {
            id: item.id,
            title: item.title,
            synopsis: item.synopsis,
            year: item.year,
        },
    }
}

fn load_wasmi_runtime(
    dir: &std::path::Path,
    db: &Arc<Database>,
    vault: &Arc<dyn SecretVault>,
) -> Result<RuntimePair, AndroidCoreError> {
    let manifest = Manifest::parse(&std::fs::read_to_string(dir.join("manifest.toml")).map_err(
        |error| AndroidCoreError::Addon {
            reason: error.to_string(),
        },
    )?)
    .map_err(|error| AndroidCoreError::Addon {
        reason: error.to_string(),
    })?;
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
