//! The real Prowlarr wasm guest under Wasmi and the bridged host, as on
//! Android, with the default fuel and time limits, against a scripted
//! Prowlarr: a full page (75 results) for an episode, and a title search
//! for a film as long as a popular one's (300 results, a megabyte). Needs
//! the guest built first (with any PROWLARR_URL), so it is ignored by
//! default:
//!
//! ```sh
//! PROWLARR_URL=https://prowlarr.example.com examples/addons/prowlarr/build.sh
//! cargo test -p rvah-android --test prowlarr_guest -- --ignored
//! ```
use std::sync::{Arc, Mutex};

use rvah_addon_host::{
    AddonManager, BridgedHostApi, CapabilityGated, InstallDecision, Manifest, WasmiLimits,
    WasmiRuntime,
};
use rvah_addon_sdk::{HttpRequest, HttpResponse, SourceKind, SourceRequest};
use rvah_auth::{MemoryVault, SecretVault};
use rvah_core::AppCore;
use rvah_data::Database;
use rvah_net::{NetError, Transport};
use serde_json::json;

struct FakeProwlarr {
    log: Mutex<Vec<HttpRequest>>,
    answer: fn(&HttpRequest) -> Vec<serde_json::Value>,
}

impl Transport for FakeProwlarr {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        self.log.lock().unwrap().push(req.clone());
        let mut body = serde_json::to_vec(&(self.answer)(req)).unwrap();
        // As rvah-net reads it: no further than asked.
        let cap = req.max_bytes.map_or(usize::MAX, |n| n as usize);
        let truncated = body.len() > cap;
        body.truncate(cap);
        Ok(HttpResponse {
            status: 200,
            truncated,
            headers: Default::default(),
            body,
        })
    }
}

/// A full page of an episode's season, for any search.
fn season_page(_: &HttpRequest) -> Vec<serde_json::Value> {
    (0..75)
            .map(|n| {
                let episode = n % 10 + 1;
                json!({
                    "guid": format!("https://indexer.example/details/{n}?some=tracking&more=params"),
                    "age": 12, "ageHours": 290.5, "ageMinutes": 17430.2, "size": 2_147_483_648u64 + n,
                    "files": 1, "grabs": 300, "indexerId": 3, "indexer": "Example Indexer",
                    "title": format!("Acme.Corp.S01E{episode:02}.1080p.WEB-DL.DDP5.1.H.264-GROUP{n}"),
                    "sortTitle": format!("acme corp s01e{episode:02} 1080p web dl ddp5 1 h 264 group{n}"),
                    "imdbId": 944947, "tmdbId": 1399, "tvdbId": 121361, "tvMazeId": 82,
                    "publishDate": "2026-09-28T12:34:56Z",
                    "commentUrl": format!("https://indexer.example/details/{n}#comments"),
                    "downloadUrl": format!("https://prowlarr.example.com/3/download?apikey=x&link=aHR0cHM6Ly9leGFtcGxlLmNvbS9maWxlLnRvcnJlbnQ{n}&file=Acme.Corp"),
                    "infoUrl": format!("https://indexer.example/details/{n}"),
                    "posterUrl": "https://image.tmdb.org/t/p/w600/poster.jpg",
                    "indexerFlags": ["freeleech"],
                    "categories": [{ "id": 5000, "name": "TV", "subCategories": [] }, { "id": 5040, "name": "TV/HD", "subCategories": [] }],
                    "infoHash": format!("{n:040x}"), "seeders": 100 - n, "leechers": 4,
                    "protocol": "torrent", "fileName": format!("Acme.Corp.S01E{episode:02}.torrent")
                })
            })
            .collect()
}

