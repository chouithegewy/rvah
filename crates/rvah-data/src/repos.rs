use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::Database;

#[derive(Debug, thiserror::Error)]
pub enum DataError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("migration {version} failed and was rolled back: {cause}")]
    Migration { version: i64, cause: String },
    #[error("storage quota exceeded for add-on {addon_id} ({used} + {requested} > {quota} bytes)")]
    StorageQuotaExceeded {
        addon_id: String,
        used: u64,
        requested: u64,
        quota: u64,
    },
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddonStatus {
    Enabled,
    Disabled,
    Quarantined,
}

impl AddonStatus {
    fn as_str(&self) -> &'static str {
        match self {
            AddonStatus::Enabled => "enabled",
            AddonStatus::Disabled => "disabled",
            AddonStatus::Quarantined => "quarantined",
        }
    }

    fn from_str(s: &str) -> AddonStatus {
        match s {
            "enabled" => AddonStatus::Enabled,
            "quarantined" => AddonStatus::Quarantined,
            _ => AddonStatus::Disabled,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AddonRecord {
    pub id: String,
    pub version: String,
    pub package_hash: String,
    pub status: AddonStatus,
    pub install_source: String,
    pub sdk_range: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WatchState {
    pub addon_id: String,
    pub item_id: String,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub watched: bool,
    pub last_played_at: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FavoriteRecord {
    pub addon_id: String,
    pub item_id: String,
    pub title: String,
    pub artwork_json: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiagnosticEvent {
    pub level: String,
    pub addon_id: Option<String>,
    pub code: String,
    pub message: String,
}

impl Database {
    // ---- addons ----

    pub fn upsert_addon(&self, rec: &AddonRecord) -> Result<(), DataError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO addons (id, version, package_hash, status, install_source, sdk_range, capabilities_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET version = ?2, package_hash = ?3, status = ?4,
                 install_source = ?5, sdk_range = ?6, capabilities_json = ?7",
            params![
                rec.id,
                rec.version,
                rec.package_hash,
                rec.status.as_str(),
                rec.install_source,
                rec.sdk_range,
                serde_json::to_string(&rec.capabilities)?,
            ],
        )?;
        Ok(())
    }

    pub fn get_addon(&self, id: &str) -> Result<Option<AddonRecord>, DataError> {
        let conn = self.conn.lock().unwrap();
        let rec = conn
            .query_row(
                "SELECT id, version, package_hash, status, install_source, sdk_range, capabilities_json
                 FROM addons WHERE id = ?1",
                [id],
                |r| {
                    Ok(AddonRecord {
                        id: r.get(0)?,
                        version: r.get(1)?,
                        package_hash: r.get(2)?,
                        status: AddonStatus::from_str(&r.get::<_, String>(3)?),
                        install_source: r.get(4)?,
                        sdk_range: r.get(5)?,
                        capabilities: serde_json::from_str(&r.get::<_, String>(6)?)
                            .unwrap_or_default(),
                    })
                },
            )
            .optional()?;
        Ok(rec)
    }

    pub fn list_addons(&self) -> Result<Vec<AddonRecord>, DataError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, version, package_hash, status, install_source, sdk_range, capabilities_json
             FROM addons ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(AddonRecord {
                id: r.get(0)?,
                version: r.get(1)?,
                package_hash: r.get(2)?,
                status: AddonStatus::from_str(&r.get::<_, String>(3)?),
                install_source: r.get(4)?,
                sdk_range: r.get(5)?,
                capabilities: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_addon_status(&self, id: &str, status: AddonStatus) -> Result<(), DataError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE addons SET status = ?2 WHERE id = ?1",
            params![id, status.as_str()],
        )?;
        Ok(())
    }

    /// Remove the add-on row; with `purge_data` also removes settings,
    /// storage, permissions, favorites, watch state, and cache entries
    /// (TC-ADD-007). Without it, user data is retained for reinstall by the
    /// same id (TC-ADD-008).
    pub fn remove_addon(&self, id: &str, purge_data: bool) -> Result<(), DataError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM addons WHERE id = ?1", [id])?;
        if purge_data {
            for table in [
                "addon_permissions",
                "addon_settings",
                "addon_storage",
                "favorites",
                "watch_state",
                "cache_index",
            ] {
                tx.execute(&format!("DELETE FROM {table} WHERE addon_id = ?1"), [id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // ---- permissions ----

    pub fn record_permissions(
        &self,
        addon_id: &str,
        permissions_json: &str,
        hash: &str,
    ) -> Result<(), DataError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO addon_permissions (addon_id, permissions_json, manifest_permission_hash)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(addon_id) DO UPDATE SET permissions_json = ?2,
                 manifest_permission_hash = ?3, approved_at = datetime('now')",
            params![addon_id, permissions_json, hash],
        )?;
        Ok(())
    }

    pub fn get_permission_hash(&self, addon_id: &str) -> Result<Option<String>, DataError> {
        let conn = self.conn.lock().unwrap();
        let h = conn
            .query_row(
                "SELECT manifest_permission_hash FROM addon_permissions WHERE addon_id = ?1",
                [addon_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(h)
    }

    pub fn revoke_permissions(&self, addon_id: &str) -> Result<(), DataError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM addon_permissions WHERE addon_id = ?1",
            [addon_id],
        )?;
        Ok(())
    }

    // ---- settings ----

    pub fn set_setting(&self, addon_id: &str, key: &str, value: &str) -> Result<(), DataError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO addon_settings (addon_id, key, value_json) VALUES (?1, ?2, ?3)
             ON CONFLICT(addon_id, key) DO UPDATE SET value_json = ?3",
            params![addon_id, key, serde_json::to_string(value)?],
        )?;
        Ok(())
    }

    pub fn get_setting(&self, addon_id: &str, key: &str) -> Result<Option<String>, DataError> {
        let conn = self.conn.lock().unwrap();
        let v: Option<String> = conn
            .query_row(
                "SELECT value_json FROM addon_settings WHERE addon_id = ?1 AND key = ?2",
                params![addon_id, key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(match v {
            Some(json) => Some(serde_json::from_str(&json)?),
            None => None,
        })
    }

    // ---- per-add-on key-value storage with quota (FR-101) ----

    pub fn storage_used(&self, addon_id: &str) -> Result<u64, DataError> {
        let conn = self.conn.lock().unwrap();
        let used: i64 = conn.query_row(
            "SELECT COALESCE(SUM(LENGTH(value) + LENGTH(key)), 0) FROM addon_storage WHERE addon_id = ?1",
            [addon_id],
            |r| r.get(0),
        )?;
        Ok(used as u64)
    }

    pub fn storage_set(
        &self,
        addon_id: &str,
        key: &str,
        value: &[u8],
        quota_bytes: u64,
    ) -> Result<(), DataError> {
        let existing_len: u64 = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT LENGTH(value) + LENGTH(key) FROM addon_storage WHERE addon_id = ?1 AND key = ?2",
                params![addon_id, key],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0) as u64
        };
        let used = self.storage_used(addon_id)? - existing_len;
        let requested = (value.len() + key.len()) as u64;
        if used + requested > quota_bytes {
            return Err(DataError::StorageQuotaExceeded {
                addon_id: addon_id.to_string(),
                used,
                requested,
                quota: quota_bytes,
            });
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO addon_storage (addon_id, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT(addon_id, key) DO UPDATE SET value = ?3",
            params![addon_id, key, value],
        )?;
        Ok(())
    }

    pub fn storage_get(&self, addon_id: &str, key: &str) -> Result<Option<Vec<u8>>, DataError> {
        let conn = self.conn.lock().unwrap();
        let v = conn
            .query_row(
                "SELECT value FROM addon_storage WHERE addon_id = ?1 AND key = ?2",
                params![addon_id, key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v)
    }

    pub fn storage_delete(&self, addon_id: &str, key: &str) -> Result<(), DataError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM addon_storage WHERE addon_id = ?1 AND key = ?2",
            params![addon_id, key],
        )?;
        Ok(())
    }

    // ---- favorites ----

    pub fn add_favorite(&self, fav: &FavoriteRecord) -> Result<(), DataError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO favorites (addon_id, item_id, title, artwork_json) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(addon_id, item_id) DO UPDATE SET title = ?3, artwork_json = ?4",
            params![fav.addon_id, fav.item_id, fav.title, fav.artwork_json],
        )?;
        Ok(())
    }

    pub fn list_favorites(&self) -> Result<Vec<FavoriteRecord>, DataError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT addon_id, item_id, title, artwork_json FROM favorites ORDER BY added_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(FavoriteRecord {
                addon_id: r.get(0)?,
                item_id: r.get(1)?,
                title: r.get(2)?,
                artwork_json: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn remove_favorite(&self, addon_id: &str, item_id: &str) -> Result<(), DataError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM favorites WHERE addon_id = ?1 AND item_id = ?2",
            params![addon_id, item_id],
        )?;
        Ok(())
    }

    // ---- watch state (FR-065) ----

    pub fn set_watch_state(&self, ws: &WatchState) -> Result<(), DataError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO watch_state (addon_id, item_id, position_secs, duration_secs, watched, last_played_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(addon_id, item_id) DO UPDATE SET position_secs = ?3, duration_secs = ?4,
                 watched = ?5, last_played_at = ?6",
            params![
                ws.addon_id,
                ws.item_id,
                ws.position_secs,
                ws.duration_secs,
                ws.watched as i64,
                ws.last_played_at as i64,
            ],
        )?;
        Ok(())
    }

    pub fn get_watch_state(
        &self,
        addon_id: &str,
        item_id: &str,
    ) -> Result<Option<WatchState>, DataError> {
        let conn = self.conn.lock().unwrap();
        let ws = conn
            .query_row(
                "SELECT position_secs, duration_secs, watched, last_played_at
                 FROM watch_state WHERE addon_id = ?1 AND item_id = ?2",
                params![addon_id, item_id],
                |r| {
                    Ok(WatchState {
                        addon_id: addon_id.to_string(),
                        item_id: item_id.to_string(),
                        position_secs: r.get(0)?,
                        duration_secs: r.get(1)?,
                        watched: r.get::<_, i64>(2)? != 0,
                        last_played_at: r.get::<_, i64>(3)? as u64,
                    })
                },
            )
            .optional()?;
        Ok(ws)
    }

    // ---- diagnostics (FR-103) ----

    pub fn log_event(&self, ev: &DiagnosticEvent) -> Result<(), DataError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO diagnostic_events (level, addon_id, code, message) VALUES (?1, ?2, ?3, ?4)",
            params![ev.level, ev.addon_id, ev.code, ev.message],
        )?;
        Ok(())
    }

    pub fn recent_events(&self, limit: u32) -> Result<Vec<DiagnosticEvent>, DataError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT level, addon_id, code, message FROM diagnostic_events
             ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |r| {
            Ok(DiagnosticEvent {
                level: r.get(0)?,
                addon_id: r.get(1)?,
                code: r.get(2)?,
                message: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Migration, MIGRATIONS};

    fn db() -> Database {
        Database::open_in_memory().unwrap()
    }

    fn addon(id: &str) -> AddonRecord {
        AddonRecord {
            id: id.to_string(),
            version: "1.0.0".into(),
            package_hash: "abc123".into(),
            status: AddonStatus::Enabled,
            install_source: "local".into(),
            sdk_range: ">=0.1, <0.2".into(),
            capabilities: vec!["catalog".into(), "resolve".into()],
        }
    }

    #[test]
    fn migrations_apply_fresh() {
        let db = db();
        assert_eq!(db.schema_version().unwrap(), 1);
    }

    #[test]
    fn failed_migration_rolls_back() {
        let db = db();
        let bad = [Migration {
            version: 2,
            sql: "CREATE TABLE half_done (id INTEGER); THIS IS NOT SQL;",
        }];
        let err = db.migrate_with(&bad).unwrap_err();
        assert!(matches!(err, DataError::Migration { version: 2, .. }));
        // Version unchanged and partial table absent.
        assert_eq!(db.schema_version().unwrap(), 1);
        let conn = db.conn.lock().unwrap();
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'half_done'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 0, "partial migration must be rolled back");
        drop(conn);
        // A subsequent good migration still applies.
        let good = [Migration {
            version: 2,
            sql: "CREATE TABLE ok_table (id INTEGER);",
        }];
        db.migrate_with(&good).unwrap();
        assert_eq!(db.schema_version().unwrap(), 2);
        let _ = MIGRATIONS; // silence unused in cfg(test) path
    }

    #[test]
    fn addon_crud_roundtrip() {
        let db = db();
        db.upsert_addon(&addon("a.b")).unwrap();
        let got = db.get_addon("a.b").unwrap().unwrap();
        assert_eq!(got.capabilities, vec!["catalog", "resolve"]);
        db.set_addon_status("a.b", AddonStatus::Quarantined)
            .unwrap();
        assert_eq!(
            db.get_addon("a.b").unwrap().unwrap().status,
            AddonStatus::Quarantined
        );
        assert_eq!(db.list_addons().unwrap().len(), 1);
        db.remove_addon("a.b", true).unwrap();
        assert!(db.get_addon("a.b").unwrap().is_none());
    }

    #[test]
    fn storage_quota_enforced() {
        let db = db();
        let quota = 64;
        db.storage_set("a.b", "k1", &[0u8; 30], quota).unwrap();
        let err = db.storage_set("a.b", "k2", &[0u8; 40], quota).unwrap_err();
        assert!(matches!(err, DataError::StorageQuotaExceeded { .. }));
        // Overwriting the same key does not double-count.
        db.storage_set("a.b", "k1", &[1u8; 40], quota).unwrap();
        assert_eq!(db.storage_get("a.b", "k1").unwrap().unwrap(), vec![1u8; 40]);
        // Another add-on has an independent budget.
        db.storage_set("c.d", "k1", &[0u8; 30], quota).unwrap();
        db.storage_delete("a.b", "k1").unwrap();
        assert!(db.storage_get("a.b", "k1").unwrap().is_none());
    }

    #[test]
    fn watch_state_roundtrip() {
        let db = db();
        let ws = WatchState {
            addon_id: "a.b".into(),
            item_id: "m1".into(),
            position_secs: 120.5,
            duration_secs: 3600.0,
            watched: false,
            last_played_at: 1_800_000_000,
        };
        db.set_watch_state(&ws).unwrap();
        assert_eq!(db.get_watch_state("a.b", "m1").unwrap().unwrap(), ws);
        let done = WatchState {
            watched: true,
            position_secs: 3590.0,
            ..ws
        };
        db.set_watch_state(&done).unwrap();
        assert!(db.get_watch_state("a.b", "m1").unwrap().unwrap().watched);
    }

    #[test]
    fn remove_addon_purges_or_retains_data() {
        let db = db();
        for id in ["keep.data", "purge.data"] {
            db.upsert_addon(&addon(id)).unwrap();
            db.set_setting(id, "region", "us").unwrap();
            db.storage_set(id, "k", b"v", 1024).unwrap();
            db.record_permissions(id, "{}", "hash").unwrap();
        }
        db.remove_addon("keep.data", false).unwrap();
        assert!(db.get_addon("keep.data").unwrap().is_none());
        assert_eq!(
            db.get_setting("keep.data", "region").unwrap().unwrap(),
            "us"
        );

        db.remove_addon("purge.data", true).unwrap();
        assert!(db.get_setting("purge.data", "region").unwrap().is_none());
        assert!(db.storage_get("purge.data", "k").unwrap().is_none());
        assert!(db.get_permission_hash("purge.data").unwrap().is_none());
    }

    #[test]
    fn favorites_and_events() {
        let db = db();
        db.add_favorite(&FavoriteRecord {
            addon_id: "a.b".into(),
            item_id: "m1".into(),
            title: "Example".into(),
            artwork_json: "{}".into(),
        })
        .unwrap();
        assert_eq!(db.list_favorites().unwrap().len(), 1);
        db.remove_favorite("a.b", "m1").unwrap();
        assert!(db.list_favorites().unwrap().is_empty());

        db.log_event(&DiagnosticEvent {
            level: "error".into(),
            addon_id: Some("a.b".into()),
            code: "provider_timeout".into(),
            message: "timed out".into(),
        })
        .unwrap();
        assert_eq!(db.recent_events(10).unwrap().len(), 1);
    }
}
