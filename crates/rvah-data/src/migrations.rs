/// One schema migration. `sql` runs inside a transaction together with the
/// version bump; any failure rolls the whole migration back.
pub struct Migration {
    pub version: i64,
    pub sql: &'static str,
}

pub static MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: r#"
CREATE TABLE addons (
    id TEXT PRIMARY KEY,
    version TEXT NOT NULL,
    package_hash TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('enabled', 'disabled', 'quarantined')),
    install_source TEXT NOT NULL,
    sdk_range TEXT NOT NULL,
    capabilities_json TEXT NOT NULL
);

CREATE TABLE addon_permissions (
    addon_id TEXT PRIMARY KEY,
    permissions_json TEXT NOT NULL,
    manifest_permission_hash TEXT NOT NULL,
    approved_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE addon_settings (
    addon_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value_json TEXT NOT NULL,
    PRIMARY KEY (addon_id, key)
);

CREATE TABLE addon_storage (
    addon_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value BLOB NOT NULL,
    PRIMARY KEY (addon_id, key)
);

CREATE TABLE favorites (
    addon_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    title TEXT NOT NULL,
    artwork_json TEXT NOT NULL DEFAULT '{}',
    added_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (addon_id, item_id)
);

CREATE TABLE watch_state (
    addon_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    position_secs REAL NOT NULL,
    duration_secs REAL NOT NULL,
    watched INTEGER NOT NULL DEFAULT 0,
    last_played_at INTEGER NOT NULL,
    PRIMARY KEY (addon_id, item_id)
);

CREATE TABLE cache_index (
    key TEXT PRIMARY KEY,
    addon_id TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    expires_at INTEGER
);

CREATE TABLE diagnostic_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts TEXT NOT NULL DEFAULT (datetime('now')),
    level TEXT NOT NULL,
    addon_id TEXT,
    code TEXT NOT NULL,
    message TEXT NOT NULL
);
"#,
}];
