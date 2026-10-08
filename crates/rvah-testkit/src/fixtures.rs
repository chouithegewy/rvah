use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use rvah_addon_host::{keygen, package_dir, sign_package};
use rvah_addon_sdk::{
    AccountStatus, AddonError, AddonErrorCode, AddonProvider, Artwork, DirectoryItem, Filter,
    HostApi, HttpRequest, HttpResponse, LogLevel, MediaItem, Page, PageItem, PlayableSource,
    Protocol, ResolveResult,
};

pub const ECHO_FIXTURE_ID: &str = "com.rvah.fixtures.echo";

/// Render a valid fixture manifest with the given capability/permission mix.
pub fn fixture_manifest(id: &str, caps: &[&str], network: &[&str], secrets: &[&str]) -> String {
    let quote = |xs: &[&str]| {
        xs.iter()
            .map(|x| format!("\"{x}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        r#"
[addon]
id = "{id}"
name = "Fixture {id}"
version = "1.0.0"
sdk = ">=0.1, <0.2"
entry = "main"
capabilities = [{caps}]

[permissions]
network = [{network}]
storage = "1MiB"
secrets = [{secrets}]
"#,
        caps = quote(caps),
        network = quote(network),
        secrets = quote(secrets),
    )
}

/// Build, package, and sign the echo fixture add-on inside `dir`.
/// Returns the package path, its detached signature, and the verify key.
pub fn write_fixture_package(dir: &Path) -> (PathBuf, [u8; 64], VerifyingKey) {
    write_named_fixture_package(dir, ECHO_FIXTURE_ID, &["catalog", "search", "resolve"])
}

/// Same as [`write_fixture_package`] with a custom id and capability set.
pub fn write_named_fixture_package(
    dir: &Path,
    id: &str,
    caps: &[&str],
) -> (PathBuf, [u8; 64], VerifyingKey) {
    let src = dir.join(format!("{id}-src"));
    std::fs::create_dir_all(&src).expect("fixture src dir");
    std::fs::write(
        src.join("manifest.toml"),
        fixture_manifest(id, caps, &[], &[]),
    )
    .expect("fixture manifest");
    std::fs::write(src.join("main"), "fixture entry").expect("fixture entry");
    let pkg = dir.join(format!("{id}.rvah"));
    package_dir(&src, &pkg).expect("package fixture");
    let (signing, verifying) = keygen();
    let sig = sign_package(&pkg, &signing).expect("sign fixture");
    (pkg, sig, verifying)
}

fn media(id: &str, title: &str) -> PageItem {
    PageItem::Media(MediaItem {
        id: id.into(),
        content_type: rvah_addon_sdk::ContentType::Movie,
        title: title.into(),
        year: Some(2020),
        season: None,
        episode: None,
        runtime_secs: Some(5400),
        synopsis: None,
        rating: None,
        artwork: Artwork::default(),
        provider_ids: BTreeMap::new(),
    })
}

fn directory(id: &str, title: &str, path: &str) -> PageItem {
    PageItem::Directory(DirectoryItem {
        id: id.into(),
        title: title.into(),
        subtitle: None,
        artwork: Artwork::default(),
        path: path.into(),
        badges: vec![],
    })
}

pub fn mp4_source(url: &str) -> PlayableSource {
    PlayableSource {
        url: url.into(),
        protocol: Protocol::Https,
        headers: BTreeMap::new(),
        subtitles: vec![],
        quality: Some("1080p".into()),
        audio_lang: None,
        expires_at: None,
        drm: None,
    }
}

/// Deterministic catalog: root lists two directories; `/movies` lists two
/// media items; search echoes the query; `m1` resolves to a direct mp4.
pub struct CatalogEchoProvider;

impl AddonProvider for CatalogEchoProvider {
    fn list(
        &self,
        _ctx: &dyn HostApi,
        path: Option<&str>,
        _cursor: Option<&str>,
    ) -> Result<Page, AddonError> {
        let mut page = Page::default();
        match path.unwrap_or("/") {
            "/" => {
                page.items.push(directory("movies", "Movies", "/movies"));
                page.items.push(directory("shows", "Shows", "/shows"));
            }
            "/movies" => {
                page.items.push(media("m1", "First Movie"));
                page.items.push(media("m2", "Second Movie"));
            }
            other => {
                return Err(AddonError::new(
                    AddonErrorCode::NotFound,
                    format!("no such path: {other}"),
                ))
            }
        }
        Ok(page)
    }

    fn search(
        &self,
        _ctx: &dyn HostApi,
        query: &str,
        _filters: &[Filter],
        _cursor: Option<&str>,
    ) -> Result<Page, AddonError> {
        let mut page = Page::default();
        page.items.push(media("s1", &format!("result: {query}")));
        Ok(page)
    }

    fn resolve(
        &self,
        _ctx: &dyn HostApi,
        item_id: &str,
        _source_id: Option<&str>,
    ) -> Result<ResolveResult, AddonError> {
        match item_id {
            "m1" => Ok(ResolveResult::Playables(vec![mp4_source(
                "https://cdn.example.com/m1.mp4",
            )])),
            _ => Ok(ResolveResult::Unavailable {
                reason: format!("{item_id} has no sources"),
            }),
        }
    }

    fn account_status(&self, _ctx: &dyn HostApi) -> Result<AccountStatus, AddonError> {
        Ok(AccountStatus::SignedOut)
    }
}

/// Resolves every item to a fixed set of sources.
pub struct ResolverStaticProvider {
    pub sources: Vec<PlayableSource>,
}

impl AddonProvider for ResolverStaticProvider {
    fn resolve(
        &self,
        _ctx: &dyn HostApi,
        _item_id: &str,
        _source_id: Option<&str>,
    ) -> Result<ResolveResult, AddonError> {
        Ok(ResolveResult::Playables(self.sources.clone()))
    }
}

/// Sleeps before answering — for timeout tests (TC-RUN-004).
pub struct SlowProvider {
    pub delay: Duration,
}

impl AddonProvider for SlowProvider {
    fn list(
        &self,
        _ctx: &dyn HostApi,
        _path: Option<&str>,
        _cursor: Option<&str>,
    ) -> Result<Page, AddonError> {
        std::thread::sleep(self.delay);
        Ok(Page::default())
    }
}

/// Always fails with the configured code — for quarantine and error-class
/// tests (TC-RUN-003, TC-NFR-005).
pub struct FailingProvider {
    pub code: AddonErrorCode,
}

impl AddonProvider for FailingProvider {
    fn list(
        &self,
        _ctx: &dyn HostApi,
        _path: Option<&str>,
        _cursor: Option<&str>,
    ) -> Result<Page, AddonError> {
        Err(AddonError::new(self.code, "fixture failure"))
    }
}

/// Attempts host access it never declared; whatever the host answers is
/// surfaced verbatim (TC-RUN-006).
pub struct BadPermissionsProvider;

impl AddonProvider for BadPermissionsProvider {
    fn list(
        &self,
        ctx: &dyn HostApi,
        _path: Option<&str>,
        _cursor: Option<&str>,
    ) -> Result<Page, AddonError> {
        ctx.secret_get("undeclared_secret")?;
        Ok(Page::default())
    }
}

/// `total` media items served `page_size` at a time via numeric cursors, for
/// both `list` and `search` (which ignores the query).
pub struct LargeCatalogProvider {
    pub total: usize,
    pub page_size: usize,
}

impl LargeCatalogProvider {
    fn page(&self, cursor: Option<&str>) -> Result<Page, AddonError> {
        let offset: usize = match cursor {
            None => 0,
            Some(c) => c
                .parse()
                .map_err(|_| AddonError::new(AddonErrorCode::Malformed, "bad cursor"))?,
        };
        let end = (offset + self.page_size).min(self.total);
        Ok(Page {
            items: (offset..end)
                .map(|i| media(&format!("item-{i}"), &format!("Item {i}")))
                .collect(),
            cursor_next: (end < self.total).then(|| end.to_string()),
            total_hint: Some(self.total as u64),
        })
    }
}

impl AddonProvider for LargeCatalogProvider {
    fn list(
        &self,
        _ctx: &dyn HostApi,
        _path: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<Page, AddonError> {
        self.page(cursor)
    }

    fn search(
        &self,
        _ctx: &dyn HostApi,
        _query: &str,
        _filters: &[Filter],
        cursor: Option<&str>,
    ) -> Result<Page, AddonError> {
        self.page(cursor)
    }
}

/// Inert [`HostApi`] for tests that don't exercise host services: network and
/// secrets are denied (matching an empty permission grant), storage is an
/// in-memory map, logs are dropped.
#[derive(Default)]
pub struct NoopHost {
    storage: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl HostApi for NoopHost {
    fn http_fetch(&self, _req: HttpRequest) -> Result<HttpResponse, AddonError> {
        Err(AddonError::new(
            AddonErrorCode::PermissionDenied,
            "network not granted",
        ))
    }

    fn setting_get(&self, _key: &str) -> Option<String> {
        None
    }

    fn secret_get(&self, name: &str) -> Result<Option<String>, AddonError> {
        Err(AddonError::new(
            AddonErrorCode::PermissionDenied,
            format!("secret {name:?} not declared in manifest"),
        ))
    }

    fn storage_get(&self, key: &str) -> Option<Vec<u8>> {
        self.storage.lock().unwrap().get(key).cloned()
    }

    fn storage_set(&self, key: &str, value: &[u8]) -> Result<(), AddonError> {
        self.storage
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_vec());
        Ok(())
    }

    fn storage_delete(&self, key: &str) {
        self.storage.lock().unwrap().remove(key);
    }

    fn log(&self, _level: LogLevel, _message: &str) {}
}
