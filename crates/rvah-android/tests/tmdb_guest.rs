//! The real TMDB wasm guest under Wasmi and the bridged host, as on
//! Android, with the default fuel and time limits, against a scripted TMDB
//! and MDBList: rows, a row page with artwork through the core's artwork
//! rule, and a details page with trailers and ratings. Needs the guest
//! built first, so it is ignored by default:
//!
//! ```sh
//! examples/addons/tmdb/build.sh
//! cargo test -p rvah-android --test tmdb_guest -- --ignored
//! ```
use std::sync::{Arc, Mutex};

use rvah_addon_host::{
    AddonManager, BridgedHostApi, CapabilityGated, InstallDecision, Manifest, WasmiLimits,
    WasmiRuntime,
};
use rvah_addon_sdk::{HttpRequest, HttpResponse, PageItem};
use rvah_auth::{MemoryVault, SecretVault};
use rvah_core::AppCore;
use rvah_data::Database;
use rvah_net::{NetError, Transport};
use serde_json::json;

struct FakeTmdb {
    log: Mutex<Vec<String>>,
}

fn answer(value: serde_json::Value) -> Result<HttpResponse, NetError> {
    Ok(HttpResponse {
        status: 200,
        headers: Default::default(),
        body: serde_json::to_vec(&value).unwrap(),
    })
}

impl Transport for FakeTmdb {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        self.log.lock().unwrap().push(req.url.clone());
        let url = req.url.as_str();
        if url.starts_with("https://api.themoviedb.org/3/movie/now_playing?") {
            let results: Vec<_> = (0..20)
                .map(|n| {
                    json!({
                        "id": 600 + n, "title": format!("Film {n}"), "release_date": "2026-09-01",
                        "overview": "A film about things that happen to people in places.",
                        "poster_path": format!("/poster{n}.jpg"), "backdrop_path": format!("/backdrop{n}.jpg"),
                        "vote_average": 7.1, "vote_count": 321, "genre_ids": [18, 35],
                        "original_language": "en", "popularity": 123.4, "adult": false, "video": false
                    })
                })
                .collect();
            return answer(json!({
                "page": 1, "results": results, "total_pages": 9, "total_results": 170,
                "dates": { "maximum": "2026-10-10", "minimum": "2026-08-23" }
            }));
        }
        if url.starts_with("https://api.themoviedb.org/3/movie/603?") {
            return answer(json!({
                "id": 603, "title": "Coyote vs. Acme", "overview": "A coyote sues.",
                "tagline": "Beep beep.", "release_date": "2026-08-28", "runtime": 91,
                "genres": [{ "id": 35, "name": "Comedy" }], "imdb_id": "tt0000603",
                "poster_path": "/p603.jpg", "backdrop_path": "/b603.jpg",
                "vote_average": 7.4, "vote_count": 1200,
                "videos": { "results": [
                    { "name": "Official Trailer", "site": "YouTube", "key": "official123", "type": "Trailer", "official": true }
                ] }
            }));
        }
        if url.starts_with("https://api.mdblist.com/tmdb/movie/603?") {
            return answer(json!({ "ratings": [
                { "source": "tomatoes", "value": 93, "score": 93, "votes": 210 },
                { "source": "imdb", "value": 7.8, "score": 78, "votes": 51000 }
            ] }));
        }
        Ok(HttpResponse {
            status: 404,
            headers: Default::default(),
            body: b"not routed".to_vec(),
        })
    }
}

struct Shared(Arc<FakeTmdb>);
impl Transport for Shared {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        self.0.execute(req)
    }
}

#[test]
#[ignore = "needs examples/addons/tmdb/build.sh"]
fn tmdb_guest_lists_rows_and_opens_details() {
    let tmp = tempfile::tempdir().unwrap();
    let dist =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/addons/tmdb/dist");
    let pkg = tmp.path().join("tmdb.rvah");
    rvah_addon_host::package_dir(&dist, &pkg).unwrap();
    let (signing, verifying) = rvah_addon_host::keygen();
    let sig = rvah_addon_host::sign_package(&pkg, &signing).unwrap();

    let db = Arc::new(Database::open_in_memory().unwrap());
    let manager = AddonManager::new(Arc::clone(&db), tmp.path().join("addons"));
    manager
        .install(&pkg, &sig, &verifying, InstallDecision::Approve)
        .unwrap();
    let manifest =
        Manifest::parse(&std::fs::read_to_string(dist.join("manifest.toml")).unwrap()).unwrap();
    let id = manifest.addon.id.clone();
    let vault: Arc<dyn SecretVault> = Arc::new(MemoryVault::new());
    vault
        .set(&id, "tmdb_key", "0123456789abcdef0123456789abcdef")
        .unwrap();
    vault.set(&id, "mdblist_key", "mdb").unwrap();
    let tmdb = Arc::new(FakeTmdb {
        log: Mutex::new(Vec::new()),
    });
    let runtime = WasmiRuntime::load(
        &std::fs::read(dist.join("module.wasm")).unwrap(),
        WasmiLimits::default(),
    )
    .unwrap();
    let mut core = AppCore::new(
        manager,
        Arc::clone(&db),
        Arc::clone(&vault),
        rvah_player::MockPlayer::new(),
    );
    core.register_runtime(
        id.clone(),
        Box::new(CapabilityGated::new(
            runtime,
            manifest.addon.capabilities.clone(),
        )),
        Arc::new(BridgedHostApi::new(
            &manifest,
            Arc::clone(&db),
            Arc::clone(&vault),
            Box::new(Shared(Arc::clone(&tmdb))),
            100,
        )),
    );

    let rows = core.browse(&id, None, None).unwrap();
    // Two streaming rows plus the twelve default genre columns.
    assert_eq!(rows.items.len(), 14);

    let row = core
        .browse(&id, Some("movie/now_playing".into()), None)
        .unwrap();
    assert_eq!(row.items.len(), 20);
    assert_eq!(row.cursor_next.as_deref(), Some("2"));
    let PageItem::Media(first) = &row.items[0] else {
        panic!("expected media");
    };
    // image.tmdb.org is declared, so the artwork survives the core's rule.
    assert_eq!(
        first.artwork.poster.as_deref(),
        Some("https://image.tmdb.org/t/p/w342/poster0.jpg")
    );
    assert!(tmdb.log.lock().unwrap()[0].contains("region=US"));

    let details = core.details(&id, "movie:603").unwrap();
    assert_eq!(details.genres, ["Comedy"]);
    assert_eq!(details.trailers[0].key, "official123");
    let ratings: Vec<(&str, &str)> = details
        .ratings
        .iter()
        .map(|r| (r.source.as_str(), r.value.as_str()))
        .collect();
    assert_eq!(
        ratings,
        [
            ("rotten_tomatoes", "93%"),
            ("imdb", "7.8/10"),
            ("tmdb", "7.4/10")
        ]
    );
}
