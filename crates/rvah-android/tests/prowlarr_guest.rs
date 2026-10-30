//! The real Prowlarr wasm guest under Wasmi and the bridged host, as on
//! Android, with the default fuel and time limits, against a scripted
//! Prowlarr answering a full page (75 results) for an episode. Needs the
//! guest built first (with any PROWLARR_URL), so it is ignored by default:
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
}

impl Transport for FakeProwlarr {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        self.log.lock().unwrap().push(req.clone());
        let results: Vec<_> = (0..75)
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
            .collect();
        Ok(HttpResponse {
            status: 200,
            truncated: false,
            headers: Default::default(),
            body: serde_json::to_vec(&results).unwrap(),
        })
    }
}

struct Shared(Arc<FakeProwlarr>);
impl Transport for Shared {
    fn execute(&self, req: &HttpRequest) -> Result<HttpResponse, NetError> {
        self.0.execute(req)
    }
}

#[test]
#[ignore = "needs examples/addons/prowlarr/build.sh"]
fn prowlarr_guest_finds_an_episode_in_a_full_page() {
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
            Box::new(Shared(Arc::clone(&prowlarr))),
            100,
        )),
    );

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