/// A popular film, as public indexers answer for one: 300 results of
/// about 3.5 KB each (their magnet links carry long tracker lists), by id
/// as by title, so both answers are cut.
fn popular_film(req: &HttpRequest) -> Vec<serde_json::Value> {
    let by_id = req.url.contains("ImdbId");
    let trackers = "&tr=udp%3A%2F%2Ftracker.example.org%3A1337%2Fannounce".repeat(18);
    (0..300)
        .map(|n| {
            let hash = format!("{:040x}", n + if by_id { 10_000 } else { 0 });
            json!({
                "guid": format!("magnet:?xt=urn:btih:{hash}&dn=The.Film.2008.1080p.BluRay.x264-GROUP{n}{trackers}"),
                "age": 12, "ageHours": 290.5, "ageMinutes": 17430.2, "size": 2_147_483_648u64 + n,
                "files": 1, "grabs": 300, "indexerId": 3, "indexer": "Example Indexer",
                "title": format!("The.Film.2008.1080p.BluRay.x264-GROUP{n}"),
                "sortTitle": format!("the film 2008 1080p bluray x264 group{n}"),
                "imdbId": 0, "tmdbId": 0, "tvdbId": 0, "tvMazeId": 0,
                "publishDate": "2026-09-28T12:34:56Z",
                "downloadUrl": format!("https://prowlarr.example.com/3/download?apikey=x&link=aHR0cHM6Ly9leGFtcGxlLmNvbS9maWxlLnRvcnJlbnQ{n}&file=The.Film"),
                "magnetUrl": format!("magnet:?xt=urn:btih:{hash}&dn=The.Film.2008.1080p.BluRay.x264-GROUP{n}{trackers}"),
                "infoUrl": format!("https://indexer.example/details/{n}"),
                "indexerFlags": [],
                "categories": [
                    { "id": 2000, "name": "Movies", "subCategories": [{ "id": 2040, "name": "Movies/HD", "subCategories": [] }] },
                    { "id": 100_042, "name": "Movies/HD", "subCategories": [] }
                ],
                "infoHash": hash, "seeders": n, "leechers": 4,
                "protocol": "torrent", "fileName": "The.Film.2008.1080p.torrent"
            })
        })
        .collect()
}

struct Shared(Arc<FakeProwlarr>);
impl Transport for Shared {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        self.0.execute(req)
    }
}

/// The installed guest, its core and the Prowlarr it talks to.
fn guest(
    answer: fn(&HttpRequest) -> Vec<serde_json::Value>,
    limits: WasmiLimits,
) -> (
    AppCore<rvah_player::MockPlayer>,
    String,
    Arc<FakeProwlarr>,
    tempfile::TempDir,
) {
    let tmp = tempfile::tempdir().unwrap();
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/addons/prowlarr/dist");
    let pkg = tmp.path().join("prowlarr.rvah");
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
        .set(&id, "api_key", "0123456789abcdef0123456789abcdef")
        .unwrap();
    let prowlarr = Arc::new(FakeProwlarr {
        log: Mutex::new(Vec::new()),
        answer,
    });
    let runtime =
        WasmiRuntime::load(&std::fs::read(dist.join("module.wasm")).unwrap(), limits).unwrap();
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
            Box::new(Shared(Arc::clone(&prowlarr))),
            100,
        )),
    );

    (core, id, prowlarr, tmp)
}

#[test]
#[ignore = "needs examples/addons/prowlarr/build.sh"]
fn prowlarr_guest_finds_an_episode_in_a_full_page() {
    let (mut core, id, prowlarr, _tmp) = guest(season_page, WasmiLimits::default());
    let releases = core
        .sources(
            &id,
            SourceRequest {
                kind: SourceKind::Episode,
                imdb_id: Some("tt0944947".into()),
                title: "Acme Corp".into(),
                year: None,
                season: Some(1),
                episode: Some(2),
            },
        )
        .unwrap();
    // 75 results, every tenth of them episode 2.
    assert_eq!(releases.len(), 8);
    assert!(releases.iter().all(|r| r.name.contains("S01E02")));
    let sent = &prowlarr.log.lock().unwrap()[0];
    assert!(sent
        .url
        .contains("/api/v1/search?query=%7BImdbId%3Att0944947%7D%7BSeason%3A1%7D"));
    assert_eq!(
        sent.headers["x-api-key"],
        "0123456789abcdef0123456789abcdef"
    );
}

#[test]
#[ignore = "needs examples/addons/prowlarr/build.sh"]
fn prowlarr_guest_reads_a_popular_films_title_search_within_its_fuel() {
    // Three quarters of the fuel a call gets, so there is room to spare
    // (it takes about two thirds).
    let limits = WasmiLimits {
        fuel: WasmiLimits::default().fuel * 3 / 4,
        ..WasmiLimits::default()
    };
    let (mut core, id, prowlarr, _tmp) = guest(popular_film, limits);
    let releases = core
        .sources(
            &id,
            SourceRequest {
                kind: SourceKind::Movie,
                imdb_id: Some("tt0468569".into()),
                title: "The Film".into(),
                year: Some(2008),
                season: None,
                episode: None,
            },
        )
        .unwrap();
    // As many whole results as the cuts leave: some 9 by id, 27 by title.
    assert!((30..=40).contains(&releases.len()), "{}", releases.len());
    let log = prowlarr.log.lock().unwrap();
    assert_eq!(log.len(), 2);
    assert!(log.iter().all(|sent| sent.max_bytes.is_some()));
}
