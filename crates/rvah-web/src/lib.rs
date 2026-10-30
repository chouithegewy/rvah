//! Small, auditable WebAssembly boundary for RVAH profiles, signed add-ons,
//! and transient relay verification.
//!
//! Browser persistence, WebCrypto device wrapping, and Veilid transport live
//! in the web shell. This module owns the shared encrypted profile format and
//! verified snapshot layout so native and browser clients remain compatible.

use std::collections::BTreeMap;
mod addon;
mod relay;

use js_sys::{Array, Uint8Array};
use rvah_profile::{AddonProfile, PortableProfile, ProfileKey};
use rvah_sync::{PreparedSnapshot as SyncSnapshot, SnapshotHead};
use serde::Serialize;
use wasm_bindgen::prelude::*;

const MAX_PROFILE_FIELDS: usize = 256;
const MAX_FIELD_NAME_BYTES: usize = 256;
const MAX_FIELD_VALUE_BYTES: usize = 64 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileSummary {
    format_version: u16,
    created_at_unix: u64,
    addons: Vec<AddonSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AddonSummary {
    id: String,
    source_version: String,
    setting_names: Vec<String>,
    secret_names: Vec<String>,
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

#[wasm_bindgen]
pub fn format_version() -> String {
    env!("CARGO_PKG_VERSION").into()
}

#[wasm_bindgen]
pub fn snapshot_subkey_count() -> u32 {
    rvah_sync::DHT_SUBKEY_COUNT
}

#[wasm_bindgen]
pub fn generate_profile_key() -> String {
    ProfileKey::generate().to_hex()
}

#[wasm_bindgen]
pub fn create_empty_profile(profile_key_hex: &str) -> Result<Vec<u8>, JsValue> {
    let key = parse_key(profile_key_hex)?;
    PortableProfile::empty().encrypt(&key).map_err(js_error)
}

#[wasm_bindgen]
pub fn inspect_profile(
    encrypted_profile: &[u8],
    profile_key_hex: &str,
) -> Result<JsValue, JsValue> {
    let profile = decrypt(encrypted_profile, profile_key_hex)?;
    let addons = profile
        .addons
        .into_iter()
        .map(|(id, addon)| AddonSummary {
            id,
            source_version: addon.source_version,
            setting_names: addon.settings.into_keys().collect(),
            secret_names: addon.secrets.into_keys().collect(),
        })
        .collect();
    serde_wasm_bindgen::to_value(&ProfileSummary {
        format_version: profile.format_version,
        created_at_unix: profile.created_at_unix,
        addons,
    })
    .map_err(js_error)
}

/// Add or update explicit settings and secrets without returning existing
/// plaintext values to JavaScript. Supplied names overlay existing values.
#[wasm_bindgen]
pub fn put_addon_profile(
    encrypted_profile: &[u8],
    profile_key_hex: &str,
    addon_id: &str,
    source_version: &str,
    settings_json: &str,
    secrets_json: &str,
) -> Result<Vec<u8>, JsValue> {
    validate_label("add-on id", addon_id)?;
    validate_label("source version", source_version)?;
    let settings = parse_fields("settings", settings_json)?;
    let secrets = parse_fields("secrets", secrets_json)?;
    let key = parse_key(profile_key_hex)?;
    let mut profile = PortableProfile::decrypt(encrypted_profile, &key).map_err(js_error)?;
    let addon = profile
        .addons
        .entry(addon_id.to_owned())
        .or_insert_with(|| AddonProfile {
            source_version: source_version.to_owned(),
            settings: BTreeMap::new(),
            secrets: BTreeMap::new(),
            restore: None,
        });
    addon.source_version = source_version.to_owned();
    addon.settings.extend(settings);
    addon.secrets.extend(secrets);
    profile.encrypt(&key).map_err(js_error)
}

#[wasm_bindgen]
pub fn remove_addon_profile(
    encrypted_profile: &[u8],
    profile_key_hex: &str,
    addon_id: &str,
) -> Result<Vec<u8>, JsValue> {
    validate_label("add-on id", addon_id)?;
    let key = parse_key(profile_key_hex)?;
    let mut profile = PortableProfile::decrypt(encrypted_profile, &key).map_err(js_error)?;
    profile.addons.remove(addon_id);
    profile.encrypt(&key).map_err(js_error)
}

#[wasm_bindgen]
pub fn rekey_profile(
    encrypted_profile: &[u8],
    old_profile_key_hex: &str,
    new_profile_key_hex: &str,
) -> Result<Vec<u8>, JsValue> {
    let profile = decrypt(encrypted_profile, old_profile_key_hex)?;
    let new_key = parse_key(new_profile_key_hex)?;
    profile.encrypt(&new_key).map_err(js_error)
}

#[wasm_bindgen]
pub struct PreparedSnapshot {
    inner: SyncSnapshot,
}

#[wasm_bindgen]
impl PreparedSnapshot {
    #[wasm_bindgen(constructor)]
    pub fn new(epoch: u32, encrypted_profile: &[u8]) -> Result<PreparedSnapshot, JsValue> {
        Ok(Self {
            inner: SyncSnapshot::new(u64::from(epoch), encrypted_profile).map_err(js_error)?,
        })
    }

    #[wasm_bindgen(getter)]
    pub fn epoch(&self) -> u32 {
        self.inner.head().epoch as u32
    }

    #[wasm_bindgen(getter, js_name = chunkCount)]
    pub fn chunk_count(&self) -> usize {
        self.inner.chunks().len()
    }

    #[wasm_bindgen(js_name = headBytes)]
    pub fn head_bytes(&self) -> Vec<u8> {
        self.inner.head_bytes().to_vec()
    }

    pub fn chunk(&self, index: usize) -> Result<Vec<u8>, JsValue> {
        self.inner
            .chunks()
            .get(index)
            .cloned()
            .ok_or_else(|| JsValue::from_str("snapshot chunk index is out of range"))
    }
}

#[wasm_bindgen]
pub fn assemble_snapshot(
    head_bytes: &[u8],
    chunks: &Array,
    minimum_epoch: u32,
) -> Result<Vec<u8>, JsValue> {
    let mut native_chunks = Vec::with_capacity(chunks.length() as usize);
    for value in chunks.iter() {
        if !value.is_instance_of::<Uint8Array>() {
            return Err(JsValue::from_str(
                "snapshot chunks must be Uint8Array values",
            ));
        }
        native_chunks.push(Uint8Array::new(&value).to_vec());
    }
    rvah_sync::assemble(head_bytes, &native_chunks, u64::from(minimum_epoch)).map_err(js_error)
}

#[wasm_bindgen]
pub fn inspect_snapshot_head(head_bytes: &[u8]) -> Result<JsValue, JsValue> {
    let head: SnapshotHead = rvah_sync::parse_head(head_bytes).map_err(js_error)?;
    serde_wasm_bindgen::to_value(&head).map_err(js_error)
}

fn decrypt(bytes: &[u8], key_hex: &str) -> Result<PortableProfile, JsValue> {
    let key = parse_key(key_hex)?;
    PortableProfile::decrypt(bytes, &key).map_err(js_error)
}

fn parse_key(value: &str) -> Result<ProfileKey, JsValue> {
    ProfileKey::from_hex(value).map_err(js_error)
}

fn validate_label(label: &str, value: &str) -> Result<(), JsValue> {
    if value.trim().is_empty() || value.len() > MAX_FIELD_NAME_BYTES {
        return Err(JsValue::from_str(&format!(
            "{label} must be between 1 and {MAX_FIELD_NAME_BYTES} bytes"
        )));
    }
    Ok(())
}

fn parse_fields(label: &str, json: &str) -> Result<BTreeMap<String, String>, JsValue> {
    let values: BTreeMap<String, String> = serde_json::from_str(json)
        .map_err(|error| JsValue::from_str(&format!("invalid {label} JSON: {error}")))?;
    if values.len() > MAX_PROFILE_FIELDS {
        return Err(JsValue::from_str(&format!(
            "{label} may contain at most {MAX_PROFILE_FIELDS} fields"
        )));
    }
    for (name, value) in &values {
        validate_label("field name", name)?;
        if value.len() > MAX_FIELD_VALUE_BYTES {
            return Err(JsValue::from_str(&format!(
                "field {name} exceeds the {MAX_FIELD_VALUE_BYTES}-byte limit"
            )));
        }
    }
    Ok(values)
}

fn js_error(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}
